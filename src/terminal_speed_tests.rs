use super::*;
use axum::{
    Json, Router,
    response::IntoResponse,
    routing::{get, post},
};
use futures_util::StreamExt;
use std::{
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
    time::{Duration, Instant},
};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn persistence_and_lock_wait_leave_networking_responsive() {
    let state = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let hub = Hub::open(Client::test_client(), state.path().into()).unwrap();
    let view = hub
        .create(Create {
            name: "latency".into(),
            cwd: work.path().to_string_lossy().into(),
            message: "".into(),
        })
        .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new().route("/", get(|| async { "ready" })),
        )
        .await
        .unwrap();
    });
    let started = Arc::new(Notify::new());
    let (release, wait) = std::sync::mpsc::channel();
    let writer = hub.clone();
    let id = view.id.clone();
    let signal = started.clone();
    let saving = tokio::spawn(async move {
        writer
            .update(&id, |record| {
                signal.notify_one();
                wait.recv().unwrap();
                record.view.name = "committed".into();
                Ok(())
            })
            .unwrap();
    });
    started.notified().await;
    // A real lock contender occupies the second worker in the old implementation.
    let reader = hub.clone();
    let waiting = tokio::spawn(async move { reader.config() });
    let release_thread = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(700));
        release.send(()).unwrap();
    });
    let begin = Instant::now();
    let result = reqwest::get(url).await.unwrap();
    assert_eq!(result.text().await.unwrap(), "ready");
    let latency = begin.elapsed();
    saving.await.unwrap();
    waiting.await.unwrap();
    release_thread.join().unwrap();
    server.abort();
    assert!(
        latency < Duration::from_millis(350),
        "network stalled for {latency:?}"
    );
    hub.shutdown().await;
    drop(hub);
    let reopened = Hub::open(Client::test_client(), state.path().into()).unwrap();
    assert_eq!(reopened.view(&view.id).unwrap().name, "committed");
    reopened.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "Isolated real-terminal browser latency fixture; no provider requests"]
async fn terminal_latency_fixture() {
    assert_eq!(std::env::var("BOID_TERMINAL_FIXTURE").as_deref(), Ok("1"));
    let state = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let hub = Hub::open(Client::test_client(), state.path().into()).unwrap();
    let view = hub
        .create(Create {
            name: "load".into(),
            cwd: work.path().to_string_lossy().into(),
            message: "".into(),
        })
        .unwrap();
    hub.stop(&view.id).unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        while hub.view(&view.id).unwrap().status != "stopped" {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("fixture worker must finish its stop checkpoint");
    hub.update(&view.id, |record| {
        let text = "x".repeat(32 * 1024 * 1024);
        record
            .view
            .entries
            .push(Hub::make_entry("assistant", text.clone()));
        let mut engine = serde_json::to_value(&record.engine)?;
        engine["history"] = json!([{"role":"assistant","content":text}]);
        record.engine = serde_json::from_value(engine)?;
        Ok(())
    })
    .unwrap();
    let terminals =
        crate::terminal::TerminalHub::open(work.path().into(), state.path().into(), hub.clone())
            .unwrap();
    let loading = Arc::new(AtomicBool::new(false));
    let done = Arc::new(Notify::new());
    let writes = Arc::new(AtomicUsize::new(0));
    let load_hub = hub.clone();
    let load_id = view.id.clone();
    let load_flag = loading.clone();
    let load_done = done.clone();
    let load_writes = writes.clone();
    let start = move || {
        let hub = load_hub.clone();
        let id = load_id.clone();
        let flag = load_flag.clone();
        let done = load_done.clone();
        let writes = load_writes.clone();
        async move {
            if !flag.swap(true, Ordering::SeqCst) {
                tokio::spawn(async move {
                    for _ in 0..40 {
                        if !flag.load(Ordering::SeqCst) {
                            break;
                        }
                        hub.update(&id, |r| {
                            r.order += 1;
                            Ok(())
                        })
                        .unwrap();
                        writes.fetch_add(1, Ordering::SeqCst);
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                    flag.store(false, Ordering::SeqCst);
                    done.notify_one();
                });
            }
            Json(json!({"started":true}))
        }
    };
    let reader = hub.clone();
    let reader_flag = loading.clone();
    let contention = tokio::spawn(async move {
        loop {
            if reader_flag.load(Ordering::SeqCst) {
                reader.profile(&view.id).unwrap();
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    });
    let stop_flag = loading.clone();
    let stop_done = done.clone();
    let stop_writes = writes.clone();
    let snapshot = hub.subscribe_history(true).0;
    let config = hub.config();
    let token_path = PathBuf::from(
        std::env::var("BOID_FIXTURE_TOKEN_FILE")
            .expect("supply a fresh token file path for the browser fixture"),
    );
    let token = Uuid::new_v4().simple().to_string();
    {
        use std::{io::Write, os::unix::fs::OpenOptionsExt};
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&token_path)
            .unwrap();
        file.write_all(token.as_bytes()).unwrap();
    }
    let router = Router::new()
        .route(
            "/api/config",
            get(move || {
                let config = config.clone();
                async move { Json(config) }
            }),
        )
        .route(
            "/api/server",
            get(|| async { Json(json!({"fixture":true})) }),
        )
        .route(
            "/api/events",
            get(move || {
                let snapshot = snapshot.clone();
                async move {
                    axum::response::Sse::new(
                        futures_util::stream::once(async move {
                            Ok::<_, std::convert::Infallible>(
                                axum::response::sse::Event::default()
                                    .json_data(snapshot)
                                    .unwrap(),
                            )
                        })
                        .chain(futures_util::stream::pending()),
                    )
                    .keep_alive(axum::response::sse::KeepAlive::default())
                }
            }),
        )
        .route("/load", post(start))
        .route(
            "/end-load",
            post(move || {
                let flag = stop_flag.clone();
                let done = stop_done.clone();
                let writes = stop_writes.clone();
                async move {
                    if flag.swap(false, Ordering::SeqCst) {
                        done.notified().await;
                    }
                    Json(json!({"writes":writes.load(Ordering::SeqCst)}))
                }
            }),
        )
        .merge(terminals.router())
        .route(
            "/",
            get(|| async { axum::response::Html(include_str!("web/index.html")) }),
        )
        .route(
            "/{*path}",
            get(
                |axum::extract::Path(path): axum::extract::Path<String>| async move {
                    if !matches!(
                        path.as_str(),
                        "app.js"
                            | "style.css"
                            | "vendor/xterm.js"
                            | "vendor/xterm.css"
                            | "vendor/addon-fit.js"
                            | "vendor/markdown-it.min.js"
                    ) {
                        return axum::http::StatusCode::NOT_FOUND.into_response();
                    }
                    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/web");
                    let mime = if path.ends_with(".js") {
                        "text/javascript"
                    } else if path.ends_with(".css") {
                        "text/css"
                    } else {
                        "text/plain"
                    };
                    (
                        [("content-type", mime)],
                        std::fs::read(root.join(path)).unwrap_or_default(),
                    )
                        .into_response()
                },
            ),
        )
        .layer(axum::middleware::from_fn(
            move |request: axum::extract::Request, next: axum::middleware::Next| {
                let token = token.clone();
                async move {
                    let valid = request
                        .headers()
                        .get("cookie")
                        .and_then(|h| h.to_str().ok())
                        .is_some_and(|cookie| {
                            cookie.split(';').any(|part| {
                                part.trim().strip_prefix("boid_fixture_token=")
                                    == Some(token.as_str())
                            })
                        });
                    if !valid {
                        return axum::http::StatusCode::UNAUTHORIZED.into_response();
                    }
                    next.run(request).await
                }
            },
        ));
    let address = std::env::var("BOID_FIXTURE_PORT").unwrap_or_else(|_| "17836".into());
    let listener = tokio::net::TcpListener::bind(format!("127.0.0.1:{address}"))
        .await
        .unwrap();
    println!("terminal fixture ready on {address}");
    tokio::select! {
        result = axum::serve(listener, router) => result.unwrap(),
        _ = tokio::signal::ctrl_c() => {},
    }
    loading.store(false, Ordering::SeqCst);
    contention.abort();
    terminals.shutdown().await.unwrap();
    hub.shutdown().await;
    std::fs::remove_file(token_path).unwrap();
}
