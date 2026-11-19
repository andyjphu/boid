use super::*;
use axum::{
    Json, Router, extract::OriginalUri, http::StatusCode, response::IntoResponse, routing::post,
};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};

type Request = (String, Value, oneshot::Sender<(StatusCode, Value)>);
struct Mock {
    endpoint: String,
    requests: mpsc::Receiver<Request>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Mock {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Mock {
    async fn new() -> Self {
        let (sent, requests) = mpsc::channel::<Request>(8);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}/", listener.local_addr().unwrap());
        let handler = move |OriginalUri(uri): OriginalUri, Json(body): Json<Value>| {
            let sent = sent.clone();
            async move {
                let (release, wait) = oneshot::channel();
                sent.send((uri.path().into(), body, release)).await.unwrap();
                let (status, result) = wait
                    .await
                    .unwrap_or((StatusCode::BAD_GATEWAY, json!({"error":"fixture ended"})));
                if uri.path() == "/compact" {
                    (status, Json(result)).into_response()
                } else {
                    let response = if result.is_array() {
                        json!({"output":result})
                    } else {
                        result
                    };
                    (
                        status,
                        [("content-type", "text/event-stream")],
                        format!(
                            "data: {}\n\n",
                            json!({"type":"response.completed","response":response})
                        ),
                    )
                        .into_response()
                }
            }
        };
        let router = Router::new()
            .route("/", post(handler.clone()))
            .route("/compact", post(handler));
        let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        Self {
            endpoint,
            requests,
            task,
        }
    }
    async fn request(&mut self) -> Request {
        tokio::time::timeout(Duration::from_secs(5), self.requests.recv())
            .await
            .unwrap()
            .unwrap()
    }
}
async fn idle(hub: &Hub, id: &str, status: &str) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while {
            let view = hub.view(id).unwrap();
            view.status != status || (status == "idle" && !view.pending.is_empty())
        } {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
}
fn flush_recording(hub: &Hub, id: &str) -> Result<Option<Record>> {
    let capacity = hub.0.data.lock().unwrap().sessions[id]
        .persistence
        .capacity();
    capacity.durable_record()
}
fn engine(hub: &Hub, id: &str) -> Value {
    serde_json::to_value(&hub.0.data.lock().unwrap().sessions[id].record.engine).unwrap()
}
fn window() -> Value {
    json!([
        {"type":"message","id":"retained","role":"user","content":[{"type":"input_text","text":"Retain this task"}]},
        {"type":"compaction","id":"cmp-item","encrypted_content":"opaque-fixture"},
        {"type":"message","role":"assistant","content":[{"type":"output_text","text":"retained tail"}]}
    ])
}
fn compacted(output: Value) -> Value {
    json!({"object":"response.compaction","id":"cmp-response","output":output,
        "usage":{"input_tokens":900,"output_tokens":100,"total_tokens":1000,
        "input_tokens_details":{"cached_tokens":0,"cache_write_tokens":0}}})
}
fn create(hub: &Hub, work: &std::path::Path, message: &str) -> View {
    hub.create(Create {
        name: "canary".into(),
        cwd: work.to_string_lossy().into_owned(),
        message: message.into(),
    })
    .unwrap()
}

fn used(output: Value, total: u64) -> Value {
    json!({"output":output,"usage":{"input_tokens":total-1,"output_tokens":1}})
}

#[tokio::test]
async fn auto_compaction_threshold_idle_turn_and_persistent_preference() {
    let mut mock = Mock::new().await;
    let state = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let hub = Hub::open(
        Client::test_client_at(mock.endpoint.clone()),
        state.path().into(),
    )
    .unwrap();
    let view = create(&hub, work.path(), "");
    let other = create(&hub, work.path(), "");
    assert_eq!(engine(&hub, &view.id)["auto_oai_compact"], false);
    hub.message(&view.id, "/auto-oai-compact".into()).unwrap();
    idle(&hub, &view.id, "idle").await;
    assert_eq!(engine(&hub, &view.id)["auto_oai_compact"], true);
    assert_eq!(engine(&hub, &other.id)["auto_oai_compact"], false);
    assert!(mock.requests.try_recv().is_err());
    hub.message(&view.id, "At threshold".into()).unwrap();
    let (_, request, release) = mock.request().await;
    assert!(!request["input"].to_string().contains("/auto-oai-compact"));
    release
        .send((StatusCode::OK, used(json!([]), 900_000)))
        .unwrap();
    idle(&hub, &view.id, "idle").await;
    assert!(mock.requests.try_recv().is_err());
    hub.message(&view.id, "Cross threshold".into()).unwrap();
    let (_, _, release) = mock.request().await;
    release
        .send((StatusCode::OK, used(json!([]), 900_001)))
        .unwrap();
    let (path, request, release) = mock.request().await;
    assert_eq!(path, "/compact");
    assert!(request["input"].to_string().contains("Cross threshold"));
    release.send((StatusCode::OK, compacted(window()))).unwrap();
    idle(&hub, &view.id, "idle").await;
    assert_eq!(engine(&hub, &view.id)["history"], window());
    assert!(mock.requests.try_recv().is_err());
    assert!(
        hub.view(&view.id)
            .unwrap()
            .entries
            .iter()
            .any(|e| e.text.as_str() == "Cross threshold")
    );
    flush_recording(&hub, &view.id).unwrap();
    assert_eq!(
        std::fs::read_dir(state.path().join("sessions/compactions"))
            .unwrap()
            .count(),
        1
    );
    hub.message(&view.id, "/clear".into()).unwrap();
    idle(&hub, &view.id, "idle").await;
    assert_eq!(engine(&hub, &view.id)["auto_oai_compact"], true);
    hub.shutdown().await;
    drop(hub);
    let hub = Hub::open(
        Client::test_client_at(mock.endpoint.clone()),
        state.path().into(),
    )
    .unwrap();
    assert_eq!(engine(&hub, &view.id)["auto_oai_compact"], true);
    hub.message(&view.id, "/auto-oai-compact off".into())
        .unwrap();
    idle(&hub, &view.id, "idle").await;
    assert_eq!(engine(&hub, &view.id)["auto_oai_compact"], false);
    assert!(mock.requests.try_recv().is_err());
    hub.shutdown().await;
}

#[tokio::test]
async fn auto_compaction_waits_for_tools_and_disables_after_failure_or_interrupt() {
    let mut mock = Mock::new().await;
    let hub = Hub::new(Client::test_client_at(mock.endpoint.clone())).unwrap();
    let work = tempfile::tempdir().unwrap();
    let view = create(&hub, work.path(), "/auto-oai-compact on");
    idle(&hub, &view.id, "idle").await;
    hub.message(&view.id, "Run a tool".into()).unwrap();
    let (_, _, release) = mock.request().await;
    release.send((StatusCode::OK, used(json!([
        {"type":"function_call","call_id":"first","name":"list_agents","arguments":"{}"},
        {"type":"function_call","call_id":"second","name":"list_agents","arguments":"{}"}
    ]), 900_001))).unwrap();
    let (path, request, release) = mock.request().await;
    assert_eq!(path, "/compact");
    for id in ["first", "second"] {
        assert!(
            request["input"]
                .as_array()
                .unwrap()
                .iter()
                .any(|i| i["type"] == "function_call_output" && i["call_id"] == id)
        );
    }
    let before = engine(&hub, &view.id)["history"].clone();
    release
        .send((
            StatusCode::BAD_GATEWAY,
            json!({"error":"fixture unavailable"}),
        ))
        .unwrap();
    let (path, request, release) = mock.request().await;
    assert_eq!(path, "/");
    assert_eq!(
        &request["input"].as_array().unwrap()[..before.as_array().unwrap().len()],
        before.as_array().unwrap()
    );
    release
        .send((StatusCode::OK, used(json!([]), 900_001)))
        .unwrap();
    idle(&hub, &view.id, "idle").await;
    assert_eq!(engine(&hub, &view.id)["auto_oai_compact"], false);
    assert!(mock.requests.try_recv().is_err());
    hub.message(&view.id, "/auto-oai-compact on".into())
        .unwrap();
    let (path, _, _release) = mock.request().await;
    assert_eq!(path, "/compact");
    hub.interrupt(&view.id).unwrap();
    idle(&hub, &view.id, "idle").await;
    assert_eq!(engine(&hub, &view.id)["auto_oai_compact"], false);
    assert!(
        engine(&hub, &view.id)["history"]
            .to_string()
            .contains("Run a tool")
    );
    assert!(mock.requests.try_recv().is_err());
    hub.shutdown().await;
}

fn use_subscription(hub: &Hub, id: &str) {
    let mut data = hub.0.data.lock().unwrap();
    let group = data
        .screens
        .groups
        .values_mut()
        .find(|group| group.tabs.contains(&format!("agent:{id}")))
        .unwrap();
    group.credential_id = Some("fixture-subscription".into());
    group.credential_type = Some("chatgpt".into());
}

#[tokio::test]
async fn auto_compaction_subscription_command_and_route_change_make_no_api_call() {
    let mut mock = Mock::new().await;
    let hub = Hub::new(Client::test_client_at(mock.endpoint.clone())).unwrap();
    let work = tempfile::tempdir().unwrap();
    let view = create(&hub, work.path(), "");
    use_subscription(&hub, &view.id);
    hub.message(&view.id, "/auto-oai-compact on".into())
        .unwrap();
    idle(&hub, &view.id, "idle").await;
    assert_eq!(engine(&hub, &view.id)["auto_oai_compact"], false);
    assert!(
        hub.view(&view.id)
            .unwrap()
            .entries
            .iter()
            .any(|e| e.text.contains("unavailable on subscription"))
    );
    assert!(
        !engine(&hub, &view.id)["history"]
            .to_string()
            .contains("/auto-oai-compact")
    );
    assert_eq!(
        hub.profile(&view.id).unwrap()["credential_id"],
        "fixture-subscription"
    );
    assert!(mock.requests.try_recv().is_err());

    let api = create(&hub, work.path(), "/auto-oai-compact on");
    idle(&hub, &api.id, "idle").await;
    hub.message(&api.id, "Finish before credential change".into())
        .unwrap();
    let (_, _, release) = mock.request().await;
    use_subscription(&hub, &api.id);
    release
        .send((StatusCode::OK, used(json!([]), 900_001)))
        .unwrap();
    idle(&hub, &api.id, "idle").await;
    assert_eq!(engine(&hub, &api.id)["auto_oai_compact"], false);
    assert!(mock.requests.try_recv().is_err());
    assert_eq!(
        hub.profile(&api.id).unwrap()["credential_id"],
        "fixture-subscription"
    );
    hub.shutdown().await;
}

#[tokio::test]
async fn clear_waits_for_tools_preserves_transcript_and_starts_fresh_after_restart() {
    let mut mock = Mock::new().await;
    let state = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let hub = Hub::open(
        Client::test_client_at(mock.endpoint.clone()),
        state.path().into(),
    )
    .unwrap();
    let view = create(&hub, work.path(), "Old task marker");
    let (_, _, release) = mock.request().await;
    let clear_id = hub.message(&view.id, "/clear".into()).unwrap();
    release.send((StatusCode::OK, json!([
        {"type":"function_call","call_id":"roster-before-clear","name":"list_agents","arguments":"{}"}
    ]))).unwrap();
    idle(&hub, &view.id, "idle").await;
    let cleared = engine(&hub, &view.id);
    assert_eq!(cleared["history"], json!([]));
    assert_eq!(cleared["model_requests"], 1);
    assert!(cleared["context_usage"].is_null());
    assert!(mock.requests.try_recv().is_err());
    let saved = hub.view(&view.id).unwrap();
    assert!(
        saved
            .entries
            .iter()
            .any(|e| e.text.contains("Old task marker"))
    );
    assert!(
        saved
            .entries
            .iter()
            .any(|e| e.kind == "tool_result" && e.text.contains("roster-before-clear"))
    );
    assert!(
        saved
            .entries
            .iter()
            .any(|e| e.text.starts_with("Context cleared."))
    );
    flush_recording(&hub, &view.id).unwrap();
    let archive: Value = serde_json::from_slice(
        &std::fs::read(
            state
                .path()
                .join(format!("sessions/clears/{}-{clear_id}.json", view.id)),
        )
        .unwrap(),
    )
    .unwrap();
    assert!(
        archive["engine"]["history"]
            .to_string()
            .contains("Old task marker")
    );
    assert!(
        archive["engine"]["history"]
            .to_string()
            .contains("function_call_output")
    );
    hub.shutdown().await;
    drop(hub);
    let hub = Hub::open(
        Client::test_client_at(mock.endpoint.clone()),
        state.path().into(),
    )
    .unwrap();
    assert_eq!(engine(&hub, &view.id)["history"], json!([]));
    hub.message(&view.id, "Fresh task marker".into()).unwrap();
    let (path, request, release) = mock.request().await;
    assert_eq!(path, "/");
    assert!(request["input"].to_string().contains("Fresh task marker"));
    assert!(!request["input"].to_string().contains("Old task marker"));
    assert!(!request["input"].to_string().contains("/clear"));
    assert!(
        request["input"].as_array().unwrap().last().unwrap()["content"]
            .as_str()
            .unwrap()
            .contains(&view.id)
    );
    assert!(
        request["input"].as_array().unwrap().last().unwrap()["content"]
            .as_str()
            .unwrap()
            .contains("canary")
    );
    assert_eq!(request["tools"], cleared["tools"]);
    release.send((StatusCode::OK, json!([]))).unwrap();
    idle(&hub, &view.id, "idle").await;
    assert_eq!(engine(&hub, &view.id)["model_requests"], 2);
    hub.shutdown().await;
}

#[tokio::test]
async fn clear_archive_failure_retains_original_capture_and_reports_the_fault() {
    let mut mock = Mock::new().await;
    let state = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let hub = Hub::open(
        Client::test_client_at(mock.endpoint.clone()),
        state.path().into(),
    )
    .unwrap();
    let view = create(&hub, work.path(), "Original context");
    let (_, _, release) = mock.request().await;
    std::fs::write(
        state.path().join("sessions/clears"),
        "block archive directory",
    )
    .unwrap();
    let clear = hub.message(&view.id, "/clear".into()).unwrap();
    let fresh = hub.message(&view.id, "New queued task".into()).unwrap();
    release.send((StatusCode::OK, json!([]))).unwrap();
    idle(&hub, &view.id, "error").await;
    assert!(flush_recording(&hub, &view.id).is_err());
    let retained = hub.0.data.lock().unwrap().sessions[&view.id]
        .persistence
        .retained_archive_records();
    assert!(retained.iter().any(|record| {
        serde_json::to_value(&record.engine).unwrap()["history"]
            .to_string()
            .contains("Original context")
    }));
    let visible = hub.view(&view.id).unwrap();
    for id in [clear, fresh] {
        assert!(visible.entries.iter().any(|entry| entry.id == id));
    }
    // Work can advance before an asynchronous failure is observed. Once known,
    // the failure rejects new work while keeping all captured input/context.
    assert!(
        hub.message(&view.id, "cannot continue after recording failure".into())
            .is_err()
    );
    hub.shutdown().await;
}

#[tokio::test]
async fn direct_compaction_preserves_canonical_window_archive_and_restart() {
    let mut mock = Mock::new().await;
    let state = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let hub = Hub::open(
        Client::test_client_at(mock.endpoint.clone()),
        state.path().into(),
    )
    .unwrap();
    let view = create(&hub, work.path(), "");
    let before = engine(&hub, &view.id);
    assert!(view.entries.is_empty());
    hub.message(&view.id, "/oai-compact".into()).unwrap();
    let initial_context = hub.view(&view.id).unwrap().entries[0].text.clone();
    let (path, request, release) = mock.request().await;
    assert_eq!(path, "/compact");
    assert_eq!(request["input"], before["history"]);
    assert!(
        request["instructions"]
            .as_str()
            .unwrap()
            .contains("\"name\":\"canary\"")
    );
    release.send((StatusCode::OK, compacted(window()))).unwrap();
    idle(&hub, &view.id, "idle").await;
    assert_eq!(engine(&hub, &view.id)["history"], window());
    assert!(mock.requests.try_recv().is_err()); // Direct compaction does not start inference.
    assert_eq!(hub.view(&view.id).unwrap().entries[0].text, initial_context);
    flush_recording(&hub, &view.id).unwrap();
    let archive: Value = serde_json::from_slice(
        &std::fs::read(
            state
                .path()
                .join(format!("sessions/compactions/{}-1.json", view.id)),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(archive["engine"]["history"], before["history"]);
    assert!(
        hub.0.data.lock().unwrap().sessions[&view.id]
            .record
            .usage
            .records
            .is_empty()
    );
    hub.shutdown().await;
    drop(hub);
    let hub = Hub::open(
        Client::test_client_at(mock.endpoint.clone()),
        state.path().into(),
    )
    .unwrap();
    assert_eq!(engine(&hub, &view.id)["history"], window());
    hub.message(&view.id, "Continue the task".into()).unwrap();
    let (path, next, release) = mock.request().await;
    assert_eq!(path, "/");
    assert_eq!(
        &next["input"].as_array().unwrap()[..3],
        window().as_array().unwrap()
    );
    release.send((StatusCode::OK, json!([]))).unwrap();
    idle(&hub, &view.id, "idle").await;
    hub.shutdown().await;
}

#[tokio::test]
async fn failed_and_cancelled_compaction_keep_prior_context_and_queued_input() {
    let mut mock = Mock::new().await;
    let hub = Hub::new(Client::test_client_at(mock.endpoint.clone())).unwrap();
    let work = tempfile::tempdir().unwrap();
    let view = create(&hub, work.path(), "");
    let original = engine(&hub, &view.id)["history"].clone();
    for (status, body) in [
        (StatusCode::BAD_GATEWAY, json!({"error":"unavailable"})),
        (
            StatusCode::OK,
            json!({"object":"response.compaction","output":[]}),
        ),
    ] {
        hub.message(&view.id, "/oai-compact".into()).unwrap();
        let (_, _, release) = mock.request().await;
        release.send((status, body)).unwrap();
        idle(&hub, &view.id, "idle").await;
        assert_eq!(engine(&hub, &view.id)["history"], original);
        assert!(mock.requests.try_recv().is_err());
    }
    hub.message(&view.id, "/oai-compact".into()).unwrap();
    let (_, _, release) = mock.request().await;
    hub.interrupt(&view.id).unwrap();
    idle(&hub, &view.id, "idle").await;
    let _ = release.send((StatusCode::OK, compacted(window())));
    assert_eq!(engine(&hub, &view.id)["history"], original);
    hub.message(&view.id, "/oai-compact".into()).unwrap();
    let (_, _, release) = mock.request().await;
    hub.message(&view.id, "Queued while compacting".into())
        .unwrap();
    release.send((StatusCode::OK, compacted(window()))).unwrap();
    let (path, next, release) = mock.request().await;
    assert_eq!(path, "/");
    assert_eq!(
        &next["input"].as_array().unwrap()[..3],
        window().as_array().unwrap()
    );
    assert!(
        next["input"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v["content"] == "Queued while compacting")
    );
    release.send((StatusCode::OK, json!([]))).unwrap();
    idle(&hub, &view.id, "idle").await;
    hub.shutdown().await;
}

#[tokio::test]
async fn selected_skill_compacts_after_complete_tool_batch() {
    let mut mock = Mock::new().await;
    let state = tempfile::tempdir().unwrap();
    let hub = Hub::open(
        Client::test_client_at(mock.endpoint.clone()),
        state.path().into(),
    )
    .unwrap();
    let work = tempfile::tempdir().unwrap();
    let skill = state.path().join("skills/oai-compact");
    std::fs::create_dir_all(&skill).unwrap();
    std::fs::write(
        skill.join("SKILL.md"),
        include_str!("../skills/oai-compact/SKILL.md"),
    )
    .unwrap();
    let view = create(&hub, work.path(), "$oai-compact");
    let (path, first, release) = mock.request().await;
    assert_eq!(path, "/");
    assert!(first["input"].to_string().contains("Call `oai_compact`"));
    assert!(
        first["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["name"] == "oai_compact")
    );
    release
        .send((
            StatusCode::OK,
            json!([
                {"type":"function_call","call_id":"note","name":"set_working_context","arguments":"{\"text\":\"Delegate independent work.\"}"},
                {"type":"function_call","call_id":"compact","name":"oai_compact","arguments":"{}"},
                {"type":"function_call","call_id":"agents","name":"list_agents","arguments":"{}"}
            ]),
        ))
        .unwrap();
    let (path, compact, release) = mock.request().await;
    assert_eq!(path, "/compact");
    for id in ["note", "compact", "agents"] {
        assert!(
            compact["input"]
                .as_array()
                .unwrap()
                .iter()
                .any(|v| v["type"] == "function_call_output" && v["call_id"] == id)
        );
    }
    assert!(!compact["input"].as_array().unwrap().iter().any(|item| {
        item["content"]
            .as_str()
            .is_some_and(|text| text.starts_with("## Identity"))
    }));
    release.send((StatusCode::OK, compacted(window()))).unwrap();
    let (path, next, release) = mock.request().await;
    assert_eq!(path, "/");
    assert_eq!(
        hub.0.data.lock().unwrap().sessions[&view.id]
            .record
            .working_context,
        "Delegate independent work."
    );
    assert!(
        next["input"].as_array().unwrap().last().unwrap()["content"]
            .as_str()
            .unwrap()
            .contains("Delegate independent work.")
    );
    assert_eq!(
        &next["input"].as_array().unwrap()[..3],
        window().as_array().unwrap()
    );
    assert!(
        next["input"]
            .as_array()
            .unwrap()
            .iter()
            .rev()
            .nth(1)
            .unwrap()["content"]
            .as_str()
            .unwrap()
            .contains("Context compacted")
    );
    release.send((StatusCode::OK, json!([]))).unwrap();
    idle(&hub, &view.id, "idle").await;
    hub.shutdown().await;
}

#[tokio::test]
async fn archive_failure_preserves_capture_and_stops_subsequent_requests() {
    let mut mock = Mock::new().await;
    let state = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let hub = Hub::open(
        Client::test_client_at(mock.endpoint.clone()),
        state.path().into(),
    )
    .unwrap();
    let view = create(&hub, work.path(), "");
    let before = engine(&hub, &view.id)["history"].clone();
    std::fs::write(
        state.path().join("sessions/compactions"),
        "blocked directory",
    )
    .unwrap();
    hub.message(&view.id, "/oai-compact".into()).unwrap();
    idle(&hub, &view.id, "error").await;
    assert!(flush_recording(&hub, &view.id).is_err());
    let retained = hub.0.data.lock().unwrap().sessions[&view.id]
        .persistence
        .retained_archive_records();
    assert!(
        retained
            .iter()
            .any(|record| serde_json::to_value(&record.engine).unwrap()["history"] == before)
    );
    // The provider request is allowed to start before the recorder reports its
    // failure. No response was released, so model context remains unchanged.
    assert_eq!(engine(&hub, &view.id)["history"], before);
    if let Ok((path, request, _release)) = mock.requests.try_recv() {
        assert_eq!(path, "/compact");
        assert_eq!(request["input"], before);
    }
    assert!(
        hub.message(&view.id, "cannot continue after recording failure".into())
            .is_err()
    );
    hub.shutdown().await;
}

#[tokio::test]
#[ignore = "Isolated interactive browser fixture; uses synthetic provider responses"]
async fn browser_fixture() {
    let mut mock = Mock::new().await;
    let server = tokio::spawn(crate::server::serve_client(Client::test_client_at(
        mock.endpoint.clone(),
    )));
    tokio::pin!(server);
    let mut number = 0;
    loop {
        tokio::select! {
            result=&mut server => { result.unwrap().unwrap(); break; }
            request=mock.requests.recv()=>{
                let Some((path,body,release))=request else {break;};
                if path=="/compact" {
                    number+=1;
                    tokio::time::sleep(Duration::from_millis(if number==3 {3000} else {200})).await;
                    let result=if number==2 {(StatusCode::BAD_GATEWAY,json!({"error":"fixture compaction unavailable"}))}
                    else {(StatusCode::OK,compacted(window()))};
                    let _=release.send(result);
                } else {
                    let selected=body["input"].to_string().contains("Operator-selected skill");
                    let already=body["input"].to_string().contains("Compaction will run");
                    let result=if selected && !already {json!([{"type":"function_call","call_id":"browser-compact","name":"oai_compact","arguments":"{}"}])}
                    else {json!([{"type":"message","role":"assistant","content":[{"type":"output_text","text":"Fixture response."}]}])};
                    let _=release.send((StatusCode::OK,result));
                }
            }
        }
    }
}

#[tokio::test]
#[ignore = "Uses the configured OpenAI key for one small synthetic compact/continue smoke"]
async fn live_provider_smoke() {
    let client = Client::new().unwrap();
    assert_eq!(client.model, "gpt-6-sol");
    let instructions = "Reply briefly and preserve the user's codeword.";
    let history = json!([
        {"role":"user","content":"The synthetic test codeword is MAGPIE7."},
        {"role":"assistant","content":"The codeword is MAGPIE7."},
        {"role":"user","content":"Remember the codeword for the next turn."}
    ]);
    let result = client
        .compact(
            history.as_array().unwrap(),
            &client.model,
            instructions,
            &client.config(),
        )
        .await
        .unwrap();
    let mut input = result["output"].as_array().unwrap().clone();
    input.push(
        json!({"role":"user","content":"What is the codeword? Reply with only the codeword."}),
    );
    let mut usage = Value::Null;
    let output = client
        .respond_with(
            &input,
            json!([]),
            &client.model,
            instructions,
            &client.config(),
            |event| {
                if event["type"] == "response.completed" {
                    usage = event["response"]["usage"].clone();
                }
            },
        )
        .await
        .unwrap();
    assert!(serde_json::to_string(&output).unwrap().contains("MAGPIE7"));
    println!(
        "{}",
        json!({"check":"synthetic compact and continuation","model":client.model,
        "profile":client.config(),"compaction_items":result["output"].as_array().unwrap().len(),
        "compaction_usage":result["usage"],"continuation_usage":usage,"codeword_retained":true})
    );
}

#[tokio::test]
async fn clear_first_half_preserves_recent_context_transcript_and_working_note() {
    let mut mock = Mock::new().await;
    let hub = Hub::new(Client::test_client_at(mock.endpoint.clone())).unwrap();
    let work = tempfile::tempdir().unwrap();
    let view = create(&hub, work.path(), "");
    let old = "old payload ".repeat(1000);
    hub.update(&view.id, |record| {
        record.engine.restore_history(vec![
            std::sync::Arc::new(json!({"role":"user","content":old})),
            std::sync::Arc::new(json!({"type":"function_call_output","call_id":"removed","output":"orphan"})),
            std::sync::Arc::new(json!({"role":"user","content":"recent task"})),
            std::sync::Arc::new(json!({"type":"function_call","call_id":"retained","name":"read_file","arguments":"{}"})),
            std::sync::Arc::new(json!({"type":"function_call_output","call_id":"retained","output":"recent evidence"})),
        ]);
        record.working_context = "Keep this role".into();
        Ok(())
    }).unwrap();
    hub.message(&view.id, "/clear-first-half".into()).unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while !hub
            .view(&view.id)
            .unwrap()
            .entries
            .iter()
            .any(|e| e.text.starts_with("Older context cleared:"))
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let state = engine(&hub, &view.id);
    let history = state["history"].as_array().unwrap();
    assert_eq!(history.len(), 3);
    assert_eq!(history[0]["content"], "recent task");
    assert_eq!(history[1]["call_id"], history[2]["call_id"]);
    assert_eq!(
        hub.0.data.lock().unwrap().sessions[&view.id]
            .record
            .working_context,
        "Keep this role"
    );
    assert!(mock.requests.try_recv().is_err());
    hub.shutdown().await;
}

#[tokio::test]
async fn take_context_waits_for_recipient_tools_preserves_archive_and_survives_restart() {
    let mut mock = Mock::new().await;
    let state = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let hub = Hub::open(
        Client::test_client_at(mock.endpoint.clone()),
        state.path().into(),
    )
    .unwrap();
    let source = hub
        .create(Create {
            name: "swift".into(),
            cwd: work.path().to_string_lossy().into(),
            message: "Source task".into(),
        })
        .unwrap();
    let (_, _, release) = mock.request().await;
    release.send((StatusCode::OK, json!([
        {"type":"reasoning","id":"rs-source","encrypted_content":"opaque-source"},
        {"type":"message","role":"assistant","content":[{"type":"output_text","text":"Source result"}]}
    ]))).unwrap();
    idle(&hub, &source.id, "idle").await;
    let original_source = engine(&hub, &source.id);
    let target = hub
        .create(Create {
            name: "xenops".into(),
            cwd: work.path().to_string_lossy().into(),
            message: "Old recipient task".into(),
        })
        .unwrap();
    let (_, _, release) = mock.request().await;
    hub.update(&target.id, |r| {
        r.working_context = "Recipient role".into();
        Ok(())
    })
    .unwrap();
    let profile = hub.profile(&target.id).unwrap();
    let copy_id = hub
        .message(&target.id, format!("/take-context {}", source.id))
        .unwrap();
    release.send((StatusCode::OK,json!([
        {"type":"function_call","call_id":"recipient-tool","name":"list_agents","arguments":"{}"}
    ]))).unwrap();
    idle(&hub, &target.id, "idle").await;
    let copied = engine(&hub, &target.id);
    let items = copied["history"].as_array().unwrap();
    assert_eq!(
        &items[..items.len() - 1],
        original_source["history"].as_array().unwrap()
    );
    assert!(
        items.last().unwrap()["content"]
            .as_str()
            .unwrap()
            .contains(&format!("xenops ({})", target.id))
    );
    assert!(!copied["history"].to_string().contains("Old recipient task"));
    assert_eq!(engine(&hub, &source.id), original_source);
    assert_eq!(hub.profile(&target.id).unwrap(), profile);
    assert!(mock.requests.try_recv().is_err());
    flush_recording(&hub, &target.id).unwrap();
    let archived: Value = serde_json::from_slice(
        &std::fs::read(
            state
                .path()
                .join(format!("sessions/clears/{}-{copy_id}.json", target.id)),
        )
        .unwrap(),
    )
    .unwrap();
    assert!(
        archived["engine"]["history"]
            .to_string()
            .contains("Old recipient task")
    );
    assert!(
        archived["engine"]["history"]
            .as_array()
            .unwrap()
            .iter()
            .any(|i| i["type"] == "function_call_output" && i["call_id"] == "recipient-tool")
    );
    assert!(
        hub.view(&target.id)
            .unwrap()
            .entries
            .iter()
            .any(|e| e.text.as_str() == "Old recipient task")
    );
    hub.shutdown().await;
    drop(hub);
    let hub = Hub::open(
        Client::test_client_at(mock.endpoint.clone()),
        state.path().into(),
    )
    .unwrap();
    assert_eq!(engine(&hub, &target.id)["history"], copied["history"]);
    assert_eq!(
        hub.0.data.lock().unwrap().sessions[&target.id]
            .record
            .working_context,
        "Recipient role"
    );
    hub.message(&target.id, "Continue as xenops".into())
        .unwrap();
    let (_, request, release) = mock.request().await;
    assert!(request["input"].to_string().contains("Source task"));
    assert!(!request["input"].to_string().contains("Old recipient task"));
    assert!(
        request["input"].as_array().unwrap().last().unwrap()["content"]
            .as_str()
            .unwrap()
            .contains(&target.id)
    );
    release.send((StatusCode::OK, json!([]))).unwrap();
    idle(&hub, &target.id, "idle").await;
    hub.shutdown().await;
}

#[tokio::test]
async fn take_context_copies_images_closes_unfinished_calls_and_retains_opaque_items() {
    let mut mock = Mock::new().await;
    let hub = Hub::new(Client::test_client_at(mock.endpoint.clone())).unwrap();
    let work = tempfile::tempdir().unwrap();
    let source = create(&hub, work.path(), "");
    let target = create(&hub, work.path(), "");
    let store = hub.image_store();
    let mut png = std::io::Cursor::new(Vec::new());
    image::DynamicImage::new_rgb8(1, 1)
        .write_to(&mut png, image::ImageFormat::Png)
        .unwrap();
    let image = store.put(&source.id, "image/png", png.get_ref()).unwrap();
    let history = vec![
        Arc::new(
            json!({"role":"user","content":[{"type":"input_image","image_url":crate::images::ImageStore::reference(&source.id,&image)}]}),
        ),
        Arc::new(
            json!({"type":"compaction","id":"cmp-source","encrypted_content":"retained-opaque"}),
        ),
        Arc::new(
            json!({"type":"function_call","call_id":"pending-source","name":"exec_command","arguments":"{\"cmd\":\"must never execute\"}"}),
        ),
    ];
    hub.update(&source.id, |r| {
        r.engine.restore_history(history.clone());
        Ok(())
    })
    .unwrap();
    let before = engine(&hub, &source.id);
    hub.message(&target.id, format!("/take-context {}", source.id))
        .unwrap();
    idle(&hub, &target.id, "idle").await;
    assert_eq!(engine(&hub, &source.id), before);
    assert!(mock.requests.try_recv().is_err());
    let copied = engine(&hub, &target.id);
    assert_eq!(copied["history"][1], *history[1]);
    assert!(
        copied["history"][0]["content"][0]["image_url"]
            .as_str()
            .unwrap()
            .contains(&target.id)
    );
    assert_eq!(
        store.read(&target.id, &image.id).unwrap().1.as_slice(),
        png.get_ref()
    );
    assert!(copied["history"].as_array().unwrap().iter().any(|i| {
        i["type"] == "function_call_output"
            && i["call_id"] == "pending-source"
            && i["output"]
                .as_str()
                .unwrap()
                .contains("source agent may still")
    }));
    hub.message(&target.id, "Describe the image".into())
        .unwrap();
    let (_, request, release) = mock.request().await;
    assert!(
        request["input"][0]["content"][0]["image_url"]
            .as_str()
            .unwrap()
            .starts_with("data:image/png;base64,")
    );
    release.send((StatusCode::OK, json!([]))).unwrap();
    idle(&hub, &target.id, "idle").await;
    hub.shutdown().await;
}

#[tokio::test]
async fn take_context_invalid_or_missing_peer_preserves_recipient_and_consumes_failed_command() {
    let mut mock = Mock::new().await;
    let hub = Hub::new(Client::test_client_at(mock.endpoint.clone())).unwrap();
    let work = tempfile::tempdir().unwrap();
    let source = create(&hub, work.path(), "");
    let target = create(&hub, work.path(), "Original task");
    let (_, _, release) = mock.request().await;
    assert!(
        hub.message(&target.id, format!("/take-context {}", target.id))
            .is_err()
    );
    assert!(
        hub.message(&target.id, "/take-context missing".into())
            .is_err()
    );
    let peers = hub.context_peers(&target.id).unwrap();
    assert_eq!(peers["peers"].as_array().unwrap().len(), 1);
    assert_eq!(peers["peers"][0]["id"], source.id);
    hub.message(&target.id, format!("/take-context {}", source.id))
        .unwrap();
    hub.close(&source.id).await.unwrap();
    release.send((StatusCode::OK, json!([]))).unwrap();
    idle(&hub, &target.id, "idle").await;
    assert!(
        engine(&hub, &target.id)["history"]
            .to_string()
            .contains("Original task")
    );
    assert!(
        hub.view(&target.id)
            .unwrap()
            .entries
            .iter()
            .any(|e| e.text.starts_with("Context unchanged:"))
    );
    assert!(mock.requests.try_recv().is_err());
    hub.message(&target.id, "Still usable".into()).unwrap();
    let (_, request, release) = mock.request().await;
    assert!(request["input"].to_string().contains("Still usable"));
    assert!(!request["input"].to_string().contains("/take-context"));
    release.send((StatusCode::OK, json!([]))).unwrap();
    idle(&hub, &target.id, "idle").await;
    hub.shutdown().await;
}

#[tokio::test]
async fn reload_refreshes_base_without_clearing_history_or_starting_inference() {
    let mut session = Session::new(
        "test".into(),
        "old base\nretained catalog".into(),
        json!([]),
        "identity".into(),
    );
    session.skill_base = Some("old base".into());
    session.user("Retain this conversation".into());
    let before = serde_json::to_value(&session).unwrap();
    session.reload_system_prompt();
    let after = serde_json::to_value(&session).unwrap();
    assert_eq!(after["history"], before["history"]);
    assert_eq!(after["skill_base"], include_str!("PRINCIPLES.md"));
    assert!(
        after["instructions"]
            .as_str()
            .unwrap()
            .ends_with("\nretained catalog")
    );
    assert!(!after["instructions"].as_str().unwrap().contains("old base"));
    for field in [
        "model",
        "profile",
        "tools",
        "model_requests",
        "auto_oai_compact",
    ] {
        assert_eq!(after[field], before[field]);
    }
    let mut mock = Mock::new().await;
    let hub = Hub::new(Client::test_client_at(mock.endpoint.clone())).unwrap();
    let work = tempfile::tempdir().unwrap();
    let target = create(&hub, work.path(), "");
    let history = engine(&hub, &target.id)["history"].clone();
    hub.message(&target.id, "/reload".into()).unwrap();
    idle(&hub, &target.id, "idle").await;
    assert_eq!(engine(&hub, &target.id)["history"], history);
    assert!(
        hub.view(&target.id)
            .unwrap()
            .entries
            .iter()
            .any(|e| e.text.as_str() == "System prompt reloaded.")
    );
    assert!(mock.requests.try_recv().is_err());
    hub.shutdown().await;
}
