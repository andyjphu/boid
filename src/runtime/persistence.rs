//! Ordered disk work. Never wait for this lane while holding the roster lock.
use super::*;
#[cfg(test)]
use std::sync::mpsc;
use std::sync::{Condvar, Weak};

const CAPACITY: usize = 16;
type SkillSnapshot = (crate::skills::Skills, Option<String>);

pub(super) enum Operation {
    Snapshot(Box<Record>),
    Delta(Delta),
    Context(u64, Box<Record>),
    Clear(String, Box<Record>),
    #[cfg(test)]
    Gate(mpsc::Sender<()>, mpsc::Receiver<()>),
}

#[derive(Default)]
struct Progress {
    completed: u64,
    error: Option<String>,
    // Includes reserved slots and the operation currently being written.
    used: usize,
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
}
impl Capacity {
    pub(super) fn reserve(&self, slots: usize) -> Result<Permit> {
        assert!((1..=CAPACITY).contains(&slots));
        let mut progress = self.0.0.lock().unwrap();
        loop {
            if let Some(error) = &progress.error {
                bail!("Session persistence failed: {error}");
            }
            if progress.used + slots <= CAPACITY {
                progress.used += slots;
                return Ok(Permit {
                    progress: self.0.clone(),
                    slots,
                });
            }
            progress = self.0.1.wait(progress).unwrap();
        }
    }
    fn try_reserve(&self) -> Result<Permit> {
        let mut progress = self.0.0.lock().unwrap();
        if let Some(error) = &progress.error {
            bail!("Session persistence failed: {error}");
        }
        if progress.used == CAPACITY {
            bail!("Session storage is catching up; retry shortly.");
        }
        progress.used += 1;
        Ok(Permit {
            progress: self.0.clone(),
            slots: 1,
        })
    }
}
impl Drop for Permit {
    fn drop(&mut self) {
        if self.slots != 0 {
            self.progress.0.lock().unwrap().used -= self.slots;
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
        let mut permit = self.capacity().try_reserve()?;
        self.enqueue_reserved(operation, &mut permit)
    }
    pub(super) fn enqueue_reserved(
        &mut self,
        operation: Operation,
        permit: &mut Permit,
    ) -> Result<Receipt> {
        assert!(Arc::ptr_eq(&permit.progress, &self.progress) && permit.slots > 0);
        let mut progress = self.progress.0.lock().unwrap();
        if let Some(error) = &progress.error {
            bail!("Session persistence failed: {error}");
        }
        self.revision += 1;
        permit.slots -= 1;
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
        let (revision, operation) = {
            let mut progress = shared.0.lock().unwrap();
            let Some(next) = progress.queue.pop_front() else {
                progress.skills = skills;
                progress.running = false;
                shared.1.notify_all();
                return;
            };
            next
        };
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
                Operation::Delta(delta) => hub.0.store.append(&id, &delta),
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
            progress.used -= 1 + progress.queue.len();
            progress.queue.clear();
            progress.running = false;
            shared.1.notify_all();
            return;
        }
        // Release the state-directory owner before signaling completion.
        drop(hub);
        let mut progress = shared.0.lock().unwrap();
        progress.completed = revision;
        progress.used -= 1;
        shared.1.notify_all();
    }
}
