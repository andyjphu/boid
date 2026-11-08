use crate::{ExecOutput, OUTPUT_LIMIT, Output, Tools, guard};
use std::io;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;

impl Tools {
    /// Run Bash with a time limit. Dropping this future kills its process group.
    /// Commands run with the container's authority; the guard is only a heuristic.
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
        let mut child = Command::new(program)
            .args(args)
            .current_dir(&self.cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0)
            .kill_on_drop(true)
            .spawn()?;
        let group = ProcessGroup(child.id().expect("new child has a process id") as i32);
        let stdout = child.stdout.take().expect("stdout was piped");
        let stderr = child.stderr.take().expect("stderr was piped");
        let mut out = Vec::new();
        let mut err = Vec::new();
        let completion = async {
            let wait = async {
                let status = child.wait().await?;
                // A command owns its background children. End them before pipe EOF.
                group.kill();
                Ok::<_, io::Error>(status)
            };
            let (status, (), ()) =
                tokio::try_join!(wait, capture(stdout, &mut out), capture(stderr, &mut err),)?;
            Ok::<_, io::Error>(status)
        };
        let (status, timed_out) = match tokio::time::timeout(timeout, completion).await {
            Ok(status) => (status?, false),
            Err(_) => {
                group.kill();
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

struct ProcessGroup(i32);

impl ProcessGroup {
    fn kill(&self) {
        // The child was spawned with PGID equal to its PID. Never use a name lookup.
        unsafe { libc::kill(-self.0, libc::SIGKILL) };
    }
}

impl Drop for ProcessGroup {
    fn drop(&mut self) {
        self.kill();
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
