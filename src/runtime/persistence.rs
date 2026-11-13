//! Ordered disk work. Never wait for this lane while holding the roster lock.
use super::*;
use std::sync::{Condvar, Weak, mpsc};

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
}

#[derive(Clone)]
pub(super) struct Receipt {
    revision: u64,
    progress: Arc<(Mutex<Progress>, Condvar)>,
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

pub(super) struct Lane {
    sender: mpsc::SyncSender<(u64, Operation)>,
    progress: Arc<(Mutex<Progress>, Condvar)>,
    revision: u64,
    inputs: VecDeque<Receipt>,
}
impl Lane {
    pub(super) fn new(hub: Weak<Inner>, id: String) -> Result<Self> {
        // Bound queued full-record copies even if a filesystem stops responding.
        let (sender, receiver) = mpsc::sync_channel::<(u64, Operation)>(16);
        let progress = Arc::new((Mutex::new(Progress::default()), Condvar::new()));
        let finished = progress.clone();
        std::thread::Builder::new()
            .name(format!("save-{id}"))
            .spawn(move || {
                let mut skills: Option<(crate::skills::Skills, Option<String>)> = None;
                while let Ok((revision, operation)) = receiver.recv() {
                    let Some(inner) = hub.upgrade() else { break };
                    let hub = Hub(inner);
                    let result = match operation {
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
                    };
                    if let Err(error) = result {
                        let message = format!("{error:#}");
                        hub.fault(&id, error);
                        drop(hub);
                        finished.0.lock().unwrap().error = Some(message);
                        finished.1.notify_all();
                        return;
                    }
                    // Release the state-directory owner before signaling completion.
                    drop(hub);
                    finished.0.lock().unwrap().completed = revision;
                    finished.1.notify_all();
                }
            })?;
        Ok(Self {
            sender,
            progress,
            revision: 0,
            inputs: VecDeque::new(),
        })
    }
    pub(super) fn enqueue(&mut self, operation: Operation) -> Result<Receipt> {
        if let Some(error) = &self.progress.0.lock().unwrap().error {
            bail!("Session persistence failed: {error}");
        }
        let revision = self.revision + 1;
        self.sender
            .try_send((revision, operation))
            .map_err(|error| match error {
                mpsc::TrySendError::Full(_) => {
                    anyhow::anyhow!("Session storage is catching up; retry shortly.")
                }
                mpsc::TrySendError::Disconnected(_) => {
                    anyhow::anyhow!("Session persistence worker stopped.")
                }
            })?;
        self.revision = revision;
        Ok(Receipt {
            revision,
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
}
