use super::persistence::Operation;
use super::*;
use std::time::Duration;

// Drop also releases the writer when an assertion fails, so a failed regression
// does not leave a runtime or storage thread permanently waiting on its fixture.
struct WriterGate(Option<std::sync::mpsc::Sender<()>>);
impl WriterGate {
    fn release(mut self) {
        if let Some(release) = self.0.take() {
            let _ = release.send(());
        }
    }
}
impl Drop for WriterGate {
    fn drop(&mut self) {
        if let Some(release) = self.0.take() {
            let _ = release.send(());
        }
    }
}

async fn hold_writer(hub: &Hub, id: &str) -> WriterGate {
    let (started, release) = {
        let mut data = hub.0.data.lock().unwrap();
        data.sessions.get_mut(id).unwrap().persistence.gate()
    };
    let gate = WriterGate(Some(release));
    tokio::task::spawn_blocking(move || started.recv_timeout(Duration::from_secs(3)))
        .await
        .unwrap()
        .expect("session writer did not reach its gate");
    gate
}

async fn idle_agent(hub: &Hub, work: &std::path::Path, name: &str) -> View {
    let view = hub
        .create(Create {
            name: name.into(),
            cwd: work.to_string_lossy().into_owned(),
            message: String::new(),
        })
        .unwrap();
    // Leave input consumption under test control. No model endpoint is called.
    let task = {
        let mut data = hub.0.data.lock().unwrap();
        data.sessions.get_mut(&view.id).unwrap().task.take()
    };
    if let Some(task) = task {
        task.abort();
        let _ = task.await;
    }
    view
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn accepted_messages_are_real_server_state_while_storage_is_blocked() {
    let state = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let hub = Hub::open(Client::test_client(), state.path().into()).unwrap();
    let target = idle_agent(&hub, work.path(), "queued").await;
    let other = idle_agent(&hub, work.path(), "independent").await;
    let (_, mut events) = hub.subscribe();
    let gate = hold_writer(&hub, &target.id).await;
    let submit_hub = hub.clone();
    let id = target.id.clone();
    let accepted = tokio::time::timeout(
        Duration::from_secs(1),
        tokio::task::spawn_blocking(move || {
            (0..3)
                .map(|n| submit_hub.message(&id, format!("accepted-{n}")))
                .collect::<Result<Vec<_>>>()
        }),
    )
    .await
    .expect("message acceptance waited for disk")
    .unwrap()
    .unwrap();

    let (before, order) = {
        let data = hub.0.data.lock().unwrap();
        let record = &data.sessions[&target.id].record;
        assert_eq!(record.inputs.len(), 3);
        assert_eq!(record.view.pending, accepted);
        (serde_json::to_value(record).unwrap(), record.order)
    };
    let mut emitted = Vec::new();
    while let Ok(event) = events.try_recv() {
        if event["type"] == "entry" && event["session_id"] == target.id {
            if let Some(id) = event["entry"]["id"].as_str() {
                emitted.push(id.to_owned());
            }
        }
    }
    for id in &accepted {
        assert!(
            emitted.contains(id),
            "accepted input lacked its server event"
        );
    }
    assert!(hub.message(&target.id, "fourth must wait".into()).is_err());
    {
        let data = hub.0.data.lock().unwrap();
        let record = &data.sessions[&target.id].record;
        assert_eq!(record.order, order);
        assert_eq!(serde_json::to_value(record).unwrap(), before);
    }

    let independent_hub = hub.clone();
    let other_id = other.id.clone();
    tokio::time::timeout(
        Duration::from_secs(1),
        tokio::task::spawn_blocking(move || {
            independent_hub.rename(&other_id, "still-responsive".into())
        }),
    )
    .await
    .expect("another session waited for the blocked writer")
    .unwrap()
    .unwrap();
    let persisted: Record = serde_json::from_slice(
        &std::fs::read(
            state
                .path()
                .join("sessions")
                .join(format!("{}.json", other.id)),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(persisted.view.name, "still-responsive");

    gate.release();
    // A durable update fences all preceding message saves, freeing capacity.
    hub.rename(&target.id, "drained".into()).unwrap();
    hub.message(&target.id, "accepted-after-drain".into())
        .unwrap();
    hub.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn newer_stream_delta_survives_an_earlier_queued_snapshot() {
    let state = tempfile::tempdir().unwrap();
    let restored_state = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let hub = Hub::open(Client::test_client(), state.path().into()).unwrap();
    let target = idle_agent(&hub, work.path(), "stream-order").await;
    let entry = hub.entry(&target.id, "assistant", "");
    let gate = hold_writer(&hub, &target.id).await;
    let receipt = {
        let mut data = hub.0.data.lock().unwrap();
        let handle = data.sessions.get_mut(&target.id).unwrap();
        let record = handle.record.clone();
        let seq = record.stream_seq + 1;
        handle
            .persistence
            .enqueue(Operation::Snapshot(Box::new(record)))
            .unwrap();
        handle
            .persistence
            .enqueue(Operation::Delta(Delta {
                seq,
                entry: entry.clone(),
                text: "newer output".into(),
            }))
            .unwrap()
    };
    gate.release();
    tokio::task::spawn_blocking(move || receipt.wait())
        .await
        .unwrap()
        .unwrap();

    // Copy precisely the durable checkpoint and journal into a crash-recovery
    // fixture. Graceful shutdown would create another checkpoint first.
    let source = state.path().join("sessions");
    let destination = restored_state.path().join("sessions");
    std::fs::create_dir_all(&destination).unwrap();
    for extension in ["json", "stream"] {
        let filename = format!("{}.{}", target.id, extension);
        std::fs::copy(source.join(&filename), destination.join(filename)).unwrap();
    }
    let saved: Record = serde_json::from_slice(
        &std::fs::read(destination.join(format!("{}.json", target.id))).unwrap(),
    )
    .unwrap();
    if let Some(snapshot) = saved.engine.skill_snapshot {
        let directory = restored_state.path().join("skill-snapshots");
        std::fs::create_dir_all(&directory).unwrap();
        let filename = format!("{snapshot}.json");
        std::fs::copy(
            state.path().join("skill-snapshots").join(&filename),
            directory.join(filename),
        )
        .unwrap();
    }
    let restored = Hub::open(Client::test_client(), restored_state.path().into()).unwrap();
    let view = restored.view(&target.id).unwrap();
    assert_eq!(
        view.entries.iter().find(|e| e.id == entry).unwrap().text,
        "newer output"
    );
    restored.shutdown().await;
    hub.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn tool_claim_waits_until_running_call_is_durable() {
    let state = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let hub = Hub::open(Client::test_client(), state.path().into()).unwrap();
    let target = idle_agent(&hub, work.path(), "claim-fence").await;
    let mut serialized = {
        let data = hub.0.data.lock().unwrap();
        serde_json::to_value(&data.sessions[&target.id].record.engine).unwrap()
    };
    serialized["running"] = json!({
        "id":"durable-before-effect", "name":"write_file",
        "arguments":"{}", "argument_entry":null
    });
    let session: Session = serde_json::from_value(serialized).unwrap();
    let gate = hold_writer(&hub, &target.id).await;
    let claiming = hub.clone();
    let id = target.id.clone();
    let mut claim = tokio::task::spawn_blocking(move || claiming.claim_tool(&id, &session));
    assert!(
        tokio::time::timeout(Duration::from_millis(100), &mut claim)
            .await
            .is_err()
    );
    gate.release();
    assert!(
        tokio::time::timeout(Duration::from_secs(3), claim)
            .await
            .unwrap()
            .unwrap()
            .unwrap()
    );
    let saved: Value = serde_json::from_slice(
        &std::fs::read(
            state
                .path()
                .join("sessions")
                .join(format!("{}.json", target.id)),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(saved["engine"]["running"]["id"], "durable-before-effect");
    hub.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn failed_checkpoint_prevents_a_pending_tool_side_effect() {
    let state = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let hub = Hub::open(Client::test_client(), state.path().into()).unwrap();
    let target = idle_agent(&hub, work.path(), "storage-failure").await;
    let mut serialized = {
        let data = hub.0.data.lock().unwrap();
        serde_json::to_value(&data.sessions[&target.id].record.engine).unwrap()
    };
    serialized["pending"] = json!([{
        "id":"must-not-run",
        "name":"write_file",
        "arguments":json!({"path":"forbidden-effect","content":"not durable"}).to_string(),
        "argument_entry":null
    }]);
    let mut session: Session = serde_json::from_value(serialized).unwrap();
    let checkpoint = state
        .path()
        .join("sessions")
        .join(format!("{}.json", target.id));
    let previous = std::fs::read(&checkpoint).unwrap();
    std::fs::remove_file(&checkpoint).unwrap();
    std::fs::create_dir(&checkpoint).unwrap();
    let tools = Tools::new(work.path()).unwrap();
    let result = session.step(&hub.0.client, &tools, &hub, &target.id).await;
    // entry_replacing reports a storage fault through the Hub. The subsequent
    // claim can then return false, leaving the call pending instead of returning
    // an error from step. Either path must keep the effect unexecuted.
    if result.is_ok() {
        let after = serde_json::to_value(&session).unwrap();
        assert!(after["running"].is_null());
        assert!(
            after["pending"]
                .as_array()
                .unwrap()
                .iter()
                .any(|call| call["id"] == "must-not-run")
        );
    }
    assert!(!work.path().join("forbidden-effect").exists());
    assert_eq!(hub.view(&target.id).unwrap().status, "error");
    assert!(hub.message(&target.id, "cannot continue".into()).is_err());
    hub.shutdown().await;
    std::fs::remove_dir(&checkpoint).unwrap();
    std::fs::write(checkpoint, previous).unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn graceful_shutdown_drains_accepted_inputs_before_returning() {
    let state = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let hub = Hub::open(Client::test_client(), state.path().into()).unwrap();
    let target = idle_agent(&hub, work.path(), "shutdown-drain").await;
    let gate = hold_writer(&hub, &target.id).await;
    let submitting = hub.clone();
    let id = target.id.clone();
    let message = tokio::time::timeout(
        Duration::from_secs(1),
        tokio::task::spawn_blocking(move || submitting.message(&id, "retain on shutdown".into())),
    )
    .await
    .expect("message acceptance waited for disk")
    .unwrap()
    .unwrap();
    let closing = hub.clone();
    let mut shutdown = tokio::spawn(async move { closing.shutdown().await });
    assert!(
        tokio::time::timeout(Duration::from_millis(100), &mut shutdown)
            .await
            .is_err()
    );
    gate.release();
    tokio::time::timeout(Duration::from_secs(3), shutdown)
        .await
        .unwrap()
        .unwrap();
    let saved: Record = serde_json::from_slice(
        &std::fs::read(
            state
                .path()
                .join("sessions")
                .join(format!("{}.json", target.id)),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(
        saved
            .inputs
            .iter()
            .filter(|input| input.id == message)
            .count(),
        1
    );
    assert_eq!(
        saved
            .view
            .entries
            .iter()
            .filter(|entry| entry.id == message)
            .count(),
        1
    );
    assert_eq!(
        saved
            .view
            .pending
            .iter()
            .filter(|id| **id == message)
            .count(),
        1
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn saturated_storage_waits_without_faulting_or_blocking_the_roster() {
    let state = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let hub = Hub::open(Client::test_client(), state.path().into()).unwrap();
    let target = idle_agent(&hub, work.path(), "saturated").await;
    let other = idle_agent(&hub, work.path(), "unblocked").await;
    let entry = hub.entry(&target.id, "assistant", "");
    let gate = hold_writer(&hub, &target.id).await;
    {
        let mut data = hub.0.data.lock().unwrap();
        let handle = data.sessions.get_mut(&target.id).unwrap();
        // One active gate plus fifteen queued snapshots exhaust the bound.
        for _ in 0..15 {
            handle
                .persistence
                .enqueue(Operation::Snapshot(Box::new(handle.record.clone())))
                .unwrap();
        }
    }
    let before =
        serde_json::to_value(&hub.0.data.lock().unwrap().sessions[&target.id].record).unwrap();
    assert!(
        hub.message(&target.id, "retry without accepting".into())
            .is_err()
    );
    assert_eq!(
        serde_json::to_value(&hub.0.data.lock().unwrap().sessions[&target.id].record).unwrap(),
        before
    );

    let writing = hub.clone();
    let id = target.id.clone();
    let entry_id = entry.clone();
    let (started, ready) = tokio::sync::oneshot::channel();
    let mut delta = tokio::task::spawn_blocking(move || {
        started.send(()).unwrap();
        writing.delta(&id, &entry_id, "retained after backpressure");
    });
    ready.await.unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(100), &mut delta)
            .await
            .is_err(),
        "a strict write must wait for capacity, not escalate saturation to a fault"
    );
    assert!(
        hub.0.data.lock().unwrap().sessions[&target.id]
            .storage_error
            .is_none()
    );
    let independent = hub.clone();
    let other_id = other.id.clone();
    tokio::time::timeout(
        Duration::from_secs(1),
        tokio::task::spawn_blocking(move || {
            independent.rename(&other_id, "still-unblocked".into())
        }),
    )
    .await
    .unwrap()
    .unwrap()
    .unwrap();
    gate.release();
    tokio::time::timeout(Duration::from_secs(3), delta)
        .await
        .unwrap()
        .unwrap();
    assert!(
        hub.0.data.lock().unwrap().sessions[&target.id]
            .storage_error
            .is_none()
    );
    assert_eq!(
        hub.view(&target.id)
            .unwrap()
            .entries
            .iter()
            .find(|e| e.id == entry)
            .unwrap()
            .text,
        "retained after backpressure"
    );
    hub.shutdown().await;
}

async fn wait_for_writer_exit(hub: &Hub, id: &str) {
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if !hub.0.data.lock().unwrap().sessions[id]
                .persistence
                .is_running()
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("empty persistence lane retained a running worker");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn idle_and_archived_lanes_release_workers_and_restart_on_demand() {
    let state = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let hub = Hub::open(Client::test_client(), state.path().into()).unwrap();
    let target = idle_agent(&hub, work.path(), "lazy-writer").await;
    wait_for_writer_exit(&hub, &target.id).await;
    let gate = hold_writer(&hub, &target.id).await;
    assert!(
        hub.0.data.lock().unwrap().sessions[&target.id]
            .persistence
            .is_running()
    );
    gate.release();
    wait_for_writer_exit(&hub, &target.id).await;
    hub.rename(&target.id, "new-drain".into()).unwrap();
    wait_for_writer_exit(&hub, &target.id).await;
    assert!(hub.close(&target.id).await.unwrap());
    wait_for_writer_exit(&hub, &target.id).await;
    hub.reopen(&target.id).unwrap();
    wait_for_writer_exit(&hub, &target.id).await;
    assert_eq!(hub.view(&target.id).unwrap().name, "new-drain");
    hub.shutdown().await;
}

#[test]
fn disk_progress_does_not_depend_on_available_tokio_blocking_workers() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .max_blocking_threads(1)
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let hub = Hub::new(Client::test_client()).unwrap();
        let mut lane = Lane::new(Arc::downgrade(&hub.0), Uuid::new_v4().to_string()).unwrap();
        let (occupied, ready) = tokio::sync::oneshot::channel();
        let (release_pool, wait_pool) = std::sync::mpsc::channel();
        let pool_guard = WriterGate(Some(release_pool));
        let pool_task = tokio::task::spawn_blocking(move || {
            occupied.send(()).unwrap();
            let _ = wait_pool.recv();
        });
        ready.await.unwrap();
        let (started, release_writer) = lane.gate();
        let writer_guard = WriterGate(Some(release_writer));
        // Do not use spawn_blocking for the observer: the pool is deliberately full.
        let progressed = started.recv_timeout(Duration::from_secs(1)).is_ok();
        pool_guard.release();
        writer_guard.release();
        pool_task.await.unwrap();
        assert!(
            progressed,
            "receipt waiters can starve the persistence writer"
        );
    });
}
