use super::*;
use axum::{Json, Router, routing::post};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};
type Request = (Value, oneshot::Sender<Value>);

async fn fixture(
    state: Option<PathBuf>,
) -> (Hub, mpsc::Receiver<Request>, tokio::task::JoinHandle<()>) {
    let (requests, received) = mpsc::channel::<Request>(16);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let client = Client::test_client_at(format!("http://{}/", listener.local_addr().unwrap()));
    let router = Router::new().route(
        "/",
        post(move |Json(input): Json<Value>| {
            let requests = requests.clone();
            async move {
                let (release, wait) = oneshot::channel();
                requests.send((input, release)).await.unwrap();
                let output = wait.await.unwrap_or(json!([]));
                let response = if output.is_array() {
                    json!({"output":output})
                } else {
                    output
                };
                (
                    [("content-type", "text/event-stream")],
                    format!(
                        "data: {}\n\n",
                        if response.get("type").is_some() {
                            response
                        } else {
                            json!({"type":"response.completed","response":response})
                        }
                    ),
                )
            }
        }),
    );
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let hub = match state {
        Some(path) => Hub::open(client, path),
        None => Hub::new(client),
    }
    .unwrap();
    (hub, received, server)
}
async fn next(rx: &mut mpsc::Receiver<Request>) -> Request {
    tokio::time::timeout(Duration::from_secs(3), rx.recv())
        .await
        .unwrap()
        .unwrap()
}
fn agent(hub: &Hub, work: &std::path::Path, name: &str, message: &str) -> View {
    hub.create(Create {
        name: name.into(),
        cwd: work.to_string_lossy().into(),
        message: message.into(),
    })
    .unwrap()
}
fn call(id: &str, name: &str, arguments: Value) -> Value {
    json!({"type":"function_call","call_id":id,"name":name,"arguments":arguments.to_string()})
}

fn prompt(id: &str, text: &str, seconds: Option<u64>) -> Value {
    call(
        id,
        "prompt_user",
        json!({"question":text,"blocking":seconds.is_some(),"wait_seconds":seconds}),
    )
}

async fn question(hub: &Hub, id: &str) -> Question {
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if let Some(q) = hub.view(id).unwrap().questions.first() {
                return q.clone();
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap()
}

async fn idle(hub: &Hub, id: &str, status: &str) {
    tokio::time::timeout(Duration::from_secs(3), async {
        while hub.view(id).unwrap().status != status {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
}

fn result(request: &Value, call_id: &str) -> Value {
    let item = request["input"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["type"] == "function_call_output" && item["call_id"] == call_id)
        .unwrap();
    serde_json::from_str(item["output"].as_str().unwrap()).unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancelled_queued_answer_can_be_submitted_again_after_stop_or_end() {
    for end in [false, true] {
        let work = tempfile::tempdir().unwrap();
        let (hub, mut rx, server) = fixture(None).await;
        let a = agent(&hub, work.path(), "cancel-reply", "Start");
        next(&mut rx)
            .await
            .1
            .send(json!([prompt("queued", "Which target?", None)]))
            .unwrap();
        let (_, hold_response) = next(&mut rx).await;
        let q = question(&hub, &a.id).await;
        let first = hub.answer_question(&a.id, &q.id, "Staging".into()).unwrap();
        if end {
            hub.close(&a.id).await.unwrap();
            hub.reopen(&a.id).unwrap();
        } else {
            hub.stop(&a.id).unwrap();
            idle(&hub, &a.id, "stopped").await;
        }
        let open = hub.view(&a.id).unwrap().questions;
        assert_eq!(open.len(), 1);
        assert_eq!(open[0].id, q.id);
        assert_eq!(open[0].wait_outcome.as_deref(), Some("answer_cancelled"));
        let second = hub.answer_question(&a.id, &q.id, "Staging".into()).unwrap();
        assert_ne!(first, second);
        let (request, release) = next(&mut rx).await;
        assert_eq!(
            request["input"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|item| item["role"] == "user"
                    && item["content"]
                        .as_str()
                        .is_some_and(|s| s.contains("Staging")))
                .count(),
            1
        );
        release.send(json!([])).unwrap();
        drop(hold_response);
        hub.shutdown().await;
        server.abort();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn nonblocking_questions_continue_and_deduplicate_with_durable_answers() {
    let state = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let (hub, mut rx, server) = fixture(Some(state.path().into())).await;
    let a = agent(&hub, work.path(), "questions", "Start");
    let (request, release) = next(&mut rx).await;
    assert!(
        request["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["name"] == "prompt_user")
    );
    release
        .send(json!([
            prompt("q1", "Which color?", None),
            prompt("q2", "Which color?", None)
        ]))
        .unwrap();
    let (request, release) = next(&mut rx).await;
    let q = question(&hub, &a.id).await;
    assert_eq!(hub.view(&a.id).unwrap().questions.len(), 1);
    assert_eq!(result(&request, "q1")["question_id"], q.id);
    assert_eq!(result(&request, "q2")["question_id"], q.id);
    assert_eq!(result(&request, "q1")["status"], "open");
    release.send(json!([])).unwrap();
    idle(&hub, &a.id, "idle").await;
    hub.shutdown().await;
    drop(hub);
    server.abort();

    let (hub, mut rx, server) = fixture(Some(state.path().into())).await;
    assert_eq!(hub.view(&a.id).unwrap().questions[0].id, q.id);
    let message_id = hub.answer_question(&a.id, &q.id, "Blue".into()).unwrap();
    assert_eq!(
        hub.answer_question(&a.id, &q.id, "Blue".into()).unwrap(),
        message_id
    );
    assert!(hub.answer_question(&a.id, &q.id, "Red".into()).is_err());
    assert!(hub.view(&a.id).unwrap().questions.is_empty());
    let (request, release) = next(&mut rx).await;
    assert_eq!(
        request["input"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|item| item["role"] == "user"
                && item["content"].as_str().is_some_and(|s| s.contains("Blue")))
            .count(),
        1
    );
    assert_eq!(
        hub.view(&a.id)
            .unwrap()
            .entries
            .iter()
            .filter(|e| e.id == message_id)
            .count(),
        1
    );
    release.send(json!([])).unwrap();
    hub.shutdown().await;
    server.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn blocking_question_waits_for_answer_and_never_executes_answer_as_slash_command() {
    let work = tempfile::tempdir().unwrap();
    let (hub, mut rx, server) = fixture(None).await;
    let a = agent(&hub, work.path(), "waiting", "Keep this task context");
    next(&mut rx)
        .await
        .1
        .send(json!([prompt("blocking", "What next?", Some(30))]))
        .unwrap();
    let q = question(&hub, &a.id).await;
    assert!(
        tokio::time::timeout(Duration::from_millis(100), rx.recv())
            .await
            .is_err()
    );
    hub.answer_question(&a.id, &q.id, "/clear".into()).unwrap();
    let (request, release) = next(&mut rx).await;
    assert_eq!(result(&request, "blocking")["status"], "answered");
    assert!(
        request["input"]
            .to_string()
            .contains("Keep this task context")
    );
    assert!(request["input"].to_string().contains("/clear"));
    release.send(json!([])).unwrap();
    hub.shutdown().await;
    server.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn timeout_returns_control_keeps_question_and_accepts_late_answer() {
    let work = tempfile::tempdir().unwrap();
    let (hub, mut rx, server) = fixture(None).await;
    let a = agent(&hub, work.path(), "overnight", "Start");
    next(&mut rx)
        .await
        .1
        .send(json!([prompt("timeout", "Which target?", Some(1))]))
        .unwrap();
    let q = question(&hub, &a.id).await;
    let (request, release) = next(&mut rx).await;
    assert_eq!(result(&request, "timeout")["status"], "timed_out");
    assert!(
        result(&request, "timeout")["message"]
            .as_str()
            .unwrap()
            .contains("not an answer or approval")
    );
    assert_eq!(
        hub.view(&a.id).unwrap().questions[0]
            .wait_outcome
            .as_deref(),
        Some("timed_out")
    );
    // A repeated call must return immediately and preserve the original deadline.
    release
        .send(json!([prompt("again", "Which target?", Some(86400))]))
        .unwrap();
    let (request, release) = next(&mut rx).await;
    assert_eq!(result(&request, "again")["question_id"], q.id);
    assert_eq!(
        result(&request, "again")["wait_until_ms"],
        q.wait_until_ms.unwrap()
    );
    assert_eq!(result(&request, "again")["status"], "timed_out");
    release.send(json!([])).unwrap();
    idle(&hub, &a.id, "idle").await;
    hub.answer_question(&a.id, &q.id, "Staging".into()).unwrap();
    let (request, release) = next(&mut rx).await;
    assert!(request["input"].to_string().contains("Staging"));
    release.send(json!([])).unwrap();
    hub.shutdown().await;
    server.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stop_cancels_wait_and_restart_retains_question_and_receipt() {
    let work = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let (hub, mut rx, server) = fixture(Some(state.path().into())).await;
    let a = agent(&hub, work.path(), "stoppable", "Start");
    next(&mut rx)
        .await
        .1
        .send(json!([prompt(
            "stop-question",
            "Approve the next step?",
            Some(1)
        )]))
        .unwrap();
    let q = question(&hub, &a.id).await;
    hub.stop(&a.id).unwrap();
    idle(&hub, &a.id, "stopped").await;
    assert!(
        tokio::time::timeout(Duration::from_millis(1100), rx.recv())
            .await
            .is_err()
    );
    assert_eq!(
        hub.view(&a.id).unwrap().questions[0]
            .wait_outcome
            .as_deref(),
        Some("interrupted")
    );
    hub.shutdown().await;
    drop(hub);
    server.abort();
    let (hub, mut rx, server) = fixture(Some(state.path().into())).await;
    assert_eq!(hub.view(&a.id).unwrap().status, "stopped");
    hub.answer_question(&a.id, &q.id, "Continue with staging only".into())
        .unwrap();
    let (request, release) = next(&mut rx).await;
    assert_eq!(result(&request, "stop-question")["question_id"], q.id);
    assert!(
        request["input"]
            .to_string()
            .contains("Continue with staging only")
    );
    release.send(json!([])).unwrap();
    hub.shutdown().await;
    server.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ordinary_steering_releases_wait_without_answering_the_question() {
    let work = tempfile::tempdir().unwrap();
    let (hub, mut rx, server) = fixture(None).await;
    let a = agent(&hub, work.path(), "steering", "Start");
    next(&mut rx)
        .await
        .1
        .send(json!([prompt("steer", "Which target?", Some(60))]))
        .unwrap();
    question(&hub, &a.id).await;
    hub.message(&a.id, "Work on the documentation meanwhile".into())
        .unwrap();
    let (request, release) = next(&mut rx).await;
    assert_eq!(
        result(&request, "steer")["status"],
        "continued_for_user_message"
    );
    assert!(
        request["input"]
            .to_string()
            .contains("documentation meanwhile")
    );
    assert!(hub.view(&a.id).unwrap().questions[0].answer.is_none());
    release.send(json!([])).unwrap();
    hub.shutdown().await;
    server.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn escape_preserves_posted_question_and_receipt_before_followup() {
    let work = tempfile::tempdir().unwrap();
    let (hub, mut rx, server) = fixture(None).await;
    let a = agent(&hub, work.path(), "escape", "Start");
    next(&mut rx)
        .await
        .1
        .send(json!([prompt("escape", "Which target?", Some(60))]))
        .unwrap();
    let q = question(&hub, &a.id).await;
    hub.interrupt(&a.id).unwrap();
    idle(&hub, &a.id, "stopped").await;
    assert_eq!(
        hub.view(&a.id).unwrap().questions[0]
            .wait_outcome
            .as_deref(),
        Some("interrupted")
    );
    hub.message(&a.id, "Continue with the independent work".into())
        .unwrap();
    let (request, release) = next(&mut rx).await;
    assert_eq!(result(&request, "escape")["question_id"], q.id);
    assert_eq!(
        request["input"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|item| item["type"] == "function_call_output" && item["call_id"] == "escape")
            .count(),
        1
    );
    release.send(json!([])).unwrap();
    hub.shutdown().await;
    server.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn crash_between_question_commit_and_tool_result_recovers_receipt_without_replay() {
    let state = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let (hub, mut rx, server) = fixture(Some(state.path().into())).await;
    let a = agent(&hub, work.path(), "crash", "Start");
    next(&mut rx)
        .await
        .1
        .send(json!([prompt("crash", "Which target?", Some(60))]))
        .unwrap();
    let q = question(&hub, &a.id).await;
    let path = state.path().join("sessions").join(format!("{}.json", a.id));
    let crash_record = std::fs::read(&path).unwrap();
    let record: Value = serde_json::from_slice(&crash_record).unwrap();
    assert_eq!(record["engine"]["running"]["id"], "crash");
    assert_eq!(record["version"], 4);
    assert!(
        state
            .path()
            .join("sessions")
            .join(format!("{}.before-prompt-user", a.id))
            .exists()
    );
    hub.shutdown().await;
    drop(hub);
    server.abort();
    // Restore the exact on-disk state observed before a tool result was committed.
    std::fs::write(&path, crash_record).unwrap();
    let (hub, mut rx, server) = fixture(Some(state.path().into())).await;
    assert_eq!(hub.view(&a.id).unwrap().status, "stopped");
    assert_eq!(hub.view(&a.id).unwrap().questions.len(), 1);
    hub.answer_question(&a.id, &q.id, "The staging target".into())
        .unwrap();
    let (request, release) = next(&mut rx).await;
    assert_eq!(result(&request, "crash")["question_id"], q.id);
    assert_eq!(result(&request, "crash")["status"], "interrupted");
    assert!(request["input"].to_string().contains("The staging target"));
    release.send(json!([])).unwrap();
    hub.shutdown().await;
    server.abort();
}
