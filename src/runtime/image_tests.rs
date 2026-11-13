use super::*;
use axum::{Json, Router, extract::OriginalUri, response::IntoResponse, routing::post};
use base64::{Engine, engine::general_purpose::STANDARD};

fn create(hub: &Hub, cwd: &Path) -> View {
    hub.create(Create {
        name: String::new(),
        cwd: cwd.to_string_lossy().into_owned(),
        message: String::new(),
    })
    .unwrap()
}
fn abort_worker(hub: &Hub, id: &str) {
    if let Some(task) = &hub.0.data.lock().unwrap().sessions[id].task {
        task.abort();
    }
}
async fn idle(hub: &Hub, id: &str) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let data = hub.0.data.lock().unwrap();
            let record = &data.sessions[id].record;
            let done = record.inputs.is_empty() && record.view.status == "idle";
            drop(data);
            if done {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn image_queue_restart_and_old_text_json_remain_faithful() {
    let state = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let hub = Hub::open(Client::test_client(), state.path().into()).unwrap();
    let target = create(&hub, work.path());
    let other = create(&hub, work.path());
    let old = json!({"id":"fixture-entry","kind":"user","text":"unchanged"});
    let entry: Entry = serde_json::from_value(old.clone()).unwrap();
    assert_eq!(serde_json::to_value(entry).unwrap(), old);
    let png = crate::images::fixture_png();
    let image = hub.upload_image(&target.id, "image/png", &png).unwrap();
    assert!(hub.view(&target.id).unwrap().entries.is_empty());
    assert!(
        hub.0.data.lock().unwrap().sessions[&target.id]
            .draft
            .is_some()
    );
    assert_eq!(
        std::fs::read_dir(state.path().join("sessions"))
            .unwrap()
            .count(),
        0
    );
    assert!(
        hub.message_images_timed(&other.id, "wrong pane".into(), vec![image.id.clone()])
            .0
            .is_err()
    );
    let message = hub
        .message_images_timed(&target.id, String::new(), vec![image.id.clone()])
        .0
        .unwrap();
    abort_worker(&hub, &target.id);
    let mut engine = hub.0.data.lock().unwrap().sessions[&target.id]
        .record
        .engine
        .clone();
    hub.next_input(&target.id, &mut engine, false).unwrap();
    assert!(engine.has_image_refs());
    let visible = hub.view(&target.id).unwrap();
    let accepted = visible
        .entries
        .iter()
        .find(|entry| entry.id == message)
        .unwrap();
    assert!(accepted.text.is_empty());
    assert_eq!(accepted.images[0].label, "Image #1");
    let serialized =
        serde_json::to_string(&hub.0.data.lock().unwrap().sessions[&target.id].record).unwrap();
    assert!(serialized.contains("boid-image://"));
    assert!(!serialized.contains("data:image"));
    let peer = hub.read_peer_trace(
        &other.id,
        &json!({"to":target.id,"kind":"user"}).to_string(),
    );
    assert_eq!(peer["entries"][0]["images"][0]["id"], image.id);
    hub.shutdown().await;
    drop(hub);
    let restored = Hub::open(Client::test_client(), state.path().into()).unwrap();
    assert!(
        restored.0.data.lock().unwrap().sessions[&target.id]
            .record
            .engine
            .has_image_refs()
    );
    assert_eq!(
        restored.0.data.lock().unwrap().sessions[&target.id]
            .record
            .version,
        5
    );
    assert_eq!(
        restored
            .image_store()
            .read(&target.id, &image.id)
            .unwrap()
            .1
            .as_slice(),
        png
    );
    restored.close(&target.id).await.unwrap();
    assert!(restored.ensure_image_target(&target.id).is_err());
    assert_eq!(
        restored
            .image_store()
            .read(&target.id, &image.id)
            .unwrap()
            .1
            .as_slice(),
        png
    );
    restored.shutdown().await;
}

#[tokio::test]
async fn image_requests_reach_api_and_subscription_and_survive_compaction() {
    for subscription in [false, true] {
        let state = tempfile::tempdir().unwrap();
        let work = tempfile::tempdir().unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}/", listener.local_addr().unwrap());
        let (sent, mut requests) = tokio::sync::mpsc::channel(8);
        let handler = move |OriginalUri(uri): OriginalUri, Json(body): Json<Value>| {
            let sent = sent.clone();
            async move {
                sent.send((uri.path().to_owned(), body.clone()))
                    .await
                    .unwrap();
                if uri.path() == "/compact" {
                    let mut output = body["input"].as_array().unwrap().clone();
                    output.push(json!({"type":"compaction","encrypted_content":"opaque fixture"}));
                    Json(json!({"object":"response.compaction","output":output})).into_response()
                } else {
                    (
                        [("content-type", "text/event-stream")],
                        "data: {\"type\":\"response.completed\",\"response\":{\"output\":[]}}\n\n",
                    )
                        .into_response()
                }
            }
        };
        let router = Router::new()
            .route("/", post(handler.clone()))
            .route("/compact", post(handler));
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let mut client = Client::test_client_at(endpoint);
        if subscription {
            let directory = state.path().join("auth");
            let auth =
                Arc::new(crate::chatgpt::Auth::open(directory.clone(), "chatgpt", "test").unwrap());
            let path = directory.join("accounts.json");
            let mut saved: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
            let id = Uuid::new_v4().to_string();
            saved["selected"] = json!(id);
            saved["default_credential_id"] = json!(id);
            saved["registry_migrated"] = json!(true);
            saved["accounts"][&id] = json!({"id":id,"name":"fixture","client_id":"fixture","subject":"fixture","email":"fixture@example.invalid","welcomed":true,"paused":false,"access_token":"fixture-access","refresh_token":"fixture-refresh","expires_at":u64::MAX,"scopes":["chatgpt.tokens.use.direct"]});
            std::fs::write(path, serde_json::to_vec(&saved).unwrap()).unwrap();
            client.auth = Some(auth);
        }
        let hub = Hub::open(client, state.path().join("state")).unwrap();
        let target = create(&hub, work.path());
        let png = crate::images::fixture_png();
        let image = hub.upload_image(&target.id, "image/png", &png).unwrap();
        hub.message_images_timed(
            &target.id,
            String::new(),
            vec![image.id.clone(), image.id.clone()],
        )
        .0
        .unwrap();
        let (_, first) = tokio::time::timeout(Duration::from_secs(5), requests.recv())
            .await
            .unwrap()
            .unwrap();
        let content = &first["input"].as_array().unwrap().last().unwrap()["content"];
        assert_eq!(content[0]["text"], "[Image #1]");
        assert_eq!(content[2]["text"], "[Image #2]");
        for index in [1, 3] {
            let data = content[index]["image_url"].as_str().unwrap();
            assert_eq!(content[index]["type"], "input_image");
            assert_eq!(
                STANDARD.decode(data.split_once(',').unwrap().1).unwrap(),
                png
            );
        }
        assert_eq!(
            first["tools"][0]["type"],
            if subscription {
                "namespace"
            } else {
                "function"
            }
        );
        idle(&hub, &target.id).await;
        if !subscription {
            hub.message(&target.id, "/oai-compact".into()).unwrap();
            let (path, compact) = tokio::time::timeout(Duration::from_secs(5), requests.recv())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(path, "/compact");
            assert!(compact.to_string().contains("data:image/png;base64,"));
            idle(&hub, &target.id).await;
            let saved = serde_json::to_string(
                &hub.0.data.lock().unwrap().sessions[&target.id]
                    .record
                    .engine,
            )
            .unwrap();
            assert!(saved.contains("boid-image://"));
            assert!(!saved.contains("data:image"));
        }
        // Attached images make slash text literal, preserving both the text and images.
        hub.message_images_timed(&target.id, "/clear".into(), vec![image.id.clone()])
            .0
            .unwrap();
        let (_, second) = tokio::time::timeout(Duration::from_secs(5), requests.recv())
            .await
            .unwrap()
            .unwrap();
        let content = &second["input"].as_array().unwrap().last().unwrap()["content"];
        assert_eq!(content[0]["text"], "/clear");
        assert_eq!(content[2]["type"], "input_image");
        assert!(second["input"].as_array().unwrap().len() > 2);
        idle(&hub, &target.id).await;
        hub.shutdown().await;
        server.abort();
    }
}

// Real authenticated Boid routes/assets, isolated state and a local mock provider.
// Intended for the release worker's actual browser-paste check, never production.
#[tokio::test]
#[ignore]
async fn image_browser_fixture() {
    assert_eq!(
        std::env::var("BOID_IMAGE_BROWSER_FIXTURE").as_deref(),
        Ok("1")
    );
    let capture =
        PathBuf::from(std::env::var("BOID_IMAGE_CAPTURE").expect("isolated capture path"));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut client = Client::test_client_at(format!("http://{}/", listener.local_addr().unwrap()));
    client.model = "gpt-6-sol".into();
    let router = Router::new().route("/", post(move |Json(input): Json<Value>| {
        let capture = capture.clone();
        async move {
            std::fs::write(capture, serde_json::to_vec(&input).unwrap()).unwrap();
            ([("content-type", "text/event-stream")], "data: {\"type\":\"response.completed\",\"response\":{\"output\":[{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"Fixture received image input.\"}]}]}}\n\n")
        }
    }));
    let mock = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    crate::server::serve_client(client).await.unwrap();
    mock.abort();
}
