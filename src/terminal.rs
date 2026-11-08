use axum::{
    Json, Router,
    extract::{
        Path, State, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    http::StatusCode,
    response::Response,
    routing::{delete, get},
};
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    io::{Read, Write},
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::sync::broadcast;

const REPLAY_BYTES: usize = 512 * 1024;
type Error = (StatusCode, String);

#[derive(Clone)]
pub struct TerminalHub(Arc<Hub>);

struct Hub {
    cwd: PathBuf,
    closing: AtomicBool,
    terminals: Mutex<BTreeMap<String, Arc<Terminal>>>,
}

struct Terminal {
    id: String,
    cwd: String,
    running: AtomicBool,
    output_done: AtomicBool,
    closed: AtomicBool,
    master: Mutex<Box<dyn MasterPty + Send>>,
    writer: Mutex<Box<dyn Write + Send>>,
    child: Mutex<Box<dyn Child + Send + Sync>>,
    output: Mutex<Output>,
}

struct Output {
    bytes: VecDeque<u8>,
    dropped: usize,
    tx: broadcast::Sender<Vec<u8>>,
}

#[derive(Serialize)]
struct Summary {
    id: String,
    cwd: String,
    running: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Create {
    cwd: Option<String>,
    cols: Option<u16>,
    rows: Option<u16>,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum Input {
    Input { data: String },
    Resize { cols: u16, rows: u16 },
}

impl TerminalHub {
    pub fn new(cwd: PathBuf) -> Self {
        Self(Arc::new(Hub {
            cwd,
            closing: AtomicBool::new(false),
            terminals: Mutex::new(BTreeMap::new()),
        }))
    }

    pub fn router(&self) -> Router {
        Router::new()
            .route("/api/terminals", get(list).post(create))
            .route("/api/terminals/{id}", delete(close))
            .route("/api/terminals/{id}/ws", get(connect))
            .with_state(self.clone())
    }

    pub async fn shutdown(&self) -> anyhow::Result<()> {
        self.0.closing.store(true, Ordering::SeqCst);
        let terminals = std::mem::take(&mut *self.0.terminals.lock().unwrap());
        tokio::task::spawn_blocking(move || {
            let mut failure = None;
            for terminal in terminals.into_values() {
                if let Err(error) = terminal.close() {
                    failure = Some(error);
                }
            }
            match failure {
                Some(error) => Err(error),
                None => Ok(()),
            }
        })
        .await?
    }

    fn find(&self, id: &str) -> Result<Arc<Terminal>, Error> {
        self.0
            .terminals
            .lock()
            .unwrap()
            .get(id)
            .cloned()
            .ok_or((StatusCode::NOT_FOUND, "Terminal not found.".into()))
    }
}

impl Terminal {
    fn open(id: String, cwd: PathBuf, size: PtySize) -> anyhow::Result<Arc<Self>> {
        let pair = native_pty_system().openpty(size)?;
        let mut command = CommandBuilder::new("/bin/bash");
        command.args(["--noprofile", "--norc", "-i"]);
        command.cwd(&cwd);
        command.env("TERM", "xterm-256color");
        command.env_remove("OPENAI_API_KEY");
        command.env_remove("BOID_TOKEN");
        let reader = pair.master.try_clone_reader()?;
        let writer = pair.master.take_writer()?;
        let child = pair.slave.spawn_command(command)?;
        drop(pair.slave);
        let terminal = Arc::new(Self {
            id,
            cwd: cwd.to_string_lossy().into_owned(),
            running: AtomicBool::new(true),
            output_done: AtomicBool::new(false),
            closed: AtomicBool::new(false),
            master: Mutex::new(pair.master),
            writer: Mutex::new(writer),
            child: Mutex::new(child),
            output: Mutex::new(Output {
                bytes: VecDeque::new(),
                dropped: 0,
                tx: broadcast::channel(64).0,
            }),
        });
        let reading = terminal.clone();
        if let Err(error) = std::thread::Builder::new()
            .name(format!("{}-output", terminal.id))
            .stack_size(128 * 1024)
            .spawn(move || reading.read(reader))
        {
            let _ = terminal.close();
            return Err(error.into());
        }
        Ok(terminal)
    }

    fn summary(&self) -> Summary {
        Summary {
            id: self.id.clone(),
            cwd: self.cwd.clone(),
            running: self.running.load(Ordering::SeqCst),
        }
    }

    fn publish(&self, bytes: Vec<u8>) {
        let mut output = self.output.lock().unwrap();
        output.bytes.extend(&bytes);
        let extra = output.bytes.len().saturating_sub(REPLAY_BYTES);
        output.bytes.drain(..extra);
        output.dropped += extra;
        let _ = output.tx.send(bytes);
    }

    fn read(&self, mut reader: Box<dyn Read + Send>) {
        let mut bytes = [0; 8192];
        let descriptor = self.master.lock().unwrap().as_raw_fd();
        while !self.closed.load(Ordering::SeqCst) {
            if let Some(fd) = descriptor {
                // Poll lets explicit close release this reader even if a detached
                // child keeps the slave open. The master remains owned by self.
                let mut event = libc::pollfd {
                    fd,
                    events: libc::POLLIN,
                    revents: 0,
                };
                let ready = unsafe { libc::poll(&mut event, 1, 100) };
                if ready == 0 {
                    continue;
                }
                if ready < 0 {
                    if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                        continue;
                    }
                    break;
                }
            }
            match reader.read(&mut bytes) {
                Ok(0) => break,
                Ok(count) => self.publish(bytes[..count].to_vec()),
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(error) => {
                    self.publish(format!("\r\n[Terminal read error: {error}]\r\n").into_bytes());
                    break;
                }
            }
        }
        self.output_done.store(true, Ordering::SeqCst);
    }

    fn attach(&self) -> (Vec<u8>, broadcast::Receiver<Vec<u8>>) {
        let output = self.output.lock().unwrap();
        let mut replay = Vec::new();
        if output.dropped > 0 {
            replay.extend_from_slice(format!(
                "\r\n[Earlier terminal output omitted: {} bytes. Replaying the retained tail.]\r\n",
                output.dropped
            ).as_bytes());
        }
        replay.extend(output.bytes.iter());
        (replay, output.tx.subscribe())
    }

    fn close(&self) -> anyhow::Result<()> {
        let mut child = self.child.lock().unwrap();
        if child.try_wait()?.is_none() {
            if let Some(pid) = child.process_id() {
                let foreground = self.master.lock().unwrap().process_group_leader();
                kill_session_groups(pid as i32, foreground);
            }
            child.kill()?;
            child.wait()?;
        }
        self.running.store(false, Ordering::SeqCst);
        self.closed.store(true, Ordering::SeqCst);
        Ok(())
    }
}

// portable-pty creates a new Unix session for this child. Keep its unreaped
// handle locked while identifying groups; deliberately detached sessions still
// require container cleanup.
fn kill_session_groups(pid: i32, foreground: Option<i32>) {
    let mut groups = BTreeSet::new();
    if let Some(group) = foreground.filter(|group| *group > 1 && *group != pid) {
        groups.insert(group);
    }
    #[cfg(target_os = "linux")]
    if let Ok(entries) = std::fs::read_dir("/proc") {
        for entry in entries.flatten() {
            let Ok(stat) = std::fs::read_to_string(entry.path().join("stat")) else {
                continue;
            };
            let Some((_, fields)) = stat.rsplit_once(") ") else {
                continue;
            };
            let fields: Vec<_> = fields.split_whitespace().take(4).collect();
            if fields.len() == 4 && fields[3].parse::<i32>() == Ok(pid) {
                if let Ok(group) = fields[2].parse::<i32>() {
                    if group > 1 && group != pid {
                        groups.insert(group);
                    }
                }
            }
        }
    }
    for group in groups {
        unsafe { libc::kill(-group, libc::SIGKILL) };
    }
    if pid > 1 {
        unsafe { libc::kill(-pid, libc::SIGKILL) };
    }
}

fn size(cols: u16, rows: u16) -> Result<PtySize, Error> {
    if !(1..=500).contains(&cols) || !(1..=500).contains(&rows) {
        return Err((
            StatusCode::BAD_REQUEST,
            "Terminal dimensions must be between 1 and 500.".into(),
        ));
    }
    Ok(PtySize {
        cols,
        rows,
        pixel_width: 0,
        pixel_height: 0,
    })
}

async fn list(State(hub): State<TerminalHub>) -> Json<Vec<Summary>> {
    Json(
        hub.0
            .terminals
            .lock()
            .unwrap()
            .values()
            .map(|t| t.summary())
            .collect(),
    )
}

async fn create(
    State(hub): State<TerminalHub>,
    Json(request): Json<Create>,
) -> Result<Json<Summary>, Error> {
    let size = size(request.cols.unwrap_or(80), request.rows.unwrap_or(24))?;
    let cwd = hub.0.cwd.join(request.cwd.unwrap_or_default());
    let cwd = cwd
        .canonicalize()
        .map_err(|error| (StatusCode::BAD_REQUEST, error.to_string()))?;
    if !cwd.is_dir() {
        return Err((
            StatusCode::BAD_REQUEST,
            "Terminal working directory must be a directory.".into(),
        ));
    }
    let id = format!("term-{}", uuid::Uuid::new_v4());
    let terminal = tokio::task::spawn_blocking(move || Terminal::open(id, cwd, size))
        .await
        .map_err(internal)?
        .map_err(internal)?;
    {
        let mut terminals = hub.0.terminals.lock().unwrap();
        if hub.0.closing.load(Ordering::SeqCst) {
            drop(terminals);
            terminal.close().map_err(internal)?;
            return Err((
                StatusCode::SERVICE_UNAVAILABLE,
                "Server is shutting down.".into(),
            ));
        }
        terminals.insert(terminal.id.clone(), terminal.clone());
    }
    let monitoring = terminal.clone();
    tokio::spawn(async move {
        while monitoring.running.load(Ordering::SeqCst) {
            let outcome = {
                let mut child = monitoring.child.lock().unwrap();
                match child.try_wait() {
                    Ok(Some(status)) => {
                        monitoring.running.store(false, Ordering::SeqCst);
                        Some(format!("\r\n[Shell exited: {status}]\r\n"))
                    }
                    Err(error) => Some(format!("\r\n[Shell status error: {error}]\r\n")),
                    Ok(None) => None,
                }
            };
            if let Some(message) = outcome {
                monitoring.publish(message.into_bytes());
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    });
    Ok(Json(terminal.summary()))
}

async fn close(
    State(hub): State<TerminalHub>,
    Path(id): Path<String>,
) -> Result<StatusCode, Error> {
    let terminal = hub.find(&id)?;
    tokio::task::spawn_blocking(move || terminal.close())
        .await
        .map_err(internal)?
        .map_err(internal)?;
    hub.0.terminals.lock().unwrap().remove(&id);
    Ok(StatusCode::NO_CONTENT)
}

async fn connect(
    State(hub): State<TerminalHub>,
    Path(id): Path<String>,
    ws: WebSocketUpgrade,
) -> Result<Response, Error> {
    let terminal = hub.find(&id)?;
    Ok(ws
        .max_message_size(64 * 1024)
        .on_upgrade(move |socket| connected(terminal, socket)))
}

async fn connected(terminal: Arc<Terminal>, mut socket: WebSocket) {
    let (replay, mut output) = terminal.attach();
    if socket.send(Message::Binary(replay.into())).await.is_err() {
        return;
    }
    let mut interval = tokio::time::interval(Duration::from_millis(200));
    loop {
        tokio::select! {
            result = output.recv() => match result {
                Ok(bytes) => if socket.send(Message::Binary(bytes.into())).await.is_err() { break },
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    let (replay, receiver) = terminal.attach();
                    output = receiver;
                    let mut reset = b"\x1bc\r\n[Display fell behind; replaying retained terminal output.]\r\n".to_vec();
                    reset.extend(replay);
                    if socket.send(Message::Binary(reset.into())).await.is_err() { break }
                }
                Err(_) => break,
            },
            incoming = socket.recv() => {
                let Some(Ok(message)) = incoming else { break };
                let Message::Text(text) = message else {
                    if matches!(message, Message::Close(_)) { break }
                    continue;
                };
                let result = match serde_json::from_str::<Input>(&text) {
                    Ok(Input::Input { data }) => {
                        let writing = terminal.clone();
                        tokio::task::spawn_blocking(move || {
                            let mut writer = writing.writer.lock().unwrap();
                            writer.write_all(data.as_bytes()).and_then(|_| writer.flush())
                        }).await.map_err(|error| error.to_string())
                            .and_then(|result| result.map_err(|error| error.to_string()))
                    }
                    Ok(Input::Resize { cols, rows }) => match size(cols, rows) {
                        Ok(size) => terminal.master.lock().unwrap().resize(size).map_err(|error| error.to_string()),
                        Err((_, message)) => Err(message),
                    },
                    Err(error) => Err(format!("Invalid terminal input: {error}")),
                };
                if let Err(error) = result {
                    let message = format!("\r\n[Terminal error: {error}]\r\n");
                    if socket.send(Message::Binary(message.into_bytes().into())).await.is_err() { break }
                }
            },
            _ = interval.tick() => {
                if !terminal.running.load(Ordering::SeqCst)
                    && terminal.output_done.load(Ordering::SeqCst)
                    && output.is_empty()
                {
                    let _ = socket.send(Message::Close(None)).await;
                    break;
                }
            }
        }
    }
}

fn internal(error: impl std::fmt::Display) -> Error {
    (StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
}
