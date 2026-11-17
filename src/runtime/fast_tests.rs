use super::*;
use axum::{Json, Router, routing::post};
use tokio::sync::{mpsc, oneshot};

#[tokio::test]
async fn fast_setting_is_idempotent_preserves_drafts_and_changes_only_next_request() {
    let (sent, mut requests) = mpsc::channel::<(Value, oneshot::Sender<Value>)>(4);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}/", listener.local_addr().unwrap());
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
    let work = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let hub = Hub::open(
        Client::test_client_at(endpoint.clone()),
        state.path().into(),
    )
    .unwrap();
    let view = hub
        .create(Create {
            name: String::new(),
            cwd: work.path().to_string_lossy().into_owned(),
            message: String::new(),
        })
        .unwrap();
    let sibling = hub
        .create_in_group(
            Create {
                name: "sibling".into(),
                cwd: work.path().to_string_lossy().into_owned(),
                message: String::new(),
            },
            1,
            view.profile["pane_group_id"].as_str(),
        )
        .unwrap();
    let before = hub.profile(&view.id).unwrap();
    let fast = hub
        .set_fast(&view.id, true, &hub.profile(&view.id).unwrap())
        .unwrap();
    assert_eq!(fast["service_tier"], "priority");
    assert_ne!(
        hub.profile(&sibling.id).unwrap()["service_tier"],
        "priority",
        "speed is per agent, even within one credential pane"
    );
    assert_eq!(fast["model"], before["model"]);
    assert_eq!(fast["reasoning_effort"], before["reasoning_effort"]);
    let entries = hub.view(&view.id).unwrap().entries;
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].kind, "notice");
    assert_eq!(entries[0].text.as_str(), "Fast mode enabled");
    assert!(
        hub.0.data.lock().unwrap().sessions[&view.id]
            .draft
            .is_some()
    );
    assert!(
        !state
            .path()
            .join("sessions")
            .join(format!("{}.json", view.id))
            .exists()
    );
    let seq = hub.0.data.lock().unwrap().seq;
    assert_eq!(
        hub.set_fast(&view.id, true, &hub.profile(&view.id).unwrap())
            .unwrap(),
        fast
    );
    assert_eq!(
        hub.0.data.lock().unwrap().seq,
        seq,
        "idempotent retry emits no new mutation"
    );
    // A chooser opened before /fast must preserve that newer preference.
    let mut model = before.clone();
    model["model"] = json!("chosen-model");
    hub.select_profile(&view.id, &before, model).unwrap();
    assert_eq!(hub.profile(&view.id).unwrap()["service_tier"], "priority");
    hub.message(&view.id, "Start".into()).unwrap();
    let (first, release) = tokio::time::timeout(Duration::from_secs(3), requests.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(first["service_tier"], "priority");
    assert!(!first.to_string().contains("Fast mode enabled"), "UI notice must not enter model input");
    assert_eq!(first["model"], "chosen-model");
    assert_eq!(first["reasoning"]["effort"], before["reasoning_effort"]);
    hub.set_fast(&view.id, false, &hub.profile(&view.id).unwrap())
        .unwrap();
    assert_eq!(hub.view(&view.id).unwrap().status, "working");
    assert_eq!(
        hub.0.data.lock().unwrap().sessions[&view.id]
            .record
            .engine
            .profile["service_tier"],
        "priority"
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(30), requests.recv())
            .await
            .is_err()
    );
    release.send(json!([{"type":"function_call","call_id":"roster","name":"list_agents","arguments":"{}"}])).unwrap();
    let (second, release) = tokio::time::timeout(Duration::from_secs(3), requests.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(second["service_tier"], "default");
    assert_eq!(second["model"], first["model"]);
    assert_eq!(second["reasoning"], first["reasoning"]);
    hub.set_fast(&view.id, true, &hub.profile(&view.id).unwrap())
        .unwrap();
    release.send(json!([])).unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        while hub.view(&view.id).unwrap().status != "idle" {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        hub.profile(&view.id).unwrap()["service_tier"],
        "priority",
        "old worker checkpoint cannot overwrite next-request preference"
    );
    hub.shutdown().await;
    drop(hub);
    let restored = Hub::open(Client::test_client_at(endpoint), state.path().into()).unwrap();
    assert_eq!(
        restored.profile(&view.id).unwrap()["service_tier"],
        "priority"
    );
    assert_eq!(restored.profile(&view.id).unwrap()["model"], "chosen-model");
    assert_eq!(restored.view(&view.id).unwrap().entries.iter()
        .filter(|entry| entry.text.as_str() == "Fast mode enabled").count(), 2,
        "each enable transition is retained once across retries and restart");
    restored.shutdown().await;
    server.abort();
}
