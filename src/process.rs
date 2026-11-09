use crate::{ExecOutput, OUTPUT_LIMIT, Output, Tools, guard};
use std::io;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;
use tokio::signal::unix::{Signal, SignalKind, signal};

impl Tools {
    /// Run Bash with a time limit. Dropping this future kills its process group.
    /// Commands run with the host user's authority; the guard is only a heuristic.
    /// Deliberately detached process groups and abrupt host termination are not contained.
    pub async fn exec(&self, command: &str, timeout: Duration) -> io::Result<ExecOutput> {
        guard::check(command)
            .map_err(|reason| io::Error::new(io::ErrorKind::PermissionDenied, reason))?;
        self.run(
            "/bin/bash",
            &["--noprofile", "--norc", "-c", command],
            timeout,
        )
        .await
    }

    pub(crate) async fn run(
        &self,
        program: &str,
        args: &[&str],
        timeout: Duration,
    ) -> io::Result<ExecOutput> {
        // Register before spawn so an immediate exit cannot miss the wakeup.
        let mut exited = signal(SignalKind::child())?;
        let mut child = Command::new(program)
            .args(args)
            .current_dir(&self.cwd)
            .env_remove("OPENAI_API_KEY")
            .env_remove("BOID_TOKEN")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0)
            .kill_on_drop(true)
            .spawn()?;
        let pid = child.id().expect("new child has a process id") as i32;
        // Declared after child: cancellation drops the group guard before the
        // child handle can reap its PID.
        let mut group = ProcessGroup(Some(pid));
        let stdout = child.stdout.take().expect("stdout was piped");
        let stderr = child.stderr.take().expect("stderr was piped");
        let mut out = Vec::new();
        let mut err = Vec::new();
        let completion = async {
            let wait = async {
                wait_unreaped(pid, &mut exited).await?;
                // Keep the leader unreaped until its background group has been
                // stopped. Reaping first would allow this PGID to be reused.
                group.kill()?;
                child.wait().await
            };
            let (status, (), ()) =
                tokio::try_join!(wait, capture(stdout, &mut out), capture(stderr, &mut err),)?;
            Ok::<_, io::Error>(status)
        };
        let (status, timed_out) = match tokio::time::timeout(timeout, completion).await {
            Ok(status) => (status?, false),
            Err(_) => {
                group.kill()?;
                (child.wait().await?, true)
            }
        };
        Ok(ExecOutput {
            status,
            stdout: Output::from_bytes(&out),
            stderr: Output::from_bytes(&err),
            timed_out,
        })
    }
}

struct ProcessGroup(Option<i32>);

impl ProcessGroup {
    fn kill(&mut self) -> io::Result<()> {
        let Some(pid) = self.0.take() else {
            return Ok(());
        };
        // The root child is still owned and unreaped, so its PID cannot refer
        // to another group. Never repeat this signal after reaping that child.
        if unsafe { libc::kill(-pid, libc::SIGKILL) } == -1 {
            let error = io::Error::last_os_error();
            #[cfg(target_os = "macos")]
            if error.raw_os_error() == Some(libc::EPERM) && group_has_no_live_members(pid) {
                return Ok(());
            }
            if error.raw_os_error() != Some(libc::ESRCH) {
                return Err(error);
            }
        }
        Ok(())
    }
}

impl Drop for ProcessGroup {
    fn drop(&mut self) {
        let _ = self.kill();
    }
}

// Darwin reports EPERM for a group containing only unreaped zombies. Do not
// turn a successful command into an error, or hide a failure to kill live work.
#[cfg(target_os = "macos")]
fn group_has_no_live_members(group: i32) -> bool {
    let count = unsafe { libc::proc_listpgrppids(group, std::ptr::null_mut(), 0) };
    if count < 0 {
        return false;
    }
    let mut members = vec![0i32; count as usize + 16];
    let found = unsafe {
        libc::proc_listpgrppids(
            group,
            members.as_mut_ptr().cast(),
            (members.len() * std::mem::size_of::<i32>()) as i32,
        )
    };
    if found < 0 || found as usize >= members.len() {
        return false;
    }
    members.into_iter().take(found as usize).all(|pid| {
        let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
        let size = std::mem::size_of_val(&info) as i32;
        let read = unsafe {
            libc::proc_pidinfo(
                pid,
                libc::PROC_PIDTBSDINFO,
                0,
                (&mut info as *mut libc::proc_bsdinfo).cast(),
                size,
            )
        };
        if read == size {
            info.pbi_pgid != group as u32 || info.pbi_status == libc::SZOMB
        } else {
            // proc_pidinfo reports ESRCH for zombies as well as vanished PIDs.
            io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
        }
    })
}

async fn wait_unreaped(pid: i32, exited: &mut Signal) -> io::Result<()> {
    loop {
        // Darwin siginfo_t contains a raw pointer. Keep it entirely within
        // this synchronous block so the tool future remains Send.
        let finished = {
            let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
            let result = unsafe {
                libc::waitid(
                    libc::P_PID,
                    pid as libc::id_t,
                    &mut info,
                    libc::WEXITED | libc::WNOWAIT | libc::WNOHANG,
                )
            };
            if result == -1 {
                let error = io::Error::last_os_error();
                if error.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(error);
            }
            (unsafe { info.si_pid() }) == pid
        };
        if finished {
            return Ok(());
        }
        if exited.recv().await.is_none() {
            return Err(io::Error::other("Child exit notifications closed."));
        }
    }
}

async fn capture(mut stream: impl AsyncRead + Unpin, kept: &mut Vec<u8>) -> io::Result<()> {
    let mut chunk = [0u8; 8192];
    loop {
        let count = stream.read(&mut chunk).await?;
        if count == 0 {
            break;
        }
        let keep = count.min((OUTPUT_LIMIT + 1).saturating_sub(kept.len()));
        kept.extend_from_slice(&chunk[..keep]);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn guard_prevents_the_entire_script_from_starting() {
        let dir = tempfile::tempdir().unwrap();
        let tools = Tools::new(dir.path()).unwrap();
        let error = tools
            .exec("touch marker; rm -f marker", Duration::from_secs(2))
            .await
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        assert!(!dir.path().join("marker").exists());
    }

    #[tokio::test]
    async fn keeps_exit_status_and_bounded_output() {
        let dir = tempfile::tempdir().unwrap();
        let tools = Tools::new(dir.path()).unwrap();
        let result = tools
            .exec(
                "head -c 65536 /dev/zero; printf error >&2; exit 7",
                Duration::from_secs(2),
            )
            .await
            .unwrap();
        assert_eq!(result.status.code(), Some(7));
        assert_eq!(result.stdout.text.len(), OUTPUT_LIMIT);
        assert!(result.stdout.truncated);
        assert_eq!(result.stderr.text, "error");
        assert!(!result.timed_out);
    }

    #[tokio::test]
    async fn completion_stops_background_work_before_pipe_eof() {
        let dir = tempfile::tempdir().unwrap();
        let tools = Tools::new(dir.path()).unwrap();
        let result = tools
            .exec(
                "(sleep 0.3; touch marker) & printf completed; exit 7",
                Duration::from_secs(2),
            )
            .await
            .unwrap();
        assert_eq!(result.status.code(), Some(7));
        assert_eq!(result.stdout.text, "completed");
        assert!(!result.timed_out);
        tokio::time::sleep(Duration::from_millis(400)).await;
        assert!(!dir.path().join("marker").exists());
    }

    #[tokio::test]
    async fn timeout_stops_background_work() {
        let dir = tempfile::tempdir().unwrap();
        let tools = Tools::new(dir.path()).unwrap();
        let result = tools
            .exec(
                "(sleep 0.3; touch marker) & wait",
                Duration::from_millis(30),
            )
            .await
            .unwrap();
        assert!(result.timed_out);
        tokio::time::sleep(Duration::from_millis(400)).await;
        assert!(!dir.path().join("marker").exists());
    }

    #[tokio::test]
    async fn cancellation_stops_background_work() {
        let dir = tempfile::tempdir().unwrap();
        let tools = Tools::new(dir.path()).unwrap();
        let result = tokio::time::timeout(
            Duration::from_millis(30),
            tools.exec("(sleep 0.3; touch marker) & wait", Duration::from_secs(2)),
        )
        .await;
        assert!(result.is_err());
        tokio::time::sleep(Duration::from_millis(400)).await;
        assert!(!dir.path().join("marker").exists());
    }
}
