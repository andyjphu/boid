//! Ordered disk work. Never wait for this lane while holding the roster lock.
use super::*;
#[cfg(test)]
use std::sync::mpsc;
use std::sync::{Condvar, Weak};

const CAPACITY: usize = 16;
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
    // Includes reserved slots and the operation currently being written.
    used: usize,
    delta_bytes: usize,
    running: bool,
    queue: VecDeque<(u64, Operation)>,
    skills: Option<SkillSnapshot>,
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
    fn complete(&self) -> bool {
        self.progress.0.lock().unwrap().completed >= self.revision
    }
}

// A reservation carries no snapshot. Concurrent callers wait for capacity before
// cloning or mutating their session, and always outside the roster lock.
#[derive(Clone)]
pub(super) struct Capacity(Shared);
pub(super) struct Permit {
    progress: Shared,
    slots: usize,
    delta_bytes: usize,
}
impl Capacity {
    pub(super) fn reserve(&self, slots: usize) -> Result<Permit> {
        self.reserve_bytes(slots, 0)
    }
    pub(super) fn reserve_delta(&self, bytes: usize) -> Result<Permit> {
        self.reserve_bytes(1, bytes)
    }
    fn available(progress: &Progress, slots: usize, bytes: usize) -> bool {
        progress.used + slots <= CAPACITY
            && (bytes == 0
                || (bytes > DELTA_BYTES && progress.used == 0)
                || (bytes <= DELTA_BYTES && progress.delta_bytes <= DELTA_BYTES - bytes))
    }
    fn reserve_bytes(&self, slots: usize, bytes: usize) -> Result<Permit> {
        assert!((1..=CAPACITY).contains(&slots));
        let mut progress = self.0.0.lock().unwrap();
        loop {
            if let Some(error) = &progress.error {
                bail!("Session persistence failed: {error}");
            }
            if Self::available(&progress, slots, bytes) {
                progress.used += slots;
                progress.delta_bytes += bytes;
                return Ok(Permit {
                    progress: self.0.clone(),
                    slots,
                    delta_bytes: bytes,
                });
            }
            progress = self.0.1.wait(progress).unwrap();
        }
    }
    fn try_reserve(&self, bytes: usize) -> Result<Permit> {
        let mut progress = self.0.0.lock().unwrap();
        if let Some(error) = &progress.error {
            bail!("Session persistence failed: {error}");
        }
        if !Self::available(&progress, 1, bytes) {
            bail!("Session storage is catching up; retry shortly.");
        }
        progress.used += 1;
        progress.delta_bytes += bytes;
        Ok(Permit {
            progress: self.0.clone(),
            slots: 1,
            delta_bytes: bytes,
        })
    }
}
impl Drop for Permit {
    fn drop(&mut self) {
        if self.slots != 0 {
            let mut progress = self.progress.0.lock().unwrap();
            progress.used -= self.slots;
            progress.delta_bytes -= self.delta_bytes;
            drop(progress);
            self.progress.1.notify_all();
        }
    }
}

pub(super) struct Lane {
    hub: Weak<Inner>,
    id: String,
    progress: Shared,
    revision: u64,
    inputs: VecDeque<Receipt>,
}
impl Lane {
    pub(super) fn new(hub: Weak<Inner>, id: String) -> Result<Self> {
        Ok(Self {
            hub,
            id,
            progress: Arc::new((Mutex::new(Progress::default()), Condvar::new())),
            revision: 0,
            inputs: VecDeque::new(),
        })
    }
    pub(super) fn capacity(&self) -> Capacity {
        Capacity(self.progress.clone())
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
        self.revision += 1;
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
    pub(super) fn check_input_capacity(&mut self) -> Result<()> {
        while self.inputs.front().is_some_and(Receipt::complete) {
            self.inputs.pop_front();
        }
        if self.inputs.len() >= 3 {
            bail!("Three messages are awaiting storage; retry when saving catches up.");
        }
        Ok(())
    }
    pub(super) fn track_input(&mut self, receipt: Receipt) {
        self.inputs.push_back(receipt);
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
    let mut skills = shared.0.lock().unwrap().skills.take();
    loop {
        let (mut revision, operation) = {
            let mut progress = shared.0.lock().unwrap();
            let Some(next) = progress.queue.pop_front() else {
                progress.skills = skills;
                progress.running = false;
                shared.1.notify_all();
                return;
            };
            next
        };
        let mut count = 1;
        let mut bytes = operation.delta_bytes();
        let hub = owner.upgrade().map(Hub);
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
                    let result = hub.save_record(&id, &mut record);
                    if result.is_ok() {
                        hub.mark_durable(&id, &record);
                        skills = Some((record.engine.skills, record.engine.skill_snapshot));
                    }
                    result
                }
                Operation::Delta(delta) => {
                    let mut deltas = vec![delta];
                    {
                        let mut progress = shared.0.lock().unwrap();
                        // Only take ready deltas. A checkpoint must retain its
                        // place because saving it also truncates the journal.
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
                    hub.0.store.append_batch(&id, &deltas)
                }
                Operation::Context(attempt, mut record) => hub
                    .prepare_record(&mut record)
                    .and_then(|()| hub.0.store.archive_context(&id, attempt, &record)),
                Operation::Clear(command, mut record) => hub
                    .prepare_record(&mut record)
                    .and_then(|()| hub.0.store.archive_clear(&id, &command, &record)),
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
            progress.used -= count + progress.queue.len();
            let queued_bytes = progress
                .queue
                .iter()
                .map(|(_, operation)| operation.delta_bytes())
                .sum::<usize>();
            progress.delta_bytes -= bytes + queued_bytes;
            progress.queue.clear();
            progress.running = false;
            shared.1.notify_all();
            return;
        }
        // Release the state-directory owner before signaling completion.
        drop(hub);
        let mut progress = shared.0.lock().unwrap();
        progress.completed = revision;
        progress.used -= count;
        progress.delta_bytes -= bytes;
        shared.1.notify_all();
    }
}
