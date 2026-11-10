use serde_json::{Value, json};
use std::{
    net::TcpListener,
    path::{Path, PathBuf},
    process::{Command, Output},
    time::Duration,
};

struct Fixture {
    directory: tempfile::TempDir,
    address: String,
}
impl Fixture {
    fn new() -> Self {
        // Keep independently running fixtures from selecting the same recently freed port.
        static PORTS: std::sync::OnceLock<std::sync::Mutex<std::collections::HashSet<u16>>> =
            std::sync::OnceLock::new();
        let mut ports = PORTS.get_or_init(Default::default).lock().unwrap();
        let address = loop {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            if ports.insert(address.port()) {
                break address.to_string();
            }
        };
        Self {
            directory: tempfile::tempdir().unwrap(),
            address,
        }
    }
    fn state(&self) -> PathBuf {
        self.directory.path().join("state")
    }
    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_boid"));
        for key in [
            "BOID_URL",
            "BOID_TOKEN",
            "BOID_CWD",
            "BOID_MODEL",
            "BOID_AUTH_ROUTE",
            "BOID_CYBER_ACCESS_PROGRAM",
        ] {
            command.env_remove(key);
        }
        command
            .args(args)
            .current_dir(self.directory.path())
            .env("BOID_LISTEN", &self.address)
            .env("BOID_STATE_DIR", self.state())
            .env("OPENAI_API_KEY", "synthetic-startup-test-no-provider-call");
        command
    }
    fn run(&self, args: &[&str]) -> Output {
        self.command(args).output().unwrap()
    }
    fn ok(&self, args: &[&str]) -> Output {
        let output = self.run(args);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        output
    }
    async fn get(&self, path: &str) -> Value {
        reqwest::Client::new()
            .get(format!("http://{}{path}", self.address))
            .bearer_auth(std::fs::read_to_string(self.state().join("token")).unwrap())
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json()
            .await
            .unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self.run(&["stop"]);
    }
}
fn cwd_from_output(output: &Output) -> String {
    let text = String::from_utf8_lossy(&output.stdout);
    let url = reqwest::Url::parse(
        text.lines()
            .find_map(|line| line.strip_prefix("Open "))
            .unwrap(),
    )
    .unwrap();
    let fragment =
        reqwest::Url::parse(&format!("http://localhost/?{}", url.fragment().unwrap())).unwrap();
    fragment
        .query_pairs()
        .find(|(key, _)| key == "cwd")
        .unwrap()
        .1
        .into_owned()
}
fn mkdir(path: &Path) {
    std::fs::create_dir_all(path).unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn simultaneous_launches_reuse_one_server_and_preserve_invocation_defaults() {
    let fixture = Fixture::new();
    let work_a = fixture.directory.path().join("work a");
    let work_b = fixture.directory.path().join("work & b");
    mkdir(&work_a);
    mkdir(&work_b);
    std::thread::scope(|scope| {
        let mut threads = vec![];
        for index in 0..6 {
            let fixture = &fixture;
            let cwd = if index % 2 == 0 { &work_a } else { &work_b };
            threads.push(scope.spawn(move || {
                let output = fixture
                    .command(&["--no-open"])
                    .current_dir(cwd)
                    .output()
                    .unwrap();
                assert!(
                    output.status.success(),
                    "{}",
                    String::from_utf8_lossy(&output.stderr)
                );
                assert_eq!(
                    cwd_from_output(&output),
                    cwd.canonicalize().unwrap().to_string_lossy()
                );
            }));
        }
        for thread in threads {
            thread.join().unwrap();
        }
    });
    let info = fixture.get("/api/server").await;
    assert_eq!(info["version"], env!("CARGO_PKG_VERSION"));
    let default_cwd = fixture.get("/api/config").await["cwd"].clone();
    let warm = fixture
        .command(&["--no-open"])
        .env_remove("OPENAI_API_KEY")
        .current_dir(&work_b)
        .output()
        .unwrap();
    assert!(warm.status.success());
    assert_eq!(fixture.get("/api/server").await["pid"], info["pid"]);
    assert_eq!(fixture.get("/api/config").await["cwd"], default_cwd);
    assert!(
        fixture
            .command(&[""])
            .current_dir(&work_b)
            .output()
            .unwrap()
            .status
            .success()
    );
    let sessions = fixture.get("/api/sessions").await;
    assert_eq!(sessions.as_array().unwrap().len(), 1);
    assert_eq!(
        sessions[0]["cwd"],
        work_b.canonicalize().unwrap().to_string_lossy().as_ref()
    );
    let session_id = sessions[0]["id"].clone();
    fixture.ok(&["stop"]);
    fixture.ok(&["--no-open"]);
    assert_ne!(fixture.get("/api/server").await["pid"], info["pid"]);
    assert_eq!(fixture.get("/api/sessions").await[0]["id"], session_id);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn authentication_failure_does_not_replace_server_and_remote_attach_needs_no_key() {
    let fixture = Fixture::new();
    fixture.ok(&["--no-open"]);
    let pid = fixture.get("/api/server").await["pid"].clone();
    let rejected = fixture
        .command(&["--no-open"])
        .env("BOID_TOKEN", "wrong-token-with-at-least-24-characters")
        .output()
        .unwrap();
    assert!(!rejected.status.success());
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("rejected authentication"));
    assert_eq!(fixture.get("/api/server").await["pid"], pid);
    let remote = fixture
        .command(&["--no-open"])
        .env("BOID_URL", format!("http://{}", fixture.address))
        .env_remove("OPENAI_API_KEY")
        .output()
        .unwrap();
    assert!(remote.status.success());
    assert_eq!(fixture.get("/api/server").await["pid"], pid);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unrelated_port_owner_is_not_started_over_or_stopped() {
    let fixture = Fixture::new();
    let listener = tokio::net::TcpListener::bind(&fixture.address)
        .await
        .unwrap();
    let app = axum::Router::new().route(
        "/api/server",
        axum::routing::get(|| async {
            axum::Json(json!({"application":"other", "protocol":1, "version":"1", "pid":123}))
        }),
    );
    let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let output = fixture.run(&["--no-open"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("not a compatible Boid"));
    assert!(!fixture.run(&["stop"]).status.success());
    assert!(!task.is_finished());
    assert!(!fixture.state().join("server.log").exists());
    task.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn first_login_without_key_and_corrupt_state_recovery() {
    let fixture = Fixture::new();
    let output = fixture
        .command(&["--no-open"])
        .env_remove("OPENAI_API_KEY")
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(
        fixture.get("/api/accounts").await["api_key_available"],
        false
    );
    let client = reqwest::Client::new();
    let token = std::fs::read_to_string(fixture.state().join("token")).unwrap();
    let endpoint = format!("http://{}", fixture.address);
    let denied = client
        .post(format!("{endpoint}/api/sessions"))
        .bearer_auth(&token)
        .json(&json!({"name":"canary","cwd":fixture.directory.path()}))
        .send()
        .await
        .unwrap();
    assert!(!denied.status().is_success());
    assert!(denied.text().await.unwrap().contains("/login"));
    let synthetic = "synthetic-login-key-no-provider-call";
    client
        .post(format!("{endpoint}/api/accounts/api-key"))
        .bearer_auth(&token)
        .json(&json!({"key":synthetic}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    let accounts = fixture.get("/api/accounts").await;
    assert_eq!(accounts["api_key_available"], true);
    assert!(!accounts.to_string().contains(synthetic));
    assert!(
        !fixture
            .get("/api/config")
            .await
            .to_string()
            .contains(synthetic)
    );
    fixture.ok(&["stop"]);
    let output = fixture
        .command(&["--no-open"])
        .env_remove("OPENAI_API_KEY")
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(
        fixture.get("/api/accounts").await["api_key_available"],
        true
    );
    fixture.ok(&["stop"]);
    mkdir(&fixture.state().join("sessions"));
    let broken = fixture.state().join("sessions/broken.json");
    std::fs::write(&broken, "invalid json").unwrap();
    let output = fixture.run(&["--no-open"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("server exited"));
    assert!(TcpListener::bind(&fixture.address).is_ok());
    std::fs::remove_file(broken).unwrap();
    fixture.ok(&["--no-open"]);
    fixture.ok(&["stop"]);
    fixture.ok(&["stop"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn foreground_serve_can_be_attached_and_stopped() {
    let fixture = Fixture::new();
    let mut child = fixture
        .command(&["serve"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    for _ in 0..100 {
        if fixture.state().join("token").exists()
            && std::net::TcpStream::connect(&fixture.address).is_ok()
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let output = fixture.ok(&["--no-open"]);
    assert!(String::from_utf8_lossy(&output.stdout).contains(&format!("PID {}", child.id())));
    fixture.ok(&["stop"]);
    assert!(child.wait().unwrap().success());
}

#[cfg(target_os = "macos")]
#[test]
fn desktop_launch_opens_the_workspace_once_and_no_open_skips_it() {
    use std::os::unix::fs::PermissionsExt;
    let fixture = Fixture::new();
    let bin = fixture.directory.path().join("bin");
    mkdir(&bin);
    let opener = bin.join("open");
    std::fs::write(
        &opener,
        "#!/bin/sh\nprintf '%s' \"$1\" > \"$BOID_TEST_OPEN_OUTPUT\"\n",
    )
    .unwrap();
    std::fs::set_permissions(&opener, std::fs::Permissions::from_mode(0o755)).unwrap();
    let recorded = fixture.directory.path().join("opened-url");
    let launch = |args: &[&str]| {
        fixture
            .command(args)
            .env_remove("SSH_CONNECTION")
            .env_remove("SSH_TTY")
            .env(
                "PATH",
                format!(
                    "{}:{}",
                    bin.display(),
                    std::env::var("PATH").unwrap_or_default()
                ),
            )
            .env("BOID_TEST_OPEN_OUTPUT", &recorded)
            .output()
            .unwrap()
    };
    let output = launch(&[]);
    assert!(output.status.success());
    let opened = std::fs::read_to_string(&recorded).unwrap();
    assert!(
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .any(|line| line.strip_prefix("Open ") == Some(&opened))
    );
    std::fs::remove_file(&recorded).unwrap();
    assert!(launch(&["--no-open"]).status.success());
    assert!(!recorded.exists());
}
