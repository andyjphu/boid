use super::*;
use std::time::Duration;

fn create(hub: &Hub, name: &str, message: &str) -> View {
    hub.create(Create {
        name: name.into(),
        cwd: String::new(),
        message: message.into(),
    })
    .unwrap()
}
fn read(hub: &Hub, reader: &View, args: Value) -> Value {
    let result = hub.read_peer_trace(&reader.id, &args.to_string());
    assert!(result.get("error").is_none(), "{result}");
    result
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn trace_reads_live_text_without_disk_access_peer_changes_or_events() {
    let state = tempfile::tempdir().unwrap();
    let hub = Hub::open(Client::test_client(), state.path().into()).unwrap();
    let reader = create(&hub, "reader", "");
    let peer = create(&hub, "peer", "");
    let entry = hub.entry(&peer.id, "assistant", "");
    hub.delta(&peer.id, &entry, "The newest received text 🐦");
    let path = state
        .path()
        .join("sessions")
        .join(format!("{}.json", peer.id));
    let checkpoint = std::fs::read(&path).unwrap();
    let saved: Value = serde_json::from_slice(&checkpoint).unwrap();
    assert_eq!(
        saved["view"]["entries"].as_array().unwrap().last().unwrap()["text"],
        ""
    );
    let before = {
        let data = hub.0.data.lock().unwrap();
        serde_json::to_value(&data.sessions[&peer.id].record).unwrap()
    };
    let (_, mut events) = hub.subscribe();
    // The read must still work when the checkpoint file is unavailable.
    std::fs::rename(&path, path.with_extension("held")).unwrap();
    let result = read(
        &hub,
        &reader,
        json!({"to":"PEER","kind":"assistant","limit":1}),
    );
    assert_eq!(result["entries"][0]["text"], "The newest received text 🐦");
    assert_eq!(result["source"], "live_memory");
    let after = {
        let data = hub.0.data.lock().unwrap();
        serde_json::to_value(&data.sessions[&peer.id].record).unwrap()
    };
    assert_eq!(before, after, "reads do not mutate or wake the peer");
    assert!(matches!(
        events.try_recv(),
        Err(broadcast::error::TryRecvError::Empty)
    ));
    assert_eq!(
        std::fs::read(path.with_extension("held")).unwrap(),
        checkpoint
    );
    std::fs::rename(path.with_extension("held"), &path).unwrap();
    hub.close(&peer.id).await.unwrap();
    let ended = read(&hub, &reader, json!({"to":peer.id,"kind":"assistant"}));
    assert_eq!(ended["peer"]["archived"], true);
    assert_eq!(ended["entries"][0]["text"], "The newest received text 🐦");
    hub.shutdown().await;
}

#[tokio::test]
async fn trace_pages_preserve_latest_tail_utf8_attribution_and_identity() {
    let hub = Hub::new(Client::test_client()).unwrap();
    let reader = create(&hub, "reader", "");
    let peer = create(&hub, "peer", "");
    for i in 0..25 {
        hub.entry(&peer.id, "assistant", format!("entry {i}"));
    }
    hub.entry(&peer.id, "reasoning", "not a visible trace message");
    let args = hub.entry(&peer.id, "tool_arguments", "partial args");
    hub.entry_replacing(&peer.id, "tool_call", "completed tool call", Some(args));
    let latest = read(
        &hub,
        &reader,
        json!({"to":peer.id,"kind":"assistant","limit":2}),
    );
    assert_eq!(latest["entries"][0]["text"], "entry 23");
    assert_eq!(latest["entries"][1]["text"], "entry 24");
    let older = read(
        &hub,
        &reader,
        json!({"to":peer.id,"kind":"assistant","limit":2,"before":latest["next_before"]}),
    );
    assert_eq!(older["entries"][1]["text"], "entry 22");
    hub.entry(&peer.id, "skill", "Operator-selected guidance");
    hub.entry(&peer.id, "skill", "Catalog refresh warning");
    let skills = read(&hub, &reader, json!({"to":peer.id,"kind":"skill"}));
    assert_eq!(skills["entries"][0]["text"], "Operator-selected guidance");
    assert_eq!(skills["entries"][1]["text"], "Catalog refresh warning");
    let all = read(&hub, &reader, json!({"to":peer.id}));
    assert!(all.to_string().contains("Operator-selected guidance"));
    assert!(all.to_string().contains("Catalog refresh warning"));
    assert!(!all.to_string().contains("not a visible trace message"));
    assert!(!all.to_string().contains("partial args"));
    let text = "🐦".repeat(12000) + "NEWEST";
    let id = hub.entry(&peer.id, "assistant", &text);
    let tail = read(
        &hub,
        &reader,
        json!({"to":peer.id,"kind":"assistant","limit":1}),
    );
    assert!(
        tail["entries"][0]["text"]
            .as_str()
            .unwrap()
            .ends_with("NEWEST")
    );
    assert!(tail["entries"][0]["text"].as_str().unwrap().len() <= 16 * 1024);
    assert_eq!(tail["entries"][0]["text_complete"], false);
    let mut offset = json!(0);
    let mut reconstructed = String::new();
    loop {
        let page = read(
            &hub,
            &reader,
            json!({"to":peer.id,"entry_id":id,"offset":offset}),
        );
        reconstructed.push_str(page["entries"][0]["text"].as_str().unwrap());
        offset = page["entries"][0]["next_offset"].clone();
        if offset.is_null() {
            break;
        }
    }
    assert_eq!(reconstructed, text);
    for bad in [
        json!({"to":peer.id,"limit":0}),
        json!({"to":peer.id,"entry_id":id,"offset":1}),
        json!({"to":peer.id,"offset":0}),
        json!({"to":peer.id,"before":999999}),
        json!({"to":peer.id,"kind":"reasoning"}),
        json!({"to":reader.id}),
        json!({"to":"missing"}),
    ] {
        assert!(
            hub.read_peer_trace(&reader.id, &bad.to_string())
                .get("error")
                .is_some()
        );
    }
    hub.stop(&peer.id).unwrap();
    hub.send_peer(&reader.id, &peer.id, "Existing peer exchange")
        .unwrap();
    let exchange = read(&hub, &reader, json!({"to":peer.id,"kind":"peer","limit":1}));
    assert_eq!(exchange["entries"][0]["text"], "Existing peer exchange");
    assert_eq!(
        exchange["entries"][0]["peer_message"]["sender_id"],
        reader.id
    );
    assert_eq!(
        exchange["entries"][0]["peer_message"]["recipient_id"],
        peer.id
    );
    assert_eq!(exchange["entries"][0]["peer_message"]["state"], "queued");
    let duplicate = create(&hub, "peer", "");
    assert!(
        hub.read_peer_trace(&reader.id, r#"{"to":"peer"}"#)
            .get("error")
            .is_some()
    );
    assert_eq!(
        read(&hub, &reader, json!({"to":duplicate.id}))["peer"]["id"],
        duplicate.id
    );
    hub.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn restored_sessions_receive_the_tool_without_operator_refresh() {
    let state = tempfile::tempdir().unwrap();
    let hub = Hub::open(Client::test_client(), state.path().into()).unwrap();
    let old = create(&hub, "old", "");
    hub.shutdown().await;
    drop(hub);
    // Emulate a pre-tool saved session only after all workers have stopped.
    let path = state
        .path()
        .join("sessions")
        .join(format!("{}.json", old.id));
    let mut saved: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    saved["engine"]["tools"]
        .as_array_mut()
        .unwrap()
        .retain(|tool| tool["name"] != "read_peer_trace");
    std::fs::write(&path, serde_json::to_vec(&saved).unwrap()).unwrap();
    let saved: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert!(
        saved["engine"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .all(|tool| tool["name"] != "read_peer_trace")
    );
    let restored = Hub::open(Client::test_client(), state.path().into()).unwrap();
    {
        let data = restored.0.data.lock().unwrap();
        let tools = data.sessions[&old.id]
            .record
            .engine
            .tools
            .as_array()
            .unwrap();
        assert_eq!(
            tools
                .iter()
                .filter(|tool| tool["name"] == "read_peer_trace")
                .count(),
            1
        );
    }
    restored.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn agent_tool_reads_a_peer_while_its_response_is_still_streaming() {
    use axum::{Json, Router, body::Body, routing::post};
    use tokio::sync::mpsc;
    type Captured = (Value, mpsc::UnboundedSender<String>);
    let (requests, mut received) = mpsc::unbounded_channel::<Captured>();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}/", listener.local_addr().unwrap());
    let app = Router::new().route(
        "/",
        post(move |Json(input): Json<Value>| {
            let requests = requests.clone();
            async move {
                let (tx, rx) = mpsc::unbounded_channel::<String>();
                requests.send((input, tx)).unwrap();
                let stream = futures_util::stream::unfold(rx, |mut rx| async move {
                    rx.recv()
                        .await
                        .map(|text| (Ok::<_, std::convert::Infallible>(text), rx))
                });
                axum::response::Response::builder()
                    .header("content-type", "text/event-stream")
                    .body(Body::from_stream(stream))
                    .unwrap()
            }
        }),
    );
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let hub = Hub::new(Client::test_client_at(endpoint)).unwrap();
    let peer = create(&hub, "streaming-peer", "Begin");
    let (_, peer_stream) = tokio::time::timeout(Duration::from_secs(3), received.recv())
        .await
        .unwrap()
        .unwrap();
    let emit = |tx: &mpsc::UnboundedSender<String>, event: Value| {
        tx.send(format!("data: {event}\n\n")).unwrap()
    };
    emit(
        &peer_stream,
        json!({"type":"response.output_text.delta","output_index":0,"delta":"Live partial"}),
    );
    tokio::time::timeout(Duration::from_secs(3), async {
        while !hub
            .view(&peer.id)
            .unwrap()
            .entries
            .iter()
            .any(|e| e.text == "Live partial")
        {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    let reader = create(&hub, "reading-peer", "Inspect existing peer work");
    let (request, answer) = tokio::time::timeout(Duration::from_secs(3), received.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(
        request["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|tool| tool["name"] == "read_peer_trace")
    );
    assert!(
        request["instructions"]
            .as_str()
            .unwrap()
            .contains(tool_calls::PEER_TRACE_GUIDANCE)
    );
    let completed =
        |output: Value| json!({"type":"response.completed","response":{"output":output}});
    let call = |id: &str| {
        json!([{"type":"function_call","call_id":id,"name":"read_peer_trace",
        "arguments":json!({"to":peer.id,"kind":"assistant","limit":1,"before":null,"entry_id":null,"offset":null}).to_string()}])
    };
    emit(&answer, completed(call("read-first")));
    drop(answer);
    let (request, answer) = tokio::time::timeout(Duration::from_secs(3), received.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(
        request["instructions"]
            .as_str()
            .unwrap()
            .contains(&reader.id)
    );
    let output: Value = serde_json::from_str(
        request["input"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["call_id"] == "read-first" && item["type"] == "function_call_output")
            .unwrap()["output"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(output["entries"][0]["text"], "Live partial");
    assert_eq!(output["peer"]["status"], "working");
    emit(
        &peer_stream,
        json!({"type":"response.output_text.delta","output_index":0,"delta":"; still generating"}),
    );
    tokio::time::timeout(Duration::from_secs(3), async {
        while !hub
            .view(&peer.id)
            .unwrap()
            .entries
            .iter()
            .any(|e| e.text.ends_with("still generating"))
        {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    emit(&answer, completed(call("read-second")));
    drop(answer);
    let (request, answer) = tokio::time::timeout(Duration::from_secs(3), received.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(
        request["instructions"]
            .as_str()
            .unwrap()
            .contains(&reader.id),
        "reading must not call the peer model"
    );
    let output: Value = serde_json::from_str(
        request["input"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["call_id"] == "read-second" && item["type"] == "function_call_output")
            .unwrap()["output"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        output["entries"][0]["text"],
        "Live partial; still generating"
    );
    assert_eq!(output["peer"]["status"], "working");
    println!("LIVE_TOOL_RESULT {}", output);
    emit(&answer, completed(json!([])));
    drop(answer);
    emit(&peer_stream, completed(json!([])));
    drop(peer_stream);
    hub.shutdown().await;
    server.abort();
    let _ = server.await;
}
