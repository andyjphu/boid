//! Actual Session::step handoffs using loopback HTTP and a real temporary-file tool.
//! Synthetic host measurements, not live provider or browser timings.
use super::*;
use axum::{Router, body::to_bytes, extract::Request, routing::post};
use tokio::sync::{mpsc, oneshot};

#[derive(Default)]
struct Marks(Mutex<Vec<(&'static str, Instant)>>);
impl Marks {
    fn mark(&self, name: &'static str) {
        self.0.lock().unwrap().push((name, Instant::now()));
    }
    fn nth(&self, name: &str, index: usize) -> Instant {
        self.0
            .lock()
            .unwrap()
            .iter()
            .filter(|(n, _)| *n == name)
            .nth(index)
            .unwrap_or_else(|| panic!("Missing {name}[{index}]"))
            .1
    }
}

struct RequestSeen {
    arrived: Instant,
    body_read: Instant,
    parsed: Instant,
    bytes: usize,
    body: Value,
    reply: oneshot::Sender<Value>,
}

// Release even on a failed assertion, so no storage thread remains held.
struct WriterGate(Option<std::sync::mpsc::Sender<()>>);
impl Drop for WriterGate {
    fn drop(&mut self) {
        if let Some(sender) = self.0.take() {
            let _ = sender.send(());
        }
    }
}

async fn hold_writer(hub: &Hub, id: &str) -> WriterGate {
    let (started, release) = hub
        .0
        .data
        .lock()
        .unwrap()
        .sessions
        .get_mut(id)
        .unwrap()
        .persistence
        .gate();
    let gate = WriterGate(Some(release));
    tokio::task::spawn_blocking(move || started.recv_timeout(Duration::from_secs(5)))
        .await
        .unwrap()
        .expect("writer never reached fixture gate");
    gate
}

fn ms(from: Instant, to: Instant) -> f64 {
    to.duration_since(from).as_secs_f64() * 1000.0
}

async fn handoff_case(history_mib: usize, held_writer: bool) -> Value {
    let marks = Arc::new(Marks::default());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}/", listener.local_addr().unwrap());
    let (sent, mut requests) = mpsc::channel::<RequestSeen>(4);
    let router = Router::new().route(
        "/",
        post(move |request: Request| {
            let sent = sent.clone();
            async move {
                let arrived = Instant::now();
                let bytes = to_bytes(request.into_body(), 32 * 1024 * 1024)
                    .await
                    .unwrap();
                let body_read = Instant::now();
                let body: Value = serde_json::from_slice(&bytes).unwrap();
                let parsed = Instant::now();
                let (reply, response) = oneshot::channel();
                sent.send(RequestSeen {
                    arrived,
                    body_read,
                    parsed,
                    bytes: bytes.len(),
                    body,
                    reply,
                })
                .await
                .unwrap();
                let output = response.await.unwrap_or_else(|_| json!([]));
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
    let mut client = Client::test_client_at(endpoint);
    let callback_marks = marks.clone();
    client.set_test_handoff_probe(Arc::new(move |stage| callback_marks.mark(stage)));
    let state = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let hub = Hub::open(client, state.path().into()).unwrap();
    let target = hub
        .create(Create {
            name: "handoff-fixture".into(),
            cwd: work.path().to_string_lossy().into_owned(),
            message: String::new(),
        })
        .unwrap();
    hub.entry(&target.id, "assistant", "retained synthetic fixture");
    let worker = hub
        .0
        .data
        .lock()
        .unwrap()
        .sessions
        .get_mut(&target.id)
        .unwrap()
        .task
        .take();
    if let Some(worker) = worker {
        worker.abort();
        let _ = worker.await;
    }
    let mut session = hub.0.data.lock().unwrap().sessions[&target.id]
        .record
        .engine
        .clone();
    if history_mib > 0 {
        let skills = session.skills.clone();
        let mut encoded = serde_json::to_value(&session).unwrap();
        encoded["history"] = Value::Array(
            (0..history_mib * 16)
                .map(|_| json!({"role":"assistant","content":"h".repeat(64 * 1024)}))
                .collect(),
        );
        session = serde_json::from_value(encoded).unwrap();
        session.skills = skills;
    }
    let _ = session.operator("Write handoff-marker.txt, then finish.".into());
    // 4 MiB history case has 4096 small transcript entries totalling 8 MiB.
    // This exposes per-entry copying that one giant shared string would hide.
    let transcript_entries = history_mib * 1024;
    hub.update(&target.id, |record| {
        record.engine = session.clone();
        record.view.entries.extend(
            (0..transcript_entries).map(|_| Hub::make_entry("assistant", "t".repeat(2048))),
        );
        Ok(())
    })
    .unwrap();
    let capacity = hub.0.data.lock().unwrap().sessions[&target.id]
        .persistence
        .capacity();
    capacity.durable_record().unwrap();
    hub.0.compactions.wait(Some(&target.id));

    let driver_hub = hub.clone();
    let id = target.id.clone();
    let cwd = work.path().to_path_buf();
    let driver_marks = marks.clone();
    let driver = tokio::spawn(async move {
        let tools = Tools::new(cwd).unwrap();
        assert!(
            session
                .step(&driver_hub.0.client, &tools, &driver_hub, &id)
                .await
                .unwrap()
        );
        driver_marks.mark("model_step_return");
        assert!(
            session
                .step(&driver_hub.0.client, &tools, &driver_hub, &id)
                .await
                .unwrap()
        );
        driver_marks.mark("tool_step_return");
        assert!(
            !session
                .step(&driver_hub.0.client, &tools, &driver_hub, &id)
                .await
                .unwrap()
        );
    });
    let first = tokio::time::timeout(Duration::from_secs(10), requests.recv())
        .await
        .unwrap()
        .unwrap();
    let gate = if held_writer {
        Some(hold_writer(&hub, &target.id).await)
    } else {
        None
    };
    marks.mark("mock_response_released");
    first.reply.send(json!([{"type":"function_call","id":"fc_handoff",
        "call_id":"handoff_write","name":"write_file",
        "arguments":json!({"path":"handoff-marker.txt","content":"actual safe tool completed"}).to_string()}]))
        .unwrap();
    let early = if held_writer {
        tokio::time::timeout(Duration::from_millis(500), requests.recv())
            .await
            .ok()
            .flatten()
    } else {
        None
    };
    let progressed_while_held = held_writer && early.is_some();
    marks.mark("writer_released");
    drop(gate);
    let second = match early {
        Some(request) => request,
        None => tokio::time::timeout(Duration::from_secs(10), requests.recv())
            .await
            .expect("next HTTP request did not arrive after writer release")
            .unwrap(),
    };
    assert_eq!(
        std::fs::read_to_string(work.path().join("handoff-marker.txt")).unwrap(),
        "actual safe tool completed"
    );
    let result = second.body["input"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["type"] == "function_call_output" && item["call_id"] == "handoff_write")
        .expect("next real request omitted the tool result");
    assert_eq!(
        serde_json::from_str::<Value>(result["output"].as_str().unwrap()).unwrap(),
        json!({"ok":true})
    );
    second.reply.send(json!([])).unwrap();
    tokio::time::timeout(Duration::from_secs(10), driver)
        .await
        .unwrap()
        .unwrap();

    let response = marks.nth("response_observed", 0);
    let dispatch = marks.nth("tool_dispatch", 0);
    let returned = marks.nth("tool_return", 0);
    let encode_start = marks.nth("payload_encode_start", 1);
    let encode_end = marks.nth("payload_encode_end", 1);
    let send = marks.nth("http_send", 1);
    let report = json!({
        "history_mib":history_mib,"transcript_mib":history_mib * 2,
        "transcript_entries":transcript_entries,
        "held_writer":held_writer,"held_window_ms":if held_writer {500} else {0},
        "progressed_while_writer_held":progressed_while_held,
        "first_request_bytes":first.bytes,"next_request_bytes":second.bytes,
        "mock_release_to_response_observed_ms":ms(marks.nth("mock_response_released",0), response),
        "response_observed_to_tool_dispatch_ms":ms(response, dispatch),
        "response_observed_to_model_step_return_ms":ms(response, marks.nth("model_step_return",0)),
        "actual_tool_execution_ms":ms(dispatch, returned),
        "tool_return_to_next_encode_start_ms":ms(returned, encode_start),
        "next_payload_encoding_ms":ms(encode_start, encode_end),
        "encode_end_to_http_send_ms":ms(encode_end, send),
        "tool_return_to_next_http_send_ms":ms(returned, send),
        "http_send_to_mock_handler_ms":ms(send, second.arrived),
        "mock_request_body_read_ms":ms(second.arrived, second.body_read),
        "mock_request_json_parse_ms":ms(second.body_read, second.parsed),
        "tool_return_to_mock_request_arrival_ms":ms(returned, second.arrived),
        "timestamps_ms_since_response_observed":{
            "tool_dispatch":ms(response, dispatch),"tool_return":ms(response, returned),
            "next_encode_start":ms(response, encode_start),"next_encode_end":ms(response, encode_end),
            "next_http_send":ms(response, send),"next_http_arrival":ms(response, second.arrived)
        },
        "measurement":"actual Session::step, loopback HTTP Responses, native temp write_file; synthetic fixture, no live provider",
        "transport_scope":"HTTP send-to-handler includes local socket/client/server scheduling; body read and mock JSON parse reported separately"
    });
    eprintln!("SYNTHETIC_HANDOFF {report}");
    hub.shutdown().await;
    server.abort();
    report
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn model_tool_handoff_proceeds_while_writer_is_held() {
    let report = handoff_case(0, true).await;
    assert_eq!(
        report["progressed_while_writer_held"], true,
        "completed response -> actual tool -> next HTTP request waited for held persistence writer"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "Synthetic model/tool handoff benchmark; run only on assigned host"]
async fn model_tool_handoff_benchmark() {
    for history_mib in [0, 4] {
        for _ in 0..3 {
            handoff_case(history_mib, false).await;
        }
        // Record both implementations; the separate regression asserts new policy.
        handoff_case(history_mib, true).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn repeated_model_tool_handoffs_do_not_wait_for_writer_capacity() {
    const CYCLES: usize = 8;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}/", listener.local_addr().unwrap());
    let (sent, mut requests) = mpsc::channel::<(Value, oneshot::Sender<Value>)>(1);
    let router = Router::new().route(
        "/",
        post(move |request: Request| {
            let sent = sent.clone();
            async move {
                let bytes = to_bytes(request.into_body(), 1024 * 1024).await.unwrap();
                let body: Value = serde_json::from_slice(&bytes).unwrap();
                let (reply, response) = oneshot::channel();
                sent.send((body, reply)).await.unwrap();
                let output = response.await.unwrap_or_else(|_| json!([]));
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
    let state = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let hub = Hub::open(Client::test_client_at(endpoint), state.path().into()).unwrap();
    let target = hub
        .create(Create {
            name: "repeated-handoff-fixture".into(),
            cwd: work.path().to_string_lossy().into_owned(),
            message: String::new(),
        })
        .unwrap();
    hub.entry(&target.id, "assistant", "retained repeated handoff fixture");
    let worker = hub
        .0
        .data
        .lock()
        .unwrap()
        .sessions
        .get_mut(&target.id)
        .unwrap()
        .task
        .take();
    if let Some(worker) = worker {
        worker.abort();
        let _ = worker.await;
    }
    let mut session = hub.0.data.lock().unwrap().sessions[&target.id]
        .record
        .engine
        .clone();
    let _ = session.operator("Write each requested marker, then finish.".into());
    hub.checkpoint(&target.id, &session).unwrap();
    let capacity = hub.0.data.lock().unwrap().sessions[&target.id]
        .persistence
        .capacity();
    capacity.durable_record().unwrap();
    hub.0.compactions.wait(Some(&target.id));

    let driver_hub = hub.clone();
    let id = target.id.clone();
    let cwd = work.path().to_path_buf();
    let driver = tokio::spawn(async move {
        let tools = Tools::new(cwd).unwrap();
        let mut continued_steps = 0;
        while session
            .step(&driver_hub.0.client, &tools, &driver_hub, &id)
            .await
            .unwrap()
        {
            continued_steps += 1;
            assert!(continued_steps <= CYCLES * 2);
        }
        assert_eq!(continued_steps, CYCLES * 2);
    });
    let (_, mut reply) = tokio::time::timeout(Duration::from_secs(5), requests.recv())
        .await
        .unwrap()
        .unwrap();
    let gate = hold_writer(&hub, &target.id).await;
    // Each cycle performs request admission, response checkpoint, tool claim,
    // and result checkpoint. Eight cycles exceed the former 16-record bound.
    let progress = tokio::time::timeout(Duration::from_secs(2), async {
        for cycle in 0..CYCLES {
            let call_id = format!("handoff_{cycle}");
            let path = format!("marker-{cycle}.txt");
            reply.send(json!([{"type":"function_call","id":format!("fc_{cycle}"),
                "call_id":call_id,"name":"write_file",
                "arguments":json!({"path":path,"content":format!("completed {cycle}")}).to_string()}]))
                .unwrap();
            let (body, next_reply) = requests.recv().await.unwrap();
            assert_eq!(std::fs::read_to_string(work.path().join(path)).unwrap(),
                format!("completed {cycle}"));
            let output = body["input"].as_array().unwrap().iter().find(|item|
                item["type"] == "function_call_output" && item["call_id"] == call_id)
                .expect("actual next request omitted tool output");
            assert_eq!(serde_json::from_str::<Value>(output["output"].as_str().unwrap()).unwrap(),
                json!({"ok":true}));
            reply = next_reply;
        }
        reply.send(json!([])).unwrap();
    }).await;
    drop(gate);
    // Cleanup precedes the policy assertion, including on the old implementation.
    if progress.is_err() {
        driver.abort();
    }
    let finished = tokio::time::timeout(Duration::from_secs(5), driver).await;
    hub.shutdown().await;
    server.abort();
    assert!(
        progress.is_ok(),
        "eight actual model/tool cycles stalled behind held persistence writer"
    );
    finished.unwrap().unwrap();
}
