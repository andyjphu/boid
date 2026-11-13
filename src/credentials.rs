//! Named, server-owned credentials. Public projections never contain key/token material.
use super::*;

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct ApiKey {
    name: String,
    pub(super) key: String,
}

fn name(value: &str) -> Result<String> {
    let value = value.trim();
    if value.is_empty() || value.chars().count() > 120 || value.chars().any(char::is_control) {
        bail!("Credential name must contain 1 to 120 characters without control characters.");
    }
    Ok(value.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn named_registry_migrates_once_and_never_restores_deleted_env_key() {
        let dir = tempfile::tempdir().unwrap();
        let auth = Auth::open(dir.path().join("auth"), "api_key", "fixture-model").unwrap();
        let mut legacy = auth.load().unwrap();
        legacy.api_key = "fixture-config-key".into();
        auth.save(&legacy).unwrap();
        auth.migrate_credentials("fixture-env-key").unwrap();
        let original = auth.credentials().unwrap();
        assert_eq!(original["credentials"].as_array().unwrap().len(), 2);
        let id = original["default_credential_id"]
            .as_str()
            .unwrap()
            .to_owned();
        assert_eq!(auth.named_api_key(&id).unwrap(), "fixture-config-key");
        assert_eq!(
            auth.legacy_credential(&json!({"billing_route":"api_key"}))
                .unwrap(),
            Some(id.clone())
        );
        auth.edit_credential(&id, Some("alpha"), Some("fixture-rekeyed"))
            .await
            .unwrap();
        let extra = auth
            .create_credential("Zulu", "fixture-extra")
            .await
            .unwrap();
        assert_ne!(id, extra["id"]);
        let (automatic_one, automatic_two) = tokio::join!(
            auth.create_credential("", "fixture-auto-one"),
            auth.create_credential("   ", "fixture-auto-two")
        );
        let one = automatic_one.unwrap()["id"].as_str().unwrap().to_owned();
        let two = automatic_two.unwrap()["id"].as_str().unwrap().to_owned();
        assert_ne!(one, two);
        let registry = auth.credentials().unwrap();
        let names: Vec<_> = registry["credentials"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|row| row["id"] == one || row["id"] == two)
            .map(|row| row["name"].as_str().unwrap())
            .collect();
        // These are public display labels, not credential values.
        let expected_names: Vec<_> = (1..=2).map(|n| format!("openai-api-key-{n}")).collect();
        assert_eq!(names, expected_names);
        let public = auth.credentials().unwrap();
        assert_eq!(public["credentials"][0]["name"], "alpha");
        for secret in [
            "fixture-config-key",
            "fixture-env-key",
            "fixture-rekeyed",
            "fixture-extra",
        ] {
            assert!(!public.to_string().contains(secret));
        }
        for row in public["credentials"].as_array().unwrap() {
            auth.delete_credential(row["id"].as_str().unwrap())
                .await
                .unwrap();
        }
        let reopened = Auth::open(auth.directory.clone(), "api_key", "other").unwrap();
        reopened.migrate_credentials("fixture-env-key").unwrap();
        assert!(
            reopened.credentials().unwrap()["credentials"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        assert!(reopened.profile().unwrap()["credential_id"].is_null());
        assert!(reopened.named_api_key(&id).is_err());
        assert!(reopened.api_key().unwrap().is_none());
        assert_eq!(
            fs::metadata(reopened.path()).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[tokio::test]
    async fn request_keeps_original_api_key_across_rekey_and_delete_during_retry() {
        use axum::{
            Json, Router,
            extract::State,
            http::{HeaderMap, StatusCode},
            response::IntoResponse,
            routing::post,
        };
        #[derive(Clone)]
        struct Fixture {
            auth: Arc<Auth>,
            id: String,
            seen: Arc<Mutex<Vec<String>>>,
        }
        async fn reply(State(f): State<Fixture>, headers: HeaderMap) -> axum::response::Response {
            let count = {
                let mut seen = f.seen.lock().unwrap();
                seen.push(headers["authorization"].to_str().unwrap().to_owned());
                seen.len()
            };
            if count == 1 {
                f.auth
                    .edit_credential(&f.id, None, Some("fixture-new-secret"))
                    .await
                    .unwrap();
                f.auth.delete_credential(&f.id).await.unwrap();
                return (
                    StatusCode::SERVICE_UNAVAILABLE,
                    Json(json!({"error":{"message":"fixture-original-secret unavailable"}})),
                )
                    .into_response();
            }
            (
                [("content-type", "text/event-stream")],
                "data: {\"type\":\"response.completed\",\"response\":{\"output\":[]}}\n\n",
            )
                .into_response()
        }
        let dir = tempfile::tempdir().unwrap();
        let auth =
            Arc::new(Auth::open(dir.path().join("auth"), "api_key", "fixture-model").unwrap());
        auth.migrate_credentials("fixture-original-secret").unwrap();
        let id = auth.profile().unwrap()["credential_id"]
            .as_str()
            .unwrap()
            .to_owned();
        let fixture = Fixture {
            auth: auth.clone(),
            id,
            seen: Default::default(),
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}/responses", listener.local_addr().unwrap());
        let router = Router::new()
            .route("/responses", post(reply))
            .with_state(fixture.clone());
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let mut client = crate::api::Client::test_client_at(endpoint);
        client.auth = Some(auth);
        let profile = client.config();
        let mut events = Vec::new();
        client
            .respond_with(
                &[] as &[Value],
                json!([]),
                "fixture-model",
                "fixture",
                &profile,
                |e| events.push(e.clone()),
            )
            .await
            .unwrap();
        assert_eq!(
            *fixture.seen.lock().unwrap(),
            vec![
                "Bearer fixture-original-secret",
                "Bearer fixture-original-secret"
            ]
        );
        assert!(
            !json!(events)
                .to_string()
                .contains("fixture-original-secret")
        );
        assert!(
            client
                .respond_with(
                    &[] as &[Value],
                    json!([]),
                    "fixture-model",
                    "fixture",
                    &profile,
                    |_| {}
                )
                .await
                .is_err()
        );
        assert_eq!(fixture.seen.lock().unwrap().len(), 2);
        server.abort();
    }
}
fn key(value: &str) -> Result<String> {
    let value = value.trim();
    if value.is_empty() || value.len() > 4096 || value.chars().any(char::is_whitespace) {
        bail!("Enter a non-empty OpenAI API key without whitespace.");
    }
    Ok(value.to_owned())
}
impl Auth {
    /// Runs once before workers start. An explicit deletion is never undone by env fallback.
    pub fn migrate_credentials(&self, environment_key: &str) -> Result<()> {
        let lock = self.lock_file()?;
        if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX) } != 0 {
            bail!("Cannot lock credentials for migration.");
        }
        let mut store = self.load()?;
        if store.registry_migrated {
            return Ok(());
        }
        let legacy = std::mem::take(&mut store.api_key);
        let mut first = None;
        for (label, secret) in [
            ("OpenAI API", legacy.as_str()),
            ("OpenAI API (environment)", environment_key.trim()),
        ] {
            if secret.is_empty() || store.api_keys.values().any(|v| v.key == secret) {
                continue;
            }
            let id = Uuid::new_v4().to_string();
            store.api_keys.insert(
                id.clone(),
                ApiKey {
                    name: label.into(),
                    key: key(secret)?,
                },
            );
            if first.is_none() {
                first = Some(id);
            }
        }
        for account in store.accounts.values_mut() {
            if account.name.is_empty() {
                account.name = if account.email.is_empty() {
                    "ChatGPT".into()
                } else {
                    account.email.clone()
                };
            }
        }
        store.legacy_api_credential_id = first.clone();
        store.default_credential_id = if store.route == "chatgpt" {
            store.selected.clone()
        } else {
            first
        };
        store.registry_migrated = true;
        store.revision += 1;
        self.save(&store)
    }
    pub fn credentials(&self) -> Result<Value> {
        let store = self.load()?;
        let mut rows: Vec<Value> = store.api_keys.iter().map(|(id, key)| json!({
            "id":id,"type":"api_key","name":key.name,"connected":true,"paused":false,"plan_enabled":false
        })).chain(store.accounts.values().map(|a| json!({
            "id":a.id,"type":"chatgpt","name":a.name,
            "connected":!a.refresh_token.is_empty(),"paused":a.paused,"plan_enabled":a.allowed()
        }))).collect();
        rows.sort_by_key(|r| {
            (
                r["name"].as_str().unwrap_or("").to_lowercase(),
                r["id"].as_str().unwrap_or("").to_owned(),
            )
        });
        Ok(
            json!({"revision":store.revision,"credentials":rows,"default_credential_id":store.default_credential_id}),
        )
    }
    pub fn account_exists(&self, id: &str) -> Result<bool> {
        Ok(self.load()?.accounts.contains_key(id))
    }
    pub fn credential_type(&self, id: &str) -> Result<String> {
        let store = self.load()?;
        if store.api_keys.contains_key(id) {
            return Ok("api_key".into());
        }
        if store.accounts.contains_key(id) {
            return Ok("chatgpt".into());
        }
        bail!("Credential no longer exists. Choose a saved credential.");
    }
    pub fn named_api_key(&self, id: &str) -> Result<String> {
        self.load()?
            .api_keys
            .get(id)
            .map(|v| v.key.clone())
            .context(
                "API credential no longer exists. Choose a saved credential; no fallback was used.",
            )
    }
    pub fn legacy_credential(&self, profile: &Value) -> Result<Option<String>> {
        let store = self.load()?;
        if profile.get("credential_id").is_some() {
            return Ok(profile["credential_id"]
                .as_str()
                .filter(|id| store.api_keys.contains_key(*id) || store.accounts.contains_key(*id))
                .map(str::to_owned));
        }
        if profile["billing_route"] == "chatgpt" {
            return Ok(profile["chatgpt_account"]
                .as_str()
                .filter(|id| store.accounts.contains_key(*id))
                .map(str::to_owned));
        }
        Ok(store
            .legacy_api_credential_id
            .filter(|id| store.api_keys.contains_key(id)))
    }
    pub async fn select_credential_default(&self, id: &str) -> Result<()> {
        let _lock = self.lock().await?;
        let mut store = self.load()?;
        if store.api_keys.contains_key(id) {
            store.route = "api_key".into();
            store.selected = None;
        } else if store.accounts.contains_key(id) {
            store.route = "chatgpt".into();
            store.selected = Some(id.into());
        } else {
            bail!("Credential no longer exists.");
        }
        store.default_credential_id = Some(id.into());
        store.revision += 1;
        self.save(&store)
    }
    pub async fn create_credential(&self, label: &str, secret: &str) -> Result<Value> {
        let label = if label.trim().is_empty() {
            None
        } else {
            Some(name(label)?)
        };
        let key = key(secret)?;
        let _lock = self.lock().await?;
        let mut store = self.load()?;
        let name = label.unwrap_or_else(|| {
            (1..)
                .map(|n| format!("openai-api-key-{n}"))
                .find(|candidate| {
                    !store
                        .api_keys
                        .values()
                        .any(|key| key.name.eq_ignore_ascii_case(candidate))
                        && !store
                            .accounts
                            .values()
                            .any(|account| account.name.eq_ignore_ascii_case(candidate))
                })
                .unwrap()
        });
        let entry = ApiKey { name, key };
        let id = Uuid::new_v4().to_string();
        store.api_keys.insert(id.clone(), entry);
        // New credentials never silently replace a selected or deleted credential.
        store.registry_migrated = true;
        store.revision += 1;
        self.save(&store)?;
        Ok(json!({"id":id}))
    }
    pub async fn edit_credential(
        &self,
        id: &str,
        label: Option<&str>,
        secret: Option<&str>,
    ) -> Result<()> {
        let label = label.map(name).transpose()?;
        let secret = secret.map(key).transpose()?;
        let _lock = self.lock().await?;
        let mut store = self.load()?;
        if let Some(entry) = store.api_keys.get_mut(id) {
            if let Some(label) = label {
                entry.name = label;
            }
            if let Some(secret) = secret {
                entry.key = secret;
            }
        } else if let Some(account) = store.accounts.get_mut(id) {
            if secret.is_some() {
                bail!("Use sign-in to reauthorize a subscription credential.");
            }
            if let Some(label) = label {
                account.name = label;
            }
        } else {
            bail!("Credential no longer exists.");
        }
        store.revision += 1;
        self.save(&store)
    }
    pub async fn delete_credential(&self, id: &str) -> Result<()> {
        let _lock = self.lock().await?;
        let mut store = self.load()?;
        let removed = store.api_keys.remove(id).is_some() | store.accounts.remove(id).is_some();
        if !removed {
            bail!("Credential no longer exists.");
        }
        if store.default_credential_id.as_deref() == Some(id) {
            store.default_credential_id = None;
        }
        if store.selected.as_deref() == Some(id) {
            store.selected = None;
        }
        store.revision += 1;
        self.save(&store)
    }
    pub fn begin_credential(
        &self,
        origin: &str,
        id: Option<&str>,
        label: Option<&str>,
        consent: bool,
    ) -> Result<String> {
        let label = label.map(name).transpose()?;
        let url = self.begin(origin, id, consent)?;
        let ticket = url
            .split_once("ticket=")
            .context("Invalid local sign-in ticket.")?
            .1;
        self.attempts
            .lock()
            .unwrap()
            .get_mut(ticket)
            .context("Missing sign-in attempt.")?
            .name = label;
        Ok(url)
    }
}
