use super::*;

struct WriterRelease(std::sync::mpsc::Sender<()>);
impl Drop for WriterRelease {
    fn drop(&mut self) {
        let _ = self.0.send(());
    }
}

fn draft(hub: &Hub, cwd: &Path) -> View {
    hub.create(Create {
        name: String::new(),
        cwd: cwd.to_string_lossy().into_owned(),
        message: String::new(),
    })
    .unwrap()
}
fn abort_worker(hub: &Hub, id: &str) {
    if let Some(task) = hub
        .0
        .data
        .lock()
        .unwrap()
        .sessions
        .get_mut(id)
        .and_then(|h| h.task.take())
    {
        task.abort();
    }
}
fn durable(hub: &Hub, id: &str) {
    let receipt = hub.0.data.lock().unwrap().sessions[id]
        .persistence
        .barrier();
    receipt.wait().unwrap();
}
fn session_files(root: &Path) -> usize {
    std::fs::read_dir(root.join("sessions")).unwrap().count()
}

#[tokio::test]
async fn unused_and_configured_drafts_never_save_or_reserve_names() {
    let state = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let auth = Arc::new(
        crate::chatgpt::Auth::open(state.path().join("auth"), "api_key", "fixture").unwrap(),
    );
    auth.migrate_credentials("fixture-initial-key").unwrap();
    let added = auth
        .create_credential("Draft fixture", "fixture-configured-key")
        .await
        .unwrap();
    let mut client = Client::test_client();
    client.auth = Some(auth.clone());
    let hub = Hub::open(client, state.path().into()).unwrap();
    let first_available = hub.0.agent_names[0].clone();
    let mut drafts = vec![];
    for _ in 0..hub.0.agent_names.len() + 2 {
        let view = draft(&hub, work.path());
        assert!(view.entries.is_empty());
        assert!(!view.name.ends_with("-2"));
        let data = hub.0.data.lock().unwrap();
        assert!(data.sessions[&view.id].draft.is_some());
        assert!(data.sessions[&view.id].task.is_none());
        assert_eq!(
            next_agent_name(&data, &hub.0.agent_names, 0),
            first_available
        );
        drafts.push(view);
    }
    let id = &drafts[0].id;
    hub.rename(id, "deliberately named".into()).unwrap();
    let profile = hub.profile(id).unwrap();
    let mut changed = profile.clone();
    changed["model"] = json!("configured-fixture-model");
    hub.select_profile(id, &profile, changed.clone()).unwrap();
    hub.select_profile(id, &changed, changed.clone()).unwrap();
    let group = hub.profile(id).unwrap()["pane_group_id"]
        .as_str()
        .unwrap()
        .to_owned();
    hub.assign_group_credential(
        &group,
        added["id"].as_str(),
        hub.pane_groups()["revision"].as_u64().unwrap(),
    )
    .unwrap();
    let inherited = hub
        .create_in_group(
            Create {
                name: "explicit draft name".into(),
                cwd: work.path().to_string_lossy().into_owned(),
                message: String::new(),
            },
            1,
            Some(&group),
        )
        .unwrap();
    assert!(inherited.entries.is_empty());
    assert_eq!(inherited.profile["credential_id"], added["id"]);
    hub.interrupt(id).unwrap();
    hub.stop(id).unwrap();
    assert!(hub.message(id, "   ".into()).is_err());
    assert!(hub.message(id, "/login".into()).is_err());
    assert!(hub.rename(id, "bad\nname".into()).is_err());
    assert!(hub.view(id).unwrap().entries.is_empty());
    assert_eq!(session_files(state.path()), 0);
    // Browser snapshots still contain the same in-memory pane IDs.
    assert_eq!(
        hub.subscribe_screen(true, Some(1)).0["sessions"]
            .as_array()
            .unwrap()
            .len(),
        drafts.len() + 1
    );
    hub.close(&drafts[1].id).await.unwrap();
    assert!(hub.view(&drafts[1].id).is_none());
    assert_eq!(session_files(state.path()), 0);
    hub.shutdown().await;
    assert_eq!(session_files(state.path()), 0);
    drop(hub);
    let mut client = Client::test_client();
    client.auth = Some(auth);
    let restored = Hub::open(client, state.path().into()).unwrap();
    assert!(restored.views().is_empty());
    assert!(
        restored.pane_groups()["groups"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    restored.shutdown().await;
}

#[tokio::test]
async fn first_input_commits_context_and_input_without_waiting_for_storage() {
    let state = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let hub = Hub::open(Client::test_client(), state.path().into()).unwrap();
    let view = draft(&hub, work.path());
    let (started, release) = hub
        .0
        .data
        .lock()
        .unwrap()
        .sessions
        .get_mut(&view.id)
        .unwrap()
        .persistence
        .gate();
    let release = WriterRelease(release);
    started.recv_timeout(Duration::from_secs(2)).unwrap();
    // The first message must return while disk work is blocked. This test uses
    // a current-thread runtime; abort the worker before yielding, so no request runs.
    let input = hub.message(&view.id, "first real input".into()).unwrap();
    abort_worker(&hub, &view.id);
    {
        let data = hub.0.data.lock().unwrap();
        let handle = &data.sessions[&view.id];
        assert!(handle.draft.is_none());
        assert_eq!(handle.record.inputs[0].id, input);
        assert_eq!(handle.record.view.entries.len(), 2);
        assert_eq!(handle.record.view.entries[0].kind, "context");
        assert_eq!(
            handle.record.view.entries[1].text.as_str(),
            "first real input"
        );
    }
    assert_eq!(session_files(state.path()), 0);
    release.0.send(()).unwrap();
    durable(&hub, &view.id);
    let saved: Vec<Record> = hub.0.store.load().unwrap();
    assert_eq!(saved.len(), 1);
    assert_eq!(saved[0].inputs[0].id, input);
    assert_eq!(saved[0].view.entries[1].id, input);
    hub.shutdown().await;
    drop(hub);
    let restored = Hub::open(Client::test_client(), state.path().into()).unwrap();
    abort_worker(&restored, &view.id);
    assert!(
        restored
            .view(&view.id)
            .unwrap()
            .entries
            .iter()
            .any(|e| e.id == input)
    );
    assert!(
        restored.0.data.lock().unwrap().sessions[&view.id]
            .draft
            .is_none()
    );
    restored.shutdown().await;
}

#[tokio::test]
async fn simultaneous_activation_rechecks_provisional_names_and_preserves_explicit_names() {
    let state = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let hub = Hub::open(Client::test_client(), state.path().into()).unwrap();
    let one = draft(&hub, work.path());
    let two = draft(&hub, work.path());
    let provisional = hub.0.agent_names[0].clone();
    {
        let mut data = hub.0.data.lock().unwrap();
        data.sessions.get_mut(&one.id).unwrap().record.view.name = provisional.clone();
        data.sessions.get_mut(&two.id).unwrap().record.view.name = provisional.clone();
    }
    let runtime = tokio::runtime::Handle::current();
    let barrier = std::sync::Barrier::new(3);
    std::thread::scope(|scope| {
        for id in [&one.id, &two.id, &one.id] {
            let hub = &hub;
            let runtime = &runtime;
            let barrier = &barrier;
            scope.spawn(move || {
                let _runtime = runtime.enter();
                barrier.wait();
                hub.message(id, "accepted concurrently".into()).unwrap();
            });
        }
    });
    abort_worker(&hub, &one.id);
    abort_worker(&hub, &two.id);
    let one = hub.view(&one.id).unwrap();
    let two = hub.view(&two.id).unwrap();
    assert_ne!(one.name, two.name);
    assert!(one.name == provisional || two.name == provisional);
    for view in [&one, &two] {
        let data = hub.0.data.lock().unwrap();
        let record = &data.sessions[&view.id].record;
        assert_eq!(
            record
                .view
                .entries
                .iter()
                .filter(|e| e.kind == "context")
                .count(),
            1
        );
        assert!(
            record.view.entries[0]
                .text
                .contains(&format!("\"name\":\"{}\"", view.name))
        );
    }
    let custom = draft(&hub, work.path());
    hub.rename(&custom.id, "chosen name".into()).unwrap();
    assert!(
        hub.0.data.lock().unwrap().sessions[&custom.id]
            .draft
            .is_some()
    );
    hub.message(&custom.id, "activate custom".into()).unwrap();
    abort_worker(&hub, &custom.id);
    assert_eq!(hub.view(&custom.id).unwrap().name, "chosen name");
    hub.shutdown().await;
}

#[tokio::test]
async fn peer_input_activates_draft_and_records_final_recipient_name() {
    let state = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let hub = Hub::open(Client::test_client(), state.path().into()).unwrap();
    let sender = draft(&hub, work.path());
    hub.entry(&sender.id, "assistant", "recorded sender work");
    abort_worker(&hub, &sender.id);
    let target = draft(&hub, work.path());
    {
        let mut data = hub.0.data.lock().unwrap();
        data.sessions.get_mut(&target.id).unwrap().record.view.name = sender.name.clone();
    }
    hub.stop(&target.id).unwrap(); // A stopped draft still accepts durable peer input.
    let receipt = hub
        .send_communication(
            &sender.id,
            &target.id,
            "peer work",
            Urgency::Regular,
            "draft-delivery",
            "fixture-entry",
        )
        .unwrap();
    abort_worker(&hub, &target.id);
    assert_eq!(receipt["state"], "queued");
    assert_eq!(receipt["blocked"], true);
    let target = hub.view(&target.id).unwrap();
    assert_ne!(target.name, sender.name);
    {
        let data = hub.0.data.lock().unwrap();
        let handle = &data.sessions[&target.id];
        assert!(handle.draft.is_none());
        assert_eq!(handle.record.messages[0].recipient_name, target.name);
        assert_eq!(handle.record.inbox.len(), 1);
        assert_eq!(handle.record.view.entries[0].kind, "context");
        assert_eq!(handle.record.view.entries[1].kind, "peer");
    }
    let saved: Vec<Record> = hub.0.store.load().unwrap();
    assert_eq!(saved.len(), 2);
    hub.shutdown().await;
}

#[tokio::test]
async fn historical_context_only_records_remain_committed() {
    let state = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let hub = Hub::open(Client::test_client(), state.path().into()).unwrap();
    let view = draft(&hub, work.path());
    let mut historical = hub.0.data.lock().unwrap().sessions[&view.id].record.clone();
    historical
        .view
        .entries
        .push(Hub::initial_context(&historical));
    hub.save_record(&view.id, &mut historical).unwrap();
    hub.shutdown().await;
    drop(hub);
    let restored = Hub::open(Client::test_client(), state.path().into()).unwrap();
    assert_eq!(restored.view(&view.id).unwrap().name, view.name);
    assert!(
        restored.0.data.lock().unwrap().sessions[&view.id]
            .draft
            .is_none()
    );
    restored.shutdown().await;
}

#[tokio::test]
async fn end_and_first_input_preserve_both_admission_orders() {
    let state = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let hub = Hub::open(Client::test_client(), state.path().into()).unwrap();
    let empty = draft(&hub, work.path());
    let (closed, rejected) = tokio::join!(biased; hub.close(&empty.id), async {
        hub.message(&empty.id, "too late".into())
    });
    assert!(closed.unwrap());
    assert!(rejected.is_err());
    assert_eq!(session_files(state.path()), 0);

    let used = draft(&hub, work.path());
    let (accepted, closed) = tokio::join!(biased;
        async {
            let accepted = hub.message(&used.id, "accepted before End".into()).unwrap();
            abort_worker(&hub, &used.id);
            accepted
        },
        hub.close(&used.id)
    );
    assert!(closed.unwrap());
    let archived = hub.ended_record(&used.id);
    assert!(
        archived
            .view
            .entries
            .iter()
            .any(|entry| entry.id == accepted)
    );
    assert!(hub.view(&used.id).is_none());
    hub.shutdown().await;
}
