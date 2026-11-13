use super::*;
use axum::response::IntoResponse;
use axum::{
    Form, Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    routing::{get, post},
};
use jsonwebtoken::{EncodingKey, Header, encode};
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Clone)]
struct Provider {
    origin: String,
    key: Arc<Vec<u8>>,
    jwks: Value,
    expected: Arc<Mutex<BTreeMap<String, String>>>,
    requests: Arc<Mutex<Vec<Value>>>,
    refreshes: Arc<AtomicUsize>,
    failure: Arc<Mutex<String>>,
}
struct Fixture {
    auth: Arc<Auth>,
    provider: Provider,
    _dir: tempfile::TempDir,
    server: tokio::task::JoinHandle<()>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}
impl Fixture {
    async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let keyfile = dir.path().join("test-signing.pem");
        let output = std::process::Command::new("openssl")
            .args([
                "genpkey",
                "-algorithm",
                "RSA",
                "-pkeyopt",
                "rsa_keygen_bits:2048",
                "-out",
            ])
            .arg(&keyfile)
            .output()
            .unwrap();
        assert!(output.status.success());
        let modulus = std::process::Command::new("openssl")
            .args(["rsa", "-noout", "-modulus", "-in"])
            .arg(&keyfile)
            .output()
            .unwrap();
        assert!(modulus.status.success());
        let hex = String::from_utf8(modulus.stdout).unwrap();
        let hex = hex.trim().strip_prefix("Modulus=").unwrap();
        let n: Vec<u8> = (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
            .collect();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let provider = Provider {
            origin: origin.clone(),
            key: Arc::new(fs::read(keyfile).unwrap()),
            jwks: json!({"keys":[{"kty":"RSA","alg":"RS256","use":"sig","kid":"fixture","n":URL_SAFE_NO_PAD.encode(n),"e":"AQAB"}]}),
            expected: Default::default(),
            requests: Default::default(),
            refreshes: Default::default(),
            failure: Default::default(),
        };
        let router = Router::new()
            .route("/.well-known/openid-configuration", get(|State(p):State<Provider>|async move {Json(json!({"issuer":p.origin,"jwks_uri":format!("{}/jwks",p.origin),"revocation_endpoint":format!("{}/revoke",p.origin)}))}))
            .route("/jwks", get(|State(p):State<Provider>|async move {Json(p.jwks)}))
            .route("/api/accounts/oauth/token",post(exchange))
            .route("/responses",post(inference))
            .route("/revoke",post(||async {StatusCode::SERVICE_UNAVAILABLE}))
            .route("/models",get(|headers:HeaderMap|async move {
                assert!(headers["authorization"].to_str().unwrap().starts_with("Bearer fixture-access"));
                Json(json!({"models":[{"slug":"first","display_name":"First model","visibility":"list"},{"slug":"hidden","display_name":"Hidden","visibility":"hide"},{"slug":"second","display_name":"Second model","visibility":"list"}]}))
            })).with_state(provider.clone());
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let mut auth = Auth::open(dir.path().join("auth"), "api_key", "api-model").unwrap();
        auth.migrate_credentials("fixture-api-key").unwrap();
        auth.issuer = origin.clone();
        auth.api = origin;
        Self {
            auth: Arc::new(auth),
            provider,
            _dir: dir,
            server,
        }
    }
    fn begin(&self, account: Option<&str>) -> BTreeMap<String, String> {
        let start = self
            .auth
            .begin("http://127.0.0.1:9999", account, false)
            .unwrap();
        let ticket = start.split_once("ticket=").unwrap().1;
        let url = self.auth.authorization(ticket).unwrap();
        assert!(self.auth.authorization(ticket).is_err());
        let args: BTreeMap<_, _> = reqwest::Url::parse(&url)
            .unwrap()
            .query_pairs()
            .into_owned()
            .collect();
        *self.provider.expected.lock().unwrap() = args.clone();
        args
    }
    async fn login(&self) -> String {
        let args = self.begin(None);
        assert_eq!(args["client_id"], "dynamic_agent_client");
        assert_eq!(args["agent_name_hint"], "Boid");
        assert_eq!(args["code_challenge_method"], "S256");
        let callback = BTreeMap::from([
            ("state".into(), args["state"].clone()),
            ("client_id".into(), format!("issued-{}", Uuid::new_v4())),
            ("code".into(), "fixture-code".into()),
        ]);
        self.auth.complete(&callback).await.unwrap();
        assert!(self.auth.complete(&callback).await.is_err());
        self.auth
            .load()
            .unwrap()
            .accounts
            .values()
            .find(|a| a.client_id == callback["client_id"])
            .unwrap()
            .id
            .clone()
    }
    fn expire(&self, id: &str) {
        let mut store = self.auth.load().unwrap();
        store.accounts.get_mut(id).unwrap().expires_at = 0;
        self.auth.save(&store).unwrap();
    }
}
fn signed(p: &Provider, client: &str, nonce: &str) -> String {
    let mut header = Header::new(Algorithm::RS256);
    header.kid = Some("fixture".into());
    encode(&header,&json!({"iss":p.origin,"aud":client,"sub":"fixture-subject","email":"same@example.invalid","nonce":nonce,"exp":now()+3600}),&EncodingKey::from_rsa_pem(&p.key).unwrap()).unwrap()
}
async fn exchange(
    State(p): State<Provider>,
    Form(form): Form<BTreeMap<String, String>>,
) -> (StatusCode, Json<Value>) {
    let refresh = form["grant_type"] == "refresh_token";
    if refresh {
        p.refreshes.fetch_add(1, Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(100)).await;
        let failure = p.failure.lock().unwrap().clone();
        if !failure.is_empty() {
            return (
                if failure == "temporary" {
                    StatusCode::SERVICE_UNAVAILABLE
                } else {
                    StatusCode::BAD_REQUEST
                },
                Json(json!({"error":failure})),
            );
        }
    } else {
        let expected = p.expected.lock().unwrap();
        assert_eq!(form["code"], "fixture-code");
        assert_eq!(form["resource"], RESOURCE);
        assert_eq!(form["redirect_uri"], expected["redirect_uri"]);
        assert_eq!(
            URL_SAFE_NO_PAD.encode(Sha256::digest(form["code_verifier"].as_bytes())),
            expected["code_challenge"]
        );
    }
    let mut tokens = json!({"token_type":"Bearer","access_token":"fixture-access-rotated","refresh_token":"fixture-refresh-rotated","expires_in":3600,"scope":SCOPES});
    if !refresh {
        tokens["id_token"] = json!(signed(
            &p,
            &form["client_id"],
            &p.expected.lock().unwrap()["nonce"]
        ));
    }
    (StatusCode::OK, Json(tokens))
}

#[tokio::test]
async fn registration_validates_state_identity_pkce_and_account_selection() {
    let f = Fixture::new().await;
    assert!(f.auth.begin("https://example.com", None, false).is_err());
    assert!(
        f.auth
            .complete(&BTreeMap::from([("state".into(), "forged".into())]))
            .await
            .is_err()
    );
    let id = f.login().await;
    assert!(f.auth.profile().unwrap()["chatgpt_account"].is_null()); // Login alone never switches billing.
    let models = f.auth.models(&id).await.unwrap();
    assert_eq!(
        models
            .iter()
            .map(|v| v["slug"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["first", "second"]
    );
    assert!(
        f.auth
            .select("chatgpt", Some(&id), "not-listed")
            .await
            .is_err()
    );
    assert!(f.auth.select("chatgpt", Some(&id), "second").await.unwrap());
    assert!(!f.auth.select("chatgpt", Some(&id), "second").await.unwrap());
    let profile = f.auth.profile().unwrap();
    assert_eq!(profile["model"], "second");
    let second = f.login().await;
    assert_ne!(id, second);
    assert_eq!(f.auth.profile().unwrap(), profile); // Same email, distinct registrations; preserve selected account.
    let args = f.begin(Some(&id));
    assert_ne!(args["client_id"], "dynamic_agent_client");
    assert!(!args.contains_key("agent_name_hint"));
    assert!(args.contains_key("id_token_hint"));
    let valid = signed(&f.provider, &args["client_id"], &args["nonce"]);
    assert!(
        f.auth
            .identity(&valid, &args["client_id"], Some("wrong-nonce"))
            .await
            .is_err()
    );
    assert!(
        f.auth
            .identity(&valid, "wrong-client", Some(&args["nonce"]))
            .await
            .is_err()
    );
    assert_eq!(
        fs::metadata(f.auth.path()).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let public = f.auth.status(true).unwrap().to_string();
    assert!(!public.contains("fixture-access"));
    assert!(!public.contains("fixture-refresh"));
    assert!(!public.contains("id_token"));
    let restored = Auth::open(f.auth.directory.clone(), "api_key", "different-default").unwrap();
    assert_eq!(restored.profile().unwrap(), profile);
}

#[tokio::test]
async fn identity_only_consent_is_retained_without_plan_requests() {
    let f = Fixture::new().await;
    let id = f.login().await;
    let mut store = f.auth.load().unwrap();
    let account = store.accounts.get_mut(&id).unwrap();
    apply_tokens(account,&json!({"token_type":"Bearer","access_token":"fixture-identity","expires_in":3600,"scope":"openid profile email"}),false).unwrap();
    f.auth.save(&store).unwrap();
    let status = f.auth.status(false).unwrap();
    assert_eq!(status["accounts"][0]["connected"], true);
    assert_eq!(status["accounts"][0]["plan_enabled"], false);
    assert!(f.auth.token(&id).await.is_err());
    assert_eq!(f.provider.refreshes.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn concurrent_refresh_survives_cancellation_and_terminal_errors_clear_tokens() {
    let f = Fixture::new().await;
    let id = f.login().await;
    f.expire(&id);
    let auth = f.auth.clone();
    let account = id.clone();
    let cancelled = tokio::spawn(async move { auth.token(&account).await });
    while f.provider.refreshes.load(Ordering::SeqCst) == 0 {
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    cancelled.abort();
    let (a, b) = tokio::join!(f.auth.token(&id), f.auth.token(&id));
    assert_eq!(a.unwrap(), b.unwrap());
    assert_eq!(f.provider.refreshes.load(Ordering::SeqCst), 1);
    assert_eq!(
        f.auth.load().unwrap().accounts[&id].refresh_token,
        "fixture-refresh-rotated"
    );
    f.expire(&id);
    *f.provider.failure.lock().unwrap() = "temporary".into();
    assert!(f.auth.token(&id).await.is_err());
    assert!(
        !f.auth.load().unwrap().accounts[&id]
            .refresh_token
            .is_empty()
    );
    *f.provider.failure.lock().unwrap() = "invalid_grant".into();
    assert!(f.auth.token(&id).await.is_err());
    assert!(
        f.auth.load().unwrap().accounts[&id]
            .refresh_token
            .is_empty()
    );
    assert_eq!(
        f.begin(Some(&id))["client_id"],
        f.auth.load().unwrap().accounts[&id].client_id
    );
}

#[tokio::test]
async fn permission_loss_pause_and_logout_fail_closed_without_switching_billing() {
    let f = Fixture::new().await;
    let id = f.login().await;
    f.auth.select("chatgpt", Some(&id), "first").await.unwrap();
    f.auth.pause(&id, true).await.unwrap();
    assert!(f.auth.check_inference(&id).is_err());
    f.auth.pause(&id, false).await.unwrap();
    f.auth.check_inference(&id).unwrap();
    let mut store = f.auth.load().unwrap();
    store.accounts.get_mut(&id).unwrap().scopes = vec!["openid".into()];
    f.auth.save(&store).unwrap();
    assert!(f.auth.token(&id).await.is_err());
    assert_eq!(f.provider.refreshes.load(Ordering::SeqCst), 0);
    assert!(!f.auth.logout(&id).await.unwrap()); // Mock revocation is unavailable; local sign-out still succeeds.
    let store = f.auth.load().unwrap();
    assert!(store.accounts[&id].access_token.is_empty());
    assert!(store.accounts[&id].refresh_token.is_empty());
    assert_eq!(store.route, "chatgpt");
    assert_eq!(store.selected.as_deref(), Some(id.as_str()));
}

#[tokio::test]
async fn deleted_subscription_finishes_bound_retries_without_restoring_credential() {
    let f = Fixture::new().await;
    let id = f.login().await;
    f.auth.select("chatgpt", Some(&id), "first").await.unwrap();
    *f.provider.failure.lock().unwrap() = "transient-once".into();
    let mut client = crate::api::Client::test_client_at(format!("{}/responses", f.provider.origin));
    client.auth = Some(f.auth.clone());
    let profile = client.config();
    let auth = f.auth.clone();
    let seen = f.provider.requests.clone();
    let remove = tokio::spawn(async move {
        while seen.lock().unwrap().is_empty() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        auth.delete_credential(&id).await.unwrap();
    });
    client
        .respond_with(
            &[] as &[Value],
            json!([]),
            "first",
            "fixture",
            &profile,
            |_| {},
        )
        .await
        .unwrap();
    remove.await.unwrap();
    assert_eq!(f.provider.requests.lock().unwrap().len(), 3);
    assert!(
        !f.auth
            .account_exists(profile["credential_id"].as_str().unwrap())
            .unwrap()
    );
    assert!(
        client
            .respond_with(
                &[] as &[Value],
                json!([]),
                "first",
                "fixture",
                &profile,
                |_| {}
            )
            .await
            .is_err()
    );
    assert_eq!(f.provider.requests.lock().unwrap().len(), 3);
}

async fn inference(
    State(p): State<Provider>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> impl axum::response::IntoResponse {
    assert_eq!(headers["authorization"], "Bearer fixture-access-rotated");
    p.requests.lock().unwrap().push(body.clone());
    if p.failure.lock().unwrap().as_str() == "transient-once"
        && p.requests.lock().unwrap().len() == 1
    {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            [("x-request-id", "subscription-retry-check")],
            "upstream reset",
        )
            .into_response();
    }
    if p.failure.lock().unwrap().as_str() == "transient-once"
        && p.requests.lock().unwrap().len() == 2
    {
        return (
            [("x-request-id", "subscription-stream-check")],
            "data: {\"type\":\"response.failed\",\"response\":{\"error\":{\"code\":\"server_is_overloaded\"}}}\n\n",
        ).into_response();
    }
    if p.failure.lock().unwrap().as_str() == "http-limit" {
        return (StatusCode::TOO_MANY_REQUESTS, Json(json!({"error":{"code":"subscription_sharing_usage_limit_exceeded","message":"Fixture limit"}}))).into_response();
    }
    let event = if p.failure.lock().unwrap().as_str() == "limit" {
        json!({"type":"response.failed","response":{"error":{"code":"subscription_sharing_usage_limit_exceeded","param":null,"message":"Fixture account limit"}}})
    } else if body["input"]
        .as_array()
        .unwrap()
        .iter()
        .any(|i| i["type"] == "function_call_output")
    {
        json!({"type":"response.completed","response":{"output":[{"type":"message","role":"assistant","content":[{"type":"output_text","text":"Fixture complete"}]}]}})
    } else {
        json!({"type":"response.completed","response":{"output":[{"type":"function_call","namespace":"boid","name":"list_agents","arguments":"{}","call_id":"fixture-tool","id":"fc_fixture"}]}})
    };
    (
        [
            ("content-type", "text/event-stream"),
            ("x-request-id", "fixture-request"),
        ],
        format!("data: {event}\n\n"),
    )
        .into_response()
}

#[tokio::test]
async fn pane_login_switches_existing_worker_preserves_history_and_never_uses_api_key() {
    use crate::{
        api::Client,
        runtime::{Create, Hub},
    };
    let f = Fixture::new().await;
    let id = f.login().await;
    let endpoint = format!("{}/responses", f.provider.origin);
    let client = || {
        let mut c = Client::test_client_at(endpoint.clone());
        c.auth = Some(f.auth.clone());
        c
    };
    let state = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let hub = Hub::open(client(), state.path().into()).unwrap();
    let view = hub
        .create(Create {
            name: "canary".into(),
            cwd: work.path().to_string_lossy().into_owned(),
            message: String::new(),
        })
        .unwrap();
    tokio::task::yield_now().await; // The existing worker has already captured its API profile.
    let other = hub
        .create(Create {
            name: "swift".into(),
            cwd: work.path().to_string_lossy().into_owned(),
            message: String::new(),
        })
        .unwrap();
    hub.entry(&view.id, "notice", "Prior transcript remains");
    let mut model_selection = view.profile.clone();
    model_selection["model"] = json!("second");
    hub.select_profile(&view.id, &view.profile, model_selection)
        .unwrap();
    hub.assign_group_credential(
        view.profile["pane_group_id"].as_str().unwrap(),
        Some(&id),
        hub.pane_groups()["revision"].as_u64().unwrap(),
    )
    .unwrap();
    let selected = hub.profile(&view.id).unwrap();
    assert!(
        hub.select_profile(&view.id, &view.profile, selected.clone())
            .is_err(),
        "Stale selections must not overwrite a newer profile"
    );
    assert_eq!(hub.profile(&other.id).unwrap()["billing_route"], "api_key");
    assert_eq!(f.auth.profile().unwrap()["billing_route"], "api_key");
    hub.message(&view.id, "Use the local roster tool".into())
        .unwrap();
    assert!(
        hub.select_profile(&view.id, &selected, view.profile.clone())
            .is_err(),
        "Queued or active requests must keep their selected billing route"
    );
    tokio::time::timeout(Duration::from_secs(5), async {
        while f.provider.requests.lock().unwrap().len() < 2
            || hub.view(&view.id).unwrap().status != "idle"
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let requests = f.provider.requests.lock().unwrap().clone();
    assert_eq!(requests.len(), 2);
    for r in &requests {
        assert_eq!(r["model"], "second");
        assert_eq!(r["stream"], true);
        assert_eq!(r["store"], false);
        assert!(r.get("previous_response_id").is_none());
        assert!(r.get("access_programs").is_none());
        assert_eq!(r["tools"][0]["type"], "namespace");
        assert_eq!(r["tools"][0]["name"], "boid");
        assert!(
            !r["tools"][0]["tools"]
                .as_array()
                .unwrap()
                .iter()
                .any(|t| t["name"] == "oai_compact")
        );
    }
    assert!(
        requests[1]["input"]
            .as_array()
            .unwrap()
            .iter()
            .any(|i| i["type"] == "function_call_output" && i["call_id"] == "fixture-tool")
    );
    assert!(
        hub.view(&view.id)
            .unwrap()
            .entries
            .iter()
            .any(|e| e.text.as_str() == "Prior transcript remains")
    );
    assert_eq!(
        hub.diagnostics()["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["id"] == view.id)
            .unwrap()["latest_response"]["billing_route"],
        "chatgpt"
    );
    let mut changed_model = selected.clone();
    changed_model["model"] = json!("first");
    hub.select_profile(&view.id, &selected, changed_model)
        .unwrap();
    hub.message(&view.id, "Continue with the selected model".into())
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while f.provider.requests.lock().unwrap().len() < 3
            || hub.view(&view.id).unwrap().status != "idle"
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        f.provider.requests.lock().unwrap().last().unwrap()["model"],
        "first"
    );
    let captured = hub.view(&view.id).unwrap().profile;
    hub.stop(&view.id).unwrap();
    hub.shutdown().await;
    drop(hub);
    f.auth.select("api_key", None, "").await.unwrap();
    let restored = Hub::open(client(), state.path().into()).unwrap();
    assert_eq!(restored.view(&view.id).unwrap().profile, captured);
    *f.provider.failure.lock().unwrap() = "limit".into();
    let c = client();
    let error = c
        .respond_with(&[] as &[Value], json!([]), "second", "", &captured, |_| {})
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("subscription_sharing_usage_limit_exceeded"));
    assert!(error.contains("fixture-request"));
    assert!(f.auth.check_inference(&id).is_err());
    let count = f.provider.requests.lock().unwrap().len();
    assert!(
        c.respond_with(&[] as &[Value], json!([]), "second", "", &captured, |_| {})
            .await
            .is_err()
    );
    assert!(
        c.compact(&[] as &[Value], "second", "", &captured)
            .await
            .is_err()
    );
    assert_eq!(f.provider.requests.lock().unwrap().len(), count);
    // Restored plan sessions retain their account even after new-agent default switches to API billing.
    assert_eq!(
        restored.view(&view.id).unwrap().profile["billing_route"],
        "chatgpt"
    );
    restored.shutdown().await;
}

#[tokio::test]
async fn subscription_retry_persists_failure_without_changing_billing_or_replaying_tools() {
    use crate::{
        api::Client,
        runtime::{Create, Hub},
    };
    let f = Fixture::new().await;
    let account = f.login().await;
    f.auth
        .select("chatgpt", Some(&account), "first")
        .await
        .unwrap();
    *f.provider.failure.lock().unwrap() = "transient-once".into();
    let client = || {
        let mut c = Client::test_client_at(format!("{}/responses", f.provider.origin));
        c.auth = Some(f.auth.clone());
        c
    };
    let state = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let hub = Hub::open(client(), state.path().into()).unwrap();
    let view = hub
        .create(Create {
            name: "retry-check".into(),
            cwd: work.path().to_string_lossy().into_owned(),
            message: "Use the roster tool".into(),
        })
        .unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        while f.provider.requests.lock().unwrap().len() < 4
            || hub.view(&view.id).unwrap().status != "idle"
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(f.provider.requests.lock().unwrap().len(), 4);
    assert_eq!(
        hub.view(&view.id)
            .unwrap()
            .entries
            .iter()
            .filter(|e| e.kind == "tool_call")
            .count(),
        1
    );
    let diagnostics = hub.diagnostics();
    let d = &diagnostics["sessions"][0];
    assert_eq!(d["request_failure_count"], 2);
    assert_eq!(d["latest_request_failure"]["http_status"], 200);
    assert_eq!(d["latest_request_failure"]["billing_route"], "chatgpt");
    assert_eq!(
        d["latest_request_failure"]["request_id"],
        "subscription-stream-check"
    );
    assert_eq!(d["latest_response"]["billing_route"], "chatgpt");
    assert_eq!(d["summary"]["recorded_requests"], 2);
    let failure = d["latest_request_failure"].clone();
    hub.shutdown().await;
    drop(hub);
    let restored = Hub::open(client(), state.path().into()).unwrap();
    assert_eq!(
        restored.diagnostics()["sessions"][0]["latest_request_failure"],
        failure
    );
    restored.shutdown().await;
    *f.provider.failure.lock().unwrap() = "http-limit".into();
    let previous = f.provider.requests.lock().unwrap().len();
    let c = client();
    let mut events = vec![];
    let error = c
        .respond_with(
            &[] as &[Value],
            json!([]),
            "first",
            "",
            &view.profile,
            |e| events.push(e.clone()),
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("settings/usage"));
    assert_eq!(f.provider.requests.lock().unwrap().len(), previous + 1);
    assert!(f.auth.load().unwrap().accounts[&account].paused);
    assert_eq!(
        events[0]["failure"]["error_code"],
        "subscription_sharing_usage_limit_exceeded"
    );
}
