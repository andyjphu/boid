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
    // This fixture represents an already-used session, not an empty draft.
    hub.entry(&view.id, "assistant", "retained fixture work");
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
        let mut record = handle.record.clone();
        handle
            .persistence
            .enqueue(Operation::Snapshot(Box::new(record.clone())))
            .unwrap();
        for seq in 1..=2 {
            handle
                .persistence
                .enqueue(Operation::Delta(Delta {
                    seq,
                    entry: entry.clone(),
                    text: "before ".into(),
                }))
                .unwrap();
        }
        record.stream_seq = 2;
        record
            .view
            .entries
            .iter_mut()
            .find(|e| e.id == entry)
            .unwrap()
            .text = "before before ".to_owned().into();
        handle
            .persistence
            .enqueue(Operation::Snapshot(Box::new(record)))
            .unwrap();
        handle
            .persistence
            .enqueue(Operation::Delta(Delta {
                seq: 3,
                entry: entry.clone(),
                text: "newer ".into(),
            }))
            .unwrap();
        handle
            .persistence
            .enqueue(Operation::Delta(Delta {
                seq: 4,
                entry: entry.clone(),
                text: "output".into(),
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
        view.entries
            .iter()
            .find(|e| e.id == entry)
            .unwrap()
            .text
            .as_str(),
        "before before newer output"
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
            .text
            .as_str(),
        "retained after backpressure"
    );
    hub.shutdown().await;
}

async fn wait_for_writer_exit(hub: &Hub, id: &str) {
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if hub
                .0
                .data
                .lock()
                .unwrap()
                .sessions
                .get(id)
                .is_none_or(|h| !h.persistence.is_running())
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
    assert!(hub.reopen(&target.id).is_err());
    assert!(hub.view(&target.id).is_none());
    assert_eq!(hub.ended_record(&target.id).view.name, "new-drain");
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

// Synthetic gate check of clock boundaries, not a measurement of live-user latency.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn admission_timings_separate_roster_wait_from_locked_work() {
    let work = tempfile::tempdir().unwrap();
    let hub = Hub::new(Client::test_client()).unwrap();
    let target = idle_agent(&hub, work.path(), "timing-boundaries").await;
    let locking = hub.clone();
    let (locked, ready) = tokio::sync::oneshot::channel();
    let (release, wait) = std::sync::mpsc::channel();
    let gate = WriterGate(Some(release));
    let holder = tokio::task::spawn_blocking(move || {
        let _guard = locking.0.data.lock().unwrap();
        locked.send(()).unwrap();
        let _ = wait.recv();
    });
    ready.await.unwrap();
    let accepting = hub.clone();
    let id = target.id.clone();
    let (started, ready) = tokio::sync::oneshot::channel();
    let admission = tokio::task::spawn_blocking(move || {
        started.send(()).unwrap();
        accepting.message_timed(&id, "synthetic timing check".into())
    });
    ready.await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    gate.release();
    holder.await.unwrap();
    let (result, timings) = admission.await.unwrap();
    let message_id = result.unwrap();
    assert!(hub.view(&target.id).unwrap().pending.contains(&message_id));
    assert!(timings.lock_wait.unwrap() >= Duration::from_millis(20));
    assert!(timings.admission >= timings.lock_wait.unwrap() + timings.mutation.unwrap());
    let (result, timings) = hub.message_timed(&target.id, " ".into());
    assert!(result.is_err());
    assert!(timings.lock_wait.is_none() && timings.mutation.is_none());
    hub.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn admitted_stream_bytes_backpressure_before_mutation_and_fence_tools() {
    let state = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let hub = Hub::open(Client::test_client(), state.path().into()).unwrap();
    let target = idle_agent(&hub, work.path(), "delta-byte-bound").await;
    let entry = hub.entry(&target.id, "assistant", "");
    let gate = hold_writer(&hub, &target.id).await;
    let h = hub.clone();
    let id = target.id.clone();
    let e = entry.clone();
    tokio::time::timeout(
        Duration::from_secs(1),
        tokio::task::spawn_blocking(move || {
            h.delta(&id, &e, &"x".repeat(5000));
            h.delta(&id, &e, &"y".repeat(5000));
        }),
    )
    .await
    .expect("admitted deltas waited for disk")
    .unwrap();
    assert_eq!(
        hub.0.data.lock().unwrap().sessions[&target.id]
            .record
            .stream_seq,
        2
    );
    let h = hub.clone();
    let id = target.id.clone();
    let e = entry.clone();
    let mut third = tokio::task::spawn_blocking(move || h.delta(&id, &e, &"z".repeat(5000)));
    assert!(
        tokio::time::timeout(Duration::from_millis(50), &mut third)
            .await
            .is_err()
    );
    assert_eq!(
        hub.0.data.lock().unwrap().sessions[&target.id]
            .record
            .stream_seq,
        2
    );
    assert_eq!(hub.view(&target.id).unwrap().status, "idle");
    let mut serialized = serde_json::to_value(
        &hub.0.data.lock().unwrap().sessions[&target.id]
            .record
            .engine,
    )
    .unwrap();
    serialized["running"] =
        json!({"id":"fenced", "name":"write_file", "arguments":"{}", "argument_entry":null});
    let session: Session = serde_json::from_value(serialized).unwrap();
    let h = hub.clone();
    let id = target.id.clone();
    let mut claim = tokio::task::spawn_blocking(move || h.claim_tool(&id, &session));
    assert!(
        tokio::time::timeout(Duration::from_millis(50), &mut claim)
            .await
            .is_err()
    );
    gate.release();
    tokio::time::timeout(Duration::from_secs(3), third)
        .await
        .unwrap()
        .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_secs(3), claim)
            .await
            .unwrap()
            .unwrap()
            .unwrap()
    );
    // The third delta can be ordered after the tool claim; fence it separately.
    let session = hub.0.data.lock().unwrap().sessions[&target.id]
        .record
        .engine
        .clone();
    hub.checkpoint(&target.id, &session).unwrap();
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
    assert_eq!(saved.stream_seq, 3);
    assert_eq!(
        saved
            .view
            .entries
            .iter()
            .find(|e| e.id == entry)
            .unwrap()
            .text
            .len(),
        15000
    );
    hub.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn admitted_delta_failure_prevents_tool_claim_and_releases_waiters() {
    let state = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let hub = Hub::open(Client::test_client(), state.path().into()).unwrap();
    let target = idle_agent(&hub, work.path(), "delta-failure").await;
    let entry = hub.entry(&target.id, "assistant", "");
    let gate = hold_writer(&hub, &target.id).await;
    let h = hub.clone();
    let id = target.id.clone();
    let e = entry.clone();
    tokio::time::timeout(
        Duration::from_secs(1),
        tokio::task::spawn_blocking(move || {
            h.delta(&id, &e, "admitted before disk failure");
        }),
    )
    .await
    .unwrap()
    .unwrap();
    let stream = state
        .path()
        .join("sessions")
        .join(format!("{}.stream", target.id));
    std::fs::remove_file(&stream).unwrap();
    std::fs::create_dir(&stream).unwrap();
    let mut serialized = serde_json::to_value(
        &hub.0.data.lock().unwrap().sessions[&target.id]
            .record
            .engine,
    )
    .unwrap();
    serialized["running"] = json!({"id":"must-not-execute", "name":"write_file", "arguments":"{}", "argument_entry":null});
    let session: Session = serde_json::from_value(serialized).unwrap();
    let h = hub.clone();
    let id = target.id.clone();
    let claim = tokio::task::spawn_blocking(move || h.claim_tool(&id, &session));
    gate.release();
    assert!(!matches!(
        tokio::time::timeout(Duration::from_secs(3), claim)
            .await
            .unwrap()
            .unwrap(),
        Ok(true)
    ));
    assert_eq!(hub.view(&target.id).unwrap().status, "error");
    assert!(hub.message(&target.id, "must not accept".into()).is_err());
    hub.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn oversized_delta_waits_for_exclusive_capacity_and_durable_receipt() {
    let state = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let hub = Hub::open(Client::test_client(), state.path().into()).unwrap();
    let target = idle_agent(&hub, work.path(), "large-delta").await;
    let entry = hub.entry(&target.id, "assistant", "");
    let gate = hold_writer(&hub, &target.id).await;
    let h = hub.clone();
    let id = target.id.clone();
    let e = entry.clone();
    let mut writing = tokio::task::spawn_blocking(move || h.delta(&id, &e, &"x".repeat(70000)));
    assert!(
        tokio::time::timeout(Duration::from_millis(50), &mut writing)
            .await
            .is_err()
    );
    assert_eq!(
        hub.0.data.lock().unwrap().sessions[&target.id]
            .record
            .stream_seq,
        0
    );
    gate.release();
    tokio::time::timeout(Duration::from_secs(3), writing)
        .await
        .unwrap()
        .unwrap();
    let deltas = hub.0.store.deltas::<Delta>(&target.id).unwrap();
    assert_eq!(deltas.len(), 1);
    assert_eq!(deltas[0].text.len(), 70000);
    hub.shutdown().await;
}

// Isolated filesystem fixture. No provider or agent requests. Include the final
// durability fence so async admission cannot hide an unpaid persistence cost.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn stream_persistence_fixture() {
    for paced in [false, true] {
        for round in 0..3 {
            let state = tempfile::tempdir().unwrap();
            let work = tempfile::tempdir().unwrap();
            let hub = Hub::open(Client::test_client(), state.path().into()).unwrap();
            let target = idle_agent(&hub, work.path(), "stream-fixture").await;
            let entry = hub.entry(&target.id, "assistant", "");
            let start = std::time::Instant::now();
            for _ in 0..128 {
                hub.delta(&target.id, &entry, &"x".repeat(256));
                if paced {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            }
            let admitted_ms = start.elapsed().as_secs_f64() * 1000.;
            let session = hub.0.data.lock().unwrap().sessions[&target.id]
                .record
                .engine
                .clone();
            hub.checkpoint(&target.id, &session).unwrap();
            let fenced_ms = start.elapsed().as_secs_f64() * 1000.;
            let record: Record = serde_json::from_slice(
                &std::fs::read(
                    state
                        .path()
                        .join("sessions")
                        .join(format!("{}.json", target.id)),
                )
                .unwrap(),
            )
            .unwrap();
            assert_eq!(record.stream_seq, 128);
            assert_eq!(
                record
                    .view
                    .entries
                    .iter()
                    .find(|e| e.id == entry)
                    .unwrap()
                    .text
                    .len(),
                128 * 256
            );
            println!(
                "{{\"paced\":{paced},\"round\":{round},\"records\":128,\"admitted_ms\":{admitted_ms:.3},\"fenced_ms\":{fenced_ms:.3}}}"
            );
            hub.shutdown().await;
        }
    }
}

// Bounded synthetic benchmark of actual Hub call boundaries. No provider calls.
// Run explicitly on the assigned native host; never report these as live timings.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "Bounded synthetic runtime benchmark, opt in on the assigned host"]
async fn bounded_runtime_call_benchmark() {
    let work = tempfile::tempdir().unwrap();
    let hub = Hub::new(Client::test_client()).unwrap();
    let target = idle_agent(&hub, work.path(), "bounded-runtime-benchmark").await;
    let mut session = {
        let mut data = hub.0.data.lock().unwrap();
        let r = &mut data.sessions.get_mut(&target.id).unwrap().record;
        let mut encoded = serde_json::to_value(&r.engine).unwrap();
        encoded["history"] = Value::Array(
            (0..128)
                .map(|_| {
                    json!({
                        "role":"assistant", "content":"x".repeat(64 * 1024)
                    })
                })
                .collect(),
        );
        let mut engine: Session = serde_json::from_value(encoded).unwrap();
        engine.skills = r.engine.skills.clone();
        r.engine = engine.clone();
        r.messages = (0..1493)
            .map(|i| Communication {
                id: format!("message-{i}"),
                sender_id: "synthetic-sender".into(),
                recipient_id: target.id.clone(),
                sender_name: "peer".into(),
                recipient_name: "subject".into(),
                call_id: format!("call-{i}"),
                sender_entry: format!("entry-{i}"),
                text: "p".repeat(700),
                urgency: Urgency::Regular,
                state: "applied".into(),
                applied_batch: Some(i),
                blocked: false,
                persisted: true,
            })
            .collect();
        engine
    };
    for trial in 0..3 {
        let start = std::time::Instant::now();
        assert!(hub.apply_inbox(&target.id, &mut session).unwrap());
        let empty_inbox = start.elapsed().as_secs_f64() * 1000.0;
        let start = std::time::Instant::now();
        hub.status(&target.id, "idle");
        let unchanged_status = start.elapsed().as_secs_f64() * 1000.0;
        let start = std::time::Instant::now();
        hub.status(&target.id, "working");
        let changed_status = start.elapsed().as_secs_f64() * 1000.0;
        hub.status(&target.id, "idle");
        eprintln!(
            "SYNTHETIC trial={trial} history_mib=8 peers=1493 empty_inbox_ms={empty_inbox:.3} unchanged_status_ms={unchanged_status:.3} changed_status_ms={changed_status:.3}"
        );
    }
    hub.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unchanged_status_and_empty_inbox_do_not_wait_for_storage() {
    let work = tempfile::tempdir().unwrap();
    let hub = Hub::new(Client::test_client()).unwrap();
    let target = idle_agent(&hub, work.path(), "no-op-state").await;
    let before = {
        let data = hub.0.data.lock().unwrap();
        serde_json::to_value(&data.sessions[&target.id].record).unwrap()
    };
    let mut session = hub.0.data.lock().unwrap().sessions[&target.id]
        .record
        .engine
        .clone();
    let gate = hold_writer(&hub, &target.id).await;
    let checking = hub.clone();
    let id = target.id.clone();
    let task = tokio::task::spawn_blocking(move || {
        assert!(checking.apply_inbox(&id, &mut session).unwrap());
        checking.status(&id, "idle");
    });
    tokio::time::timeout(Duration::from_secs(1), task)
        .await
        .expect("a no-op waited for persistence")
        .unwrap();
    assert_eq!(
        serde_json::to_value(&hub.0.data.lock().unwrap().sessions[&target.id].record).unwrap(),
        before
    );
    gate.release();
    {
        let mut data = hub.0.data.lock().unwrap();
        let record = &mut data.sessions.get_mut(&target.id).unwrap().record;
        record.view.status = "working".into();
        record.view.working_since_ms = None;
    }
    hub.status(&target.id, "working");
    assert!(hub.view(&target.id).unwrap().working_since_ms.is_some());
    {
        let mut data = hub.0.data.lock().unwrap();
        data.sessions.get_mut(&target.id).unwrap().storage_error =
            Some("synthetic storage failure".into());
    }
    let mut session = hub.0.data.lock().unwrap().sessions[&target.id]
        .record
        .engine
        .clone();
    assert!(hub.apply_inbox(&target.id, &mut session).is_err());
    hub.shutdown().await;
}

#[test]
fn communication_index_preserves_equality_with_duplicate_ids() {
    let first = Communication {
        id: "same-id".into(),
        sender_id: "a".into(),
        recipient_id: "b".into(),
        sender_name: "a".into(),
        recipient_name: "b".into(),
        call_id: "call".into(),
        sender_entry: "entry".into(),
        text: "first".into(),
        urgency: Urgency::Regular,
        state: "queued".into(),
        applied_batch: None,
        blocked: false,
        persisted: true,
    };
    let mut second = first.clone();
    second.text = "second".into();
    let messages = vec![first.clone(), second.clone()];
    let index = communication_index(&messages);
    assert!(contains_communication(&index, &first));
    assert!(contains_communication(&index, &second));
    let mut absent = first;
    absent.text = "not present".into();
    assert!(!contains_communication(&index, &absent));
}
