//! Browser access to Boid, independent of model-provider authentication.
use anyhow::{Context, Result, bail, ensure};
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Path, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    io::Write,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};
use webauthn_rs::prelude::*;

const FLOW_SECONDS: u64 = 300;
const SESSION_SECONDS: u64 = 30 * 24 * 60 * 60;
const MAX_PENDING: usize = 128;
const SESSION_COOKIE: &str = "boid_browser";
const FLOW_COOKIE: &str = "boid_passkey_flow";

type ApiResult<T> = std::result::Result<T, Failure>;
struct Failure(StatusCode, &'static str);
impl IntoResponse for Failure {
    fn into_response(self) -> Response {
        (self.0, Json(json!({"error":self.1}))).into_response()
    }
}
fn denied() -> Failure {
    Failure(
        StatusCode::UNAUTHORIZED,
        "Passkey authorization expired or was not accepted. Start again.",
    )
}
fn internal(_: impl std::fmt::Display) -> Failure {
    Failure(
        StatusCode::INTERNAL_SERVER_ERROR,
        "Could not save browser authorization.",
    )
}
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
fn secret() -> String {
    format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple())
}
fn digest(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))
}
pub(crate) fn cookie<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .get(header::COOKIE)?
        .to_str()
        .ok()?
        .split(';')
        .find_map(|part| {
            let (key, value) = part.trim().split_once('=')?;
            (key == name).then_some(value)
        })
}
#[derive(Clone, Serialize, Deserialize)]
struct Credential {
    id: String,
    created: u64,
    passkey: Passkey,
}
#[derive(Clone, Serialize, Deserialize)]
struct BrowserSession {
    id: String,
    credential: String,
    hash: String,
    created: u64,
    expires: u64,
}
#[derive(Clone, Serialize, Deserialize)]
struct Saved {
    origin: String,
    user: Uuid,
    credentials: Vec<Credential>,
    sessions: Vec<BrowserSession>,
}
enum Challenge {
    Register(PasskeyRegistration),
    Authenticate(PasskeyAuthentication),
}
struct Pending {
    expires: u64,
    challenge: Challenge,
}
struct Inner {
    saved: Saved,
    enrollments: HashMap<String, u64>,
    pending: HashMap<String, Pending>,
}
impl Inner {
    fn prune(&mut self) {
        let time = now();
        self.enrollments.retain(|_, expires| *expires > time);
        self.pending.retain(|_, flow| flow.expires > time);
    }
    fn capacity(&mut self) -> ApiResult<()> {
        self.prune();
        if self.pending.len() + self.enrollments.len() >= MAX_PENDING {
            return Err(Failure(
                StatusCode::TOO_MANY_REQUESTS,
                "Too many sign-in attempts. Try again shortly.",
            ));
        }
        Ok(())
    }
}
pub(crate) struct BrowserAuth {
    webauthn: Webauthn,
    origin: String,
    host: String,
    secure: bool,
    file: PathBuf,
    token: String,
    inner: Mutex<Inner>,
    changed: tokio::sync::watch::Sender<()>,
}
impl BrowserAuth {
    pub fn open(dir: PathBuf, token: String) -> Result<Arc<Self>> {
        let origin =
            std::env::var("BOID_BROWSER_ORIGIN").unwrap_or_else(|_| "http://localhost:7734".into());
        Self::open_at(dir, token, &origin)
    }
    fn open_at(dir: PathBuf, token: String, origin: &str) -> Result<Arc<Self>> {
        let url = Url::parse(origin).context("Invalid BOID_BROWSER_ORIGIN")?;
        ensure!(
            url.username().is_empty()
                && url.password().is_none()
                && url.path() == "/"
                && url.query().is_none()
                && url.fragment().is_none(),
            "BOID_BROWSER_ORIGIN must be an origin without credentials, path, query or fragment."
        );
        let domain = url
            .domain()
            .context("BOID_BROWSER_ORIGIN requires a DNS name; use localhost, not 127.0.0.1.")?;
        ensure!(
            url.scheme() == "https" || (url.scheme() == "http" && domain == "localhost"),
            "Passkeys require HTTPS or http://localhost."
        );
        let origin = url.origin().ascii_serialization();
        let host = origin.split_once("://").unwrap().1.to_owned();
        let webauthn = WebauthnBuilder::new(domain, &url)?
            .rp_name("Boid")
            .timeout(std::time::Duration::from_secs(FLOW_SECONDS))
            .build()?;
        let file = dir.join("browser-auth.json");
        let saved = match std::fs::read(&file) {
            Ok(data) => serde_json::from_slice::<Saved>(&data)
                .context("Cannot read browser authentication state")?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Saved {
                origin: origin.clone(),
                user: Uuid::new_v4(),
                credentials: vec![],
                sessions: vec![],
            },
            Err(error) => return Err(error.into()),
        };
        ensure!(
            saved.origin == origin,
            "BOID_BROWSER_ORIGIN differs from saved passkey origin. Restore the original origin to use existing credentials."
        );
        Ok(Arc::new(Self {
            webauthn,
            origin,
            host,
            secure: url.scheme() == "https",
            file,
            token,
            changed: tokio::sync::watch::channel(()).0,
            inner: Mutex::new(Inner {
                saved,
                enrollments: HashMap::new(),
                pending: HashMap::new(),
            }),
        }))
    }
    fn save(&self, saved: &Saved) -> Result<()> {
        let dir = self.file.parent().unwrap();
        std::fs::create_dir_all(dir)?;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
        let mut file = tempfile::NamedTempFile::new_in(dir)?;
        file.as_file()
            .set_permissions(std::fs::Permissions::from_mode(0o600))?;
        serde_json::to_writer(&mut file, saved)?;
        file.as_file().sync_all()?;
        file.persist(&self.file)?;
        std::fs::File::open(dir)?.sync_all()?;
        Ok(())
    }
    // The origin is server configuration, never learned from an untrusted Host header.
    fn browser_request(&self, headers: &HeaderMap) -> ApiResult<()> {
        if headers.get(header::ORIGIN).and_then(|v| v.to_str().ok()) != Some(self.origin.as_str())
            || !self.host_matches(headers)
        {
            return Err(Failure(
                StatusCode::FORBIDDEN,
                "Open Boid at its configured passkey address.",
            ));
        }
        Ok(())
    }
    fn host_matches(&self, headers: &HeaderMap) -> bool {
        headers.get(header::HOST).and_then(|v| v.to_str().ok()) == Some(self.host.as_str())
    }
    fn admin(&self, headers: &HeaderMap) -> ApiResult<()> {
        // Browser cookies cannot mint enrollment capabilities or administer credentials.
        if headers.contains_key(header::ORIGIN)
            || headers.contains_key("sec-fetch-site")
            || headers
                .get(header::AUTHORIZATION)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.strip_prefix("Bearer "))
                != Some(self.token.as_str())
        {
            return Err(denied());
        }
        Ok(())
    }
    pub fn authorized(&self, headers: &HeaderMap) -> bool {
        if !self.host_matches(headers) {
            return false;
        }
        if let Some(origin) = headers.get(header::ORIGIN) {
            if origin.to_str().ok() != Some(self.origin.as_str()) {
                return false;
            }
        }
        let Some(value) = cookie(headers, SESSION_COOKIE) else {
            return false;
        };
        let hash = digest(value);
        self.inner
            .lock()
            .unwrap()
            .saved
            .sessions
            .iter()
            .any(|s| s.hash == hash && s.expires > now())
    }
    pub fn lease(self: &Arc<Self>, headers: &HeaderMap) -> BrowserLease {
        BrowserLease {
            auth: self.clone(),
            hash: digest(cookie(headers, SESSION_COOKIE).unwrap_or("")),
        }
    }
    fn set_cookie(&self, name: &str, value: &str, lifetime: u64) -> HeaderValue {
        HeaderValue::from_str(&format!(
            "{name}={value}; HttpOnly; SameSite=Strict; Path=/; Max-Age={lifetime}{}",
            if self.secure { "; Secure" } else { "" }
        ))
        .unwrap()
    }
    fn session(&self, saved: &mut Saved, credential: &str) -> String {
        let time = now();
        saved.sessions.retain(|s| s.expires > time);
        // Bound durable session state for this single-operator server.
        if saved.sessions.len() >= 128 {
            saved.sessions.remove(0);
        }
        let value = secret();
        saved.sessions.push(BrowserSession {
            id: Uuid::new_v4().to_string(),
            credential: credential.to_owned(),
            hash: digest(&value),
            created: time,
            expires: time + SESSION_SECONDS,
        });
        value
    }
    fn signed_in(&self, value: &str) -> (HeaderMap, Json<Value>) {
        let mut headers = HeaderMap::new();
        headers.append(
            header::SET_COOKIE,
            self.set_cookie(SESSION_COOKIE, value, SESSION_SECONDS),
        );
        headers.append(header::SET_COOKIE, self.set_cookie(FLOW_COOKIE, "", 0));
        // A prior shared-token cookie must not bypass revocation of this session.
        headers.append(header::SET_COOKIE, self.set_cookie("boid_token", "", 0));
        (headers, Json(json!({"ok":true})))
    }
    pub fn router(self: &Arc<Self>) -> Router {
        Router::new()
            .route("/auth/passkey/status", get(status))
            .route("/auth/passkey/enroll", post(enroll))
            .route("/auth/passkey/register/start", post(register_start))
            .route("/auth/passkey/register/finish", post(register_finish))
            .route("/auth/passkey/sign-in/start", post(sign_in_start))
            .route("/auth/passkey/sign-in/finish", post(sign_in_finish))
            .route("/auth/passkey/logout", post(logout))
            .route("/auth/passkey/access", get(access))
            .route("/auth/passkey/access/{id}", axum::routing::delete(revoke))
            .layer(DefaultBodyLimit::max(64 * 1024))
            .with_state(self.clone())
    }
}
/// A browser's long-lived transports stop when its session expires or is revoked.
/// Dropping a transport never stops the server-owned work it was observing.
#[derive(Clone)]
pub(crate) struct BrowserLease {
    auth: Arc<BrowserAuth>,
    hash: String,
}
impl BrowserLease {
    pub async fn cancelled(&self) {
        let mut changed = self.auth.changed.subscribe();
        loop {
            let expires = self
                .auth
                .inner
                .lock()
                .unwrap()
                .saved
                .sessions
                .iter()
                .find(|s| s.hash == self.hash)
                .map(|s| s.expires);
            let Some(remaining) = expires
                .and_then(|expires| expires.checked_sub(now()))
                .filter(|remaining| *remaining > 0)
            else {
                return;
            };
            tokio::select! {
                _ = changed.changed() => {},
                _ = tokio::time::sleep(std::time::Duration::from_secs(remaining)) => {},
            }
        }
    }
}
pub(crate) async fn revoked(lease: Option<axum::Extension<BrowserLease>>) {
    match lease {
        Some(lease) => lease.cancelled().await,
        None => std::future::pending::<()>().await,
    }
}
async fn status(State(auth): State<Arc<BrowserAuth>>, headers: HeaderMap) -> Json<Value> {
    let enabled =
        auth.host_matches(&headers) && !auth.inner.lock().unwrap().saved.credentials.is_empty();
    Json(json!({"available":enabled, "origin":auth.origin}))
}
async fn enroll(
    State(auth): State<Arc<BrowserAuth>>,
    headers: HeaderMap,
) -> ApiResult<Json<Value>> {
    auth.admin(&headers)?;
    let mut inner = auth.inner.lock().unwrap();
    inner.capacity()?;
    let capability = secret();
    inner
        .enrollments
        .insert(digest(&capability), now() + FLOW_SECONDS);
    Ok(Json(
        json!({"url":format!("{}/#enroll={capability}", auth.origin), "expires_in":FLOW_SECONDS}),
    ))
}
#[derive(Deserialize)]
struct Enrollment {
    capability: String,
}
async fn register_start(
    State(auth): State<Arc<BrowserAuth>>,
    headers: HeaderMap,
    Json(input): Json<Enrollment>,
) -> ApiResult<(HeaderMap, Json<Value>)> {
    auth.browser_request(&headers)?;
    let mut inner = auth.inner.lock().unwrap();
    inner.prune();
    inner
        .enrollments
        .remove(&digest(&input.capability))
        .ok_or_else(denied)?;
    if inner.saved.credentials.len() >= 32 {
        return Err(Failure(
            StatusCode::CONFLICT,
            "Revoke an existing credential before adding another.",
        ));
    }
    let excluded = inner
        .saved
        .credentials
        .iter()
        .map(|c| c.passkey.cred_id().clone())
        .collect();
    let (options, challenge) = auth
        .webauthn
        .start_passkey_registration(inner.saved.user, "boid", "Boid", Some(excluded))
        .map_err(internal)?;
    let flow = secret();
    inner.pending.insert(
        digest(&flow),
        Pending {
            expires: now() + FLOW_SECONDS,
            challenge: Challenge::Register(challenge),
        },
    );
    let mut headers = HeaderMap::new();
    headers.insert(
        header::SET_COOKIE,
        auth.set_cookie(FLOW_COOKIE, &flow, FLOW_SECONDS),
    );
    Ok((
        headers,
        Json(serde_json::to_value(options).map_err(internal)?),
    ))
}
async fn register_finish(
    State(auth): State<Arc<BrowserAuth>>,
    headers: HeaderMap,
    Json(input): Json<RegisterPublicKeyCredential>,
) -> ApiResult<(HeaderMap, Json<Value>)> {
    auth.browser_request(&headers)?;
    let flow = cookie(&headers, FLOW_COOKIE).ok_or_else(denied)?;
    let mut inner = auth.inner.lock().unwrap();
    inner.prune();
    let pending = inner.pending.remove(&digest(flow)).ok_or_else(denied)?;
    let Challenge::Register(challenge) = pending.challenge else {
        return Err(denied());
    };
    let passkey = auth
        .webauthn
        .finish_passkey_registration(&input, &challenge)
        .map_err(|_| denied())?;
    if inner.saved.credentials.len() >= 32
        || inner
            .saved
            .credentials
            .iter()
            .any(|c| c.passkey.cred_id() == passkey.cred_id())
    {
        return Err(denied());
    }
    let mut saved = inner.saved.clone();
    let id = Uuid::new_v4().to_string();
    saved.credentials.push(Credential {
        id: id.clone(),
        created: now(),
        passkey,
    });
    let value = auth.session(&mut saved, &id);
    auth.save(&saved).map_err(internal)?;
    inner.saved = saved;
    auth.changed.send_replace(());
    Ok(auth.signed_in(&value))
}
async fn sign_in_start(
    State(auth): State<Arc<BrowserAuth>>,
    headers: HeaderMap,
) -> ApiResult<(HeaderMap, Json<Value>)> {
    auth.browser_request(&headers)?;
    let mut inner = auth.inner.lock().unwrap();
    inner.capacity()?;
    if inner.saved.credentials.is_empty() {
        return Err(Failure(
            StatusCode::UNAUTHORIZED,
            "Authorize passkey enrollment through SSH first.",
        ));
    }
    let passkeys: Vec<_> = inner
        .saved
        .credentials
        .iter()
        .map(|c| c.passkey.clone())
        .collect();
    let (options, challenge) = auth
        .webauthn
        .start_passkey_authentication(&passkeys)
        .map_err(internal)?;
    let flow = secret();
    inner.pending.insert(
        digest(&flow),
        Pending {
            expires: now() + FLOW_SECONDS,
            challenge: Challenge::Authenticate(challenge),
        },
    );
    let mut headers = HeaderMap::new();
    headers.insert(
        header::SET_COOKIE,
        auth.set_cookie(FLOW_COOKIE, &flow, FLOW_SECONDS),
    );
    Ok((
        headers,
        Json(serde_json::to_value(options).map_err(internal)?),
    ))
}
async fn sign_in_finish(
    State(auth): State<Arc<BrowserAuth>>,
    headers: HeaderMap,
    Json(input): Json<PublicKeyCredential>,
) -> ApiResult<(HeaderMap, Json<Value>)> {
    auth.browser_request(&headers)?;
    let flow = cookie(&headers, FLOW_COOKIE).ok_or_else(denied)?;
    let mut inner = auth.inner.lock().unwrap();
    inner.prune();
    let pending = inner.pending.remove(&digest(flow)).ok_or_else(denied)?;
    let Challenge::Authenticate(challenge) = pending.challenge else {
        return Err(denied());
    };
    let result = auth
        .webauthn
        .finish_passkey_authentication(&input, &challenge)
        .map_err(|_| denied())?;
    let mut saved = inner.saved.clone();
    // Resolve against current credentials too: revocation invalidates in-flight challenges.
    let credential = saved
        .credentials
        .iter_mut()
        .find(|c| c.passkey.cred_id() == result.cred_id())
        .ok_or_else(denied)?;
    credential.passkey.update_credential(&result);
    let id = credential.id.clone();
    let value = auth.session(&mut saved, &id);
    auth.save(&saved).map_err(internal)?;
    inner.saved = saved;
    auth.changed.send_replace(());
    Ok(auth.signed_in(&value))
}
async fn logout(
    State(auth): State<Arc<BrowserAuth>>,
    headers: HeaderMap,
) -> ApiResult<(HeaderMap, Json<Value>)> {
    auth.browser_request(&headers)?;
    let hash = cookie(&headers, SESSION_COOKIE).map(digest);
    let mut inner = auth.inner.lock().unwrap();
    let mut saved = inner.saved.clone();
    saved.sessions.retain(|s| Some(&s.hash) != hash.as_ref());
    auth.save(&saved).map_err(internal)?;
    inner.saved = saved;
    auth.changed.send_replace(());
    let mut headers = HeaderMap::new();
    headers.insert(header::SET_COOKIE, auth.set_cookie(SESSION_COOKIE, "", 0));
    Ok((headers, Json(json!({"ok":true}))))
}
async fn access(
    State(auth): State<Arc<BrowserAuth>>,
    headers: HeaderMap,
) -> ApiResult<Json<Value>> {
    auth.admin(&headers)?;
    let inner = auth.inner.lock().unwrap();
    Ok(Json(json!({
        "origin":auth.origin,
        "credentials":inner.saved.credentials.iter().map(|c| json!({"id":c.id,"created":c.created})).collect::<Vec<_>>(),
        "sessions":inner.saved.sessions.iter().filter(|s| s.expires > now()).map(|s| json!({"id":s.id,"credential":s.credential,"created":s.created,"expires":s.expires})).collect::<Vec<_>>()
    })))
}
async fn revoke(
    State(auth): State<Arc<BrowserAuth>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    auth.admin(&headers)?;
    let mut inner = auth.inner.lock().unwrap();
    let mut saved = inner.saved.clone();
    saved.credentials.retain(|c| c.id != id);
    saved.sessions.retain(|s| s.id != id && s.credential != id);
    auth.save(&saved).map_err(internal)?;
    inner.saved = saved;
    auth.changed.send_replace(());
    Ok(Json(json!({"ok":true})))
}

/// Run on the server through an authenticated SSH connection. Capabilities go
/// only to a newly created private file, never normal stdout/server logs.
pub async fn command(args: &[String]) -> Result<()> {
    let (method, path, output) = match args {
        [action, flag, path] if action == "enroll" && flag == "--output" => (
            reqwest::Method::POST,
            "auth/passkey/enroll".to_owned(),
            Some(path),
        ),
        [action] if action == "list" => {
            (reqwest::Method::GET, "auth/passkey/access".to_owned(), None)
        }
        [action, id] if action == "revoke" => {
            Uuid::parse_str(id)
                .context("Expected a credential or session ID from `boid passkey list`.")?;
            (
                reqwest::Method::DELETE,
                format!("auth/passkey/access/{id}"),
                None,
            )
        }
        _ => bail!(
            "Usage: boid passkey enroll --output PRIVATE_FILE | boid passkey list | boid passkey revoke ID"
        ),
    };
    // Do not start a new server or disclose its shared token while enrolling.
    let token = crate::server::read_token()?;
    let address = std::env::var("BOID_LISTEN").unwrap_or_else(|_| "127.0.0.1:7734".into());
    let mut url = Url::parse(&format!("http://{address}"))?;
    ensure!(
        matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "[::1]")),
        "Passkey administration must run on the server through loopback (use SSH)."
    );
    url.set_path(&path);
    let response = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(5))
        .build()?
        .request(method, url)
        .bearer_auth(token)
        .send()
        .await?;
    ensure!(
        response.status().is_success(),
        "Passkey administration failed (HTTP {}). Check the server version and state directory.",
        response.status()
    );
    let body: Value = response.json().await?;
    if let Some(output) = output {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(output)
            .context("Use a new private output file for enrollment.")?;
        file.write_all(
            body["url"]
                .as_str()
                .context("Missing enrollment URL")?
                .as_bytes(),
        )?;
        file.write_all(b"\n")?;
        file.sync_all()?;
        println!("Enrollment link saved. It expires in five minutes and can be used once.");
    } else {
        println!("{}", serde_json::to_string_pretty(&body)?);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (tempfile::TempDir, Arc<BrowserAuth>) {
        let dir = tempfile::tempdir().unwrap();
        let auth = BrowserAuth::open_at(
            dir.path().to_owned(),
            "test-admin-token".into(),
            "http://localhost:7734",
        )
        .unwrap();
        (dir, auth)
    }
    fn browser() -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, HeaderValue::from_static("localhost:7734"));
        headers.insert(
            header::ORIGIN,
            HeaderValue::from_static("http://localhost:7734"),
        );
        headers
    }
    fn admin() -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("Bearer test-admin-token"),
        );
        headers
    }
    #[test]
    fn exact_origin_and_ssh_only_administration() {
        let (_dir, auth) = fixture();
        assert!(auth.browser_request(&browser()).is_ok());
        let mut headers = browser();
        headers.insert(
            header::ORIGIN,
            HeaderValue::from_static("http://localhost:7735"),
        );
        assert!(auth.browser_request(&headers).is_err());
        headers.insert(header::HOST, HeaderValue::from_static("evil.example"));
        headers.insert(
            header::ORIGIN,
            HeaderValue::from_static("http://evil.example"),
        );
        assert!(auth.browser_request(&headers).is_err());
        assert!(auth.admin(&admin()).is_ok());
        let mut headers = admin();
        headers.insert(
            header::ORIGIN,
            HeaderValue::from_static("http://localhost:7734"),
        );
        assert!(auth.admin(&headers).is_err());
        assert!(auth.admin(&browser()).is_err());
        let dir = tempfile::tempdir().unwrap();
        for origin in [
            "http://127.0.0.1:7734",
            "http://boid.example",
            "https://boid.example/path",
            "https://user@boid.example",
        ] {
            assert!(BrowserAuth::open_at(dir.path().to_owned(), "token".into(), origin).is_err());
        }
    }
    #[tokio::test]
    async fn enrollment_is_single_use_expiring_and_flow_bound() {
        let (_dir, auth) = fixture();
        let Json(body) = enroll(State(auth.clone()), admin())
            .await
            .unwrap_or_else(|_| panic!("enroll"));
        let capability = body["url"]
            .as_str()
            .unwrap()
            .split_once("#enroll=")
            .unwrap()
            .1
            .to_owned();
        let (headers, Json(options)) = register_start(
            State(auth.clone()),
            browser(),
            Json(Enrollment {
                capability: capability.clone(),
            }),
        )
        .await
        .unwrap_or_else(|_| panic!("start"));
        assert!(headers.contains_key(header::SET_COOKIE));
        assert_eq!(
            options["publicKey"]["authenticatorSelection"]["userVerification"],
            "required"
        );
        assert!(
            register_start(
                State(auth.clone()),
                browser(),
                Json(Enrollment { capability })
            )
            .await
            .is_err()
        );
        auth.inner
            .lock()
            .unwrap()
            .enrollments
            .insert(digest("expired"), now() - 1);
        assert!(
            register_start(
                State(auth.clone()),
                browser(),
                Json(Enrollment {
                    capability: "expired".into()
                })
            )
            .await
            .is_err()
        );
        assert_eq!(auth.inner.lock().unwrap().pending.len(), 1);
        // A fresh instance retains no outstanding capability or challenge.
        let reopened = BrowserAuth::open_at(
            auth.file.parent().unwrap().to_owned(),
            "test-admin-token".into(),
            &auth.origin,
        )
        .unwrap();
        assert!(reopened.inner.lock().unwrap().pending.is_empty());
    }
    #[tokio::test]
    async fn leases_handle_revocation_races_and_other_sessions() {
        let (_dir, auth) = fixture();
        let value = {
            let mut inner = auth.inner.lock().unwrap();
            auth.session(&mut inner.saved, "first")
        };
        let mut headers = browser();
        headers.insert(
            header::COOKIE,
            HeaderValue::from_str(&format!("{SESSION_COOKIE}={value}")).unwrap(),
        );
        let lease = auth.lease(&headers);
        {
            let mut inner = auth.inner.lock().unwrap();
            auth.session(&mut inner.saved, "second");
        }
        auth.changed.send_replace(());
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(10), lease.cancelled())
                .await
                .is_err()
        );
        // Expiry or revocation before the watch subscribes is still observed.
        auth.inner.lock().unwrap().saved.sessions[0].expires = now() - 1;
        auth.changed.send_replace(());
        assert!(
            tokio::time::timeout(std::time::Duration::from_secs(1), lease.cancelled())
                .await
                .is_ok()
        );
    }
    #[tokio::test]
    async fn sessions_are_private_durable_expiring_and_revocable() {
        let (dir, auth) = fixture();
        let value;
        let id;
        {
            let mut inner = auth.inner.lock().unwrap();
            value = auth.session(&mut inner.saved, "credential-id");
            id = inner.saved.sessions[0].id.clone();
            auth.save(&inner.saved).unwrap();
        }
        let bytes = std::fs::read_to_string(&auth.file).unwrap();
        assert!(!bytes.contains(&value));
        assert_eq!(
            std::fs::metadata(&auth.file).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let mut headers = browser();
        headers.insert(
            header::COOKIE,
            HeaderValue::from_str(&format!("{SESSION_COOKIE}={value}")).unwrap(),
        );
        let reopened = BrowserAuth::open_at(
            dir.path().to_owned(),
            "test-admin-token".into(),
            &auth.origin,
        )
        .unwrap();
        assert!(reopened.authorized(&headers));
        revoke(State(reopened.clone()), admin(), Path(id))
            .await
            .unwrap_or_else(|_| panic!("revoke"));
        assert!(!reopened.authorized(&headers));
        let again = BrowserAuth::open_at(
            dir.path().to_owned(),
            "test-admin-token".into(),
            &auth.origin,
        )
        .unwrap();
        assert!(!again.authorized(&headers));
        auth.inner.lock().unwrap().saved.sessions[0].expires = now() - 1;
        assert!(!auth.authorized(&headers));
        assert!(
            BrowserAuth::open_at(
                dir.path().to_owned(),
                "test-admin-token".into(),
                "http://localhost:7735"
            )
            .is_err()
        );
    }
}
