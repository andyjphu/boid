//! Real shell processes and loopback model responses; no external provider calls.
use super::*;
use axum::{Json, Router, routing::post};
use tokio::sync::{mpsc, oneshot};

struct Model {
    endpoint: String,
    requests: mpsc::Receiver<(Value, oneshot::Sender<Value>)>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Model {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Model {
    async fn new() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}/", listener.local_addr().unwrap());
        let (send, requests) = mpsc::channel(8);
        let router = Router::new().route(
            "/",
            post(move |Json(request): Json<Value>| {
                let send = send.clone();
                async move {
                    let (reply, wait) = oneshot::channel();
                    send.send((request, reply)).await.unwrap();
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
        Self {
            endpoint,
            requests,
            task: tokio::spawn(async move {
                axum::serve(listener, router).await.unwrap();
            }),
        }
    }
    async fn next(&mut self) -> (Value, oneshot::Sender<Value>) {
        tokio::time::timeout(Duration::from_secs(10), self.requests.recv())
            .await
            .unwrap()
            .unwrap()
    }
}
fn call(id: &str, name: &str, arguments: Value) -> Value {
    json!({"type":"function_call","id":format!("fc_{id}"),"call_id":id,"name":name,"arguments":arguments.to_string()})
}
fn command(id: &str, command: String) -> Value {
    call(
        id,
        "exec_command",
        json!({"command":command,"timeout_ms":10000}),
    )
}
async fn until(mut condition: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(10), async {
        while !condition() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("parallel tool condition timed out");
}
fn results(request: &Value) -> BTreeMap<String, Value> {
    request["input"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|v| v["type"] == "function_call_output")
        .map(|v| {
            (
                v["call_id"].as_str().unwrap().to_owned(),
                serde_json::from_str(v["output"].as_str().unwrap()).unwrap(),
            )
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn parallel_tools_overlap_refill_obey_barriers_and_return_attributed_errors() {
    let mut model = Model::new().await;
    let state = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let hub = Hub::open(
        Client::test_client_at(model.endpoint.clone()),
        state.path().into(),
    )
    .unwrap();
    let view = hub
        .create(Create {
            name: "parallel".into(),
            cwd: work.path().to_string_lossy().into(),
            message: "fixture".into(),
        })
        .unwrap();
    let (request, reply) = model.next().await;
    assert_eq!(request["parallel_tool_calls"], true);
    assert!(
        request["instructions"]
            .as_str()
            .unwrap()
            .contains("batching them declares they are independent")
    );
    let mut calls=(0..10).map(|i|command(&format!("c{i}"),format!("touch start-{i}; while [ ! -f release-{i} ]; do sleep 0.01; done; printf result-{i}"))).collect::<Vec<_>>();
    calls.push(call(
        "write",
        "write_file",
        json!({"path":"barrier","content":"written"}),
    ));
    calls.push(command("after", "cat barrier; touch after".into()));
    calls.push(call("error", "read_file", json!({"path":"missing"})));
    calls.push(call(
        "timeout",
        "exec_command",
        json!({"command":"sleep 5", "timeout_ms":25}),
    ));
    reply.send(json!(calls)).unwrap();
    until(|| (0..8).all(|i| work.path().join(format!("start-{i}")).exists())).await;
    assert!(!work.path().join("start-8").exists(), "bounded fan-out");
    assert!(!work.path().join("barrier").exists());
    std::fs::write(work.path().join("release-1"), "").unwrap();
    until(|| work.path().join("start-8").exists()).await;
    assert!(!work.path().join("start-9").exists());
    std::fs::write(work.path().join("release-8"), "").unwrap();
    until(|| work.path().join("start-9").exists()).await;
    assert!(
        !work.path().join("barrier").exists(),
        "write waits for preceding group"
    );
    for i in 1..10 {
        std::fs::write(work.path().join(format!("release-{i}")), "").unwrap();
    }
    until(|| {
        hub.view(&view.id)
            .unwrap()
            .entries
            .iter()
            .filter(|e| e.kind == "tool_result")
            .count()
            == 9
    })
    .await;
    assert!(!work.path().join("after").exists());
    std::fs::write(work.path().join("release-0"), "").unwrap();
    let (request, reply) = model.next().await;
    let outputs = results(&request);
    assert_eq!(outputs.len(), 14);
    for i in 0..10 {
        assert_eq!(
            outputs[&format!("c{i}")]["stdout"]["text"],
            format!("result-{i}")
        );
    }
    assert_eq!(outputs["after"]["stdout"]["text"], "written");
    assert_eq!(outputs["timeout"]["timed_out"], true);
    assert!(outputs["error"]["error"].is_string());
    assert_eq!(
        hub.0.data.lock().unwrap().sessions[&view.id].record.version,
        10
    );
    reply.send(json!([])).unwrap();
    until(|| hub.view(&view.id).unwrap().status == "idle").await;
    hub.shutdown().await;
    drop(hub);
    let restored = Hub::open(Client::test_client(), state.path().into()).unwrap();
    assert_eq!(
        restored
            .view(&view.id)
            .unwrap()
            .entries
            .iter()
            .filter(|e| e.kind == "tool_result")
            .count(),
        14
    );
    restored.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn parallel_tools_escape_preserves_completed_kills_process_groups_and_recovers_crash_snapshot()
 {
    let mut model = Model::new().await;
    let state = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let hub = Hub::open(
        Client::test_client_at(model.endpoint.clone()),
        state.path().into(),
    )
    .unwrap();
    let view = hub
        .create(Create {
            name: "interrupt".into(),
            cwd: work.path().to_string_lossy().into(),
            message: "fixture".into(),
        })
        .unwrap();
    let (_, reply) = model.next().await;
    let mut calls = vec![command("fast", "printf retained".into())];
    for name in ["a", "b"] {
        calls.push(command(name,format!("echo $$ > leader-{name}; sleep 300 & echo $! > child-{name}; touch start-{name}; wait")));
    }
    calls.push(call(
        "unstarted",
        "write_file",
        json!({"path":"must-not-exist","content":"bad"}),
    ));
    reply.send(json!(calls)).unwrap();
    until(|| {
        work.path().join("start-a").exists()
            && work.path().join("start-b").exists()
            && hub
                .view(&view.id)
                .unwrap()
                .entries
                .iter()
                .any(|e| e.kind == "tool_result" && e.text.starts_with("exec_command (fast)"))
    })
    .await;
    let saved = hub.0.data.lock().unwrap().sessions[&view.id].record.clone();
    assert_eq!(
        serde_json::to_value(&saved.engine).unwrap()["parallel_running"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    hub.interrupt(&view.id).unwrap();
    until(|| hub.view(&view.id).unwrap().status == "idle").await;
    for name in ["a", "b"] {
        for prefix in ["leader", "child"] {
            let pid: i32 = std::fs::read_to_string(work.path().join(format!("{prefix}-{name}")))
                .unwrap()
                .trim()
                .parse()
                .unwrap();
            until(|| unsafe { libc::kill(pid, 0) } != 0).await;
        }
    }
    assert!(!work.path().join("must-not-exist").exists());
    let engine =
        serde_json::to_value(&hub.0.data.lock().unwrap().sessions[&view.id].record.engine).unwrap();
    let output = results(&json!({"input":engine["history"]}));
    assert_eq!(output.len(), 4);
    assert_eq!(output["fast"]["stdout"]["text"], "retained");
    assert!(output["a"]["error"].as_str().unwrap().contains("partially"));
    assert!(
        output["unstarted"]["error"]
            .as_str()
            .unwrap()
            .contains("Not executed")
    );
    // Restore an actual in-flight record in a separate directory. No uncertain call is replayed.
    let crash = tempfile::tempdir().unwrap();
    let store = SessionStore::open(crash.path().into()).unwrap();
    store.save(&view.id, &saved).unwrap();
    let _marker = recovery::RunMarker::start(&store).unwrap();
    drop(store);
    let recovered = Hub::open(Client::test_client(), crash.path().into()).unwrap();
    let value = serde_json::to_value(
        &recovered.0.data.lock().unwrap().sessions[&view.id]
            .record
            .engine,
    )
    .unwrap();
    assert!(value.get("parallel_running").is_none());
    let output = results(&json!({"input":value["history"]}));
    assert_eq!(output.len(), 4);
    assert_eq!(output["fast"]["stdout"]["text"], "retained");
    assert!(
        output["a"]["error"]
            .as_str()
            .unwrap()
            .contains("fully or partially")
    );
    assert!(
        output["unstarted"]["error"]
            .as_str()
            .unwrap()
            .contains("Outcome unknown")
    );
    assert!(
        recovered.0.data.lock().unwrap().sessions[&view.id]
            .record
            .paused
    );
    assert!(!work.path().join("must-not-exist").exists());
    recovered.shutdown().await;
    hub.shutdown().await;
}
