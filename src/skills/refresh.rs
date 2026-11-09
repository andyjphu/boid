//! One in-flight scan per session. Controls cancel its result immediately;
//! blocking reads finish cooperatively without starting overlapping scans.
use super::discovery::Snapshot;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use tokio::task::{JoinError, JoinHandle};

pub struct Scan {
    cancelled: Arc<AtomicBool>,
    task: JoinHandle<Option<Snapshot>>,
}
impl Scan {
    pub fn start(work: impl FnOnce(&AtomicBool) -> Option<Snapshot> + Send + 'static) -> Self {
        let cancelled = Arc::new(AtomicBool::new(false));
        let signal = cancelled.clone();
        let task = tokio::task::spawn_blocking(move || work(&signal));
        Self { cancelled, task }
    }
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }
    pub fn cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }
    pub async fn finish(&mut self) -> Result<Option<Snapshot>, JoinError> {
        (&mut self.task).await
    }
}
impl Drop for Scan {
    fn drop(&mut self) {
        self.cancel();
    }
}
