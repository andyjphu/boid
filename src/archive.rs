use super::*;
use sha2::{Digest, Sha256};

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct ArchivedReceipt {
    pub(super) message: Communication,
    text_sha256: String,
}
impl ArchivedReceipt {
    fn new(message: &Communication) -> Self {
        let text_sha256 = format!("{:x}", Sha256::digest(message.text.as_bytes()));
        let mut message = message.clone();
        message.text = String::new();
        Self {
            message,
            text_sha256,
        }
    }
    pub(super) fn matches(&self, to: &str, text: &str, urgency: Urgency) -> bool {
        self.message.recipient_id == to
            && self.message.urgency == urgency
            && self.text_sha256 == format!("{:x}", Sha256::digest(text.as_bytes()))
    }
}
#[derive(Clone, Serialize, Deserialize)]
pub(super) struct ArchivedSession {
    pub(super) view: View,
    pub(super) entry_count: usize,
    #[serde(skip_serializing, default)]
    pub(super) receipts: Vec<ArchivedReceipt>,
}
impl ArchivedSession {
    fn from_record(record: &Record) -> Self {
        Self {
            view: metadata_view(&record.view),
            entry_count: record.view.entries.len(),
            receipts: record.messages.iter().map(ArchivedReceipt::new).collect(),
        }
    }
}
fn metadata_view(source: &View) -> View {
    View {
        id: source.id.clone(),
        name: source.name.clone(),
        cwd: source.cwd.clone(),
        status: source.status.clone(),
        archived: source.archived,
        profile: source.profile.clone(),
        questions: vec![],
        context_usage: None,
        context_window: None,
        communications: vec![],
        working_since_ms: None,
        entries: vec![],
        pending: vec![],
    }
}
// Legacy migration streams past model history/transcript content. Only one peer
// message's text exists transiently while its deduplication digest is calculated.
#[derive(Default)]
struct EntryCount(usize);
impl<'de> Deserialize<'de> for EntryCount {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        struct Counter;
        impl<'de> serde::de::Visitor<'de> for Counter {
            type Value = EntryCount;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("transcript entries")
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut seq: A,
            ) -> std::result::Result<EntryCount, A::Error> {
                let mut count = 0;
                while seq.next_element::<serde::de::IgnoredAny>()?.is_some() {
                    count += 1;
                }
                Ok(EntryCount(count))
            }
        }
        deserializer.deserialize_seq(Counter)
    }
}
struct SourceReceipt(ArchivedReceipt);
impl<'de> Deserialize<'de> for SourceReceipt {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        Ok(Self(ArchivedReceipt::new(&Communication::deserialize(
            deserializer,
        )?)))
    }
}
#[derive(Deserialize)]
struct ProbeView {
    id: String,
    name: String,
    cwd: String,
    status: String,
    #[serde(default)]
    profile: Value,
    #[serde(default)]
    archived: bool,
    #[serde(default)]
    entries: EntryCount,
}
#[derive(Deserialize)]
struct ArchiveProbe {
    version: u32,
    view: ProbeView,
    #[serde(default)]
    messages: Vec<SourceReceipt>,
}
impl ArchiveProbe {
    fn summary(self) -> ArchivedSession {
        ArchivedSession {
            entry_count: self.view.entries.0,
            view: View {
                id: self.view.id,
                name: self.view.name,
                cwd: self.view.cwd,
                status: self.view.status,
                profile: self.view.profile,
                archived: self.view.archived,
                questions: vec![],
                context_usage: None,
                context_window: None,
                communications: vec![],
                working_since_ms: None,
                entries: vec![],
                pending: vec![],
            },
            receipts: self.messages.into_iter().map(|r| r.0).collect(),
        }
    }
}
impl Hub {
    fn preserve_archive(store: &SessionStore, id: &str, summary: &ArchivedSession) -> Result<()> {
        let mut senders: BTreeMap<String, BTreeMap<String, ArchivedReceipt>> = BTreeMap::new();
        for receipt in &summary.receipts {
            senders
                .entry(receipt.message.sender_id.clone())
                .or_default()
                .insert(receipt.message.call_id.clone(), receipt.clone());
        }
        for (sender, receipts) in senders {
            store.merge_archived_receipts(&sender, &receipts)?;
        }
        store.archive_session(id, summary)
    }
    pub(super) fn load_catalog(store: &SessionStore) -> Result<Vec<Record>> {
        let mut records = vec![];
        for id in store.session_ids()? {
            if store.session_status(&id)? != "archived" {
                records.push(store.read_session(&id)?);
                continue;
            }
            let probe: ArchiveProbe = store.read_session(&id)?;
            if probe.view.id != id {
                bail!("Session identity differs from filename; original retained");
            }
            if !matches!(probe.version, 1 | 2 | 3 | 4) {
                bail!(
                    "Unsupported session version {}; original retained",
                    probe.version
                );
            }
            if probe.view.archived {
                Self::preserve_archive(store, &id, &probe.summary())?;
            } else {
                records.push(store.read_session(&id)?);
            }
        }
        Ok(records)
    }
    pub(super) fn archive_finished(&self, id: &str) -> Result<()> {
        crate::blocking::run(|| {
            let (summary, barrier) = {
                let mut data = self.0.data.lock().unwrap();
                let handle = data.sessions.get_mut(id).context("Unknown session")?;
                if !handle.record.view.archived
                    || handle.record.view.status != "archived"
                    || handle.task.is_some()
                    || handle.storage_error.is_some()
                    || handle.archiving
                {
                    bail!("Session has not finished saving its end");
                }
                handle.archiving = true;
                (
                    ArchivedSession::from_record(&handle.record),
                    handle.persistence.barrier(),
                )
            };
            let result = (|| {
                barrier.wait()?;
                #[cfg(test)]
                {
                    let data = self.0.data.lock().unwrap();
                    self.0
                        .store
                        .archive_memory(id, &data.sessions[id].record, &summary)?;
                }
                Self::preserve_archive(&self.0.store, id, &summary)
            })();
            if let Err(error) = result {
                if let Some(handle) = self.0.data.lock().unwrap().sessions.get_mut(id) {
                    handle.archiving = false;
                }
                return Err(error);
            }
            let (removed, groups_saved) = {
                let mut data = self.0.data.lock().unwrap();
                data.archive_generation = data.archive_generation.wrapping_add(1);
                for receipt in &summary.receipts {
                    let key = (
                        receipt.message.sender_id.clone(),
                        receipt.message.call_id.clone(),
                    );
                    data.messages.remove(&key);
                    data.message_receipts.remove(&key);
                }
                // The trace is already durable outside the live store. Always
                // evict it, even if saving pane metadata fails. Startup prunes
                // stale memberships after loading the complete active roster.
                let removed = data.sessions.remove(id);
                let groups_saved = self.remove_group_member_locked(&mut data, &format!("agent:{id}"));
                (removed, groups_saved)
            };
            drop(removed);
            groups_saved.context("Trace archived, but pane metadata could not be saved")
        })
    }
}

#[cfg(test)]
impl Hub {
    pub(super) fn ended_record(&self, id: &str) -> Record {
        if let Some(handle) = self.0.data.lock().unwrap().sessions.get(id) {
            return handle.record.clone();
        }
        self.0.store.read_archive(id).unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    async fn agent(hub: &Hub, cwd: &Path, name: &str) -> View {
        hub.create(Create {
            name: name.into(),
            cwd: cwd.to_string_lossy().into(),
            message: String::new(),
        })
        .unwrap()
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn ended_records_leave_harness_but_files_and_send_dedup_survive_restart() {
        let state = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        let hub = Hub::open(Client::test_client(), state.path().into()).unwrap();
        let sender = agent(&hub, cwd.path(), "sender").await;
        let target = agent(&hub, cwd.path(), "target").await;
        hub.stop(&target.id).unwrap();
        hub.entry(&target.id, "notice", "retained archive marker");
        let first = hub
            .send_communication(
                &sender.id,
                &target.id,
                "private peer text",
                Urgency::Regular,
                "stable-send",
                "entry",
            )
            .unwrap();
        assert!(hub.close(&target.id).await.unwrap());
        assert!(!hub.0.data.lock().unwrap().sessions.contains_key(&target.id));
        assert!(hub.view(&target.id).is_none());
        assert!(hub.archived_views().is_empty() && hub.history_list(true).is_empty());
        assert!(
            hub.history(&target.id, None, None, None).is_err() && hub.reopen(&target.id).is_err()
        );
        assert_eq!(hub.diagnostics()["sessions"].as_array().unwrap().len(), 1);
        assert!(
            hub.ended_record(&target.id)
                .view
                .entries
                .iter()
                .any(|e| e.text.contains("retained archive marker"))
        );
        assert!(
            hub.read_peer_trace(&sender.id, &json!({"to":target.id}).to_string())["error"]
                .is_string()
        );
        let repeated = hub
            .send_communication(
                &sender.id,
                &target.id,
                "private peer text",
                Urgency::Regular,
                "stable-send",
                "entry",
            )
            .unwrap();
        assert_eq!(first["message_id"], repeated["message_id"]);
        assert!(
            hub.send_communication(
                &sender.id,
                &sender.id,
                "changed",
                Urgency::Regular,
                "stable-send",
                "entry"
            )
            .is_err()
        );
        assert!(
            hub.send_communication(
                &sender.id,
                &target.id,
                "changed",
                Urgency::Regular,
                "stable-send",
                "entry"
            )
            .is_err()
        );
        let task = hub
            .0
            .data
            .lock()
            .unwrap()
            .sessions
            .get_mut(&sender.id)
            .unwrap()
            .task
            .take()
            .unwrap();
        task.abort();
        let _ = task.await;
        hub.update(&sender.id, |r| {
            let mut engine = serde_json::to_value(&r.engine)?;
            engine["running"] = json!({"id":"stable-send","name":"send_message","arguments":"{}","argument_entry":null});
            r.engine = serde_json::from_value(engine)?; Ok(())
        }).unwrap();
        hub.shutdown().await;
        drop(hub);
        let hub = Hub::open(Client::test_client(), state.path().into()).unwrap();
        assert!(
            hub.view(&sender.id)
                .unwrap()
                .entries
                .iter()
                .any(|e| e.kind == "tool_result"
                    && e.text.contains(first["message_id"].as_str().unwrap()))
        );
        assert!(!hub.0.data.lock().unwrap().sessions.contains_key(&target.id));
        let repeated = hub
            .send_communication(
                &sender.id,
                &target.id,
                "private peer text",
                Urgency::Regular,
                "stable-send",
                "entry",
            )
            .unwrap();
        assert_eq!(first["message_id"], repeated["message_id"]);
        assert_eq!(hub.views().len(), 1); // The open, paused sender remains live.
        hub.shutdown().await;
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn failed_archive_keeps_original_and_startup_migrates_without_loading_archive_files() {
        let state = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        let hub = Hub::open(Client::test_client(), state.path().into()).unwrap();
        let target = agent(&hub, cwd.path(), "legacy").await;
        hub.entry(&target.id, "notice", "retained through archive failure");
        std::fs::write(state.path().join("archives"), b"blocked").unwrap();
        assert!(hub.close(&target.id).await.is_err());
        let original = state
            .path()
            .join("sessions")
            .join(format!("{}.json", target.id));
        assert!(original.is_file() && hub.0.data.lock().unwrap().sessions.contains_key(&target.id));
        hub.shutdown().await;
        drop(hub);
        std::fs::remove_file(state.path().join("archives")).unwrap();
        let hub = Hub::open(Client::test_client(), state.path().into()).unwrap();
        assert!(!original.exists() && hub.0.data.lock().unwrap().sessions.is_empty());
        assert!(
            hub.ended_record(&target.id)
                .view
                .entries
                .iter()
                .any(|e| e.text.contains("retained through archive failure"))
        );
        hub.shutdown().await;
        drop(hub);
        let manifest: Value = serde_json::from_slice(
            &std::fs::read(
                state
                    .path()
                    .join("archives")
                    .join(format!("{}.json", target.id)),
            )
            .unwrap(),
        )
        .unwrap();
        std::fs::write(
            state
                .path()
                .join("archives")
                .join(manifest["record"].as_str().unwrap()),
            b"corrupt fixture",
        )
        .unwrap();
        let hub = Hub::open(Client::test_client(), state.path().into()).unwrap();
        assert!(
            hub.archived_views().is_empty()
                && hub.diagnostics()["sessions"].as_array().unwrap().is_empty()
        );
        hub.shutdown().await;
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn archive_fences_admitted_saves_rejects_mutation_and_survives_shutdown() {
        let state = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        let hub = Hub::open(Client::test_client(), state.path().into()).unwrap();
        let target = agent(&hub, cwd.path(), "archive-race").await;
        let task = hub
            .0
            .data
            .lock()
            .unwrap()
            .sessions
            .get_mut(&target.id)
            .unwrap()
            .task
            .take()
            .unwrap();
        task.abort();
        let _ = task.await;
        hub.update(&target.id, |r| {
            r.view.archived = true;
            r.view.status = "archived".into();
            r.paused = true;
            Ok(())
        })
        .unwrap();
        let (started, release) = hub
            .0
            .data
            .lock()
            .unwrap()
            .sessions
            .get_mut(&target.id)
            .unwrap()
            .persistence
            .gate();
        tokio::task::spawn_blocking(move || started.recv_timeout(Duration::from_secs(3)))
            .await
            .unwrap()
            .unwrap();
        {
            let mut data = hub.0.data.lock().unwrap();
            hub.update_locked(&mut data, &target.id, None, |r| {
                r.view.entries.push(Hub::make_entry(
                    "notice",
                    "save admitted before archive".into(),
                ));
                Ok(())
            })
            .unwrap();
        }
        let archiver = hub.clone();
        let id = target.id.clone();
        let archiving = tokio::task::spawn_blocking(move || archiver.archive_finished(&id));
        tokio::time::timeout(Duration::from_secs(2), async {
            while !hub.0.data.lock().unwrap().sessions[&target.id].archiving {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(
            hub.update(&target.id, |r| {
                r.view.archived = false;
                Ok(())
            })
            .is_err()
        );
        assert!(hub.reopen(&target.id).is_err());
        let stopping = hub.clone();
        let shutdown = tokio::spawn(async move { stopping.shutdown().await });
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(!shutdown.is_finished() && !archiving.is_finished());
        release.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(3), archiving)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        tokio::time::timeout(Duration::from_secs(3), shutdown)
            .await
            .unwrap()
            .unwrap();
        assert!(!hub.0.data.lock().unwrap().sessions.contains_key(&target.id));
        assert!(
            !state
                .path()
                .join("sessions")
                .join(format!("{}.json", target.id))
                .exists()
        );
        assert!(
            hub.ended_record(&target.id)
                .view
                .entries
                .iter()
                .any(|e| e.text.contains("save admitted before archive"))
        );
    }
}

#[cfg(test)]
mod cancellation_test {
    use super::*;
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn disconnected_end_caller_cannot_abandon_finalization() {
        let state = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        let hub = Hub::open(Client::test_client(), state.path().into()).unwrap();
        let target = hub
            .create(Create {
                name: "disconnect".into(),
                cwd: cwd.path().to_string_lossy().into(),
                message: String::new(),
            })
            .unwrap();
        let (started, release) = hub
            .0
            .data
            .lock()
            .unwrap()
            .sessions
            .get_mut(&target.id)
            .unwrap()
            .persistence
            .gate();
        tokio::task::spawn_blocking(move || started.recv_timeout(Duration::from_secs(2)))
            .await
            .unwrap()
            .unwrap();
        let ending_hub = hub.clone();
        let id = target.id.clone();
        let request = tokio::spawn(async move { ending_hub.close(&id).await });
        tokio::time::timeout(Duration::from_secs(2), async {
            while !hub.0.data.lock().unwrap().ending.contains(&target.id) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        request.abort();
        let _ = request.await;
        let stopping = hub.clone();
        let shutdown = tokio::spawn(async move { stopping.shutdown().await });
        release.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(3), shutdown)
            .await
            .unwrap()
            .unwrap();
        assert!(!hub.0.data.lock().unwrap().sessions.contains_key(&target.id));
        assert!(hub.0.data.lock().unwrap().ending.is_empty());
        assert!(hub.ended_record(&target.id).view.archived);
    }
}

#[cfg(test)]
mod handoff_test {
    use super::*;
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn stale_receipt_lookup_cannot_admit_same_call_to_a_different_recipient() {
        let state = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        let hub = Hub::open(Client::test_client(), state.path().into()).unwrap();
        let create = |name: &str| {
            hub.create(Create {
                name: name.into(),
                cwd: cwd.path().to_string_lossy().into(),
                message: String::new(),
            })
            .unwrap()
        };
        let sender = create("sender");
        let original = create("original");
        let other = create("other");
        hub.stop(&original.id).unwrap();
        hub.stop(&other.id).unwrap();
        let (started, observing) = std::sync::mpsc::channel();
        let (release, held) = std::sync::mpsc::channel();
        *hub.0.archive_receipt_gate.lock().unwrap() = Some((started, held));
        let checking = hub.clone();
        let from = sender.id.clone();
        let to = other.id.clone();
        let stale = tokio::task::spawn_blocking(move || {
            checking.send_communication(
                &from,
                &to,
                "changed target",
                Urgency::Regular,
                "same-call",
                "entry",
            )
        });
        tokio::task::spawn_blocking(move || observing.recv_timeout(Duration::from_secs(2)))
            .await
            .unwrap()
            .unwrap();
        let accepted = hub
            .send_communication(
                &sender.id,
                &original.id,
                "original text",
                Urgency::Regular,
                "same-call",
                "entry",
            )
            .unwrap();
        hub.close(&original.id).await.unwrap();
        release.send(()).unwrap();
        assert!(
            tokio::time::timeout(Duration::from_secs(2), stale)
                .await
                .unwrap()
                .unwrap()
                .is_err()
        );
        assert!(
            hub.0.data.lock().unwrap().sessions[&other.id]
                .record
                .inbox
                .is_empty()
        );
        let repeated = hub
            .send_communication(
                &sender.id,
                &original.id,
                "original text",
                Urgency::Regular,
                "same-call",
                "entry",
            )
            .unwrap();
        assert_eq!(accepted["message_id"], repeated["message_id"]);
        hub.shutdown().await;
    }
}
