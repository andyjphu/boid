use crate::{
    api::Client,
    runtime::{Create, Hub},
    terminal::TerminalHub,
};
use anyhow::{Context, Result, bail};
use axum::{
    Json, Router,
    body::Body,
    extract::{DefaultBodyLimit, Path, Query, Request, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    middleware::{self, Next},
    response::{
        Html, IntoResponse, Redirect, Response, Sse,
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
use tokio::sync::{Notify, broadcast};
use uuid::Uuid;

#[derive(Clone)]
struct App {
    hub: Hub,
    token: Arc<String>,
    shutdown: Arc<Notify>,
    auth: Option<Arc<crate::chatgpt::Auth>>,
    api_key_available: bool,
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
    serve_client(Client::new()?).await
}
pub(crate) async fn serve_client(client: Client) -> Result<()> {
    let address = std::env::var("BOID_LISTEN").unwrap_or_else(|_| "127.0.0.1:7734".into());
    let listener = tokio::net::TcpListener::bind(&address).await.context("Cannot bind Boid server. The default port permits one server; connect to the running server instead of starting another.")?;
    let token = ensure_token()?;
    let state_dir = token_file()?
        .parent()
        .context("State directory has no parent.")?
        .to_path_buf();
    let auth = client.auth.clone();
    let api_key_available = client.key_available();
    let hub = Hub::open(client, state_dir.clone())?;
    let terminals = TerminalHub::open(std::env::current_dir()?, state_dir, hub.clone())?;
    let shutdown = Arc::new(Notify::new());
    let app = App {
        hub: hub.clone(),
        token: Arc::new(token.clone()),
        shutdown: shutdown.clone(),
        auth,
        api_key_available,
    };
    let protected = Router::new()
        .route("/api/login", post(login))
        .route("/api/server", get(server_info))
        .route("/api/shutdown", post(shutdown_server))
        .route("/api/config", get(config))
        .route("/api/accounts", get(accounts))
        .route("/api/accounts/sign-in", post(account_sign_in))
        .route("/api/accounts/select", post(account_select))
        .route("/api/accounts/api-key", post(account_api_key))
        .route("/api/accounts/{id}/models", get(account_models))
        .route("/api/accounts/{id}/logout", post(account_logout))
        .route("/api/accounts/{id}/resume", post(account_resume))
        .route("/api/diagnostics", get(diagnostics))
        .route("/api/sessions", get(sessions).post(create))
        .route("/api/sessions/archived", get(archived_sessions))
        .route("/api/sessions/{id}/reopen", post(reopen_session))
        .route(
            "/api/sessions/{id}",
            get(session).patch(rename_session).delete(close_session),
        )
        .route("/api/sessions/{id}/messages", post(message))
        .route("/api/sessions/{id}/skills", get(skill_catalog))
        .route("/api/sessions/{id}/stop", post(stop))
        .route("/api/sessions/{id}/interrupt", post(interrupt))
        .route("/api/events", get(events))
        .with_state(app.clone())
        .merge(terminals.router())
        .layer(DefaultBodyLimit::max(1024 * 1024))
        .layer(middleware::from_fn_with_state(app.clone(), authorize));
    let oauth = Router::new()
        .route("/auth/start", get(auth_start))
        .route("/auth/callback", get(auth_callback))
        .with_state(app);
    let router = Router::new()
        .route("/", get(index))
        .route("/{*path}", get(asset))
        .merge(protected)
        .merge(oauth)
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
        _ = shutdown.notified() => Ok(()),
    };
    hub.shutdown().await;
    let terminal_result = terminals.shutdown().await;
    result?;
    terminal_result?;
    Ok(())
}
async fn server_info() -> Json<Value> {
    Json(json!({"application":"boid", "protocol":1,
        "version":env!("CARGO_PKG_VERSION"), "pid":std::process::id()}))
}
async fn shutdown_server(State(app): State<App>) -> impl IntoResponse {
    tokio::spawn(async move {
        // Allow the acknowledgement to leave before closing the HTTP listener.
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        app.shutdown.notify_one();
    });
    (StatusCode::ACCEPTED, Json(json!({"status":"stopping"})))
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
async fn diagnostics(State(app): State<App>) -> Json<Value> {
    Json(app.hub.diagnostics())
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
struct Rename {
    name: String,
}
async fn rename_session(
    State(app): State<App>,
    Path(id): Path<String>,
    Json(input): Json<Rename>,
) -> ApiResult<Json<Value>> {
    tokio::task::spawn_blocking(move || app.hub.rename(&id, input.name))
        .await
        .map_err(invalid)?
        .map(|view| Json(json!(view)))
        .map_err(invalid)
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

fn auth(app: &App) -> ApiResult<&Arc<crate::chatgpt::Auth>> {
    app.auth
        .as_ref()
        .ok_or_else(|| invalid("ChatGPT authentication is unavailable in this server."))
}
async fn accounts(State(app): State<App>) -> ApiResult<Json<Value>> {
    Ok(Json(
        auth(&app)?.status(app.api_key_available).map_err(invalid)?,
    ))
}
#[derive(Deserialize)]
struct SignIn {
    origin: String,
    account: Option<String>,
    #[serde(default)]
    consent: bool,
}
async fn account_sign_in(
    State(app): State<App>,
    Json(input): Json<SignIn>,
) -> ApiResult<Json<Value>> {
    Ok(Json(
        json!({"start_url":auth(&app)?.begin(&input.origin,input.account.as_deref(),input.consent).map_err(invalid)?}),
    ))
}
#[derive(Deserialize)]
struct AccountSelection {
    billing_route: String,
    account: Option<String>,
    #[serde(default)]
    model: String,
}
async fn account_select(
    State(app): State<App>,
    Json(input): Json<AccountSelection>,
) -> ApiResult<Json<Value>> {
    if input.billing_route == "api_key"
        && !app.api_key_available
        && auth(&app)?.api_key().map_err(invalid)?.is_none()
    {
        return Err(invalid("Use /login to enter an API key."));
    }
    let welcome = auth(&app)?
        .select(&input.billing_route, input.account.as_deref(), &input.model)
        .await
        .map_err(invalid)?;
    Ok(Json(
        json!({"status":"selected_for_new_agents","welcome":welcome}),
    ))
}
#[derive(Deserialize)]
struct ApiKeyLogin {
    key: String,
}
async fn account_api_key(
    State(app): State<App>,
    Json(input): Json<ApiKeyLogin>,
) -> ApiResult<Json<Value>> {
    auth(&app)?
        .save_api_key(&input.key)
        .await
        .map_err(invalid)?;
    Ok(Json(json!({"ok":true})))
}
async fn account_models(State(app): State<App>, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    Ok(Json(
        json!({"models":auth(&app)?.models(&id).await.map_err(invalid)?}),
    ))
}
async fn account_resume(State(app): State<App>, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    auth(&app)?.pause(&id, false).await.map_err(invalid)?;
    Ok(Json(json!({"ok":true})))
}
async fn account_logout(State(app): State<App>, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    app.hub.stop_account(&id).map_err(invalid)?;
    let confirmed = auth(&app)?.logout(&id).await.map_err(invalid)?;
    Ok(Json(
        json!({"revocation_confirmed":confirmed,"message":if confirmed {"Signed out."} else {"Signed out locally; remote revocation was not confirmed. Disconnect Boid in ChatGPT Settings."}}),
    ))
}
async fn auth_start(
    State(app): State<App>,
    Query(query): Query<std::collections::BTreeMap<String, String>>,
) -> ApiResult<Redirect> {
    let url = auth(&app)?
        .authorization(query.get("ticket").map(String::as_str).unwrap_or(""))
        .map_err(invalid)?;
    Ok(Redirect::to(&url))
}
async fn auth_callback(
    State(app): State<App>,
    Query(query): Query<std::collections::BTreeMap<String, String>>,
) -> impl IntoResponse {
    let result = match auth(&app) {
        Ok(auth) => auth.complete(&query).await,
        Err(_) => Err(anyhow::anyhow!("Authentication unavailable.")),
    };
    let (status, text) = match result {
        Ok(()) => (
            StatusCode::OK,
            "ChatGPT account connected. Close this tab and choose a model and billing route with /login in Boid.".to_owned(),
        ),
        Err(error) => (
            StatusCode::BAD_REQUEST,
            format!("ChatGPT sign-in did not complete: {error}. Close this tab and retry with /login in Boid."),
        ),
    };
    let text = text
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    (
        status,
        Html(format!(
            "<!doctype html><meta charset=\"utf-8\"><title>Boid account</title><p>{text}</p>"
        )),
    )
}
