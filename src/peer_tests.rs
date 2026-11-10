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
fn send(hub: &Hub, from: &View, to: &View, text: &str, urgency: Urgency, call: &str) -> Value {
    let entry = hub.entry(
        &from.id,
        "tool_call",
        format!(
            "send_message ({call})\n{}",
            json!({"to":to.id,"text":text,"urgency":urgency})
        ),
    );
    hub.send_communication(&from.id, &to.id, text, urgency, call, &entry)
        .unwrap()
}
fn call(id: &str, name: &str, arguments: Value) -> Value {
    json!({"type":"function_call","call_id":id,"name":name,"arguments":arguments.to_string()})
}

#[tokio::test]
async fn diagnostics_preserve_reports_failed_usage_and_unknown_attempts_across_recovery() {
    let work = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let (hub, mut rx, server) = fixture(Some(state.path().into())).await;
    let target = agent(&hub, work.path(), "accounting", "Begin");
    let (_, release) = next(&mut rx).await;
    assert_eq!(hub.diagnostics()["aggregate"]["in_flight_requests"], 1);
    let usage = json!({"input_tokens":10000,"output_tokens":2000,"total_tokens":12000,
        "input_tokens_details":{"cached_tokens":3000,"cache_write_tokens":2000,
        "cache_write_tokens_details":{"ttl_30m_tokens":1000,"ttl_12h_tokens":1000}},
        "output_tokens_details":{"reasoning_tokens":500},"future_field":{"opaque":true}});
    release
        .send(
            json!({"id":"complete","output":[],"model":"gpt-6-sol","service_tier":"default",
        "usage":usage,"prompt_cache_diagnostics":{"type":"cache_hit","future":true}}),
        )
        .unwrap();
    wait_status(&hub, &target.id, "idle").await;
    let diagnostics = hub.diagnostics();
    assert_eq!(diagnostics["aggregate"]["recorded_requests"], 1);
    assert_eq!(diagnostics["latest_response"]["usage"], usage);
    assert_eq!(diagnostics["latest_response"]["model"], "gpt-6-sol");
    assert!(
        (diagnostics["aggregate"]["estimated_cost_usd"]
            .as_f64()
            .unwrap()
            - 0.0381)
            .abs()
            < 1e-10
    );
    assert!(
        serde_json::to_value(hub.view(&target.id))
            .unwrap()
            .get("usage")
            .is_none()
    );
    hub.message(&target.id, "Fail with usage".into()).unwrap();
    let (_, release) = next(&mut rx).await;
    release
        .send(
            json!({"type":"response.failed","response":{"id":"failed","model":"gpt-6-sol",
        "service_tier":"default","usage":usage,"error":{"message":"fixture failure"}}}),
        )
        .unwrap();
    wait_status(&hub, &target.id, "error").await;
    assert_eq!(hub.diagnostics()["latest_response"]["status"], "failed");
    hub.message(&target.id, "Cancel without report".into())
        .unwrap();
    let (_, held) = next(&mut rx).await;
    hub.close(&target.id).await.unwrap();
    let diagnostics = hub.diagnostics();
    assert_eq!(diagnostics["aggregate"]["request_attempts"], 3);
    assert_eq!(diagnostics["aggregate"]["unrecorded_requests"], 1);
    assert!(diagnostics["aggregate"]["estimated_cost_usd"].is_null());
    assert_eq!(diagnostics["sessions"][0]["archived"], true);
    hub.shutdown().await;
    drop(hub);
    drop(held);
    server.abort();
    let (restored, _, server) = fixture(Some(state.path().into())).await;
    let diagnostics = restored.diagnostics();
    assert_eq!(diagnostics["aggregate"]["recorded_requests"], 2);
    assert_eq!(diagnostics["aggregate"]["unrecorded_requests"], 1);
    assert_eq!(diagnostics["latest_response"]["usage"], usage);
    assert_eq!(diagnostics["sessions"][0]["archived"], true);
    restored.shutdown().await;
    server.abort();
}
async fn wait_status(hub: &Hub, id: &str, status: &str) {
    tokio::time::timeout(Duration::from_secs(3), async {
        while hub.view(id).unwrap().status != status {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn context_usage_is_latest_reported_measurement_and_survives_recovery() {
    let work = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let (hub, mut rx, server) = fixture(Some(state.path().into())).await;
    let (_, mut events) = hub.subscribe();
    let target = agent(&hub, work.path(), "usage", "Begin");
    assert!(hub.view(&target.id).unwrap().context_usage.is_none());
    for (index, input, output) in [(0, 100, 20), (1, 180, 10)] {
        if index > 0 {
            hub.message(&target.id, "Continue".into()).unwrap();
        }
        let (_, release) = next(&mut rx).await;
        release
            .send(json!({"output":[],"usage":{"input_tokens":input,"output_tokens":output}}))
            .unwrap();
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                if hub.view(&target.id).unwrap().status == "idle" {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let view = hub.view(&target.id).unwrap();
        let usage = view.context_usage.unwrap();
        assert_eq!(usage.input_tokens, input);
        assert_eq!(usage.output_tokens, output);
        assert_eq!(view.context_window, None); // No fabricated limit for the fixture's model.
    }
    let mut updates = vec![];
    while let Ok(event) = events.try_recv() {
        if event["type"] == "context_usage" {
            updates.push(event);
        }
    }
    assert_eq!(updates.len(), 2);
    assert_eq!(updates[1]["context_usage"]["input_tokens"], 180);
    hub.shutdown().await;
    drop(hub);
    server.abort();
    let (restored, _, server) = fixture(Some(state.path().into())).await;
    let usage = restored.view(&target.id).unwrap().context_usage.unwrap();
    assert_eq!(usage.input_tokens, 180);
    assert_eq!(usage.output_tokens, 10);
    assert_eq!(crate::api::context_window("gpt-6-sol"), Some(1_050_000));
    restored.shutdown().await;
    server.abort();
}

#[tokio::test]
async fn paused_or_closed_inbox_ends_step_without_error_or_state_change() {
    let work = tempfile::tempdir().unwrap();
    let (hub, mut rx, server) = fixture(None).await;
    let sender = agent(&hub, work.path(), "sender", "");
    let target = agent(&hub, work.path(), "receiver", "");
    hub.stop(&target.id).unwrap();
    send(
        &hub,
        &sender,
        &target,
        "remain queued",
        Urgency::Regular,
        "paused-step",
    );
    let tools = Tools::new(work.path()).unwrap();
    for archived in [false, true] {
        if archived {
            hub.close(&target.id).await.unwrap();
        }
        let mut session = hub.0.data.lock().unwrap().sessions[&target.id]
            .record
            .engine
            .clone();
        let before = serde_json::to_value(&session).unwrap();
        let view_before = serde_json::to_value(hub.view(&target.id).unwrap()).unwrap();
        assert!(
            !session
                .step(&hub.0.client, &tools, &hub, &target.id)
                .await
                .unwrap()
        );
        assert_eq!(serde_json::to_value(&session).unwrap(), before);
        assert_eq!(
            serde_json::to_value(hub.view(&target.id).unwrap()).unwrap(),
            view_before
        );
        assert!(rx.try_recv().is_err());
    }
    hub.shutdown().await;
    server.abort();
}

#[tokio::test]
async fn peer_boundaries_priority_and_running_tool_cancellation() {
    let work = tempfile::tempdir().unwrap();
    let (hub, mut rx, server) = fixture(None).await;
    let sender = agent(&hub, work.path(), "ursa", "");
    let target = agent(&hub, work.path(), "aero", "Begin");
    let (_, release) = next(&mut rx).await;
    let receipt = send(
        &hub,
        &sender,
        &target,
        "regular $humanizer",
        Urgency::Regular,
        "r1",
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(40), rx.recv())
            .await
            .is_err()
    );
    release
        .send(json!([
            call("one", "write_file", json!({"path":"one","content":"one"})),
            call("two", "write_file", json!({"path":"two","content":"two"}))
        ]))
        .unwrap();
    let (input, _held) = next(&mut rx).await;
    assert!(work.path().join("one").exists() && work.path().join("two").exists());
    let history = input["input"].as_array().unwrap();
    assert_eq!(
        history
            .iter()
            .filter(|item| item["content"]
                .as_str()
                .is_some_and(|s| s.contains("regular $humanizer")))
            .count(),
        1
    );
    assert!(!history.iter().any(|item| {
        item["content"]
            .as_str()
            .is_some_and(|s| s.starts_with("Operator-selected skill:"))
    }));
    let message = hub
        .view(&sender.id)
        .unwrap()
        .communications
        .into_iter()
        .find(|m| m.id == receipt["message_id"])
        .unwrap();
    assert_eq!(message.state, "applied");
    assert_eq!(message.applied_batch, Some(2));
    send(
        &hub,
        &sender,
        &target,
        "later regular",
        Urgency::Regular,
        "r2",
    );
    send(
        &hub,
        &sender,
        &target,
        "urgent direction",
        Urgency::Emergency,
        "u1",
    );
    let (input, release) = next(&mut rx).await;
    assert!(input["input"].to_string().contains("urgent direction"));
    assert!(!input["input"].to_string().contains("later regular"));
    release.send(json!([])).unwrap();
    let (input, release) = next(&mut rx).await;
    assert!(input["input"].to_string().contains("later regular"));
    release
        .send(json!([
            call(
                "long",
                "exec_command",
                json!({"command":"touch running; sleep 30; touch finished","timeout_ms":60000})
            ),
            call(
                "unstarted",
                "write_file",
                json!({"path":"unstarted","content":"bad"})
            )
        ]))
        .unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        while !work.path().join("running").exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    send(
        &hub,
        &sender,
        &target,
        "interrupt command",
        Urgency::Emergency,
        "u2",
    );
    let (input, release) = next(&mut rx).await;
    let history = input["input"].as_array().unwrap();
    assert!(history.iter().any(|i| {
        i["call_id"] == "long"
            && i["output"]
                .as_str()
                .is_some_and(|s| s.contains("partially completed"))
    }));
    assert!(history.iter().any(|i| {
        i["call_id"] == "unstarted"
            && i["output"]
                .as_str()
                .is_some_and(|s| s.contains("Not executed"))
    }));
    assert!(!work.path().join("finished").exists() && !work.path().join("unstarted").exists());
    release.send(json!([])).unwrap();
    hub.shutdown().await;
    server.abort();
}

#[tokio::test]
async fn emergency_bypasses_cancelled_skill_scan_and_respects_stop() {
    let work = tempfile::tempdir().unwrap();
    let (hub, mut rx, server) = fixture(None).await;
    let sender = agent(&hub, work.path(), "ursa", "");
    let target = agent(&hub, work.path(), "aero", "");
    let (release_scan, scan_rx) = std::sync::mpsc::channel();
    let gate = Arc::new(SkillScanGate {
        started: Notify::new(),
        release: Mutex::new(scan_rx),
    });
    *hub.0.skill_scan_gate.lock().unwrap() = Some(gate.clone());
    hub.message(&target.id, "operator after refresh".into())
        .unwrap();
    tokio::time::timeout(Duration::from_secs(3), gate.started.notified())
        .await
        .unwrap();
    send(
        &hub,
        &sender,
        &target,
        "emergency before scan finishes",
        Urgency::Emergency,
        "scan-emergency",
    );
    let (input, _held) = next(&mut rx).await;
    assert!(
        input["input"]
            .to_string()
            .contains("emergency before scan finishes")
    );
    assert!(
        !input["input"]
            .to_string()
            .contains("operator after refresh")
    );
    hub.stop(&target.id).unwrap();
    let receipt = send(
        &hub,
        &sender,
        &target,
        "must stay paused",
        Urgency::Emergency,
        "paused",
    );
    assert_eq!(receipt["blocked"], true);
    release_scan.send(()).unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(80), rx.recv())
            .await
            .is_err()
    );
    hub.message(&target.id, "resume".into()).unwrap();
    let (input, _release) = next(&mut rx).await;
    assert!(input["input"].to_string().contains("must stay paused"));
    assert!(
        !input["input"]
            .to_string()
            .contains("operator after refresh")
    );
    hub.shutdown().await;
    server.abort();
}

#[tokio::test]
async fn durable_peer_receipt_recovery_retry_and_end() {
    let work = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let (hub, _rx, server) = fixture(Some(state.path().into())).await;
    let sender = agent(&hub, work.path(), "ursa", "");
    let target = agent(&hub, work.path(), "aero", "");
    hub.stop(&target.id).unwrap();
    hub.update(&target.id, |r| {
        r.version = 2;
        Ok(())
    })
    .unwrap();
    let receipt = send(
        &hub,
        &sender,
        &target,
        "durable",
        Urgency::Emergency,
        "persisted-send",
    );
    let retry = hub
        .send_communication(
            &sender.id,
            &target.id,
            "durable",
            Urgency::Emergency,
            "persisted-send",
            "ignored",
        )
        .unwrap();
    assert_eq!(receipt, retry);
    assert!(
        state
            .path()
            .join("sessions")
            .join(format!("{}.before-v9", target.id))
            .exists()
    );
    assert!(
        hub.send_communication(
            &sender.id,
            &target.id,
            "changed",
            Urgency::Emergency,
            "persisted-send",
            "ignored"
        )
        .is_err()
    );
    // Persist the crash window after recipient acceptance and before sender finish.
    hub.update(&sender.id, |record| {
        let mut engine = serde_json::to_value(&record.engine)?;
        engine["running"] = json!({"id":"persisted-send","name":"send_message","arguments":"{}","argument_entry":null});
        record.engine = serde_json::from_value(engine)?;
        Ok(())
    }).unwrap();
    // Abort without graceful recovery, then drop all server-owned handles.
    let tasks: Vec<_> = {
        let mut data = hub.0.data.lock().unwrap();
        data.sessions
            .values_mut()
            .filter_map(|h| h.task.take())
            .collect()
    };
    for task in tasks {
        task.abort();
        let _ = task.await;
    }
    drop(hub);
    server.abort();
    let (restored, mut rx, server) = fixture(Some(state.path().into())).await;
    assert!(
        tokio::time::timeout(Duration::from_millis(40), rx.recv())
            .await
            .is_err()
    );
    let a = restored.view(&sender.id).unwrap();
    let b = restored.view(&target.id).unwrap();
    assert_eq!(a.communications, b.communications);
    assert_eq!(a.communications.len(), 1);
    assert!(a.entries.iter().any(|entry| entry.kind == "tool_result"
        && entry.text.contains(receipt["message_id"].as_str().unwrap())));
    assert!(restored.close(&target.id).await.unwrap());
    assert_eq!(
        restored.view(&sender.id).unwrap().communications[0].state,
        "cancelled"
    );
    restored.reopen(&target.id).unwrap();
    assert!(
        restored.0.data.lock().unwrap().sessions[&target.id]
            .record
            .inbox
            .is_empty()
    );
    for i in 0..64 {
        restored
            .send_communication(
                &sender.id,
                &target.id,
                "normal backlog",
                Urgency::Regular,
                &format!("backlog-{i}"),
                "",
            )
            .unwrap();
    }
    assert!(
        restored
            .send_communication(
                &sender.id,
                &target.id,
                "overflow",
                Urgency::Regular,
                "overflow",
                ""
            )
            .is_err()
    );
    for i in 0..8 {
        restored
            .send_communication(
                &sender.id,
                &target.id,
                "urgent backlog",
                Urgency::Emergency,
                &format!("urgent-{i}"),
                "",
            )
            .unwrap();
    }
    assert!(
        restored
            .send_communication(
                &sender.id,
                &target.id,
                "overflow",
                Urgency::Emergency,
                "urgent-overflow",
                ""
            )
            .is_err()
    );
    assert!(
        restored
            .send_communication(
                &sender.id,
                &target.id,
                "urgent backlog",
                Urgency::Emergency,
                "urgent-0",
                ""
            )
            .is_ok()
    );
    std::fs::rename(
        state.path().join("sessions"),
        state.path().join("saved-sessions"),
    )
    .unwrap();
    std::fs::write(state.path().join("sessions"), "filesystem failure fixture").unwrap();
    assert!(!restored.close(&target.id).await.unwrap());
    let view = restored.view(&sender.id).unwrap();
    let cancelled = view
        .communications
        .iter()
        .find(|m| m.call_id == "urgent-0")
        .unwrap();
    assert_eq!(cancelled.state, "cancelled");
    assert!(!cancelled.persisted);
    restored.shutdown().await;
    server.abort();
}

// Run explicitly with isolated development state. The browser drives the real
// authenticated routes, worker, persistence, and SSE stream with a mock provider.
#[tokio::test]
#[ignore]
async fn browser_fixture() {
    assert_eq!(
        std::env::var("BOID_PEER_BROWSER_FIXTURE").as_deref(),
        Ok("1")
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut client = Client::test_client_at(format!("http://{}/", listener.local_addr().unwrap()));
    client.model = "gpt-6-sol".into();
    let router = Router::new().route(
        "/",
        post(|Json(input): Json<Value>| async move {
            let last = input["input"].as_array().unwrap().last().unwrap();
            let content = last["content"].as_str().unwrap_or("");
            if content == "Hold" {
                tokio::time::sleep(Duration::from_secs(120)).await;
            }
            let output = if let Some(command) = content.strip_prefix("send|") {
                let parts: Vec<_> = command.splitn(3, '|').collect();
                json!([call(
                    &Uuid::new_v4().to_string(),
                    "send_message",
                    json!({"to":parts[0],"urgency":parts[1],"text":parts[2]})
                )])
            } else {
                json!([])
            };
            let mut response = json!({"id":Uuid::new_v4().to_string(),"output":output,
                "usage":{"input_tokens":210000,"output_tokens":5000}});
            if content == "DiagnosticsComplete" {
                response["model"] = json!("gpt-6-sol");
                response["service_tier"] = json!("default");
                response["usage"] = json!({"input_tokens":10000,"output_tokens":2000,"total_tokens":12000,
                    "input_tokens_details":{"cached_tokens":3000,"cache_write_tokens":2000,
                    "cache_write_tokens_details":{"ttl_30m_tokens":1000,"ttl_12h_tokens":1000}},
                    "output_tokens_details":{"reasoning_tokens":500}});
            }
            (
                [("content-type", "text/event-stream")],
                format!("data: {}\n\n", json!({"type":"response.completed","response":response})),
            )
        }),
    );
    let mock = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    crate::server::serve_client(client).await.unwrap();
    mock.abort();
}

#[tokio::test]
async fn restart_restores_listening_without_greetings_and_preserves_stop() {
    let work = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let (hub, _rx, server) = fixture(Some(state.path().into())).await;
    let sender = agent(&hub, work.path(), "sender", "");
    let ready = agent(&hub, work.path(), "ready", "");
    let stopped = agent(&hub, work.path(), "stopped", "");
    hub.stop(&stopped.id).unwrap();
    wait_status(&hub, &stopped.id, "stopped").await;
    hub.shutdown().await;
    drop(hub);
    server.abort();

    let (hub, mut rx, server) = fixture(Some(state.path().into())).await;
    assert_eq!(hub.view(&ready.id).unwrap().status, "idle");
    let roster = hub.peers();
    assert_eq!(roster.as_array().unwrap().len(), 3);
    assert_eq!(
        roster
            .as_array()
            .unwrap()
            .iter()
            .find(|v| v["id"] == ready.id)
            .unwrap()["listening"],
        true
    );
    assert_eq!(
        roster
            .as_array()
            .unwrap()
            .iter()
            .find(|v| v["id"] == stopped.id)
            .unwrap()["listening"],
        false
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(50), rx.recv())
            .await
            .is_err()
    );
    let receipt = send(
        &hub,
        &sender,
        &ready,
        "Peer contact after restart",
        Urgency::Regular,
        "after-restart",
    );
    assert_eq!(receipt["blocked"], false);
    let (request, release) = next(&mut rx).await;
    assert!(
        request["input"]
            .to_string()
            .contains("Peer contact after restart")
    );
    assert!(
        !hub.view(&ready.id)
            .unwrap()
            .entries
            .iter()
            .any(|e| e.kind == "user")
    );
    release.send(json!([])).unwrap();
    wait_status(&hub, &ready.id, "idle").await;
    for (urgency, call) in [
        (Urgency::Regular, "regular-stop"),
        (Urgency::Emergency, "urgent-stop"),
    ] {
        assert_eq!(
            send(&hub, &sender, &stopped, "Wait for operator", urgency, call)["blocked"],
            true
        );
    }
    assert!(
        tokio::time::timeout(Duration::from_millis(50), rx.recv())
            .await
            .is_err()
    );
    assert_eq!(hub.view(&stopped.id).unwrap().status, "stopped");
    hub.message(&stopped.id, "Resume explicitly".into())
        .unwrap();
    let (urgent, release) = next(&mut rx).await;
    assert!(urgent["input"].to_string().contains("(emergency)"));
    assert!(!urgent["input"].to_string().contains("Resume explicitly"));
    release.send(json!([])).unwrap();
    let (regular, release) = next(&mut rx).await;
    assert!(regular["input"].to_string().contains("(regular)"));
    assert!(regular["input"].to_string().contains("Resume explicitly"));
    release.send(json!([])).unwrap();
    wait_status(&hub, &stopped.id, "idle").await;
    assert_eq!(
        hub.peers()
            .as_array()
            .unwrap()
            .iter()
            .find(|v| v["id"] == stopped.id)
            .unwrap()["listening"],
        true
    );
    hub.shutdown().await;
    drop(hub);
    server.abort();
}

#[tokio::test]
async fn restart_delivers_previously_accepted_peer_after_complete_roster_install() {
    let work = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let (hub, _rx, server) = fixture(Some(state.path().into())).await;
    let sender = agent(&hub, work.path(), "sender", "");
    let target = agent(&hub, work.path(), "target", "");
    // No await between durable acceptance and shutdown: the worker has not run.
    send(
        &hub,
        &sender,
        &target,
        "Accepted before shutdown",
        Urgency::Regular,
        "queued-restart",
    );
    hub.shutdown().await;
    drop(hub);
    server.abort();
    let (hub, mut rx, server) = fixture(Some(state.path().into())).await;
    let (request, release) = next(&mut rx).await;
    assert!(
        request["input"]
            .to_string()
            .contains("Accepted before shutdown")
    );
    release
        .send(json!([call("roster", "list_agents", json!({}))]))
        .unwrap();
    let (request, release) = next(&mut rx).await;
    let result = request["input"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["type"] == "function_call_output" && v["call_id"] == "roster")
        .unwrap();
    let peers: Value = serde_json::from_str(result["output"].as_str().unwrap()).unwrap();
    assert_eq!(peers.as_array().unwrap().len(), 2);
    assert!(
        peers
            .as_array()
            .unwrap()
            .iter()
            .all(|v| v["listening"] == true)
    );
    release.send(json!([])).unwrap();
    wait_status(&hub, &target.id, "idle").await;
    hub.shutdown().await;
    server.abort();
}

#[tokio::test]
async fn legacy_restart_restores_idle_but_preserves_operator_and_recovery_boundaries() {
    let work = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let (hub, _rx, server) = fixture(Some(state.path().into())).await;
    let ready = agent(&hub, work.path(), "legacy idle", "");
    let stopped = agent(&hub, work.path(), "legacy stopped", "");
    let interrupted = agent(&hub, work.path(), "legacy interrupted", "");
    let recovered = agent(&hub, work.path(), "legacy recovered", "");
    let failed = agent(&hub, work.path(), "legacy failed", "");
    hub.stop(&stopped.id).unwrap();
    wait_status(&hub, &stopped.id, "stopped").await;
    hub.interrupt(&interrupted.id).unwrap();
    wait_status(&hub, &interrupted.id, "stopped").await;
    hub.entry(
        &recovered.id,
        "notice",
        "The model request was interrupted; no unfinished tool call was executed.",
    );
    hub.entry(&failed.id, "error", "Provider failed");
    hub.shutdown().await;
    drop(hub);
    server.abort();
    // Model the old binary's indistinguishable stopped/paused restart state.
    for view in [&ready, &stopped, &interrupted, &recovered, &failed] {
        let path = state
            .path()
            .join("sessions")
            .join(format!("{}.json", view.id));
        let mut record: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        record.as_object_mut().unwrap().remove("listen_on_restart");
        record["paused"] = json!(true);
        record["view"]["status"] = json!("stopped");
        std::fs::write(path, serde_json::to_vec(&record).unwrap()).unwrap();
    }
    let (hub, mut rx, server) = fixture(Some(state.path().into())).await;
    assert_eq!(hub.view(&ready.id).unwrap().status, "idle");
    for view in [&stopped, &interrupted, &recovered, &failed] {
        assert_eq!(
            hub.view(&view.id).unwrap().status,
            "stopped",
            "{}",
            view.name
        );
    }
    assert!(
        tokio::time::timeout(Duration::from_millis(50), rx.recv())
            .await
            .is_err()
    );
    let sender = agent(&hub, work.path(), "sender", "");
    send(
        &hub,
        &sender,
        &ready,
        "No greeting required",
        Urgency::Regular,
        "legacy-contact",
    );
    let (request, release) = next(&mut rx).await;
    assert!(
        request["input"]
            .to_string()
            .contains("No greeting required")
    );
    release.send(json!([])).unwrap();
    wait_status(&hub, &ready.id, "idle").await;
    hub.shutdown().await;
    server.abort();
}
