use super::*;
use axum::{Json, Router, routing::post};
use tokio::sync::{mpsc, oneshot};

const FAILED_TEXT: &str = "abandoned-attempt-text";
const ACCEPTED_TEXT: &str = "accepted-replacement-text";

struct ResponseFixture {
    endpoint: String,
    requests: mpsc::Receiver<(Value, oneshot::Sender<String>)>,
    server: tokio::task::JoinHandle<()>,
}

impl Drop for ResponseFixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}

impl ResponseFixture {
    async fn new() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}/", listener.local_addr().unwrap());
        let (sent, requests) = mpsc::channel(8);
        let router = Router::new().route(
            "/",
            post(move |Json(request): Json<Value>| {
                let sent = sent.clone();
                async move {
                    let (release, response) = oneshot::channel::<String>();
                    sent.send((request, release)).await.unwrap();
                    (
                        [("content-type", "text/event-stream")],
                        response.await.unwrap_or_default(),
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

    async fn next(&mut self) -> (Value, oneshot::Sender<String>) {
        tokio::time::timeout(Duration::from_secs(6), self.requests.recv())
            .await
            .expect("model request did not arrive")
            .unwrap()
    }
}

fn sse(events: Vec<Value>) -> String {
    events
        .into_iter()
        .map(|event| format!("data: {event}\n\n"))
        .collect()
}

fn failed_attempt(retry_seconds: u64) -> String {
    let arguments = json!({"path":"failed-effect", "content":"must never be written"}).to_string();
    sse(vec![
        json!({"type":"response.output_text.delta", "item_id":"failed-message", "output_index":0, "delta":FAILED_TEXT}),
        json!({"type":"response.function_call_arguments.delta", "item_id":"failed-item", "output_index":1, "delta":arguments}),
        json!({"type":"response.output_item.done", "output_index":1, "item":{
            "id":"failed-item", "type":"function_call", "call_id":"failed-call", "name":"write_file", "arguments":arguments}}),
        json!({"type":"error", "error":{"code":"server_is_overloaded", "type":"service_unavailable_error",
            "message":"fixture temporary failure", "headers":{"retry-after":retry_seconds.to_string()}}}),
    ])
}

fn replacement() -> String {
    let message = json!({"id":"accepted-message", "type":"message", "role":"assistant", "status":"completed",
        "content":[{"type":"output_text", "text":ACCEPTED_TEXT, "annotations":[]}]});
    let call = json!({"id":"accepted-item", "type":"function_call", "call_id":"accepted-call", "name":"exec_command",
        "arguments":json!({"command":"printf X >> successful-effects", "timeout_ms":1000}).to_string()});
    sse(vec![
        json!({"type":"response.output_text.delta", "item_id":"accepted-message", "output_index":0, "delta":ACCEPTED_TEXT}),
        json!({"type":"response.output_item.done", "output_index":0, "item":message}),
        json!({"type":"response.output_item.done", "output_index":1, "item":call}),
        json!({"type":"response.completed", "response":{"output":[message, call]}}),
    ])
}

fn empty_completion() -> String {
    sse(vec![
        json!({"type":"response.completed", "response":{"output":[]}}),
    ])
}

async fn wait_status(hub: &Hub, id: &str, expected: &str) {
    tokio::time::timeout(Duration::from_secs(6), async {
        loop {
            let view = hub.view(id).unwrap();
            assert_ne!(
                view.status,
                "error",
                "unexpected runtime error: {}",
                serde_json::to_string(&view.entries).unwrap()
            );
            if view.status == expected {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("runtime status did not settle");
}

async fn wait_retry(events: &mut tokio::sync::broadcast::Receiver<Value>, id: &str) {
    tokio::time::timeout(Duration::from_secs(6), async {
        loop {
            let event = events.recv().await.unwrap();
            if event["type"] == "entry"
                && event["session_id"] == id
                && event["entry"]["kind"] == "notice"
                && event["entry"]["text"]
                    .as_str()
                    .is_some_and(|text| text.contains("retrying"))
            {
                break;
            }
        }
    })
    .await
    .expect("retry notice was not published");
}

fn assert_no_failed_model_state(hub: &Hub, id: &str) {
    let data = hub.0.data.lock().unwrap();
    let engine = &data.sessions[id].record.engine;
    assert!(
        engine.partial_text.is_empty(),
        "rejected partial must be cleared in stored engine"
    );
    let history = serde_json::to_value(engine).unwrap()["history"].to_string();
    assert!(!history.contains(FAILED_TEXT));
    assert!(!history.contains("failed-call"));
}

#[tokio::test]
async fn response_retry_discards_failed_model_state_and_executes_replacement_tool_once() {
    let mut fixture = ResponseFixture::new().await;
    let state = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let hub = Hub::open(
        Client::test_client_at(fixture.endpoint.clone()),
        state.path().into(),
    )
    .unwrap();
    let view = hub
        .create(Create {
            name: "retry-fixture".into(),
            cwd: work.path().to_string_lossy().into_owned(),
            message: String::new(),
        })
        .unwrap();
    let (_, mut events) = hub.subscribe();
    hub.message(&view.id, "Run the fixture action".into())
        .unwrap();
    let (original, release) = fixture.next().await;
    release.send(failed_attempt(0)).unwrap();
    wait_retry(&mut events, &view.id).await;
    assert_no_failed_model_state(&hub, &view.id);
    assert!(!work.path().join("failed-effect").exists());

    let (retried, release) = fixture.next().await;
    assert_eq!(
        retried, original,
        "retry must preserve complete request and billing/model metadata"
    );
    release.send(replacement()).unwrap();
    let (continuation, release) = fixture.next().await;
    let input = continuation["input"].to_string();
    assert!(!input.contains(FAILED_TEXT));
    assert!(!input.contains("failed-call"));
    assert!(input.contains(ACCEPTED_TEXT));
    assert_eq!(
        continuation["input"]
            .as_array()
            .unwrap()
            .iter()
            .filter(
                |item| item["type"] == "function_call_output" && item["call_id"] == "accepted-call"
            )
            .count(),
        1
    );
    assert_eq!(
        std::fs::read_to_string(work.path().join("successful-effects")).unwrap(),
        "X"
    );
    assert!(!work.path().join("failed-effect").exists());
    release.send(empty_completion()).unwrap();
    wait_status(&hub, &view.id, "idle").await;
    assert_no_failed_model_state(&hub, &view.id);
    let entries = hub.view(&view.id).unwrap().entries;
    let failed = entries
        .iter()
        .position(|entry| entry.kind == "assistant" && entry.text.as_str() == FAILED_TEXT)
        .unwrap();
    let retry = entries
        .iter()
        .position(|entry| entry.kind == "notice" && entry.text.contains("retrying"))
        .unwrap();
    let accepted = entries
        .iter()
        .position(|entry| entry.kind == "assistant" && entry.text.as_str() == ACCEPTED_TEXT)
        .unwrap();
    assert!(
        failed < retry && retry < accepted,
        "failed display evidence must remain separated from replacement"
    );
    assert_eq!(
        entries
            .iter()
            .filter(|entry| entry.kind == "tool_call" && entry.text.contains("(accepted-call)"))
            .count(),
        1
    );
    assert!(
        !entries
            .iter()
            .any(|entry| entry.kind == "tool_call" && entry.text.contains("(failed-call)"))
    );
    hub.shutdown().await;
    drop(hub);
    let restored = Hub::open(
        Client::test_client_at(fixture.endpoint.clone()),
        state.path().into(),
    )
    .unwrap();
    assert_no_failed_model_state(&restored, &view.id);
    assert_eq!(
        std::fs::read_to_string(work.path().join("successful-effects")).unwrap(),
        "X"
    );
    restored.shutdown().await;
}

#[tokio::test]
async fn stop_during_response_backoff_cancels_retry_and_does_not_recover_failed_partial() {
    let mut fixture = ResponseFixture::new().await;
    let state = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let hub = Hub::open(
        Client::test_client_at(fixture.endpoint.clone()),
        state.path().into(),
    )
    .unwrap();
    let view = hub
        .create(Create {
            name: "stop-fixture".into(),
            cwd: work.path().to_string_lossy().into_owned(),
            message: String::new(),
        })
        .unwrap();
    let (_, mut events) = hub.subscribe();
    hub.message(&view.id, "Start the fixture".into()).unwrap();
    let (_, release) = fixture.next().await;
    release.send(failed_attempt(2)).unwrap();
    wait_retry(&mut events, &view.id).await;
    assert_no_failed_model_state(&hub, &view.id);
    hub.stop(&view.id).unwrap();
    wait_status(&hub, &view.id, "stopped").await;
    assert!(
        tokio::time::timeout(Duration::from_millis(2200), fixture.requests.recv())
            .await
            .is_err(),
        "Stop must cancel the scheduled HTTP retry, not just hide its output"
    );
    assert_no_failed_model_state(&hub, &view.id);
    assert!(!work.path().join("failed-effect").exists());
    assert!(!work.path().join("successful-effects").exists());
    hub.shutdown().await;
    drop(hub);
    let restored = Hub::open(
        Client::test_client_at(fixture.endpoint.clone()),
        state.path().into(),
    )
    .unwrap();
    assert_no_failed_model_state(&restored, &view.id);
    {
        let data = restored.0.data.lock().unwrap();
        let record = &data.sessions[&view.id].record;
        assert!(record.paused);
        assert_eq!(record.listen_on_restart, Some(false));
        assert!(!record.engine.request_in_flight());
    }
    restored.shutdown().await;
}
