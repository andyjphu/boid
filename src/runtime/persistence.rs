//! Bounded background recording. Execution publishes memory state without waiting for disk.
use super::*;
#[cfg(test)]
use std::sync::mpsc;
use std::sync::{Condvar, Weak};

const CAPACITY: usize = 16;
const ARCHIVE_CAPACITY: usize = 16;
pub(super) const DELTA_BYTES: usize = 64 * 1024;
// Allow for JSON escaping, field names and the largest sequence number.
pub(super) fn delta_bytes(entry: &str, text: &str) -> usize {
    entry
        .len()
        .saturating_add(text.len())
        .saturating_mul(6)
        .saturating_add(96)
}
type SkillSnapshot = (crate::skills::Skills, Option<String>);

pub(super) enum Operation {
    Snapshot(Box<Record>),
    Delta(Delta),
    Context(u64, Box<Record>),
    Clear(String, Box<Record>),
    #[cfg(test)]
    Gate(mpsc::Sender<()>, mpsc::Receiver<()>),
}
impl Operation {
    fn is_archive(&self) -> bool {
        matches!(self, Self::Context(..) | Self::Clear(..))
    }
    fn retained(&self) -> bool {
        match self {
            Self::Context(..) | Self::Clear(..) => true,
            #[cfg(test)]
            Self::Gate(..) => true,
            _ => false,
        }
    }
    fn delta_bytes(&self) -> usize {
        match self {
            Self::Delta(delta) => delta_bytes(&delta.entry, &delta.text),
            _ => 0,
        }
    }
}

#[derive(Default)]
struct Progress {
    completed: u64,
    error: Option<String>,
    // Includes queued operations and the one currently being written.
    used: usize,
    archives: usize,
    delta_bytes: usize,
    running: bool,
    queue: VecDeque<(u64, Operation)>,
    // Keep pre-clear/compaction contexts recoverable in memory if recording
    // fails after execution has already advanced. Admission is then stopped.
    failed_archive: Option<Operation>,
    skills: Option<SkillSnapshot>,
    durable: Option<Record>,
    journal_bytes: u64,
    journal_events: u64,
}
type Shared = Arc<(Mutex<Progress>, Condvar)>;

#[derive(Clone)]
pub(super) struct Receipt {
    revision: u64,
    progress: Shared,
}
impl Receipt {
    pub(super) fn wait(&self) -> Result<()> {
        let (lock, changed) = &*self.progress;
        let mut progress = lock.lock().unwrap();
        loop {
            if progress.completed >= self.revision {
                return Ok(());
            }
            if let Some(error) = &progress.error {
                bail!("Session persistence failed: {error}");
            }
            progress = changed.wait(progress).unwrap();
        }
    }
}

// Permits validate ownership and storage health, never wait for disk capacity.
// Latest complete states coalesce; retained context archives have a separate quota.
#[derive(Clone)]
pub(super) struct Capacity(Shared);
pub(super) struct Permit {
    progress: Shared,
    slots: usize,
    delta_bytes: usize,
}
impl Capacity {
    // Used only after admission has stopped for End/shutdown.
    pub(super) fn durable_record(&self) -> Result<Option<Record>> {
        let mut progress = self.0.0.lock().unwrap();
        while progress.running {
            progress = self.0.1.wait(progress).unwrap();
        }
        if let Some(error) = &progress.error {
            bail!("Session persistence failed: {error}");
        }
        Ok(progress.durable.clone())
    }

    pub(super) fn reserve(&self, slots: usize) -> Result<Permit> {
        self.reserve_bytes(slots, 0)
    }
    fn reserve_bytes(&self, slots: usize, bytes: usize) -> Result<Permit> {
        assert!((1..=CAPACITY).contains(&slots));
        let progress = self.0.0.lock().unwrap();
        if let Some(error) = &progress.error {
            bail!("Session persistence failed: {error}");
        }
        Ok(Permit {
            progress: self.0.clone(),
            slots,
            delta_bytes: bytes,
        })
    }
    fn try_reserve(&self, bytes: usize) -> Result<Permit> {
        self.reserve_bytes(1, bytes)
    }
}

pub(super) struct Lane {
    hub: Weak<Inner>,
    id: String,
    progress: Shared,
    revision: u64,
}
impl Lane {
    pub(super) fn new(hub: Weak<Inner>, id: String, durable: Option<Record>) -> Result<Self> {
        Ok(Self {
            hub,
            id,
            progress: Arc::new((
                Mutex::new(Progress {
                    durable,
                    ..Progress::default()
                }),
                Condvar::new(),
            )),
            revision: 0,
        })
    }
    pub(super) fn barrier(&self) -> Receipt {
        Receipt {
            revision: self.revision,
            progress: self.progress.clone(),
        }
    }
    pub(super) fn capacity(&self) -> Capacity {
        Capacity(self.progress.clone())
    }
    pub(super) fn delta_needs_snapshot(&self, bytes: usize) -> bool {
        let progress = self.progress.0.lock().unwrap();
        progress.used >= CAPACITY
            || bytes > DELTA_BYTES
            || progress.delta_bytes.saturating_add(bytes) > DELTA_BYTES
    }
    pub(super) fn enqueue(&mut self, operation: Operation) -> Result<Receipt> {
        let mut permit = self.capacity().try_reserve(operation.delta_bytes())?;
        self.enqueue_reserved(operation, &mut permit)
    }
    pub(super) fn enqueue_reserved(
        &mut self,
        operation: Operation,
        permit: &mut Permit,
    ) -> Result<Receipt> {
        assert!(Arc::ptr_eq(&permit.progress, &self.progress) && permit.slots > 0);
        let bytes = operation.delta_bytes();
        assert_eq!(permit.delta_bytes, bytes);
        let mut progress = self.progress.0.lock().unwrap();
        if let Some(error) = &progress.error {
            bail!("Session persistence failed: {error}");
        }
        let retained = operation.retained();
        if retained && progress.archives >= ARCHIVE_CAPACITY {
            bail!("Context recording is catching up; retry this archive request shortly.");
        }
        // Only the final state in this recording segment is needed. Its full
        // transcript preserves every entry, while FIFO archives remain boundaries.
        if matches!(&operation, Operation::Snapshot(_)) {
            while progress
                .queue
                .back()
                .is_some_and(|(_, queued)| !queued.retained())
            {
                let (_, superseded) = progress.queue.pop_back().unwrap();
                progress.used -= 1;
                progress.delta_bytes -= superseded.delta_bytes();
            }
        }
        self.revision += 1;
        progress.used += 1;
        progress.delta_bytes += bytes;
        if retained {
            progress.archives += 1;
        }
        permit.slots -= 1;
        permit.delta_bytes = 0;
        progress.queue.push_back((self.revision, operation));
        if !progress.running {
            progress.running = true;
            let hub = self.hub.clone();
            let id = self.id.clone();
            let shared = self.progress.clone();
            // Receipt/capacity waiters can occupy Tokio's blocking pool. Keep
            // disk progress independent of that pool, and retain no idle thread.
            if let Err(error) = std::thread::Builder::new()
                .name(format!("save-{}", self.id))
                .spawn(move || drain(hub, id, shared))
            {
                // No operation was admitted or published: restore its permit.
                progress.queue.pop_back();
                progress.used -= 1;
                progress.delta_bytes -= bytes;
                if retained {
                    progress.archives -= 1;
                }
                progress.running = false;
                self.revision -= 1;
                permit.slots += 1;
                permit.delta_bytes = bytes;
                return Err(error.into());
            }
        }
        Ok(Receipt {
            revision: self.revision,
            progress: self.progress.clone(),
        })
    }
    #[cfg(test)]
    pub(super) fn queued_recording(&self) -> (usize, usize) {
        let progress = self.progress.0.lock().unwrap();
        (progress.queue.len(), progress.delta_bytes)
    }
    #[cfg(test)]
    pub(super) fn retained_archive_records(&self) -> Vec<Record> {
        let progress = self.progress.0.lock().unwrap();
        progress
            .failed_archive
            .iter()
            .chain(progress.queue.iter().map(|(_, operation)| operation))
            .filter_map(|operation| match operation {
                Operation::Context(_, record) | Operation::Clear(_, record) => {
                    Some((**record).clone())
                }
                _ => None,
            })
            .collect()
    }
    #[cfg(test)]
    pub(super) fn gate(&mut self) -> (mpsc::Receiver<()>, mpsc::Sender<()>) {
        let (started, ready) = mpsc::channel();
        let (release, wait) = mpsc::channel();
        self.enqueue(Operation::Gate(started, wait)).unwrap();
        (ready, release)
    }
    #[cfg(test)]
    pub(super) fn is_running(&self) -> bool {
        self.progress.0.lock().unwrap().running
    }
}

fn drain(owner: Weak<Inner>, id: String, shared: Shared) {
    let (mut skills, mut durable, mut journal_bytes, mut journal_events) = {
        let mut state = shared.0.lock().unwrap();
        (
            state.skills.take(),
            state.durable.take(),
            state.journal_bytes,
            state.journal_events,
        )
    };
    loop {
        let (mut revision, operation) = {
            let mut progress = shared.0.lock().unwrap();
            let Some(next) = progress.queue.pop_front() else {
                progress.skills = skills;
                progress.durable = durable;
                progress.journal_bytes = journal_bytes;
                progress.journal_events = journal_events;
                progress.running = false;
                shared.1.notify_all();
                return;
            };
            next
        };
        let retained = operation.retained();
        let mut count = 1;
        let mut bytes = operation.delta_bytes();
        let hub = owner.upgrade().map(Hub);
        let mut failed_archive = None;
        let result = match &hub {
            Some(hub) => match operation {
                Operation::Snapshot(mut record) => {
                    if record.engine.skill_snapshot.is_none() {
                        if let Some((previous, snapshot)) = &skills {
                            if previous == &record.engine.skills {
                                record.engine.skill_snapshot = snapshot.clone();
                            }
                        }
                    }
                    let result = (|| -> Result<()> {
                        hub.prepare_record(&mut record)?;
                        for message in &mut record.messages {
                            message.persisted = true;
                        }
                        if let Some(previous) = &durable {
                            record.journal_seq = previous
                                .journal_seq
                                .checked_add(1)
                                .context("Journal sequence overflow")?;
                            let event = journal::Event::checkpoint(previous, &mut record)?;
                            journal_bytes += hub.0.store.append_events(&id, &[event])?;
                            journal_events += 1;
                        } else {
                            // Durable version fence precedes the first append.
                            hub.save_record(&id, &mut record)?;
                        }
                        hub.mark_durable(&id, &record);
                        skills = Some((
                            record.engine.skills.clone(),
                            record.engine.skill_snapshot.clone(),
                        ));
                        durable = Some(*record);
                        Ok(())
                    })();
                    result
                }
                Operation::Delta(delta) => {
                    let mut deltas = vec![delta];
                    {
                        let mut progress = shared.0.lock().unwrap();
                        // Only adjacent deltas can share a durable append;
                        // checkpoints retain their chronological position.
                        while deltas.len() < CAPACITY {
                            let Some((_, Operation::Delta(next))) = progress.queue.front() else {
                                break;
                            };
                            let next_bytes = delta_bytes(&next.entry, &next.text);
                            if bytes.saturating_add(next_bytes) > DELTA_BYTES {
                                break;
                            }
                            let (next_revision, Operation::Delta(next)) =
                                progress.queue.pop_front().unwrap()
                            else {
                                unreachable!()
                            };
                            revision = next_revision;
                            bytes += next_bytes;
                            count += 1;
                            deltas.push(next);
                        }
                    }
                    (|| -> Result<()> {
                        let record = durable
                            .as_mut()
                            .context("Stream before initial checkpoint")?;
                        let mut sequence = record.journal_seq;
                        let mut events = Vec::with_capacity(deltas.len());
                        for delta in deltas {
                            let base = sequence;
                            sequence = sequence
                                .checked_add(1)
                                .context("Journal sequence overflow")?;
                            events.push(journal::Event {
                                seq: sequence,
                                base,
                                change: journal::Change::Delta { delta },
                            });
                        }
                        journal_bytes += hub.0.store.append_events(&id, &events)?;
                        journal_events += events.len() as u64;
                        for event in events {
                            event.apply(record)?;
                        }
                        Ok(())
                    })()
                }
                Operation::Context(attempt, mut record) => {
                    let result = hub
                        .prepare_record(&mut record)
                        .and_then(|()| hub.0.store.archive_context(&id, attempt, &record));
                    if result.is_err() {
                        failed_archive = Some(Operation::Context(attempt, record));
                    }
                    result
                }
                Operation::Clear(command, mut record) => {
                    let result = hub
                        .prepare_record(&mut record)
                        .and_then(|()| hub.0.store.archive_clear(&id, &command, &record));
                    if result.is_err() {
                        failed_archive = Some(Operation::Clear(command, record));
                    }
                    result
                }
                #[cfg(test)]
                Operation::Gate(started, release) => {
                    let _ = started.send(());
                    release.recv().map_err(Into::into)
                }
            },
            None => Err(anyhow::anyhow!(
                "Session owner closed before persistence completed."
            )),
        };
        if let Err(error) = result {
            let message = format!("{error:#}");
            if let Some(hub) = &hub {
                hub.fault(&id, error);
            }
            drop(hub);
            let mut progress = shared.0.lock().unwrap();
            progress.error = Some(message);
            progress.failed_archive = failed_archive;
            progress
                .queue
                .retain(|(_, operation)| operation.is_archive());
            progress.used = progress.queue.len() + usize::from(progress.failed_archive.is_some());
            progress.archives = progress.used;
            progress.delta_bytes = 0;
            progress.running = false;
            shared.1.notify_all();
            return;
        }
        // Optional full snapshots never occupy this ordered writer. Rotation
        // only occurs when the single server-wide compactor is free.
        if journal_bytes >= 8 * 1024 * 1024 || journal_events >= 256 {
            if let (Some(hub), Some(record)) = (&hub, &durable) {
                match journal::compact(hub, &id, record) {
                    Ok(true) => {
                        journal_bytes = 0;
                        journal_events = 0;
                    }
                    Ok(false) => {}
                    Err(error) => eprintln!("Cannot compact session; journal retained: {error:#}"),
                }
            }
        }
        // Release the state-directory owner before signaling completion.
        drop(hub);
        let mut progress = shared.0.lock().unwrap();
        progress.completed = revision;
        progress.used -= count;
        if retained {
            progress.archives -= 1;
        }
        progress.delta_bytes -= bytes;
        shared.1.notify_all();
    }
}
