use crate::{
    api::Client,
    runtime::{Create, Hub},
    terminal::TerminalHub,
};
use anyhow::{Context, Result, bail};
use axum::{
    Json, Router,
    body::Body,
    extract::{DefaultBodyLimit, Path, Request, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    middleware::{self, Next},
    response::{
        IntoResponse, Response, Sse,
        sse::{Event, KeepAlive},
    },
    routing::{get, post},
};
use futures_util::stream;
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    convert::Infallible,
    io::Write,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::PathBuf,
    sync::Arc,
};
use tokio::sync::broadcast;
use uuid::Uuid;

#[derive(Clone)]
struct App {
    hub: Hub,
    token: Arc<String>,
}
type ApiResult<T> = std::result::Result<T, (StatusCode, Json<Value>)>;
fn invalid(error: impl std::fmt::Display) -> (StatusCode, Json<Value>) {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({"error":error.to_string()})),
    )
}

pub fn token_file() -> Result<PathBuf> {
    let path = match std::env::var_os("BOID_STATE_DIR") {
        Some(path) => PathBuf::from(path),
        None => {
            PathBuf::from(std::env::var_os("HOME").context("HOME or BOID_STATE_DIR is required.")?)
                .join(".local/share/boid")
        }
    };
    Ok(path.join("token"))
}
pub fn read_token() -> Result<String> {
    let token = match std::env::var("BOID_TOKEN") {
        Ok(token) => token,
        Err(_) => std::fs::read_to_string(token_file()?)
            .context("Start `boid serve` first, or set BOID_TOKEN for its server.")?,
    };
    let token = token.trim().to_owned();
    if token.len() < 24
        || token.len() > 128
        || !token
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
    {
        bail!("BOID_TOKEN must contain 24 to 128 letters, digits, hyphens or underscores.");
    }
    Ok(token)
}
fn ensure_token() -> Result<String> {
    if std::env::var_os("BOID_TOKEN").is_some() || token_file()?.exists() {
        return read_token();
    }
    let path = token_file()?;
    let dir = path.parent().unwrap();
    std::fs::create_dir_all(dir)?;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    let token = Uuid::new_v4().simple().to_string();
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(token.as_bytes())?;
    Ok(token)
}
pub async fn serve() -> Result<()> {
    let address = std::env::var("BOID_LISTEN").unwrap_or_else(|_| "127.0.0.1:7734".into());
    let listener = tokio::net::TcpListener::bind(&address).await.context("Cannot bind Boid server. The default port permits one server; connect to the running server instead of starting another.")?;
    let token = ensure_token()?;
    let state_dir = token_file()?
        .parent()
        .context("State directory has no parent.")?
        .to_path_buf();
    let hub = Hub::open(Client::new()?, state_dir.clone())?;
    let terminals = TerminalHub::open(std::env::current_dir()?, state_dir, hub.clone())?;
    let app = App {
        hub: hub.clone(),
        token: Arc::new(token.clone()),
    };
    let protected = Router::new()
        .route("/api/login", post(login))
        .route("/api/config", get(config))
        .route("/api/sessions", get(sessions).post(create))
        .route("/api/sessions/archived", get(archived_sessions))
        .route("/api/sessions/{id}/reopen", post(reopen_session))
        .route("/api/sessions/{id}", get(session).delete(close_session))
        .route("/api/sessions/{id}/messages", post(message))
        .route("/api/sessions/{id}/skills", get(skill_catalog))
        .route("/api/sessions/{id}/stop", post(stop))
        .route("/api/sessions/{id}/interrupt", post(interrupt))
        .route("/api/events", get(events))
        .with_state(app.clone())
        .merge(terminals.router())
        .layer(DefaultBodyLimit::max(1024 * 1024))
        .layer(middleware::from_fn_with_state(app, authorize));
    let router = Router::new()
        .route("/", get(index))
        .route("/{*path}", get(asset))
        .merge(protected)
        .layer(middleware::from_fn(headers));
    let display_address = if address.starts_with("0.0.0.0:") {
        address.replacen("0.0.0.0", "127.0.0.1", 1)
    } else {
        address.clone()
    };
    println!(
        "Boid server listening on {address}\nOpen http://{display_address}/#token={token}\nKeep this process running; closing the browser only disconnects its view."
    );
    let result = tokio::select! {
        result = axum::serve(listener, router) => result,
        signal = tokio::signal::ctrl_c() => signal,
        _ = terminate() => Ok(()),
    };
    hub.shutdown().await;
    let terminal_result = terminals.shutdown().await;
    result?;
    terminal_result?;
    Ok(())
}
async fn terminate() {
    if let Ok(mut signal) =
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
    {
        signal.recv().await;
    } else {
        std::future::pending::<()>().await;
    }
}
async fn authorize(State(app): State<App>, request: Request, next: Next) -> Response {
    let headers = request.headers();
    if let Some(origin) = headers.get(header::ORIGIN).and_then(|h| h.to_str().ok()) {
        let host = headers
            .get(header::HOST)
            .and_then(|h| h.to_str().ok())
            .unwrap_or("");
        if origin != format!("http://{host}") && origin != format!("https://{host}") {
            return (
                StatusCode::FORBIDDEN,
                Json(json!({"error":"Cross-origin requests are not allowed."})),
            )
                .into_response();
        }
    }
    let bearer = headers
        .get(header::AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));
    let cookie = headers
        .get(header::COOKIE)
        .and_then(|h| h.to_str().ok())
        .and_then(|v| {
            v.split(';')
                .find_map(|part| part.trim().strip_prefix("boid_token="))
        });
    if ![bearer, cookie]
        .into_iter()
        .flatten()
        .any(|value| value == app.token.as_str())
    {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error":"Connect with the token from the Boid server."})),
        )
            .into_response();
    }
    next.run(request).await
}
async fn headers(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert("referrer-policy", HeaderValue::from_static("no-referrer"));
    headers.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    headers.insert("content-security-policy", HeaderValue::from_static("default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; connect-src 'self'; img-src 'self' data:; object-src 'none'; frame-ancestors 'none'; base-uri 'none'"));
    response
}
async fn login(State(app): State<App>) -> impl IntoResponse {
    let mut headers = HeaderMap::new();
    headers.insert(
        header::SET_COOKIE,
        HeaderValue::from_str(&format!(
            "boid_token={}; HttpOnly; SameSite=Strict; Path=/",
            app.token
        ))
        .unwrap(),
    );
    (headers, Json(json!({"ok":true})))
}
async fn config(State(app): State<App>) -> Json<Value> {
    Json(app.hub.config())
}
async fn sessions(State(app): State<App>) -> Json<Value> {
    Json(json!(app.hub.views()))
}
async fn archived_sessions(State(app): State<App>) -> Json<Value> {
    Json(json!(app.hub.archived_views()))
}
async fn reopen_session(State(app): State<App>, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    app.hub
        .reopen(&id)
        .map(|view| Json(json!(view)))
        .map_err(invalid)
}
async fn session(State(app): State<App>, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    app.hub
        .view(&id)
        .map(|view| Json(json!(view)))
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                Json(json!({"error":"Unknown session."})),
            )
        })
}
async fn skill_catalog(State(app): State<App>, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    tokio::task::spawn_blocking(move || app.hub.skill_catalog(&id))
        .await
        .map_err(invalid)?
        .map(Json)
        .map_err(invalid)
}
async fn create(State(app): State<App>, Json(input): Json<Create>) -> ApiResult<Json<Value>> {
    tokio::task::spawn_blocking(move || app.hub.create(input))
        .await
        .map_err(invalid)?
        .map(|view| Json(json!(view)))
        .map_err(invalid)
}
async fn close_session(State(app): State<App>, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    let persisted = app.hub.close(&id).await.map_err(invalid)?;
    Ok(Json(json!({"status":"closed", "persisted":persisted})))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Input {
    text: String,
}
async fn message(
    State(app): State<App>,
    Path(id): Path<String>,
    Json(input): Json<Input>,
) -> ApiResult<impl IntoResponse> {
    let message_id = app.hub.message(&id, input.text).map_err(invalid)?;
    Ok((
        StatusCode::ACCEPTED,
        Json(json!({"message_id":message_id,"status":"queued"})),
    ))
}
async fn stop(State(app): State<App>, Path(id): Path<String>) -> ApiResult<impl IntoResponse> {
    app.hub.stop(&id).map_err(invalid)?;
    Ok((
        StatusCode::ACCEPTED,
        Json(json!({"status":"stop_requested"})),
    ))
}
async fn interrupt(State(app): State<App>, Path(id): Path<String>) -> ApiResult<impl IntoResponse> {
    app.hub.interrupt(&id).map_err(invalid)?;
    Ok((
        StatusCode::ACCEPTED,
        Json(json!({"status":"interrupt_requested"})),
    ))
}
async fn events(
    State(app): State<App>,
) -> Sse<impl futures_util::Stream<Item = std::result::Result<Event, Infallible>>> {
    let (snapshot, receiver) = app.hub.subscribe();
    let stream = stream::unfold(
        (Some(snapshot), receiver, app.hub),
        |(initial, mut receiver, hub)| async move {
            let value = match initial {
                Some(value) => value,
                None => match receiver.recv().await {
                    Ok(value) => value,
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        let (snapshot, new_receiver) = hub.subscribe();
                        receiver = new_receiver;
                        snapshot
                    }
                    Err(broadcast::error::RecvError::Closed) => return None,
                },
            };
            let event = Event::default()
                .id(value["seq"].to_string())
                .data(value.to_string());
            Some((Ok(event), (None, receiver, hub)))
        },
    );
    Sse::new(stream).keep_alive(KeepAlive::default())
}
async fn index() -> Response {
    ui_asset(
        "index.html",
        "text/html; charset=utf-8",
        include_bytes!("web/index.html"),
    )
}
async fn asset(Path(path): Path<String>) -> Response {
    match path.as_str() {
        "app.js" => ui_asset(
            "app.js",
            "text/javascript; charset=utf-8",
            include_bytes!("web/app.js"),
        ),
        "style.css" => ui_asset(
            "style.css",
            "text/css; charset=utf-8",
            include_bytes!("web/style.css"),
        ),
        "vendor/xterm.js" => bytes("text/javascript", include_bytes!("web/vendor/xterm.js")),
        "vendor/xterm.css" => bytes("text/css", include_bytes!("web/vendor/xterm.css")),
        "vendor/addon-fit.js" => {
            bytes("text/javascript", include_bytes!("web/vendor/addon-fit.js"))
        }
        "vendor/markdown-it.min.js" => bytes(
            "text/javascript",
            include_bytes!("web/vendor/markdown-it.min.js"),
        ),
        _ => StatusCode::NOT_FOUND.into_response(),
    }
}
fn bytes(content_type: &'static str, content: &'static [u8]) -> Response {
    ([(header::CONTENT_TYPE, content_type)], Body::from(content)).into_response()
}

fn ui_asset(_name: &'static str, content_type: &'static str, _embedded: &'static [u8]) -> Response {
    #[cfg(debug_assertions)]
    {
        let path =
            std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/src/web")).join(_name);
        match std::fs::read(path) {
            Ok(content) => {
                ([(header::CONTENT_TYPE, content_type)], Body::from(content)).into_response()
            }
            Err(error) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Cannot read development UI asset {_name}: {error}"),
            )
                .into_response(),
        }
    }
    #[cfg(not(debug_assertions))]
    {
        bytes(content_type, _embedded)
    }
}
