use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::{
    fs::{File, OpenOptions},
    os::{
        fd::AsRawFd,
        unix::{
            fs::{OpenOptionsExt, PermissionsExt},
            process::CommandExt,
        },
    },
    path::PathBuf,
    process::{Command, Stdio},
    time::Duration,
};
use tokio::time::{Instant, sleep};

#[derive(Deserialize)]
pub struct ServerInfo {
    pub application: String,
    pub protocol: u32,
    pub version: String,
    pub pid: u32,
}

pub struct Connection {
    pub url: reqwest::Url,
    pub token: String,
    pub info: ServerInfo,
}

struct Launcher {
    url: reqwest::Url,
    remote: bool,
    state: PathBuf,
    http: reqwest::Client,
}

impl Launcher {
    fn new() -> Result<Self> {
        let remote = std::env::var("BOID_URL").ok();
        let address = std::env::var("BOID_LISTEN").unwrap_or_else(|_| "127.0.0.1:7734".into());
        let url = endpoint(remote.as_deref(), &address)?;
        let state = crate::server::token_file()?
            .parent()
            .context("Missing state directory")?
            .to_path_buf();
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(1))
            .timeout(Duration::from_secs(2))
            .build()?;
        Ok(Self {
            url,
            remote: remote.is_some(),
            state,
            http,
        })
    }

    async fn lock(&self) -> Result<File> {
        std::fs::create_dir_all(&self.state).context("Cannot create Boid state directory")?;
        std::fs::set_permissions(&self.state, std::fs::Permissions::from_mode(0o700))?;
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(self.state.join("startup.lock"))?;
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
                return Ok(lock);
            }
            let error = std::io::Error::last_os_error();
            if error.kind() != std::io::ErrorKind::WouldBlock {
                return Err(error.into());
            }
            if Instant::now() >= deadline {
                bail!(
                    "Another Boid launcher is still starting or stopping the server. Try again shortly."
                );
            }
            sleep(Duration::from_millis(50)).await;
        }
    }

    async fn probe(&self) -> Result<Option<Connection>> {
        let token = crate::server::read_token();
        let mut request = self.http.get(self.url.join("api/server")?);
        if let Ok(token) = &token {
            request = request.bearer_auth(token);
        }
        let response = match request.send().await {
            Ok(response) => response,
            Err(error) if error.is_connect() => return Ok(None),
            Err(_) => bail!(
                "The server at {} did not answer. It may still be starting; try again shortly.",
                self.url
            ),
        };
        if response.status() == reqwest::StatusCode::UNAUTHORIZED {
            bail!(
                "The server at {} rejected authentication. Use its BOID_TOKEN or BOID_STATE_DIR; no second server was started.",
                self.url
            );
        }
        if !response.status().is_success() {
            bail!(
                "The service at {} does not support Boid start-or-attach (HTTP {}). An older Boid server must be restarted with v1.1 or newer; no process was stopped.",
                self.url,
                response.status()
            );
        }
        let info: ServerInfo = response
            .json()
            .await
            .context("The listening service did not return a Boid server identity")?;
        if info.application != "boid" || info.protocol != 1 {
            bail!("The listening service is not a compatible Boid server. No process was stopped.");
        }
        Ok(Some(Connection {
            url: self.url.clone(),
            token: token?,
            info,
        }))
    }

    fn state_released(&self) -> Result<bool> {
        if self.remote {
            return Ok(true);
        }
        let file = match OpenOptions::new()
            .read(true)
            .write(true)
            .open(self.state.join("sessions.lock"))
        {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(true),
            Err(error) => return Err(error.into()),
        };
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
            return Ok(true);
        }
        let error = std::io::Error::last_os_error();
        if error.kind() == std::io::ErrorKind::WouldBlock {
            return Ok(false);
        }
        Err(error.into())
    }
}

fn endpoint(remote: Option<&str>, address: &str) -> Result<reqwest::Url> {
    let local = format!("http://{address}");
    let mut url =
        reqwest::Url::parse(remote.unwrap_or(&local)).context("Invalid Boid server address")?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != "/"
    {
        bail!(
            "BOID_URL must be an HTTP(S) server origin without credentials, path, query, or fragment."
        );
    }
    if url.port() == Some(0) {
        bail!("Start-or-attach requires a fixed nonzero port in BOID_LISTEN.");
    }
    if remote.is_none() {
        match url.host_str() {
            Some("0.0.0.0") => url.set_host(Some("127.0.0.1"))?,
            Some("[::]") => url.set_host(Some("[::1]"))?,
            _ => (),
        }
    }
    Ok(url)
}

pub async fn connect() -> Result<Connection> {
    let launcher = Launcher::new()?;
    // Explicit remote connections never create a server on this machine.
    if launcher.remote {
        return launcher
            .probe()
            .await?
            .context("Cannot reach BOID_URL. Start the server on its host.");
    }
    let _lock = launcher.lock().await?;
    if let Some(connection) = launcher.probe().await? {
        return Ok(connection);
    }
    crate::api::Client::new()?; // Validate configuration before detaching a process.
    let log_path = launcher.state.join("server.log");
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(&log_path)?;
    log.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    let mut command = Command::new(std::env::current_exe()?);
    command
        .arg("serve")
        .stdin(Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log);
    // The server outlives this launcher and its controlling terminal.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = command.spawn().context("Cannot start the Boid server")?;
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if let Some(status) = child.try_wait()? {
            bail!(
                "Boid server exited ({status}). See {} for details.",
                log_path.display()
            );
        }
        match launcher.probe().await {
            Ok(Some(connection)) => return Ok(connection),
            Ok(None) => (),
            Err(_) if Instant::now() < deadline => (),
            Err(_) => break,
        }
        if Instant::now() >= deadline {
            break;
        }
        sleep(Duration::from_millis(50)).await;
    }
    // Only this launcher's child is eligible for cleanup.
    unsafe {
        libc::kill(child.id() as i32, libc::SIGTERM);
    }
    for _ in 0..50 {
        if child.try_wait()?.is_some() {
            break;
        }
        sleep(Duration::from_millis(100)).await;
    }
    if child.try_wait()?.is_none() {
        child.kill()?;
        child.wait()?;
    }
    bail!(
        "Boid did not become ready. See {} for details.",
        log_path.display()
    )
}

pub async fn stop() -> Result<()> {
    let launcher = Launcher::new()?;
    let _lock = if launcher.remote {
        None
    } else {
        Some(launcher.lock().await?)
    };
    let Some(connection) = launcher.probe().await? else {
        println!("Boid is not running at {}", launcher.url);
        return Ok(());
    };
    let response = launcher
        .http
        .post(launcher.url.join("api/shutdown")?)
        .bearer_auth(&connection.token)
        .send()
        .await
        .context("Cannot request Boid shutdown")?;
    if !response.status().is_success() {
        bail!("Boid refused shutdown (HTTP {}).", response.status());
    }
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if matches!(launcher.probe().await, Ok(None)) && launcher.state_released()? {
            println!("Boid stopped. Saved conversations remain available.");
            return Ok(());
        }
        if Instant::now() >= deadline {
            bail!("Shutdown was requested, but Boid is still responding.");
        }
        sleep(Duration::from_millis(100)).await;
    }
}

pub fn invocation_cwd() -> Result<String> {
    if let Ok(cwd) = std::env::var("BOID_CWD") {
        return Ok(cwd);
    }
    if std::env::var_os("BOID_URL").is_some() {
        return Ok(String::new());
    }
    Ok(std::env::current_dir()?.to_string_lossy().into_owned())
}

pub fn open_workspace(connection: &Connection, no_open: bool) -> Result<()> {
    let mut url = connection.url.clone();
    let cwd = invocation_cwd()?;
    let mut fragment = reqwest::Url::parse("http://localhost/")?;
    fragment
        .query_pairs_mut()
        .append_pair("token", &connection.token);
    if !cwd.is_empty() {
        fragment.query_pairs_mut().append_pair("cwd", &cwd);
    }
    url.set_fragment(fragment.query());
    println!(
        "Boid {} is running (PID {}).\nOpen {url}",
        connection.info.version, connection.info.pid
    );
    if no_open
        || std::env::var_os("SSH_CONNECTION").is_some()
        || std::env::var_os("SSH_TTY").is_some()
    {
        return Ok(());
    }
    #[cfg(target_os = "macos")]
    let opener = Some("open");
    #[cfg(not(target_os = "macos"))]
    let opener = (std::env::var_os("DISPLAY").is_some()
        || std::env::var_os("WAYLAND_DISPLAY").is_some())
    .then_some("xdg-open");
    if let Some(opener) = opener {
        match Command::new(opener)
            .arg(url.as_str())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
        {
            Ok(status) if status.success() => (),
            _ => eprintln!("Could not open a browser. Use the URL above."),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn endpoint_uses_listener_and_rejects_ambiguous_urls() {
        assert_eq!(
            endpoint(None, "0.0.0.0:8123").unwrap().as_str(),
            "http://127.0.0.1:8123/"
        );
        assert_eq!(
            endpoint(None, "[::]:8123").unwrap().as_str(),
            "http://[::1]:8123/"
        );
        assert_eq!(
            endpoint(Some("https://example.com:8123"), "ignored")
                .unwrap()
                .as_str(),
            "https://example.com:8123/"
        );
        for url in [
            "file:///tmp/x",
            "http://user:secret@localhost",
            "http://localhost/path",
            "http://localhost/?token=x",
        ] {
            assert!(endpoint(Some(url), "ignored").is_err());
        }
        assert!(endpoint(None, "127.0.0.1:0").is_err());
    }
}
