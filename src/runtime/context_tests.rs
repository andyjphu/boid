use super::*;
use axum::{Json, Router, routing::post};
use tokio::sync::{mpsc, oneshot};

struct Fixture {
    endpoint: String,
    requests: mpsc::Receiver<(Value, oneshot::Sender<Value>)>,
    server: tokio::task::JoinHandle<()>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}
impl Fixture {
    async fn new() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}/", listener.local_addr().unwrap());
        let (sent, requests) = mpsc::channel(8);
        let router = Router::new().route(
            "/",
            post(move |Json(request): Json<Value>| {
                let sent = sent.clone();
                async move {
                    let (release, wait) = oneshot::channel();
                    sent.send((request, release)).await.unwrap();
                    let output = wait.await.unwrap_or(json!([]));
                    (
                        [("content-type", "text/event-stream")],
                        format!(
                            "data: {}\n\n",
                            json!({"type":"response.completed","response":{"output":output}})
                        ),
                    )
                }
            }),
        );
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        Self {
            endpoint,
            requests,
            server,
        }
    }
    async fn next(&mut self) -> (Value, oneshot::Sender<Value>) {
        tokio::time::timeout(Duration::from_secs(6), self.requests.recv())
            .await
            .expect("local model request did not arrive")
            .unwrap()
    }
}
fn create(hub: &Hub, work: &Path, name: &str, message: &str) -> View {
    hub.create(Create {
        name: name.into(),
        cwd: work.to_string_lossy().into_owned(),
        message: message.into(),
    })
    .unwrap()
}
async fn idle(hub: &Hub, id: &str) {
    tokio::time::timeout(Duration::from_secs(6), async {
        loop {
            let view = hub.view(id).unwrap();
            assert_ne!(
                view.status,
                "error",
                "{}",
                serde_json::to_string(&view.entries).unwrap()
            );
            if view.status == "idle" && view.pending.is_empty() {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}
fn context(request: &Value) -> (Value, String) {
    let input = request["input"].as_array().unwrap();
    let contexts: Vec<_> = input
        .iter()
        .filter(|item| {
            item["role"] == "user"
                && item["content"]
                    .as_str()
                    .is_some_and(|text| text.starts_with("## Status context\n"))
        })
        .collect();
    assert_eq!(
        contexts.len(),
        1,
        "exactly one transient context belongs in each request"
    );
    assert_eq!(
        contexts[0],
        input.last().unwrap(),
        "runtime context must be the final input item"
    );
    let text = contexts[0]["content"].as_str().unwrap();
    let (status, working) = text
        .strip_prefix("## Status context\n")
        .unwrap()
        .split_once("## Role context\n")
        .unwrap();
    let working: Value = serde_json::from_str(working.trim()).unwrap();
    (
        serde_json::from_str(status.trim()).unwrap(),
        working["text"].as_str().unwrap().to_owned(),
    )
}
fn working_block(request: &Value) -> Value {
    let text = request["input"].as_array().unwrap().last().unwrap()["content"]
        .as_str()
        .unwrap();
    serde_json::from_str(text.split_once("## Role context\n").unwrap().1.trim()).unwrap()
}
fn call(id: &str, arguments: Value) -> Value {
    json!([{"type":"function_call", "id":id, "call_id":id, "name":"set_role_context", "arguments":arguments.to_string()}])
}
fn result(request: &Value, id: &str) -> Value {
    let item = request["input"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["type"] == "function_call_output" && item["call_id"] == id)
        .unwrap();
    serde_json::from_str(item["output"].as_str().unwrap()).unwrap()
}
fn assert_no_saved_snapshots(hub: &Hub, id: &str) {
    let data = hub.0.data.lock().unwrap();
    let history = data.sessions[id].record.engine.history_items();
    assert!(!history.iter().any(|item| {
        item["role"] == "user"
            && item["content"]
                .as_str()
                .is_some_and(|text| text.starts_with("## Status context\n"))
    }));
}

#[tokio::test]
async fn context_refreshes_each_request_without_rewriting_history_or_stable_instructions() {
    let mut fixture = Fixture::new().await;
    let state = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let hub = Hub::open(
        Client::test_client_at(fixture.endpoint.clone()),
        state.path().into(),
    )
    .unwrap();
    let peer = create(&hub, work.path(), "swift", "");
    let ended = create(&hub, work.path(), "retired", "");
    let captain = create(
        &hub,
        work.path(),
        "captain",
        "Coordinate this local fixture.",
    );
    let (first, release) = fixture.next().await;
    let (status, working) = context(&first);
    assert_eq!(status["self"]["id"], captain.id);
    assert_eq!(working, "");
    assert!(
        status["peers"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["id"] == peer.id && row["name"] == "swift")
    );
    assert!(
        first["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|tool| tool["name"] == "set_role_context")
    );
    hub.rename(&peer.id, "osprey".into()).unwrap();
    hub.stop(&peer.id).unwrap();
    assert!(hub.close(&ended.id).await.unwrap());
    hub.rename(&captain.id, "ursa".into()).unwrap();
    let note = "Delegate independent tasks. Keep coordination and final judgment with the captain.";
    release
        .send(call("remember", json!({"text":note})))
        .unwrap();
    let (second, release) = fixture.next().await;
    assert_eq!(second["instructions"], first["instructions"]);
    let (status, working) = context(&second);
    assert_eq!(status["self"]["name"], "ursa");
    let row = status["peers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["id"] == peer.id)
        .unwrap();
    assert_eq!(row["name"], "osprey");
    assert_eq!(row["listening"], false);
    assert_eq!(row["wakeable"], false);
    assert!(
        !status["peers"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["id"] == ended.id)
    );
    assert_eq!(working, note);
    assert_eq!(result(&second, "remember")["text"], note);
    assert_eq!(
        result(&second, "remember")["characters"],
        note.chars().count()
    );
    assert_no_saved_snapshots(&hub, &captain.id);
    release.send(json!([])).unwrap();
    idle(&hub, &captain.id).await;
    hub.shutdown().await;
    drop(hub);
    let restored = Hub::open(
        Client::test_client_at(fixture.endpoint.clone()),
        state.path().into(),
    )
    .unwrap();
    assert_eq!(
        restored.0.data.lock().unwrap().sessions[&captain.id]
            .record
            .working_context,
        note
    );
    assert_no_saved_snapshots(&restored, &captain.id);
    restored
        .message(&captain.id, "Continue this fixture.".into())
        .unwrap();
    let (third, release) = fixture.next().await;
    assert_eq!(context(&third).1, note);
    assert_eq!(third["instructions"], second["instructions"]);
    release.send(json!([])).unwrap();
    idle(&restored, &captain.id).await;
    restored.message(&captain.id, "/clear".into()).unwrap();
    idle(&restored, &captain.id).await;
    assert_eq!(
        restored.0.data.lock().unwrap().sessions[&captain.id]
            .record
            .working_context,
        note
    );
    assert_no_saved_snapshots(&restored, &captain.id);
    assert!(
        fixture.requests.try_recv().is_err(),
        "/clear must not start an inference request"
    );
    restored.shutdown().await;
}

#[tokio::test]
async fn working_context_edits_are_bounded_and_explicitly_clearable() {
    let mut fixture = Fixture::new().await;
    let hub = Hub::new(Client::test_client_at(fixture.endpoint.clone())).unwrap();
    let work = tempfile::tempdir().unwrap();
    let peer = create(&hub, work.path(), "peer", "");
    let actor = create(&hub, work.path(), "actor", "Test local context edits.");
    let (_, release) = fixture.next().await;
    let note = format!("{}界", "a ".repeat(511)) + "界";
    assert_eq!(note.chars().count(), 1024);
    release
        .send(call("boundary", json!({"text":note,"to":null})))
        .unwrap();
    let (next, mut release) = fixture.next().await;
    assert_eq!(context(&next).1, note);
    assert_eq!(result(&next, "boundary")["characters"], 1024);
    for (id, arguments) in [
        ("too-many", json!({"text":format!("{note}界")})),
        ("too-large", json!({"text":"x".repeat(1025)})),
        (
            "unknown-agent",
            json!({"text":"No mutation", "to":"missing-session-id"}),
        ),
        (
            "unknown-field",
            json!({"text":"No mutation", "recipient":peer.id}),
        ),
    ] {
        release.send(call(id, arguments)).unwrap();
        let (next, next_release) = fixture.next().await;
        assert!(result(&next, id)["error"].is_string());
        assert_eq!(
            context(&next).1,
            note,
            "rejected edit must preserve the prior note"
        );
        assert!(
            hub.0.data.lock().unwrap().sessions[&peer.id]
                .record
                .working_context
                .is_empty()
        );
        release = next_release;
    }
    release.send(call("clear", json!({"text":""}))).unwrap();
    let (next, release) = fixture.next().await;
    assert_eq!(context(&next).1, "");
    assert_eq!(result(&next, "clear")["characters"], 0);
    assert_no_saved_snapshots(&hub, &actor.id);
    release.send(json!([])).unwrap();
    idle(&hub, &actor.id).await;
    assert!(
        hub.0.data.lock().unwrap().sessions[&actor.id]
            .record
            .working_context
            .is_empty()
    );
    hub.shutdown().await;
}

#[tokio::test]
async fn peer_working_context_survives_target_checkpoint_rename_clear_and_restart() {
    let mut fixture = Fixture::new().await;
    let state = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let hub = Hub::open(
        Client::test_client_at(fixture.endpoint.clone()),
        state.path().into(),
    )
    .unwrap();
    let target = create(&hub, work.path(), "worker", "Hold this target request.");
    let (old_target_request, target_release) = fixture.next().await;
    assert_eq!(context(&old_target_request).1, "");
    let editor = create(&hub, work.path(), "captain", "Set a peer reminder.");
    let (_, editor_release) = fixture.next().await;
    let note = "Delegate independent work and report blockers to the captain.";
    editor_release
        .send(call("peer-note", json!({"text":note,"to":target.id})))
        .unwrap();
    let (editor_continuation, editor_release) = fixture.next().await;
    let receipt = result(&editor_continuation, "peer-note");
    assert_eq!(receipt["text"], note);
    assert_eq!(receipt["to"], target.id);
    assert_eq!(receipt["editor"]["id"], editor.id);
    assert_eq!(
        context(&editor_continuation).1,
        "",
        "editing a peer must not replace the editor's note"
    );
    assert_eq!(
        hub.0.data.lock().unwrap().sessions[&target.id]
            .record
            .working_context,
        note
    );
    assert!(
        hub.view(&target.id).unwrap().pending.is_empty(),
        "peer note edits must not queue a user turn"
    );
    assert_eq!(hub.view(&target.id).unwrap().status, "working");
    // A peer note is meaningful state even before the target's first message.
    let draft = create(&hub, work.path(), "unstarted", "");
    editor_release
        .send(call(
            "draft-note",
            json!({"text":"Retain this unstarted assignment.","to":draft.id}),
        ))
        .unwrap();
    let (editor_continuation, editor_release) = fixture.next().await;
    assert_eq!(context(&editor_continuation).0["self"]["id"], editor.id);
    assert_eq!(result(&editor_continuation, "draft-note")["to"], draft.id);
    assert_eq!(hub.view(&draft.id).unwrap().status, "idle");
    assert!(hub.view(&draft.id).unwrap().pending.is_empty());
    // The target still owns a pre-edit Session copy. Completing that response,
    // executing its tool and checkpointing must not overwrite the peer's edit.
    hub.rename(&target.id, "osprey".into()).unwrap();
    target_release.send(json!([{"type":"function_call","id":"target-list","call_id":"target-list","name":"list_agents","arguments":"{}"}])).unwrap();
    let (target_continuation, target_release) = fixture.next().await;
    let (status, working) = context(&target_continuation);
    assert_eq!(status["self"]["id"], target.id);
    assert_eq!(status["self"]["name"], "osprey");
    assert_eq!(working, note);
    assert_eq!(
        working_block(&target_continuation)["editor"]["id"],
        editor.id
    );
    assert_eq!(
        working_block(&target_continuation)["editor"]["name"],
        "captain"
    );
    assert_eq!(
        target_continuation["instructions"],
        old_target_request["instructions"]
    );
    assert_eq!(
        context(&old_target_request).1,
        "",
        "already prepared requests remain unchanged"
    );
    editor_release.send(json!([])).unwrap();
    target_release.send(json!([])).unwrap();
    idle(&hub, &editor.id).await;
    idle(&hub, &target.id).await;
    assert_eq!(
        hub.0.data.lock().unwrap().sessions[&target.id]
            .record
            .working_context,
        note
    );
    hub.message(&target.id, "/clear".into()).unwrap();
    idle(&hub, &target.id).await;
    assert_eq!(
        hub.0.data.lock().unwrap().sessions[&target.id]
            .record
            .working_context,
        note
    );
    assert_no_saved_snapshots(&hub, &target.id);
    hub.shutdown().await;
    drop(hub);
    let restored = Hub::open(
        Client::test_client_at(fixture.endpoint.clone()),
        state.path().into(),
    )
    .unwrap();
    assert_eq!(
        restored.0.data.lock().unwrap().sessions[&target.id]
            .record
            .working_context,
        note
    );
    assert_eq!(
        restored.0.data.lock().unwrap().sessions[&target.id]
            .record
            .version,
        9
    );
    assert!(
        restored.0.data.lock().unwrap().sessions[&editor.id]
            .record
            .working_context
            .is_empty()
    );
    assert_eq!(
        restored.0.data.lock().unwrap().sessions[&draft.id]
            .record
            .working_context,
        "Retain this unstarted assignment."
    );
    assert_eq!(restored.view(&draft.id).unwrap().status, "idle");
    restored
        .message(&target.id, "Continue after restart.".into())
        .unwrap();
    let (resumed, release) = fixture.next().await;
    assert_eq!(context(&resumed).1, note);
    assert_eq!(working_block(&resumed)["editor"]["id"], editor.id);
    assert_eq!(context(&resumed).0["self"]["name"], "osprey");
    assert_no_saved_snapshots(&restored, &target.id);
    release.send(json!([])).unwrap();
    idle(&restored, &target.id).await;
    restored.shutdown().await;
}

#[tokio::test]
async fn eject_context_exports_without_clearing_waking_or_provider_calls() {
    use std::os::unix::fs::PermissionsExt;
    let mut fixture = Fixture::new().await;
    let state = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let hub = Hub::open(
        Client::test_client_at(fixture.endpoint.clone()),
        state.path().into(),
    )
    .unwrap();
    let view = create(&hub, work.path(), "exporter", "");
    hub.set_working_context(&view.id, &json!({"text":"Keep this role"}).to_string())
        .unwrap();
    let original = vec![Arc::new(
        json!({"role":"user","content":"Preserve the exact evidence: 界"}),
    )];
    hub.update(&view.id, |record| {
        record.engine.restore_history(original.clone());
        record.paused = true;
        Ok(())
    })
    .unwrap();
    let receipt_id = hub.message(&view.id, "/eject-context".into()).unwrap();
    let entries = hub.view(&view.id).unwrap().entries;
    let notice = entries.iter().find(|entry| entry.id == receipt_id).unwrap();
    let path = PathBuf::from(notice.text.strip_prefix("Context exported: ").unwrap());
    let export: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert!(path.starts_with(state.path().join("context-exports").join(&view.id)));
    assert_eq!(export["format"], "boid-context-v1");
    assert_eq!(export["input"][0], *original[0]);
    assert_eq!(context(&export).1, "Keep this role");
    assert!(
        export["instructions"]
            .as_str()
            .unwrap()
            .starts_with("## Stable context")
    );
    assert!(
        export["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|tool| tool["name"] == "eject_context")
    );
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let second = hub.eject_context(&view.id).unwrap();
    assert_ne!(second["path"], path.to_string_lossy().as_ref());
    let directory = state.path().join("context-exports");
    std::fs::rename(&directory, state.path().join("preserved-exports")).unwrap();
    std::fs::write(&directory, b"force export directory failure").unwrap();
    assert!(hub.eject_context(&view.id).is_err());
    {
        let data = hub.0.data.lock().unwrap();
        let record = &data.sessions[&view.id].record;
        assert_eq!(record.engine.history_items(), original);
        assert_eq!(record.working_context, "Keep this role");
        assert!(record.paused);
        assert!(record.inputs.is_empty());
    }
    assert!(fixture.requests.try_recv().is_err());
    hub.shutdown().await;
}

#[tokio::test]
async fn operator_role_migration_rejects_stale_text_or_editor_without_waking() {
    let mut fixture = Fixture::new().await;
    let hub = Hub::new(Client::test_client_at(fixture.endpoint.clone())).unwrap();
    let work = tempfile::tempdir().unwrap();
    let target = create(&hub, work.path(), "target", "");
    let peer = create(&hub, work.path(), "peer", "");
    let edit = |text: &str, expected: Value| {
        serde_json::from_value::<RoleContextPatch>(json!({"text":text,"expected":expected}))
            .unwrap()
    };
    hub.set_working_context(
        &target.id,
        &json!({"text":"Preserve these results"}).to_string(),
    )
    .unwrap();
    let before = hub.role_context(&target.id).unwrap();
    hub.set_working_context(
        &peer.id,
        &json!({"text":"New constraint", "to":target.id}).to_string(),
    )
    .unwrap();
    assert!(
        hub.patch_role_context(&target.id, edit("Coordinator", before.clone()))
            .is_err()
    );
    assert_eq!(
        hub.role_context(&target.id).unwrap()["text"],
        "New constraint"
    );
    // Equal text with a different editor must also invalidate the snapshot.
    hub.set_working_context(
        &peer.id,
        &json!({"text":"Preserve these results", "to":target.id}).to_string(),
    )
    .unwrap();
    assert!(
        hub.patch_role_context(&target.id, edit("Coordinator", before))
            .is_err()
    );
    let current = hub.role_context(&target.id).unwrap();
    assert!(
        hub.patch_role_context(&target.id, edit(&"x".repeat(1025), current.clone()))
            .is_err()
    );
    assert_eq!(hub.role_context(&target.id).unwrap(), current);
    let receipt = hub
        .patch_role_context(
            &target.id,
            edit(
                "Coordinator. No provider calls. Notes: /private/task.md",
                current,
            ),
        )
        .unwrap();
    assert_eq!(receipt["editor"]["id"], "operator");
    assert_eq!(
        receipt["characters"],
        receipt["text"].as_str().unwrap().chars().count()
    );
    assert_eq!(
        hub.role_context(&target.id).unwrap()["text"],
        receipt["text"]
    );
    assert_eq!(hub.view(&target.id).unwrap().status, "idle");
    assert!(hub.view(&target.id).unwrap().pending.is_empty());
    assert!(
        fixture.requests.try_recv().is_err(),
        "editing must not trigger inference"
    );
    hub.shutdown().await;
}
