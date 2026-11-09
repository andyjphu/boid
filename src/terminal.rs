use crate::runtime::Hub as Catalog;
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
    fs::{self, File, OpenOptions},
    io::{BufRead, BufReader, Read, Write},
    os::unix::fs::OpenOptionsExt,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
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
    next_number: AtomicU64,
    terminals: Mutex<BTreeMap<String, Arc<Terminal>>>,
    state_dir: PathBuf,
    catalog: Catalog,
}

struct Terminal {
    id: String,
    name: Mutex<String>,
    cwd: Mutex<String>,
    cwd_observed: AtomicBool,
    running: AtomicBool,
    output_done: AtomicBool,
    closed: AtomicBool,
    live: Mutex<Option<Arc<LivePty>>>,
    output: Mutex<Output>,
    status: Mutex<String>,
    archived: AtomicBool,
    journal: Mutex<Journal>,
    catalog: Catalog,
    persistence_error: Mutex<Option<String>>,
}

struct LivePty {
    master: Mutex<Box<dyn MasterPty + Send>>,
    writer: Mutex<Box<dyn Write + Send>>,
    child: Mutex<Box<dyn Child + Send + Sync>>,
}

struct Output {
    history: VecDeque<TerminalEvent>,
    bytes: usize,
    dropped: usize,
    initial_size: PtySize,
    controller: Option<String>,
    tx: broadcast::Sender<TerminalEvent>,
}

#[derive(Clone)]
enum TerminalEvent {
    Data(Vec<u8>),
    Resize(PtySize),
    Control(String),
    Name(String),
}

impl TerminalEvent {
    fn cost(&self) -> usize {
        match self {
            Self::Data(bytes) => bytes.len(),
            _ => 32,
        }
    }

    fn message(self) -> Message {
        match self {
            Self::Data(bytes) => Message::Binary(bytes.into()),
            Self::Resize(size) => Message::Text(
                serde_json::json!({"type":"resize", "cols":size.cols, "rows":size.rows})
                    .to_string()
                    .into(),
            ),
            Self::Control(id) => Message::Text(
                serde_json::json!({"type":"control", "id":id})
                    .to_string()
                    .into(),
            ),
            Self::Name(name) => Message::Text(
                serde_json::json!({"type":"name", "name":name})
                    .to_string()
                    .into(),
            ),
        }
    }
}

fn marker(kind: &str) -> Message {
    Message::Text(serde_json::json!({"type":kind}).to_string().into())
}

impl Output {
    fn publish(&mut self, event: TerminalEvent) {
        self.bytes += event.cost();
        self.history.push_back(event.clone());
        while self.bytes > REPLAY_BYTES {
            let Some(old) = self.history.pop_front() else {
                break;
            };
            self.bytes -= old.cost();
            match old {
                TerminalEvent::Resize(size) => self.initial_size = size,
                TerminalEvent::Data(bytes) => self.dropped += bytes.len(),
                TerminalEvent::Control(_) | TerminalEvent::Name(_) => {}
            }
        }
        let _ = self.tx.send(event);
    }
}

#[derive(Clone, Serialize, Deserialize)]
struct Summary {
    id: String,
    #[serde(default)]
    name: String,
    cwd: String,
    #[serde(default)]
    cwd_observed: bool,
    running: bool,
    status: String,
    archived: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    persistence_error: Option<String>,
}

#[derive(Serialize, Deserialize)]
enum StoredEvent {
    Data(Vec<u8>),
    Resize { cols: u16, rows: u16 },
}
impl From<&TerminalEvent> for StoredEvent {
    fn from(event: &TerminalEvent) -> Self {
        match event {
            TerminalEvent::Data(data) => Self::Data(data.clone()),
            TerminalEvent::Resize(size) => Self::Resize {
                cols: size.cols,
                rows: size.rows,
            },
            TerminalEvent::Control(_) | TerminalEvent::Name(_) => {
                unreachable!("View control is not session history")
            }
        }
    }
}
impl StoredEvent {
    fn into_event(self) -> TerminalEvent {
        match self {
            Self::Data(data) => TerminalEvent::Data(data),
            Self::Resize { cols, rows } => TerminalEvent::Resize(PtySize {
                cols,
                rows,
                pixel_width: 0,
                pixel_height: 0,
            }),
        }
    }
}
#[derive(Serialize, Deserialize)]
enum Record {
    Format {
        version: u32,
    },
    Meta(Summary),
    Replay {
        cols: u16,
        rows: u16,
        dropped: usize,
    },
    Event(StoredEvent),
}
struct Journal {
    path: PathBuf,
    file: File,
    bytes: u64,
    repair_required: bool,
}
impl Journal {
    fn open(path: PathBuf) -> std::io::Result<Self> {
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(&path)?;
        let bytes = file.metadata()?.len();
        Ok(Self {
            path,
            file,
            bytes,
            repair_required: false,
        })
    }
    fn append(&mut self, record: &Record) -> std::io::Result<()> {
        self.append_with(record, |file, bytes| file.write_all(bytes))
    }

    fn append_with(
        &mut self,
        record: &Record,
        write: impl FnOnce(&mut File, &[u8]) -> std::io::Result<()>,
    ) -> std::io::Result<()> {
        self.repair_tail()?;
        let mut bytes = serde_json::to_vec(record)?;
        bytes.push(b'\n');
        if let Err(error) = write(&mut self.file, &bytes) {
            // write_all can fail after writing a prefix. Never let a later
            // record turn that prefix into a malformed complete JSONL line.
            self.repair_required = true;
            return match self.repair_tail() {
                Ok(()) => Err(error),
                Err(rollback) => Err(std::io::Error::new(
                    error.kind(),
                    format!("{error}; incomplete terminal record rollback failed: {rollback}"),
                )),
            };
        }
        self.bytes += bytes.len() as u64;
        Ok(())
    }

    fn repair_tail(&mut self) -> std::io::Result<()> {
        if self.repair_required {
            self.file.set_len(self.bytes)?;
            self.file.sync_data()?;
            self.repair_required = false;
        }
        Ok(())
    }
    fn compact(&mut self, meta: &Summary, output: &Output) -> std::io::Result<()> {
        let mut temporary = tempfile::NamedTempFile::new_in(self.path.parent().unwrap())?;
        let mut bytes = Vec::new();
        for record in [
            Record::Format { version: 1 },
            Record::Meta(meta.clone()),
            Record::Replay {
                cols: output.initial_size.cols,
                rows: output.initial_size.rows,
                dropped: output.dropped,
            },
        ] {
            serde_json::to_writer(&mut bytes, &record)?;
            bytes.push(b'\n');
        }
        for event in &output.history {
            if matches!(event, TerminalEvent::Control(_) | TerminalEvent::Name(_)) {
                continue;
            }
            serde_json::to_writer(&mut bytes, &Record::Event(StoredEvent::from(event)))?;
            bytes.push(b'\n');
        }
        // Serialize in memory: JSON byte arrays otherwise cause a filesystem
        // write per number/delimiter, which stalls cross-VM bind mounts.
        temporary.write_all(&bytes)?;
        temporary.as_file().sync_data()?;
        temporary.persist(&self.path).map_err(|error| error.error)?;
        self.file = OpenOptions::new().append(true).open(&self.path)?;
        self.bytes = self.file.metadata()?.len();
        self.repair_required = false;
        Ok(())
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Create {
    cwd: Option<String>,
    cols: Option<u16>,
    rows: Option<u16>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Rename {
    name: String,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum Input {
    Input { data: String },
    Resize { cols: u16, rows: u16 },
    Control,
}

impl TerminalHub {
    pub fn open(cwd: PathBuf, state_dir: PathBuf, catalog: Catalog) -> anyhow::Result<Self> {
        let state_dir = state_dir.join("terminals");
        fs::create_dir_all(&state_dir)?;
        let mut terminals = BTreeMap::new();
        for entry in fs::read_dir(&state_dir)? {
            let path = entry?.path();
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            let terminal = Terminal::restore(path, catalog.clone())?;
            catalog.terminal_upsert(serde_json::to_value(terminal.summary())?);
            terminals.insert(terminal.id.clone(), terminal);
        }
        let next_number = terminals
            .values()
            .filter_map(|t| {
                t.name
                    .lock()
                    .unwrap()
                    .strip_prefix("#")
                    .and_then(|n| n.parse::<u64>().ok())
            })
            .max()
            .unwrap_or(0)
            .max(terminals.len() as u64)
            .saturating_add(1);
        Ok(Self(Arc::new(Hub {
            cwd,
            closing: AtomicBool::new(false),
            next_number: AtomicU64::new(next_number),
            terminals: Mutex::new(terminals),
            state_dir,
            catalog,
        })))
    }

    pub fn router(&self) -> Router {
        Router::new()
            .route("/api/terminals", get(list).post(create))
            .route("/api/terminals/{id}", delete(close).patch(rename))
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
    fn open(
        id: String,
        name: String,
        cwd: PathBuf,
        size: PtySize,
        path: PathBuf,
        catalog: Catalog,
    ) -> anyhow::Result<Arc<Self>> {
        let pair = native_pty_system().openpty(size)?;
        let mut command = CommandBuilder::new_default_prog();
        command.cwd(&cwd);
        command.env("TERM", "xterm-256color");
        command.env_remove("OPENAI_API_KEY");
        command.env_remove("BOID_TOKEN");
        let reader = pair.master.try_clone_reader()?;
        let writer = pair.master.take_writer()?;
        let mut child = pair.slave.spawn_command(command)?;
        let journal = match Journal::open(path) {
            Ok(journal) => journal,
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error.into());
            }
        };
        drop(pair.slave);
        let terminal = Arc::new(Self {
            id,
            name: Mutex::new(name),
            cwd: Mutex::new(cwd.to_string_lossy().into_owned()),
            cwd_observed: AtomicBool::new(false),
            running: AtomicBool::new(true),
            output_done: AtomicBool::new(false),
            closed: AtomicBool::new(false),
            live: Mutex::new(Some(Arc::new(LivePty {
                master: Mutex::new(pair.master),
                writer: Mutex::new(writer),
                child: Mutex::new(child),
            }))),
            status: Mutex::new("running".into()),
            archived: AtomicBool::new(false),
            journal: Mutex::new(journal),
            catalog,
            persistence_error: Mutex::new(None),
            output: Mutex::new(Output {
                history: VecDeque::new(),
                bytes: 0,
                dropped: 0,
                initial_size: size,
                controller: None,
                tx: broadcast::channel(64).0,
            }),
        });
        if let Err(error) = terminal.checkpoint() {
            let _ = terminal.close();
            return Err(error);
        }
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
            name: self.name.lock().unwrap().clone(),
            cwd: self.cwd.lock().unwrap().clone(),
            cwd_observed: self.cwd_observed.load(Ordering::SeqCst),
            running: self.running.load(Ordering::SeqCst),
            status: self.status.lock().unwrap().clone(),
            archived: self.archived.load(Ordering::SeqCst),
            persistence_error: self.persistence_error.lock().unwrap().clone(),
        }
    }

    // The caller holds the unreaped child handle, so a reused PID cannot be
    // mistaken for this shell. Never infer cwd from terminal bytes or commands.
    fn observe_cwd(&self, pid: Option<u32>) {
        #[cfg(target_os = "linux")]
        let observed = pid.and_then(|pid| fs::read_link(format!("/proc/{pid}/cwd")).ok());
        #[cfg(not(target_os = "linux"))]
        let observed: Option<PathBuf> = {
            let _ = pid;
            None
        };
        let available = observed.is_some();
        let mut changed = self.cwd_observed.swap(available, Ordering::SeqCst) != available;
        if let Some(path) = observed {
            let path = path.to_string_lossy().into_owned();
            let mut cwd = self.cwd.lock().unwrap();
            if *cwd != path {
                *cwd = path;
                changed = true;
            }
        }
        if changed {
            if let Err(error) = self.checkpoint_metadata() {
                *self.persistence_error.lock().unwrap() = Some(error.to_string());
            }
            self.catalog
                .terminal_upsert(serde_json::to_value(self.summary()).unwrap());
        }
    }

    fn publish(&self, bytes: Vec<u8>) {
        self.publish_event(TerminalEvent::Data(bytes));
    }

    fn read(&self, mut reader: Box<dyn Read + Send>) {
        let mut bytes = [0; 8192];
        let Some(live) = self.live() else {
            return;
        };
        let descriptor = live.master.lock().unwrap().as_raw_fd();
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

    fn attach(
        &self,
    ) -> (
        Vec<TerminalEvent>,
        broadcast::Receiver<TerminalEvent>,
        String,
    ) {
        let output = self.output.lock().unwrap();
        let mut replay = vec![TerminalEvent::Resize(output.initial_size)];
        if output.dropped > 0 {
            replay.push(TerminalEvent::Data(format!(
                "\r\n[Earlier terminal output omitted: {} bytes. Replaying the retained tail.]\r\n", output.dropped
            ).into_bytes()));
        }
        for event in &output.history {
            match (replay.last_mut(), event) {
                (Some(TerminalEvent::Data(buffer)), TerminalEvent::Data(bytes)) => {
                    buffer.extend(bytes)
                }
                _ => replay.push(event.clone()),
            }
        }
        (
            replay,
            output.tx.subscribe(),
            output.controller.clone().unwrap_or_default(),
        )
    }

    fn release_control(&self, client_id: &str) {
        let mut output = self.output.lock().unwrap();
        if output.controller.as_deref() == Some(client_id) {
            output.controller = None;
            let _ = output.tx.send(TerminalEvent::Control(String::new()));
        }
    }

    fn live(&self) -> Option<Arc<LivePty>> {
        self.live.lock().unwrap().clone()
    }

    fn close(&self) -> anyhow::Result<()> {
        let was_running = self.running.load(Ordering::SeqCst);
        if let Some(live) = self.live() {
            let mut child = live.child.lock().unwrap();
            if child.try_wait()?.is_none() {
                self.observe_cwd(child.process_id());
                if let Some(pid) = child.process_id() {
                    let foreground = live.master.lock().unwrap().process_group_leader();
                    kill_session_groups(pid as i32, foreground);
                }
                child.kill()?;
                child.wait()?;
            }
        }
        self.running.store(false, Ordering::SeqCst);
        self.closed.store(true, Ordering::SeqCst);
        self.live.lock().unwrap().take();
        if was_running {
            *self.status.lock().unwrap() = "ended".into();
        }
        self.checkpoint()?;
        self.catalog
            .terminal_upsert(serde_json::to_value(self.summary())?);
        Ok(())
    }

    fn checkpoint_metadata(&self) -> anyhow::Result<()> {
        // Metadata does not rewrite replay or hold the PTY output lock. Read the
        // current summary under the journal lock so concurrent updates cannot
        // append an older summary after a newer checkpoint.
        let mut journal = self.journal.lock().unwrap();
        journal.append(&Record::Meta(self.summary()))?;
        journal.file.sync_data()?;
        Ok(())
    }

    fn checkpoint(&self) -> anyhow::Result<()> {
        let output = self.output.lock().unwrap();
        self.journal
            .lock()
            .unwrap()
            .compact(&self.summary(), &output)?;
        Ok(())
    }

    fn publish_event(&self, event: TerminalEvent) {
        let mut output = self.output.lock().unwrap();
        let result = {
            let mut journal = self.journal.lock().unwrap();
            let saved = journal.append(&Record::Event(StoredEvent::from(&event)));
            output.publish(event);
            saved.and_then(|_| {
                if journal.bytes > REPLAY_BYTES as u64 * 4 {
                    journal.compact(&self.summary(), &output)?;
                }
                Ok(())
            })
        };
        drop(output);
        if let Err(error) = result {
            *self.persistence_error.lock().unwrap() = Some(error.to_string());
            self.catalog
                .terminal_upsert(serde_json::to_value(self.summary()).unwrap());
        }
    }

    fn restore(path: PathBuf, catalog: Catalog) -> anyhow::Result<Arc<Self>> {
        let mut meta = None;
        let mut version = None;
        let mut output = Output {
            history: VecDeque::new(),
            bytes: 0,
            dropped: 0,
            initial_size: PtySize {
                cols: 80,
                rows: 24,
                pixel_width: 0,
                pixel_height: 0,
            },
            controller: None,
            tx: broadcast::channel(64).0,
        };
        let mut reader = BufReader::new(File::open(&path)?);
        let mut line = Vec::new();
        loop {
            line.clear();
            if reader.read_until(b'\n', &mut line)? == 0 {
                break;
            }
            if !line.ends_with(b"\n") {
                break;
            } // An interrupted append is not a complete record.
            match serde_json::from_slice::<Record>(&line)? {
                Record::Format { version: found } => version = Some(found),
                Record::Meta(summary) => meta = Some(summary),
                Record::Replay {
                    cols,
                    rows,
                    dropped,
                } => {
                    output.initial_size = PtySize {
                        cols,
                        rows,
                        pixel_width: 0,
                        pixel_height: 0,
                    };
                    output.dropped = dropped;
                }
                Record::Event(event) => output.publish(event.into_event()),
            }
        }
        if version != Some(1) {
            anyhow::bail!(
                "Unsupported terminal record version {:?}: {}",
                version,
                path.display()
            );
        }
        let mut meta = meta.ok_or_else(|| {
            anyhow::anyhow!("Terminal record has no metadata: {}", path.display())
        })?;
        if meta.running {
            meta.status = "interrupted".into();
        }
        let terminal = Arc::new(Self {
            name: Mutex::new(if meta.name.is_empty() {
                format!("Terminal {}", meta.id.trim_start_matches("term-"))
            } else {
                meta.name
            }),
            id: meta.id,
            cwd: Mutex::new(meta.cwd),
            cwd_observed: AtomicBool::new(meta.cwd_observed),
            running: AtomicBool::new(false),
            output_done: AtomicBool::new(true),
            closed: AtomicBool::new(true),
            live: Mutex::new(None),
            output: Mutex::new(output),
            status: Mutex::new(meta.status),
            archived: AtomicBool::new(meta.archived),
            journal: Mutex::new(Journal::open(path)?),
            catalog,
            persistence_error: Mutex::new(meta.persistence_error),
        });
        terminal.checkpoint()?;
        Ok(terminal)
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
    let name = format!("#{}", hub.0.next_number.fetch_add(1, Ordering::SeqCst));
    let path = hub.0.state_dir.join(format!("{id}.jsonl"));
    let catalog = hub.0.catalog.clone();
    let terminal =
        tokio::task::spawn_blocking(move || Terminal::open(id, name, cwd, size, path, catalog))
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
    hub.0
        .catalog
        .terminal_upsert(serde_json::to_value(terminal.summary()).map_err(internal)?);
    let monitoring = terminal.clone();
    tokio::spawn(async move {
        while monitoring.running.load(Ordering::SeqCst) {
            let polling = monitoring.clone();
            let poll = tokio::task::spawn_blocking(move || {
                let monitoring = polling;
                let outcome = {
                    let Some(live) = monitoring.live() else {
                        return;
                    };
                    let mut child = live.child.lock().unwrap();
                    match child.try_wait() {
                        Ok(Some(status)) => {
                            monitoring.running.store(false, Ordering::SeqCst);
                            Some(format!("\r\n[Shell exited: {status}]\r\n"))
                        }
                        Err(error) => Some(format!("\r\n[Shell status error: {error}]\r\n")),
                        Ok(None) => {
                            monitoring.observe_cwd(child.process_id());
                            None
                        }
                    }
                };
                if let Some(message) = outcome {
                    *monitoring.status.lock().unwrap() = "exited".into();
                    monitoring.running.store(false, Ordering::SeqCst);
                    monitoring.live.lock().unwrap().take();
                    monitoring.publish(message.into_bytes());
                    if let Err(error) = monitoring.checkpoint() {
                        *monitoring.persistence_error.lock().unwrap() = Some(error.to_string());
                    }
                    monitoring
                        .catalog
                        .terminal_upsert(serde_json::to_value(monitoring.summary()).unwrap());
                }
            })
            .await;
            if poll.is_err() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    });
    Ok(Json(terminal.summary()))
}

async fn rename(
    State(hub): State<TerminalHub>,
    Path(id): Path<String>,
    Json(request): Json<Rename>,
) -> Result<Json<Summary>, Error> {
    let name = request.name.trim();
    if name.is_empty() || name.chars().count() > 128 || name.chars().any(char::is_control) {
        return Err((
            StatusCode::BAD_REQUEST,
            "Terminal name must contain 1 to 128 characters without control characters.".into(),
        ));
    }
    let terminal = hub.find(&id)?;
    *terminal.name.lock().unwrap() = name.to_owned();
    let saving = terminal.clone();
    tokio::task::spawn_blocking(move || saving.checkpoint_metadata())
        .await
        .map_err(internal)?
        .map_err(internal)?;
    let summary = terminal.summary();
    terminal
        .catalog
        .terminal_upsert(serde_json::to_value(&summary).map_err(internal)?);
    let _ = terminal
        .output
        .lock()
        .unwrap()
        .tx
        .send(TerminalEvent::Name(name.to_owned()));
    Ok(Json(summary))
}

async fn close(
    State(hub): State<TerminalHub>,
    Path(id): Path<String>,
) -> Result<StatusCode, Error> {
    let terminal = hub.find(&id)?;
    tokio::task::spawn_blocking(move || {
        terminal.close()?;
        terminal.archived.store(true, Ordering::SeqCst);
        terminal.checkpoint()?;
        terminal
            .catalog
            .terminal_upsert(serde_json::to_value(terminal.summary())?);
        Ok::<_, anyhow::Error>(())
    })
    .await
    .map_err(internal)?
    .map_err(internal)?;
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
    let client_id = uuid::Uuid::new_v4().to_string();
    let hello = Message::Text(
        serde_json::json!({"type":"hello", "id":client_id})
            .to_string()
            .into(),
    );
    if socket.send(hello).await.is_err() {
        terminal.release_control(&client_id);
        return;
    }
    let (replay, mut output, controller) = terminal.attach();
    let name = terminal.name.lock().unwrap().clone();
    if socket
        .send(TerminalEvent::Name(name).message())
        .await
        .is_err()
    {
        terminal.release_control(&client_id);
        return;
    }
    if replay_to(&mut socket, replay, controller).await.is_err() {
        terminal.release_control(&client_id);
        return;
    }
    let mut interval = tokio::time::interval(Duration::from_millis(200));
    loop {
        tokio::select! {
            result = output.recv() => match result {
                Ok(event) => if socket.send(event.message()).await.is_err() { break },
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    let (replay, receiver, controller) = terminal.attach();
                    output = receiver;
                    if replay_to(&mut socket, replay, controller).await.is_err() { break }
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
                        if terminal.output.lock().unwrap().controller.as_ref() != Some(&client_id) { continue; }
                        if !terminal.running.load(Ordering::SeqCst) { continue; }
                        let writing = terminal.clone();
                        tokio::task::spawn_blocking(move || {
                            let live = writing.live().ok_or_else(|| std::io::Error::new(std::io::ErrorKind::BrokenPipe, "Terminal has ended."))?;
                            let mut writer = live.writer.lock().unwrap();
                            writer.write_all(data.as_bytes()).and_then(|_| writer.flush())
                        }).await.map_err(|error| error.to_string())
                            .and_then(|result| result.map_err(|error| error.to_string()))
                    }
                    Ok(Input::Resize { cols, rows }) => match size(cols, rows) {
                        Ok(size) => {
                            let output = terminal.output.lock().unwrap();
                            if output.controller.as_ref() != Some(&client_id) { continue; }
                            if !terminal.running.load(Ordering::SeqCst) { continue; }
                            let Some(live) = terminal.live() else { continue; };
                            let master = live.master.lock().unwrap();
                            match master.get_size().and_then(|current| {
                                if current.cols == size.cols && current.rows == size.rows { return Ok(false); }
                                master.resize(size)?;
                                Ok(true)
                            }) {
                                Ok(true) => {
                                    drop(master);
                                    drop(output);
                                    terminal.publish_event(TerminalEvent::Resize(size));
                                    Ok(())
                                },
                                Ok(false) => Ok(()),
                                Err(error) => Err(error.to_string()),
                            }
                        },
                        Err((_, message)) => Err(message),
                    },
                    Ok(Input::Control) => {
                        if !terminal.running.load(Ordering::SeqCst) { continue; }
                        let mut output = terminal.output.lock().unwrap();
                        output.controller = Some(client_id.clone());
                        let _ = output.tx.send(TerminalEvent::Control(client_id.clone()));
                        Ok(())
                    }
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
    terminal.release_control(&client_id);
}

async fn replay_to(
    socket: &mut WebSocket,
    replay: Vec<TerminalEvent>,
    controller: String,
) -> Result<(), axum::Error> {
    socket.send(marker("replay_start")).await?;
    for event in replay {
        socket.send(event.message()).await?;
    }
    socket.send(marker("replay_end")).await?;
    socket
        .send(TerminalEvent::Control(controller).message())
        .await
}

fn internal(error: impl std::fmt::Display) -> Error {
    (StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
}

#[cfg(test)]
mod persistence_tests {
    use super::*;
    use crate::api::Client;

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn shell_directory_changes_publish_and_survive_end() {
        let repository = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let root = repository.path().canonicalize().unwrap();
        let destination = root.join("nested directory");
        fs::create_dir(&destination).unwrap();
        let catalog = Catalog::new(Client::test_client()).unwrap();
        let hub = TerminalHub::open(root.clone(), state.path().into(), catalog.clone()).unwrap();
        let (_, mut events) = catalog.subscribe();
        let Json(created) = create(
            State(hub.clone()),
            Json(Create {
                cwd: None,
                cols: None,
                rows: None,
            }),
        )
        .await
        .unwrap();
        assert_eq!(created.cwd, root.to_string_lossy());
        let terminal = hub.find(&created.id).unwrap();
        {
            let live = terminal.live().unwrap();
            let mut writer = live.writer.lock().unwrap();
            writer.write_all(b"cd 'nested directory'\r").unwrap();
            writer.flush().unwrap();
        }
        let changed = tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                let event = events.recv().await.unwrap();
                if event["type"] == "terminal"
                    && event["terminal"]["id"] == created.id
                    && event["terminal"]["cwd"] == destination.to_string_lossy().as_ref()
                    && event["terminal"]["cwd_observed"] == true
                {
                    break event;
                }
            }
        })
        .await
        .unwrap();
        assert_eq!(terminal.summary().cwd, destination.to_string_lossy());
        assert_eq!(changed["terminal"]["running"], true);
        let Json(renamed) = rename(
            State(hub.clone()),
            Path(created.id.clone()),
            Json(Rename {
                name: "Retained shell".into(),
            }),
        )
        .await
        .unwrap();
        assert_eq!(renamed.name, "Retained shell");
        // Recover a copy before End: cwd/name journal records must themselves
        // be durable, without relying on the full close checkpoint.
        let copy = state.path().join("metadata-recovery.jsonl");
        fs::copy(
            state
                .path()
                .join("terminals")
                .join(format!("{}.jsonl", created.id)),
            &copy,
        )
        .unwrap();
        let from_metadata = Terminal::restore(copy, catalog.clone()).unwrap();
        assert_eq!(from_metadata.summary().cwd, destination.to_string_lossy());
        assert_eq!(from_metadata.summary().name, "Retained shell");
        assert!(from_metadata.summary().cwd_observed);
        assert!(from_metadata.live().is_none());
        close(State(hub.clone()), Path(created.id.clone()))
            .await
            .unwrap();
        assert!(terminal.summary().archived);
        assert_eq!(terminal.summary().cwd, destination.to_string_lossy());
        let recovered = Terminal::restore(
            state
                .path()
                .join("terminals")
                .join(format!("{}.jsonl", created.id)),
            catalog,
        )
        .unwrap();
        assert_eq!(recovered.summary().cwd, destination.to_string_lossy());
        assert!(recovered.summary().cwd_observed);
        assert!(recovered.summary().archived);
        assert!(recovered.live().is_none());
        hub.shutdown().await.unwrap();
    }

    #[test]
    fn partial_append_keeps_later_records_recoverable() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("term-partial.jsonl");
        let mut journal = Journal::open(path.clone()).unwrap();
        let mut meta = Summary {
            id: "term-partial".into(),
            name: "Partial write fixture".into(),
            cwd: directory.path().display().to_string(),
            cwd_observed: false,
            running: false,
            status: "ended".into(),
            archived: true,
            persistence_error: None,
        };
        journal.append(&Record::Format { version: 1 }).unwrap();
        journal.append(&Record::Meta(meta.clone())).unwrap();
        journal
            .append(&Record::Event(StoredEvent::Data(b"before\n".to_vec())))
            .unwrap();
        let error = journal
            .append_with(
                &Record::Event(StoredEvent::Data(b"failed\n".to_vec())),
                |file, bytes| {
                    file.write_all(&bytes[..bytes.len() / 2])?;
                    Err(std::io::Error::other("injected partial write"))
                },
            )
            .unwrap_err();
        assert!(error.to_string().contains("injected partial write"));
        journal
            .append(&Record::Event(StoredEvent::Data(b"after\n".to_vec())))
            .unwrap();
        // The existing caller reports a failed append through retained metadata.
        meta.persistence_error = Some(error.to_string());
        journal.append(&Record::Meta(meta)).unwrap();
        journal.file.sync_data().unwrap();
        drop(journal);

        let catalog = Catalog::new(Client::test_client()).unwrap();
        let recovered = Terminal::restore(path.clone(), catalog.clone()).unwrap();
        assert!(
            recovered
                .summary()
                .persistence_error
                .as_deref()
                .unwrap()
                .contains("injected partial write")
        );
        let (events, _, _) = recovered.attach();
        let bytes: Vec<u8> = events
            .into_iter()
            .filter_map(|event| match event {
                TerminalEvent::Data(bytes) => Some(bytes),
                _ => None,
            })
            .flatten()
            .collect();
        assert_eq!(bytes, b"before\nafter\n");
        drop(recovered);

        // A malformed complete historical record remains an error, not a tail
        // that this repair is allowed to erase or silently skip.
        OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b"{malformed complete record}\n")
            .unwrap();
        let malformed = fs::read(&path).unwrap();
        assert!(Terminal::restore(path.clone(), catalog).is_err());
        assert_eq!(fs::read(path).unwrap(), malformed);
    }

    #[test]
    fn failed_tail_rollback_blocks_appends_until_repaired() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("term-rollback.jsonl");
        let mut journal = Journal::open(path.clone()).unwrap();
        journal.append(&Record::Format { version: 1 }).unwrap();
        let error = journal
            .append_with(
                &Record::Event(StoredEvent::Data(b"failed".to_vec())),
                |file, bytes| {
                    file.write_all(&bytes[..bytes.len() / 2])?;
                    // A read-only descriptor deterministically makes rollback
                    // fail without changing global limits or other tests.
                    *file = File::open(&path)?;
                    Err(std::io::Error::other("injected partial write"))
                },
            )
            .unwrap_err();
        assert!(error.to_string().contains("rollback failed"));
        assert!(journal.repair_required);
        let partial = fs::read(&path).unwrap();
        assert!(partial.len() as u64 > journal.bytes);
        assert!(
            journal
                .append_with(&Record::Format { version: 1 }, |_, _| {
                    panic!("A new append must not follow an unrepaired partial record")
                })
                .is_err()
        );
        assert_eq!(fs::read(&path).unwrap(), partial);

        journal.file = OpenOptions::new().append(true).open(&path).unwrap();
        journal
            .append(&Record::Event(StoredEvent::Data(b"after".to_vec())))
            .unwrap();
        assert!(!journal.repair_required);
        assert_eq!(journal.file.metadata().unwrap().len(), journal.bytes);
        let records: Vec<Record> = BufReader::new(File::open(&path).unwrap())
            .lines()
            .map(|line| serde_json::from_str(&line.unwrap()).unwrap())
            .collect();
        assert_eq!(records.len(), 2);
        assert!(
            matches!(&records[1], Record::Event(StoredEvent::Data(bytes)) if bytes == b"after")
        );
    }

    #[test]
    fn terminal_recovery_keeps_replay_and_never_restarts_a_shell() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("term-recovery.jsonl");
        let meta = Summary {
            id: "term-recovery".into(),
            name: "Research shell".into(),
            cwd: directory.path().display().to_string(),
            cwd_observed: false,
            running: true,
            status: "running".into(),
            archived: false,
            persistence_error: None,
        };
        let mut output = Output {
            history: VecDeque::new(),
            bytes: 0,
            dropped: 0,
            initial_size: PtySize {
                cols: 90,
                rows: 25,
                pixel_width: 0,
                pixel_height: 0,
            },
            controller: None,
            tx: broadcast::channel(64).0,
        };
        output.publish(TerminalEvent::Data(b"before\r\n".to_vec()));
        output.publish(TerminalEvent::Resize(PtySize {
            cols: 72,
            rows: 20,
            pixel_width: 0,
            pixel_height: 0,
        }));
        output.publish(TerminalEvent::Data(b"after\r\n".to_vec()));
        let mut journal = Journal::open(path.clone()).unwrap();
        journal.compact(&meta, &output).unwrap();
        journal.file.write_all(b"{incomplete append").unwrap();
        drop(journal);

        let catalog = Catalog::new(Client::test_client()).unwrap();
        let recovered = Terminal::restore(path.clone(), catalog.clone()).unwrap();
        assert!(recovered.live().is_none());
        assert!(!recovered.summary().running);
        assert_eq!(recovered.summary().status, "interrupted");
        let (events, _, controller) = recovered.attach();
        assert!(controller.is_empty());
        assert_eq!(recovered.summary().name, "Research shell");
        assert!(matches!(events[0], TerminalEvent::Resize(size) if size.cols == 90));
        assert!(
            events
                .iter()
                .any(|event| matches!(event, TerminalEvent::Resize(size) if size.cols == 72))
        );
        let bytes: Vec<u8> = events
            .into_iter()
            .filter_map(|event| match event {
                TerminalEvent::Data(data) => Some(data),
                _ => None,
            })
            .flatten()
            .collect();
        assert_eq!(bytes, b"before\r\nafter\r\n");

        recovered.archived.store(true, Ordering::SeqCst);
        recovered.close().unwrap();
        drop(recovered);
        let archived = Terminal::restore(path, catalog).unwrap();
        assert!(archived.summary().archived);
        assert_eq!(archived.summary().status, "interrupted");
        assert!(archived.live().is_none());
    }
}
