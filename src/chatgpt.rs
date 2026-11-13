//! ChatGPT plan credentials stay in the server's private state directory.
use anyhow::{Context, Result, bail};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header, jwk::JwkSet};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::Write,
    os::{
        fd::AsRawFd,
        unix::fs::{OpenOptionsExt, PermissionsExt},
    },
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use uuid::Uuid;

#[path = "credentials.rs"]
mod credentials;

const ISSUER: &str = "https://auth.openai.com";
const RESOURCE: &str = "https://api.openai.com/v1";
const SCOPES: &str =
    "openid profile email offline_access resource.invoke chatgpt.tokens.use.direct";
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
fn random() -> String {
    format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple())
}

#[derive(Clone, Serialize, Deserialize)]
struct Account {
    id: String,
    #[serde(default)]
    name: String,
    client_id: String,
    subject: String,
    email: String,
    #[serde(default)]
    welcomed: bool,
    #[serde(default)]
    paused: bool,
    #[serde(default)]
    access_token: String,
    #[serde(default)]
    refresh_token: String,
    #[serde(default)]
    id_token: String,
    #[serde(default)]
    expires_at: u64,
    #[serde(default)]
    scopes: Vec<String>,
}
impl Account {
    fn allowed(&self) -> bool {
        self.scopes.iter().any(|s| s == "chatgpt.tokens.use.direct")
    }
    fn clear(&mut self) {
        self.access_token.clear();
        self.refresh_token.clear();
        self.id_token.clear();
        self.expires_at = 0;
    }
}
#[derive(Clone, Serialize, Deserialize)]
struct Store {
    host_id: String,
    route: String,
    selected: Option<String>,
    model: String,
    accounts: BTreeMap<String, Account>,
    #[serde(default)]
    api_key: String,
    #[serde(default)]
    registry_migrated: bool,
    #[serde(default)]
    revision: u64,
    #[serde(default)]
    default_credential_id: Option<String>,
    #[serde(default)]
    legacy_api_credential_id: Option<String>,
    #[serde(default)]
    api_keys: BTreeMap<String, credentials::ApiKey>,
}
struct Attempt {
    name: Option<String>,
    state: String,
    nonce: String,
    verifier: String,
    redirect: String,
    account: Option<Account>,
    expires: u64,
    authorization: String,
    opened: bool,
}
#[derive(Clone, Deserialize)]
struct Discovery {
    issuer: String,
    jwks_uri: String,
    revocation_endpoint: String,
}
#[derive(Deserialize)]
struct Claims {
    sub: String,
    #[serde(default)]
    email: String,
    #[serde(default)]
    nonce: String,
}

pub struct Auth {
    directory: PathBuf,
    http: reqwest::Client,
    attempts: Mutex<BTreeMap<String, Attempt>>,
    issuer: String,
    api: String,
}
impl Auth {
    pub fn open(directory: PathBuf, route: &str, model: &str) -> Result<Self> {
        if !matches!(route, "api_key" | "chatgpt") {
            bail!("BOID_AUTH_ROUTE must be api_key or chatgpt.");
        }
        fs::create_dir_all(&directory)?;
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))?;
        let auth = Self {
            directory,
            http: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .connect_timeout(Duration::from_secs(15))
                .timeout(Duration::from_secs(30))
                .build()?,
            attempts: Mutex::new(BTreeMap::new()),
            issuer: ISSUER.into(),
            api: RESOURCE.into(),
        };
        // Initialization is synchronous and precedes server startup.
        let lock = auth.lock_file()?;
        if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX) } != 0 {
            bail!("Cannot lock ChatGPT credentials.");
        }
        if !auth.path().exists() {
            auth.save(&Store {
                host_id: format!("urn:uuid:{}", Uuid::new_v4()),
                route: route.into(),
                selected: None,
                model: model.into(),
                accounts: BTreeMap::new(),
                api_key: String::new(),
                registry_migrated: false,
                revision: 0,
                default_credential_id: None,
                legacy_api_credential_id: None,
                api_keys: BTreeMap::new(),
            })?;
        }
        auth.load()?;
        Ok(auth)
    }
    fn path(&self) -> PathBuf {
        self.directory.join("accounts.json")
    }
    fn load(&self) -> Result<Store> {
        let metadata =
            fs::symlink_metadata(self.path()).context("Cannot read ChatGPT account storage.")?;
        if !metadata.is_file() || metadata.permissions().mode() & 0o077 != 0 {
            bail!("ChatGPT account storage must be an owner-only regular file (0600).");
        }
        serde_json::from_slice(&fs::read(self.path())?).context("Invalid ChatGPT account storage.")
    }
    fn save(&self, store: &Store) -> Result<()> {
        let mut file = tempfile::NamedTempFile::new_in(&self.directory)?;
        file.as_file()
            .set_permissions(fs::Permissions::from_mode(0o600))?;
        file.write_all(&serde_json::to_vec(store)?)?;
        file.as_file().sync_all()?;
        file.persist(self.path()).map_err(|e| e.error)?;
        File::open(&self.directory)?.sync_all()?;
        Ok(())
    }
    fn lock_file(&self) -> Result<File> {
        Ok(OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(self.directory.join("credentials.lock"))?)
    }
    async fn lock(&self) -> Result<File> {
        let file = self.lock_file()?;
        loop {
            if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
                return Ok(file);
            }
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() != Some(libc::EWOULDBLOCK) {
                return Err(error.into());
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
    pub fn api_key(&self) -> Result<Option<String>> {
        let store = self.load()?;
        if let Some(id) = &store.default_credential_id {
            if let Some(key) = store.api_keys.get(id) {
                return Ok(Some(key.key.clone()));
            }
        }
        Ok((!store.api_key.is_empty()).then_some(store.api_key))
    }
    pub async fn save_api_key(&self, key: &str) -> Result<()> {
        let created = self.create_credential("OpenAI API", key).await?;
        self.select_credential_default(created["id"].as_str().unwrap())
            .await
    }
    pub fn profile(&self) -> Result<Value> {
        let store = self.load()?;
        if !matches!(store.route.as_str(), "api_key" | "chatgpt") {
            bail!("Invalid saved billing route.");
        }
        Ok(
            json!({"billing_route":store.route,"chatgpt_account":store.selected,"model":store.model,
                "credential_id":store.default_credential_id}),
        )
    }
    pub fn status(&self, api_key_available: bool) -> Result<Value> {
        let store = self.load()?;
        let accounts: Vec<_> = store.accounts.values().map(|a| json!({"id":a.id,
            "label":format!("{} · {}", if a.email.is_empty() { "ChatGPT" } else { &a.email }, a.id.chars().take(8).collect::<String>()),
            "connected":!a.id_token.is_empty() || !a.refresh_token.is_empty(),"plan_enabled":a.allowed(),"paused":a.paused})).collect();
        Ok(
            json!({"billing_route":store.route,"selected_account":store.selected,"model":store.model,
            "accounts":accounts,"api_key_available":if store.registry_migrated { !store.api_keys.is_empty() } else { api_key_available || !store.api_key.is_empty() },"usage_url":"https://chatgpt.com/settings/usage"}),
        )
    }
    pub async fn select(
        self: &Arc<Self>,
        route: &str,
        account: Option<&str>,
        model: &str,
    ) -> Result<bool> {
        if !matches!(route, "api_key" | "chatgpt") {
            bail!("Unknown billing route.");
        }
        if route == "chatgpt" {
            let account = account.context("Choose a ChatGPT account.")?;
            let models = self.models(account).await?;
            if !models.iter().any(|v| v["slug"] == model) {
                bail!("Choose a model available to this ChatGPT account.");
            }
        }
        let _lock = self.lock().await?;
        let mut store = self.load()?;
        if route == "chatgpt" {
            store.selected = account.map(str::to_owned);
            store.model = model.into();
        }
        let mut welcome = false;
        if route == "chatgpt" {
            let selected = store
                .accounts
                .get_mut(account.unwrap())
                .context("Unknown ChatGPT account.")?;
            if selected.refresh_token.is_empty() || !selected.allowed() {
                bail!("Reconnect and enable this ChatGPT account before selecting it.");
            }
            welcome = !selected.welcomed;
            selected.welcomed = true;
        }
        store.route = route.into();
        store.default_credential_id = if route == "chatgpt" {
            account.map(str::to_owned)
        } else {
            store.api_keys.keys().next().cloned()
        };
        store.revision += 1;
        self.save(&store)?;
        Ok(welcome)
    }
    pub async fn welcome(self: &Arc<Self>, id: &str) -> Result<bool> {
        let _lock = self.lock().await?;
        let mut store = self.load()?;
        let account = store
            .accounts
            .get_mut(id)
            .context("Unknown ChatGPT account.")?;
        let welcome = !account.welcomed;
        account.welcomed = true;
        self.save(&store)?;
        Ok(welcome)
    }
    pub fn begin(&self, origin: &str, selected: Option<&str>, consent: bool) -> Result<String> {
        let origin =
            reqwest::Url::parse(origin).context("Open Boid through a 127.0.0.1 URL to sign in.")?;
        if origin.scheme() != "http"
            || origin.host_str() != Some("127.0.0.1")
            || !origin.username().is_empty()
            || origin.password().is_some()
            || origin.query().is_some()
            || origin.fragment().is_some()
            || origin.path() != "/"
        {
            bail!(
                "ChatGPT sign-in needs an HTTP 127.0.0.1 URL; use an SSH loopback tunnel for a remote server."
            );
        }
        let store = self.load()?;
        let account = selected
            .map(|id| {
                store
                    .accounts
                    .get(id)
                    .cloned()
                    .context("Unknown ChatGPT account.")
            })
            .transpose()?;
        let state = random();
        let nonce = random();
        let verifier = random();
        let redirect = origin.join("auth/callback")?.to_string();
        let mut url = reqwest::Url::parse(&format!("{}/api/accounts/authorize", self.issuer))?;
        {
            let mut query = url.query_pairs_mut();
            query.extend_pairs([
                (
                    "client_id",
                    account
                        .as_ref()
                        .map(|a| a.client_id.as_str())
                        .unwrap_or("dynamic_agent_client"),
                ),
                ("ext_agent_host_id", &store.host_id),
                ("response_type", "code"),
                ("redirect_uri", &redirect),
                ("scope", SCOPES),
                ("resource", RESOURCE),
                ("state", &state),
                ("nonce", &nonce),
                ("code_challenge_method", "S256"),
                (
                    "code_challenge",
                    &URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes())),
                ),
            ]);
            if let Some(account) = &account {
                if !account.id_token.is_empty() {
                    query.append_pair("id_token_hint", &account.id_token);
                }
            } else {
                query.append_pair("agent_name_hint", "Boid");
            }
            if consent {
                query.append_pair("prompt", "consent");
            }
        }
        let ticket = random();
        let mut attempts = self.attempts.lock().unwrap();
        attempts.retain(|_, a| a.expires > now());
        if attempts.len() >= 8 {
            bail!("Finish or wait for an existing sign-in attempt before starting another.");
        }
        attempts.insert(
            ticket.clone(),
            Attempt {
                name: None,
                state,
                nonce,
                verifier,
                redirect,
                account,
                expires: now() + 600,
                authorization: url.to_string(),
                opened: false,
            },
        );
        Ok(format!("/auth/start?ticket={ticket}"))
    }
    pub fn authorization(&self, ticket: &str) -> Result<String> {
        let mut attempts = self.attempts.lock().unwrap();
        let a = attempts
            .get_mut(ticket)
            .filter(|a| a.expires > now() && !a.opened)
            .context("Sign-in link expired or was already opened.")?;
        a.opened = true;
        Ok(a.authorization.clone())
    }
    async fn discovery(&self) -> Result<Discovery> {
        let d: Discovery = self
            .http
            .get(format!("{}/.well-known/openid-configuration", self.issuer))
            .send()
            .await
            .context("Cannot reach ChatGPT sign-in discovery.")?
            .error_for_status()
            .context("ChatGPT discovery failed.")?
            .json()
            .await
            .context("Invalid ChatGPT discovery.")?;
        if d.issuer != self.issuer {
            bail!("Unexpected ChatGPT identity issuer.");
        }
        for endpoint in [&d.jwks_uri, &d.revocation_endpoint] {
            let parsed = reqwest::Url::parse(endpoint)?;
            let issuer = reqwest::Url::parse(&self.issuer)?;
            if parsed.origin() != issuer.origin() {
                bail!("Unexpected ChatGPT identity endpoint.");
            }
        }
        Ok(d)
    }
    async fn identity(&self, token: &str, client: &str, nonce: Option<&str>) -> Result<Claims> {
        let discovery = self.discovery().await?;
        let jwks: JwkSet = self
            .http
            .get(discovery.jwks_uri)
            .send()
            .await
            .context("Cannot retrieve ChatGPT signing keys.")?
            .error_for_status()
            .context("ChatGPT signing-key request failed.")?
            .json()
            .await
            .context("Invalid ChatGPT signing keys.")?;
        let header = decode_header(token).context("Invalid ChatGPT ID token.")?;
        if header.alg != Algorithm::RS256 {
            bail!("Unsupported ChatGPT ID-token algorithm.");
        }
        let key = jwks
            .find(
                header
                    .kid
                    .as_deref()
                    .context("ID token has no signing-key ID.")?,
            )
            .context("Unknown ChatGPT signing key.")?;
        let mut validation = Validation::new(Algorithm::RS256);
        validation.set_issuer(&[&self.issuer]);
        validation.set_audience(&[client]);
        validation.set_required_spec_claims(&["exp", "iss", "aud", "sub"]);
        validation.validate_nbf = true;
        validation.leeway = 30;
        let claims = decode::<Claims>(token, &DecodingKey::from_jwk(key)?, &validation)
            .context("ChatGPT ID-token validation failed.")?
            .claims;
        if claims.sub.is_empty() || nonce.is_some_and(|n| n != claims.nonce) {
            bail!("ChatGPT sign-in identity validation failed.");
        }
        Ok(claims)
    }
    async fn exchange(&self, form: &[(&str, &str)]) -> Result<(u16, Value)> {
        let response = self
            .http
            .post(format!("{}/api/accounts/oauth/token", self.issuer))
            .form(form)
            .send()
            .await
            .context("Cannot reach ChatGPT token service.")?;
        let status = response.status().as_u16();
        let body = response
            .json()
            .await
            .context("Invalid ChatGPT token response.")?;
        Ok((status, body))
    }
    pub async fn complete(&self, query: &BTreeMap<String, String>) -> Result<()> {
        let state = query.get("state").context("Missing sign-in state.")?;
        let attempt = {
            let mut attempts = self.attempts.lock().unwrap();
            let key = attempts
                .iter()
                .find(|(_, a)| a.state == *state && a.expires > now() && a.opened)
                .map(|(k, _)| k.clone())
                .context("Sign-in state is invalid or expired.")?;
            attempts.remove(&key).unwrap()
        };
        if query.contains_key("error") {
            bail!(
                "ChatGPT sign-in was declined or could not finish. Previous login selections are unchanged."
            );
        }
        let client = query
            .get("client_id")
            .cloned()
            .or_else(|| attempt.account.as_ref().map(|a| a.client_id.clone()))
            .context("Registration did not return an issued client ID.")?;
        if client == "dynamic_agent_client"
            || client.is_empty()
            || attempt
                .account
                .as_ref()
                .is_some_and(|a| a.client_id != client)
        {
            bail!("ChatGPT registration client ID mismatch.");
        }
        let (status, tokens) = self
            .exchange(&[
                ("grant_type", "authorization_code"),
                ("client_id", &client),
                (
                    "code",
                    query.get("code").context("Missing authorization code.")?,
                ),
                ("code_verifier", &attempt.verifier),
                ("redirect_uri", &attempt.redirect),
                ("resource", RESOURCE),
            ])
            .await?;
        if status != 200 {
            bail!("ChatGPT code exchange failed (HTTP {status}); start a fresh sign-in.");
        }
        let claims = self
            .identity(
                tokens["id_token"]
                    .as_str()
                    .context("ChatGPT did not return an ID token.")?,
                &client,
                Some(&attempt.nonce),
            )
            .await?;
        if attempt
            .account
            .as_ref()
            .is_some_and(|a| a.subject != claims.sub)
        {
            bail!("The signed-in ChatGPT identity differs from the selected account.");
        }
        let _lock = self.lock().await?;
        let mut store = self.load()?;
        if let Some(previous) = &attempt.account {
            if !store.accounts.contains_key(&previous.id) {
                bail!("This credential was deleted while sign-in was open. Start a new sign-in.");
            }
        }
        let mut account = attempt
            .account
            .as_ref()
            .and_then(|previous| store.accounts.get(&previous.id).cloned())
            .unwrap_or_else(|| Account {
                welcomed: false,
                paused: false,
                id: Uuid::new_v4().to_string(),
                name: String::new(),
                client_id: client.clone(),
                subject: claims.sub.clone(),
                email: claims.email.clone(),
                access_token: String::new(),
                refresh_token: String::new(),
                id_token: String::new(),
                expires_at: 0,
                scopes: vec![],
            });
        if store
            .accounts
            .values()
            .any(|a| a.client_id == client && a.id != account.id)
        {
            bail!("This ChatGPT registration is already saved; reconnect the saved account.");
        }
        account.email = claims.email;
        if let Some(name) = attempt.name {
            account.name = name;
        }
        if account.name.is_empty() {
            account.name = if account.email.is_empty() {
                "ChatGPT".into()
            } else {
                account.email.clone()
            };
        }
        account.paused = false;
        apply_tokens(&mut account, &tokens, false)?;
        store.accounts.insert(account.id.clone(), account);
        store.revision += 1;
        self.save(&store)
    }
    pub fn check_inference(&self, id: &str) -> Result<()> {
        let store = self.load()?;
        let account = store.accounts.get(id).context("Unknown ChatGPT account.")?;
        if account.refresh_token.is_empty() || !account.allowed() {
            bail!(
                "Reconnect and enable this ChatGPT account in /login; no API-key fallback was used."
            );
        }
        if account.paused {
            bail!(
                "ChatGPT requests are paused after a usage limit. Review https://chatgpt.com/settings/usage, then choose Resume requests in Boid with /login. No API-key fallback was used."
            );
        }
        Ok(())
    }
    pub async fn pause(&self, id: &str, paused: bool) -> Result<()> {
        let _lock = self.lock().await?;
        let mut store = self.load()?;
        store
            .accounts
            .get_mut(id)
            .context("Unknown ChatGPT account.")?
            .paused = paused;
        store.revision += 1;
        self.save(&store)
    }
    pub async fn token(self: &Arc<Self>, id: &str) -> Result<String> {
        self.bound_token(id, false, false)
            .await?
            .context("ChatGPT credential was removed.")
    }
    /// Deletion and refresh are serialized by the same lock. Only a retry may
    /// retain an already-issued bearer when its registry entry has been deleted.
    pub async fn inference_token(
        self: &Arc<Self>,
        id: &str,
        retry: bool,
    ) -> Result<Option<String>> {
        self.bound_token(id, retry, true).await
    }
    async fn bound_token(
        self: &Arc<Self>,
        id: &str,
        allow_deleted: bool,
        inference: bool,
    ) -> Result<Option<String>> {
        // A cancelled model turn must not discard a rotated refresh token.
        let auth = self.clone();
        let id = id.to_owned();
        tokio::spawn(async move { auth.token_inner(&id, allow_deleted, inference).await })
            .await
            .context("ChatGPT refresh task failed.")?
    }
    async fn token_inner(
        &self,
        id: &str,
        allow_deleted: bool,
        inference: bool,
    ) -> Result<Option<String>> {
        let _lock = self.lock().await?;
        let mut store = self.load()?;
        let Some(account) = store.accounts.get_mut(id) else {
            if allow_deleted {
                return Ok(None);
            }
            bail!("ChatGPT account is not saved. Continue with ChatGPT in /login.");
        };
        if inference && account.paused {
            bail!(
                "ChatGPT requests are paused after a usage limit. Resume this account in Boid; no API-key fallback was used."
            );
        }
        if !account.allowed() {
            bail!(
                "ChatGPT plan usage is disabled. Enable it in /login; no API-key fallback was used."
            );
        }
        if account.access_token.is_empty() || account.refresh_token.is_empty() {
            bail!("Reconnect this ChatGPT account in /login.");
        }
        if account.expires_at > now() + 60 {
            return Ok(Some(account.access_token.clone()));
        }
        let (status, tokens) = self
            .exchange(&[
                ("grant_type", "refresh_token"),
                ("client_id", &account.client_id),
                ("refresh_token", &account.refresh_token),
                ("resource", RESOURCE),
            ])
            .await?;
        if status != 200 {
            let code = tokens["error"]
                .as_str()
                .or_else(|| tokens["error"]["code"].as_str())
                .unwrap_or("");
            if matches!(
                code,
                "invalid_grant"
                    | "invalid_refresh_token"
                    | "token_expired"
                    | "refresh_token_expired"
                    | "refresh_token_invalidated"
                    | "refresh_token_reused"
            ) {
                account.clear();
                self.save(&store)?;
                bail!("ChatGPT authorization expired or was revoked. Reconnect in /login.");
            }
            bail!(
                "ChatGPT token refresh failed (HTTP {status}); credentials retained. No API-key fallback was used."
            );
        }
        if let Some(id_token) = tokens["id_token"].as_str() {
            let claims = self.identity(id_token, &account.client_id, None).await?;
            if claims.sub != account.subject {
                bail!("Refreshed ChatGPT identity does not match the saved account.");
            }
        }
        apply_tokens(account, &tokens, true)?;
        let access = account.access_token.clone();
        let allowed = account.allowed();
        self.save(&store)?;
        if !allowed {
            bail!("ChatGPT plan permission was removed. Enable it in /login.");
        }
        Ok(Some(access))
    }
    pub async fn models(self: &Arc<Self>, id: &str) -> Result<Vec<Value>> {
        let token = self.token(id).await?;
        let response = self
            .http
            .get(format!("{}/models", self.api))
            .bearer_auth(&token)
            .send()
            .await
            .context("Cannot retrieve ChatGPT models.")?;
        if !response.status().is_success() {
            bail!(
                "ChatGPT model catalog returned HTTP {}. Reconnect or retry later; no API-key fallback was used.",
                response.status()
            );
        }
        let body: Value = response
            .json()
            .await
            .context("Invalid ChatGPT model catalog.")?;
        Ok(body["models"]
            .as_array()
            .context("ChatGPT model catalog has no models list.")?
            .iter()
            .filter(|m| m["visibility"] == "list")
            .filter_map(|m| {
                Some(json!({"slug":m["slug"].as_str()?,"display_name":m["display_name"].as_str()?}))
            })
            .collect())
    }
    pub async fn logout(&self, id: &str) -> Result<bool> {
        let _lock = self.lock().await?;
        let mut store = self.load()?;
        let account = store
            .accounts
            .get_mut(id)
            .context("Unknown ChatGPT account.")?;
        let mut confirmed = account.refresh_token.is_empty() && account.access_token.is_empty();
        if !confirmed && !account.refresh_token.is_empty() {
            if let Ok(d) = self.discovery().await {
                for delay in [0, 250, 750] {
                    tokio::time::sleep(Duration::from_millis(delay)).await;
                    let result = self
                        .http
                        .post(&d.revocation_endpoint)
                        .form(&[
                            ("token", account.refresh_token.as_str()),
                            ("token_type_hint", "refresh_token"),
                            ("client_id", account.client_id.as_str()),
                        ])
                        .send()
                        .await;
                    match result {
                        Ok(r) if r.status().as_u16() == 200 => {
                            confirmed = true;
                            break;
                        }
                        Ok(r) if !r.status().is_server_error() => break,
                        _ => (),
                    }
                }
            }
        }
        account.clear();
        self.save(&store)?;
        Ok(confirmed)
    }
}
fn apply_tokens(account: &mut Account, tokens: &Value, refresh: bool) -> Result<()> {
    if tokens["token_type"]
        .as_str()
        .is_none_or(|s| !s.eq_ignore_ascii_case("Bearer"))
    {
        bail!("Unsupported ChatGPT token type.");
    }
    let access = tokens["access_token"]
        .as_str()
        .filter(|s| !s.is_empty())
        .context("ChatGPT returned no access token.")?;
    let renewed = tokens["refresh_token"].as_str().unwrap_or("");
    let expires = tokens["expires_in"]
        .as_u64()
        .filter(|v| *v > 0 && *v <= 86400)
        .context("Invalid ChatGPT token expiry.")?;
    let scopes = match tokens["scope"].as_str() {
        Some(s) => s.split_whitespace().map(str::to_owned).collect(),
        None if refresh => account.scopes.clone(),
        None => bail!("ChatGPT did not report granted scopes."),
    };
    // Preserve a validated identity-only grant even when offline access was declined.
    if scopes
        .iter()
        .any(|scope| scope == "chatgpt.tokens.use.direct")
        && renewed.is_empty()
    {
        bail!("ChatGPT returned no renewable session for plan usage.");
    }
    account.access_token = access.into();
    account.refresh_token = renewed.into();
    account.expires_at = now() + expires;
    account.scopes = scopes;
    if let Some(token) = tokens["id_token"].as_str() {
        account.id_token = token.into();
    }
    Ok(())
}

#[cfg(test)]
#[path = "chatgpt_tests.rs"]
mod tests;
