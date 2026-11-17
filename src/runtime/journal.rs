//! Incremental checkpoint payloads and bounded snapshot compaction.
use super::*;
use std::sync::Condvar;

#[derive(Serialize, Deserialize)]
pub(super) struct Event {
    pub seq: u64,
    pub base: u64,
    #[serde(flatten)]
    pub change: Change,
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(super) enum Change {
    Checkpoint {
        metadata: Value,
        history: HistoryPatch,
        entries: EntriesPatch,
    },
    Delta {
        delta: Delta,
    },
}
#[derive(Serialize, Deserialize)]
pub(super) struct HistoryPatch {
    keep: usize,
    append: Vec<Arc<Value>>,
}
#[derive(Serialize, Deserialize)]
pub(super) struct EntriesPatch {
    length: usize,
    append: Vec<Entry>,
    updates: Vec<EntryUpdate>,
}
#[derive(Serialize, Deserialize)]
struct EntryUpdate {
    index: usize,
    entry: Entry,
}

fn same_entry(a: &Entry, b: &Entry) -> bool {
    a.id == b.id
        && a.kind == b.kind
        && a.images == b.images
        && a.replaces == b.replaces
        && a.input_batch == b.input_batch
        && (Arc::ptr_eq(&a.text, &b.text) || a.text == b.text)
}
impl Event {
    pub fn checkpoint(previous: &Record, next: &mut Record) -> Result<Self> {
        let entries = std::mem::take(&mut next.view.entries);
        let history = next.engine.take_history();
        // No historical text or model items are present in this serialization.
        let encoded = serde_json::to_value(&*next);
        next.view.entries = entries;
        next.engine.restore_history(history);
        let metadata = encoded?;
        let current = next.engine.history_items();
        let old = previous.engine.history_items();
        let keep = old
            .iter()
            .zip(current)
            .take_while(|(a, b)| Arc::ptr_eq(a, b))
            .count();
        let old_entries = &previous.view.entries;
        let new_entries = &next.view.entries;
        let common = old_entries.len().min(new_entries.len());
        let updates = (0..common)
            .filter(|&i| !same_entry(&old_entries[i], &new_entries[i]))
            .map(|index| EntryUpdate {
                index,
                entry: new_entries[index].clone(),
            })
            .collect();
        Ok(Self {
            seq: next.journal_seq,
            base: previous.journal_seq,
            change: Change::Checkpoint {
                metadata,
                history: HistoryPatch {
                    keep,
                    append: current[keep..].to_vec(),
                },
                entries: EntriesPatch {
                    length: new_entries.len(),
                    append: new_entries[common..].to_vec(),
                    updates,
                },
            },
        })
    }
    pub fn apply(self, record: &mut Record) -> Result<()> {
        if self.seq <= record.journal_seq {
            return Ok(());
        }
        if self.base != record.journal_seq
            || self.seq
                != self
                    .base
                    .checked_add(1)
                    .context("Journal sequence overflow")?
        {
            bail!("Session journal is not consecutive; original retained");
        }
        match self.change {
            Change::Delta { delta } => apply_delta(record, &delta)?,
            Change::Checkpoint {
                metadata,
                history,
                entries,
            } => {
                let mut next: Record = serde_json::from_value(metadata)?;
                if next.view.id != record.view.id
                    || !matches!(next.version, 7 | 8)
                    || next.journal_seq != self.seq
                {
                    bail!("Checkpoint identity/version/sequence differs; original retained");
                }
                let mut items = record.engine.take_history();
                if history.keep > items.len() {
                    bail!("Invalid checkpoint history prefix");
                }
                items.truncate(history.keep);
                items.extend(history.append);
                next.engine.restore_history(items);
                let mut text = std::mem::take(&mut record.view.entries);
                let common = text.len().min(entries.length);
                text.truncate(common);
                for update in entries.updates {
                    if update.index >= common {
                        bail!("Invalid checkpoint entry index");
                    }
                    text[update.index] = update.entry;
                }
                text.extend(entries.append);
                if text.len() != entries.length {
                    bail!("Invalid checkpoint entry length");
                }
                next.view.entries = text;
                *record = next;
            }
        }
        record.journal_seq = self.seq;
        Ok(())
    }
}
pub(super) fn apply_delta(record: &mut Record, delta: &Delta) -> Result<()> {
    if delta.seq
        != record
            .stream_seq
            .checked_add(1)
            .context("Stream sequence overflow")?
    {
        bail!("Session stream journal is not consecutive; original retained");
    }
    let entry = record
        .view
        .entries
        .iter_mut()
        .rev()
        .find(|entry| entry.id == delta.entry)
        .context("Stream entry missing from checkpoint; original retained")?;
    Arc::make_mut(&mut entry.text).push_str(&delta.text);
    if entry.kind == "assistant" {
        record.engine.partial_text.push_str(&delta.text);
    }
    record.stream_seq = delta.seq;
    Ok(())
}

#[derive(Default)]
pub(super) struct Compactions {
    active: Mutex<Option<String>>,
    changed: Condvar,
    #[cfg(test)]
    gate: Mutex<Option<(std::sync::mpsc::Sender<()>, std::sync::mpsc::Receiver<()>)>>,
}
impl Compactions {
    pub fn try_begin(&self, id: &str) -> bool {
        let mut active = self.active.lock().unwrap();
        if active.is_some() {
            return false;
        }
        *active = Some(id.into());
        true
    }
    fn finish(&self) {
        *self.active.lock().unwrap() = None;
        self.changed.notify_all();
    }
    pub fn wait(&self, id: Option<&str>) {
        let mut active = self.active.lock().unwrap();
        while active
            .as_deref()
            .is_some_and(|current| id.is_none_or(|id| current == id))
        {
            active = self.changed.wait(active).unwrap();
        }
    }
}
// Called only by the FIFO writer. Rotation is small; the full snapshot never
// occupies the writer lane or its effect barriers. One snapshot job per server.
pub(super) fn compact(hub: &Hub, id: &str, record: &Record) -> Result<bool> {
    if !hub.0.compactions.try_begin(id) {
        return Ok(false);
    }
    if let Err(error) = hub.0.store.rotate_events(id, record.journal_seq) {
        hub.0.compactions.finish();
        return Err(error);
    }
    let owner = hub.clone();
    let id = id.to_owned();
    let record = record.clone();
    let started = std::thread::Builder::new()
        .name("boid-snapshot".into())
        .spawn(move || {
            struct Done {
                owner: Option<Hub>,
                completion: Arc<Compactions>,
            }
            impl Drop for Done {
                fn drop(&mut self) {
                    // Release this job's state-directory owner before shutdown can
                    // observe completion and immediately reopen the store.
                    drop(self.owner.take());
                    self.completion.finish();
                }
            }
            let completion = owner.0.compactions.clone();
            let done = Done {
                owner: Some(owner),
                completion,
            };
            let owner = done.owner.as_ref().unwrap();
            #[cfg(test)]
            if let Some((started, release)) = owner.0.compactions.gate.lock().unwrap().take() {
                let _ = started.send(());
                let _ = release.recv();
            }
            let result = owner
                .0
                .store
                .save(&id, &record)
                .and_then(|()| owner.0.store.prune_events(&id, record.journal_seq));
            if let Err(error) = result {
                // Journals remain authoritative. A failed optional compaction is not
                // a failed already-durable tool/input transition.
                eprintln!("Session snapshot compaction failed; journal retained: {error:#}");
            }
        });
    if let Err(error) = started {
        hub.0.compactions.finish();
        return Err(error.into());
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Record {
        Record {
            version: 7,
            journal_seq: 0,
            stream_seq: 0,
            view: View {
                id: Uuid::new_v4().to_string(),
                name: "journal-fixture".into(),
                cwd: "/tmp".into(),
                status: "idle".into(),
                archived: false,
                profile: json!({}),
                questions: vec![],
                context_usage: None,
                context_window: None,
                communications: vec![],
                working_since_ms: None,
                retry_at_ms: None,
                entries: vec![Hub::make_entry("assistant", "retained result".into())],
                pending: vec![],
            },
            engine: Session::default(),
            usage: Default::default(),
            inputs: VecDeque::new(),
            inbox: VecDeque::new(),
            messages: vec![],
            order: 0,
            stop_through: 0,
            paused: false,
            listen_on_restart: Some(true),
            peer_restartable: Some(false),
        }
    }

    #[test]
    fn checkpoint_patch_preserves_context_and_excludes_unchanged_bulk() {
        let mut before = fixture();
        before.engine.restore_history(vec![
            Arc::new(json!({"role":"user","content":"x".repeat(1024*1024)})),
            Arc::new(json!({"type":"reasoning","encrypted_content":"opaque retained fixture"})),
            Arc::new(json!({"type":"function_call","call_id":"tool-1","name":"read_file","arguments":"{}"})),
        ]);
        before
            .view
            .entries
            .push(Hub::make_entry("tool", "y".repeat(1024 * 1024)));
        let mut next = before.clone();
        let mut history = next.engine.take_history();
        history.push(Arc::new(
            json!({"type":"function_call_output","call_id":"tool-1","output":"new result"}),
        ));
        next.engine.restore_history(history);
        next.view.name = "renamed".into();
        next.order = 4;
        next.paused = true;
        next.journal_seq = 1;
        next.view.entries[0].text = Arc::new("amended small entry".into());
        next.view
            .entries
            .push(Hub::make_entry("assistant", "new response".into()));
        let expected = serde_json::to_vec(&next).unwrap();
        let event = Event::checkpoint(&before, &mut next).unwrap();
        let bytes = serde_json::to_vec(&event).unwrap();
        assert!(
            bytes.len() < 16 * 1024,
            "checkpoint repeated old large payloads: {} bytes",
            bytes.len()
        );
        serde_json::from_slice::<Event>(&bytes)
            .unwrap()
            .apply(&mut before)
            .unwrap();
        assert_eq!(serde_json::to_vec(&before).unwrap(), expected);
        // Clear/compaction replace history instead of retaining an invalid prefix.
        next.engine.restore_history(vec![Arc::new(
            json!({"type":"compaction","encrypted_content":"replacement context"}),
        )]);
        next.view.entries.truncate(1);
        next.journal_seq = 2;
        Event::checkpoint(&before, &mut next)
            .unwrap()
            .apply(&mut before)
            .unwrap();
        assert_eq!(
            serde_json::to_vec(&before).unwrap(),
            serde_json::to_vec(&next).unwrap()
        );
    }

    #[test]
    fn journal_replay_keeps_stream_order_and_rejects_sequence_gaps() {
        let state = tempfile::tempdir().unwrap();
        let store = SessionStore::open(state.path().into()).unwrap();
        let mut record = fixture();
        let id = record.view.id.clone();
        store.save(&id, &record).unwrap();
        let delta = Event {
            seq: 1,
            base: 0,
            change: Change::Delta {
                delta: Delta {
                    seq: 1,
                    entry: record.view.entries[0].id.clone(),
                    text: " 🦉 streamed".into(),
                },
            },
        };
        store.append_events(&id, &[&delta]).unwrap();
        delta.apply(&mut record).unwrap();
        let mut next = record.clone();
        next.journal_seq = 2;
        next.paused = true;
        store
            .append_events(&id, &[Event::checkpoint(&record, &mut next).unwrap()])
            .unwrap();
        let mut replayed: Record = store.read_session(&id).unwrap();
        store
            .replay_events::<Event>(&id, |e| e.apply(&mut replayed))
            .unwrap();
        assert_eq!(
            serde_json::to_vec(&replayed).unwrap(),
            serde_json::to_vec(&next).unwrap()
        );
        // Publishing a compact base retains identical replay semantics when its
        // old segment remains after a crash before reclamation.
        store.rotate_events(&id, 2).unwrap();
        store.save(&id, &next).unwrap();
        store
            .replay_events::<Event>(&id, |e| e.apply(&mut replayed))
            .unwrap();
        assert_eq!(replayed.journal_seq, 2);
        let gap = Event {
            seq: 4,
            base: 3,
            change: Change::Delta {
                delta: Delta {
                    seq: 2,
                    entry: record.view.entries[0].id.clone(),
                    text: "invalid".into(),
                },
            },
        };
        assert!(gap.apply(&mut replayed).is_err());
        assert_eq!(
            serde_json::to_vec(&replayed).unwrap(),
            serde_json::to_vec(&next).unwrap()
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn held_compaction_allows_new_checkpoints_and_cannot_resurrect_ended_session() {
        let state = tempfile::tempdir().unwrap();
        let work = tempfile::tempdir().unwrap();
        let hub = Hub::open(Client::test_client(), state.path().into()).unwrap();
        let view = hub
            .create(Create {
                name: "snapshot-order".into(),
                cwd: work.path().to_string_lossy().into(),
                message: String::new(),
            })
            .unwrap();
        hub.entry(&view.id, "assistant", "first retained result");
        let worker = hub
            .0
            .data
            .lock()
            .unwrap()
            .sessions
            .get_mut(&view.id)
            .unwrap()
            .task
            .take();
        if let Some(worker) = worker {
            worker.abort();
            let _ = worker.await;
        }
        let lane = hub.0.data.lock().unwrap().sessions[&view.id]
            .persistence
            .capacity();
        let before = lane.durable_record().unwrap().unwrap();
        let (started, ready) = std::sync::mpsc::channel();
        let (release, wait) = std::sync::mpsc::channel();
        *hub.0.compactions.gate.lock().unwrap() = Some((started, wait));
        assert!(compact(&hub, &view.id, &before).unwrap());
        ready.recv_timeout(Duration::from_secs(3)).unwrap();
        hub.entry(
            &view.id,
            "assistant",
            "new checkpoint while snapshot blocked",
        );
        let newest = lane.durable_record().unwrap().unwrap();
        assert!(newest.journal_seq > before.journal_seq);
        let mut replayed: Record = hub.0.store.read_session(&view.id).unwrap();
        hub.0
            .store
            .replay_events::<Event>(&view.id, |e| e.apply(&mut replayed))
            .unwrap();
        assert_eq!(
            serde_json::to_vec(&replayed).unwrap(),
            serde_json::to_vec(&newest).unwrap()
        );
        let end_hub = hub.clone();
        let id = view.id.clone();
        let ending = tokio::spawn(async move { end_hub.close(&id).await });
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(!ending.is_finished(), "End bypassed the compaction fence");
        release.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(5), ending)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(
            !state
                .path()
                .join("sessions")
                .join(format!("{}.json", view.id))
                .exists()
        );
        let archived = hub.ended_record(&view.id);
        assert!(
            archived
                .view
                .entries
                .iter()
                .any(|e| e.text.as_str() == "new checkpoint while snapshot blocked")
        );
        assert!(archived.view.archived);
        hub.shutdown().await;
    }
}
