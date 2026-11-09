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
                    (
                        status,
                        [("content-type", "text/event-stream")],
                        format!(
                            "data: {}\n\n",
                            json!({"type":"response.completed","response":{"output":result}})
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
        while hub.view(id).unwrap().status != status {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
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
    hub.message(&view.id, "/oai-compact".into()).unwrap();
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
    assert_eq!(
        hub.view(&view.id).unwrap().entries[0].text,
        view.entries[0].text
    );
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
    assert_eq!(
        hub.0.data.lock().unwrap().sessions[&view.id]
            .record
            .usage
            .records
            .len(),
        1
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
    idle(&hub, &view.id, "stopped").await;
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
    let hub = Hub::new(Client::test_client_at(mock.endpoint.clone())).unwrap();
    let work = tempfile::tempdir().unwrap();
    let skill = work.path().join(".agents/skills/oai-compact");
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
                {"type":"function_call","call_id":"compact","name":"oai_compact","arguments":"{}"},
                {"type":"function_call","call_id":"agents","name":"list_agents","arguments":"{}"}
            ]),
        ))
        .unwrap();
    let (path, compact, release) = mock.request().await;
    assert_eq!(path, "/compact");
    for id in ["compact", "agents"] {
        assert!(
            compact["input"]
                .as_array()
                .unwrap()
                .iter()
                .any(|v| v["type"] == "function_call_output" && v["call_id"] == id)
        );
    }
    release.send((StatusCode::OK, compacted(window()))).unwrap();
    let (path, next, release) = mock.request().await;
    assert_eq!(path, "/");
    assert_eq!(
        &next["input"].as_array().unwrap()[..3],
        window().as_array().unwrap()
    );
    assert!(
        next["input"].as_array().unwrap().last().unwrap()["content"]
            .as_str()
            .unwrap()
            .contains("Context compacted")
    );
    release.send((StatusCode::OK, json!([]))).unwrap();
    idle(&hub, &view.id, "idle").await;
    hub.shutdown().await;
}

#[tokio::test]
async fn archive_failure_prevents_compaction_request() {
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
    assert_eq!(engine(&hub, &view.id)["history"], before);
    assert!(mock.requests.try_recv().is_err());
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
        .compact(history.as_array().unwrap(), &client.model, instructions)
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
