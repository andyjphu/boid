//! One preloaded snapshot and cancellable metadata watcher per working directory.
//! Message dispatch only clones the latest completed snapshot; it never scans.
use super::discovery::{self, Snapshot};
use anyhow::{Result, bail};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{
        Arc, Condvar, Mutex, Weak,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

#[cfg(test)]
pub(crate) struct ScanGate {
    pub started: tokio::sync::Notify,
    pub release: Mutex<std::sync::mpsc::Receiver<()>>,
}
#[derive(Default)]
pub struct Cache {
    watches: Mutex<BTreeMap<PathBuf, Arc<Mutex<Weak<Watch>>>>>,
    closed: AtomicBool,
    #[cfg(test)]
    pub(crate) gate: Arc<Mutex<Option<Arc<ScanGate>>>>,
}
struct State {
    latest: Mutex<Arc<Snapshot>>,
    cancelled: AtomicBool,
    sleep: Mutex<()>,
    wake: Condvar,
}
/// Handles own leases; the watcher thread does not. The last lease cancels it.
pub struct Watch(Arc<State>);
impl Watch {
    pub fn latest(&self) -> Arc<Snapshot> {
        self.0.latest.lock().unwrap().clone()
    }
    fn stop(&self) {
        let _sleep = self.0.sleep.lock().unwrap();
        self.0.cancelled.store(true, Ordering::Release);
        self.0.wake.notify_all();
    }
}
impl Drop for Watch {
    fn drop(&mut self) {
        self.stop();
    }
}
impl Cache {
    /// Call only during server/agent initialization, outside the roster lock.
    pub fn preload(&self, cwd: &Path) -> Result<Arc<Watch>> {
        let cwd = cwd.canonicalize().unwrap_or_else(|_| cwd.to_owned());
        if self.closed.load(Ordering::Acquire) {
            bail!("Skill cache is shutting down.");
        }
        let slot = self
            .watches
            .lock()
            .unwrap()
            .entry(cwd.clone())
            .or_default()
            .clone();
        // Only concurrent initialization for this same cwd waits here. Shutdown
        // and other cwd caches never wait on filesystem work.
        let mut slot = slot.lock().unwrap();
        if let Some(watch) = slot.upgrade() {
            if !watch.0.cancelled.load(Ordering::Acquire) {
                drop(slot);
                return self.checked_lease(watch);
            }
        }
        // Fingerprint before reading: edits during a scan trigger another refresh.
        let signal = AtomicBool::new(false);
        let mut fingerprint = discovery::fingerprint(&cwd, &signal);
        let latest = Arc::new(discovery::scan(&cwd));
        let state = Arc::new(State {
            latest: Mutex::new(latest),
            cancelled: AtomicBool::new(false),
            sleep: Mutex::new(()),
            wake: Condvar::new(),
        });
        let watch = Arc::new(Watch(state.clone()));
        #[cfg(test)]
        let gate = self.gate.clone();
        std::thread::Builder::new()
            .name("boid-skills".into())
            .spawn(move || {
                loop {
                    let sleep = state.sleep.lock().unwrap();
                    let (sleep, _) = state
                        .wake
                        .wait_timeout_while(sleep, Duration::from_millis(500), |_| {
                            !state.cancelled.load(Ordering::Acquire)
                        })
                        .unwrap();
                    drop(sleep);
                    if state.cancelled.load(Ordering::Acquire) {
                        break;
                    }
                    let next = discovery::fingerprint(&cwd, &state.cancelled);
                    if next.is_none() || next == fingerprint {
                        continue;
                    }
                    #[cfg(test)]
                    let pending_gate = gate.lock().unwrap().take();
                    #[cfg(test)]
                    if let Some(gate) = pending_gate {
                        gate.started.notify_one();
                        loop {
                            if state.cancelled.load(Ordering::Acquire) {
                                return;
                            }
                            match gate
                                .release
                                .lock()
                                .unwrap()
                                .recv_timeout(Duration::from_millis(20))
                            {
                                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => (),
                                _ => break,
                            }
                        }
                    }
                    if let Some(snapshot) = discovery::scan_cancellable(&cwd, &state.cancelled) {
                        let prior = state.latest.lock().unwrap().clone();
                        if !state.cancelled.load(Ordering::Acquire) && *prior != snapshot {
                            *state.latest.lock().unwrap() = Arc::new(snapshot);
                        }
                    }
                    fingerprint = next;
                }
            })?;
        *slot = Arc::downgrade(&watch);
        drop(slot);
        self.checked_lease(watch)
    }
    // Both preload paths release their slot before this check. A concurrent
    // shutdown either finds that slot or is observed here; neither can miss it.
    fn checked_lease(&self, watch: Arc<Watch>) -> Result<Arc<Watch>> {
        if self.closed.load(Ordering::Acquire) {
            watch.stop();
            bail!("Skill cache is shutting down.");
        }
        Ok(watch)
    }
    pub fn shutdown(&self) {
        self.closed.store(true, Ordering::Release);
        for slot in self.watches.lock().unwrap().values() {
            // An in-progress preload checks closed before publishing its lease.
            if let Ok(slot) = slot.try_lock() {
                if let Some(watch) = slot.upgrade() {
                    watch.stop();
                }
            }
        }
    }
}
impl Drop for Cache {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn wait_for(mut predicate: impl FnMut() -> bool) {
        let deadline = std::time::Instant::now() + Duration::from_secs(4);
        while !predicate() {
            assert!(
                std::time::Instant::now() < deadline,
                "background refresh timed out"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    fn package(cwd: &Path, name: &str, body: &str) -> PathBuf {
        let path = cwd.join(".agents/skills").join(name);
        std::fs::create_dir_all(&path).unwrap();
        std::fs::write(
            path.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: Local fixture\n---\n{body}\n"),
        )
        .unwrap();
        path
    }
    #[test]
    fn shared_cache_detects_changes_and_preserves_completed_snapshots() {
        let work = tempfile::tempdir().unwrap();
        let path = package(work.path(), "cache-fixture", "original");
        let id = path
            .join("SKILL.md")
            .canonicalize()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let cache = Cache::default();
        let watch = cache.preload(work.path()).unwrap();
        let sibling = cache.preload(work.path()).unwrap();
        assert!(Arc::ptr_eq(&watch, &sibling));
        let original = watch.latest();
        assert!(original.skills[&id].text.contains("original"));
        package(work.path(), "cache-fixture", "revised");
        wait_for(|| watch.latest().skills[&id].text.contains("revised"));
        assert!(original.skills[&id].text.contains("original"));
        std::fs::create_dir_all(path.join("agents")).unwrap();
        std::fs::write(
            path.join("agents/openai.yaml"),
            "policy:\n  allow_implicit_invocation: false\n",
        )
        .unwrap();
        wait_for(|| !watch.latest().skills[&id].implicit);
        assert!(watch.latest().skills[&id].enabled);
        assert!(super::super::selection::resolve(&watch.latest().skills, &id).is_ok());
        std::fs::create_dir_all(work.path().join(".boid")).unwrap();
        let config = work.path().join(".boid/skills.json");
        std::fs::write(
            &config,
            r#"{"rules":[{"name":"cache-fixture","enabled":false}]}"#,
        )
        .unwrap();
        wait_for(|| !watch.latest().skills[&id].enabled);
        std::fs::write(&config, "broken").unwrap();
        wait_for(|| watch.latest().skills.is_empty());
        assert!(!watch.latest().warnings.is_empty());
        std::fs::write(&config, "{}").unwrap();
        wait_for(|| watch.latest().skills.contains_key(&id));
        let renamed = path.with_file_name("renamed-fixture");
        std::fs::rename(&path, &renamed).unwrap();
        let renamed_id = renamed
            .join("SKILL.md")
            .canonicalize()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        wait_for(|| {
            let latest = watch.latest();
            !latest.skills.contains_key(&id) && latest.skills.contains_key(&renamed_id)
        });
        std::fs::remove_file(renamed.join("SKILL.md")).unwrap();
        wait_for(|| !watch.latest().skills.contains_key(&renamed_id));
        let state = Arc::downgrade(&watch.0);
        drop(watch);
        assert!(!sibling.0.cancelled.load(Ordering::Acquire));
        drop(sibling);
        wait_for(|| state.upgrade().is_none());
    }
    #[test]
    fn held_refresh_keeps_latest_available_and_shutdown_cancels_it() {
        let work = tempfile::tempdir().unwrap();
        let cache = Cache::default();
        let watch = cache.preload(work.path()).unwrap();
        let before = watch.latest();
        let (_release, wait) = std::sync::mpsc::channel();
        let gate = Arc::new(ScanGate {
            started: tokio::sync::Notify::new(),
            release: Mutex::new(wait),
        });
        *cache.gate.lock().unwrap() = Some(gate);
        package(work.path(), "later-fixture", "later");
        wait_for(|| cache.gate.lock().unwrap().is_none());
        for _ in 0..1000 {
            assert!(Arc::ptr_eq(&before, &watch.latest()));
        }
        cache.shutdown();
        assert!(watch.0.cancelled.load(Ordering::Acquire));
        assert!(cache.preload(work.path()).is_err());
        let state = Arc::downgrade(&watch.0);
        drop(watch);
        wait_for(|| state.upgrade().is_none());
    }
}
