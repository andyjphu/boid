//! A process-wide crash fence, independent of asynchronous session checkpoints.
//! Queue quarantine is transcript evidence, never an executable model input.
use super::*;
use std::{
    fs::{self, File},
    io::Write,
};

pub(super) const MARKER: &str = "agent-runtime-running.json";

pub(super) struct RunMarker {
    path: Option<PathBuf>,
    token: String,
}
impl RunMarker {
    pub fn start(store: &SessionStore) -> Result<(Self, bool)> {
        let token = Uuid::new_v4().to_string();
        let Some(root) = store.state_directory() else {
            return Ok((Self { path: None, token }, false));
        };
        let path = root.join(MARKER);
        let unclean = path.try_exists()?;
        let mut temporary = tempfile::NamedTempFile::new_in(&root)?;
        serde_json::to_writer(
            &mut temporary,
            &json!({"token":token,"pid":std::process::id()}),
        )?;
        temporary.flush()?;
        temporary.as_file().sync_all()?;
        temporary.persist(&path).map_err(|error| error.error)?;
        File::open(&root)?.sync_all()?;
        Ok((
            Self {
                path: Some(path),
                token,
            },
            unclean,
        ))
    }
    pub fn finish(&self) -> Result<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        let bytes = match fs::read(path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error.into()),
        };
        let marker: Value = serde_json::from_slice(&bytes)?;
        if marker["token"].as_str() != Some(self.token.as_str()) {
            bail!("Run marker ownership changed; retaining unclean shutdown marker");
        }
        fs::remove_file(path)?;
        File::open(path.parent().unwrap())?.sync_all()?;
        Ok(())
    }
}

pub(super) fn quarantine(records: &mut [Record]) -> Result<()> {
    let queued_peers: BTreeSet<_> = records
        .iter()
        .flat_map(|r| r.inbox.iter().map(|p| p.id.clone()))
        .collect();
    for record in records {
        // Keep the receipt identities even when delivery outcome is unknown.
        // This prevents a known send call ID from being submitted a second time.
        for message in &mut record.messages {
            if queued_peers.contains(&message.id) {
                message.state = "interrupted".into();
                message.blocked = true;
            }
        }
        let mut notice = String::from(
            "Boid recovered after an unclean shutdown. Recent actions may have completed without being saved. Inspect external state before retrying uncertain actions. Saved queued work will not run automatically.",
        );
        if !record.inputs.is_empty() || !record.inbox.is_empty() {
            // Preserve every queued field, including image refs and ordering,
            // in the transcript while removing dispatch eligibility. These
            // bytes are intentionally not injected into the model as a task.
            let preserved = serde_json::to_string(
                &json!({"quarantined_user_inputs":record.inputs,"quarantined_peer_inputs":record.inbox}),
            )?;
            notice.push_str("\nRetained queued input (not dispatched):\n");
            notice.push_str(&preserved);
        }
        record.inputs.clear();
        record.inbox.clear();
        record.view.pending.clear();
        record.view.entries.push(Hub::make_entry("notice", notice));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn marker_survives_drop_and_only_clean_owner_can_remove_it() {
        let root = tempfile::tempdir().unwrap();
        let store = SessionStore::open(root.path().into()).unwrap();
        let (first, unclean) = RunMarker::start(&store).unwrap();
        assert!(!unclean);
        drop(first);
        let (second, unclean) = RunMarker::start(&store).unwrap();
        assert!(unclean);
        let (third, unclean) = RunMarker::start(&store).unwrap();
        assert!(unclean);
        assert!(second.finish().is_err());
        assert!(root.path().join(MARKER).exists());
        third.finish().unwrap();
        let (fourth, unclean) = RunMarker::start(&store).unwrap();
        assert!(!unclean);
        fourth.finish().unwrap();
    }

    #[tokio::test]
    async fn crash_quarantine_keeps_idle_agent_listening_and_pending_calls_uncertain() {
        let hub = Hub::new(Client::test_client()).unwrap();
        let work = tempfile::tempdir().unwrap();
        let view = hub
            .create(Create {
                name: "recovery-fixture".into(),
                cwd: work.path().to_string_lossy().into_owned(),
                message: String::new(),
            })
            .unwrap();
        let mut record = hub.0.data.lock().unwrap().sessions[&view.id].record.clone();
        record.listen_on_restart = Some(true);
        record.paused = false;
        record.inputs.push_back(Input {
            id: "old-user".into(),
            text: "old task".into(),
            images: vec![],
            order: 1,
            compact: false,
            clear: false,
            take_context: None,
        });
        record.inbox.push_back(Peer {
            id: "old-peer".into(),
            sender: "peer".into(),
            text: "old peer task".into(),
        });
        record.view.pending.push("old-user".into());
        quarantine(std::slice::from_mut(&mut record)).unwrap();
        assert!(
            record.inputs.is_empty() && record.inbox.is_empty() && record.view.pending.is_empty()
        );
        assert!(record.restart_listening());
        assert!(
            record
                .view
                .entries
                .last()
                .unwrap()
                .text
                .contains("old peer task")
        );
        assert!(record.engine.recover_uncertain().is_empty());
        record.listen_on_restart = Some(false);
        assert!(!record.restart_listening());
        let mut saved = serde_json::to_value(&record.engine).unwrap();
        saved["request_in_flight"] = json!(true);
        saved["pending"] = json!([{"id":"uncertain-call","name":"terminal","arguments":"{}"}]);
        let mut engine: Session = serde_json::from_value(saved).unwrap();
        assert!(engine.has_unfinished_execution());
        let entries = engine.recover_uncertain();
        assert!(
            entries
                .iter()
                .any(|(_, text)| text.contains("may already have executed"))
        );
        assert!(!engine.has_unfinished_execution());
        assert!(
            entries
                .iter()
                .all(|(_, text)| !text.contains("Not executed because"))
        );
        hub.shutdown().await;
    }
    #[tokio::test]
    async fn unclean_restart_admits_new_user_and_peer_but_never_old_queued_work() {
        let state = tempfile::tempdir().unwrap();
        let work = tempfile::tempdir().unwrap();
        let hub = Hub::new(Client::test_client()).unwrap();
        let mut records = Vec::new();
        for name in ["source", "recipient", "stopped"] {
            let view = hub
                .create(Create {
                    name: name.into(),
                    cwd: work.path().to_string_lossy().into_owned(),
                    message: String::new(),
                })
                .unwrap();
            let mut record = hub.0.data.lock().unwrap().sessions[&view.id].record.clone();
            record.paused = name == "stopped";
            record.listen_on_restart = Some(name != "stopped");
            record.view.entries.push(Hub::make_entry(
                "assistant",
                "prior completed transcript".into(),
            ));
            records.push(record);
        }
        hub.shutdown().await;
        let source = records[0].view.id.clone();
        let recipient = records[1].view.id.clone();
        let stopped = records[2].view.id.clone();
        let receipt = Communication {
            id: "old-peer-id".into(),
            sender_id: source.clone(),
            recipient_id: recipient.clone(),
            sender_name: "source".into(),
            recipient_name: "recipient".into(),
            call_id: "old-call".into(),
            sender_entry: "old-call-entry".into(),
            text: "old peer work".to_owned().into(),
            urgency: Urgency::Regular,
            wake: false,
            state: "queued".into(),
            applied_batch: None,
            blocked: false,
            persisted: true,
        };
        records[1].messages.push(receipt);
        records[1].inbox.push_back(Peer {
            id: "old-peer-id".into(),
            sender: source.clone(),
            text: "old peer work".into(),
        });
        records[1].inputs.push_back(Input {
            id: "old-user-id".into(),
            text: "old user work".into(),
            images: vec![],
            order: 1,
            compact: false,
            clear: false,
            take_context: None,
        });
        records[1].order = 1;
        records[1].view.pending.push("old-user-id".into());
        {
            let store = SessionStore::open(state.path().into()).unwrap();
            let (_marker, _) = RunMarker::start(&store).unwrap();
            for record in records {
                store.save(&record.view.id, &record).unwrap();
            }
            // No clean marker removal: simulates process exit with a saved idle
            // snapshot that predates dispatch of its previously queued work.
        }
        let recovered = Hub::open(Client::test_client(), state.path().into()).unwrap();
        // Own dispatch deterministically without ever calling a model endpoint.
        let tasks: Vec<_> = recovered
            .0
            .data
            .lock()
            .unwrap()
            .sessions
            .values_mut()
            .filter_map(|h| h.task.take())
            .collect();
        for task in tasks {
            task.abort();
            let _ = task.await;
        }
        {
            let data = recovered.0.data.lock().unwrap();
            let target = &data.sessions[&recipient].record;
            assert!(!target.paused);
            assert!(target.inputs.is_empty() && target.inbox.is_empty());
            assert!(data.sessions[&stopped].record.paused);
            assert!(
                target
                    .view
                    .entries
                    .iter()
                    .any(|e| e.text.as_str() == "prior completed transcript")
            );
        }
        let duplicate = recovered
            .send_communication(
                &source,
                &recipient,
                "old peer work",
                Urgency::Regular,
                "old-call",
                "old-call-entry",
            )
            .unwrap();
        assert_eq!(duplicate["state"], "interrupted");
        assert!(
            recovered.0.data.lock().unwrap().sessions[&recipient]
                .record
                .inbox
                .is_empty()
        );
        let fresh = recovered
            .send_communication(
                &source,
                &recipient,
                "fresh peer work",
                Urgency::Regular,
                "new-call",
                "new-call-entry",
            )
            .unwrap();
        assert_eq!(fresh["blocked"], false);
        let mut engine = recovered.0.data.lock().unwrap().sessions[&recipient]
            .record
            .engine
            .clone();
        assert!(recovered.apply_inbox(&recipient, &mut engine).unwrap());
        recovered
            .message(&recipient, "fresh operator work".into())
            .unwrap();
        assert!(
            recovered
                .next_input(&recipient, &mut engine, false)
                .unwrap()
                .is_some()
        );
        let history = serde_json::to_string(engine.history_items()).unwrap();
        assert!(history.contains("fresh peer work") && history.contains("fresh operator work"));
        assert!(!history.contains("old peer work") && !history.contains("old user work"));
        recovered.shutdown().await;
        assert!(!state.path().join(MARKER).exists());
    }
}
