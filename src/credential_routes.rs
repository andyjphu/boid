//! Authenticated credential CRUD and authoritative pane assignments.
use super::*;

pub(super) fn routes() -> Router<App> {
    Router::new()
        .route("/api/credentials", get(credentials).post(add_credential))
        .route("/api/credentials/sign-in", post(sign_in))
        .route(
            "/api/credentials/{id}",
            axum::routing::patch(edit_credential).delete(delete_credential),
        )
        .route("/api/pane-groups", get(pane_groups))
        .route("/api/pane-groups/move", post(move_tab))
        .route(
            "/api/pane-groups/{id}",
            axum::routing::patch(assign_credential),
        )
}
fn registry(app: &App) -> ApiResult<Value> {
    auth(app)?.credentials().map_err(invalid)
}
fn publish(app: &App) -> ApiResult<Value> {
    let registry = registry(app)?;
    app.hub.broadcast_credentials(registry.clone());
    Ok(registry)
}
async fn credentials(State(app): State<App>) -> ApiResult<Json<Value>> {
    Ok(Json(registry(&app)?))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AddCredential {
    r#type: String,
    #[serde(default)]
    name: String,
    key: String,
}
async fn add_credential(
    State(app): State<App>,
    Json(input): Json<AddCredential>,
) -> ApiResult<Json<Value>> {
    if input.r#type != "api_key" {
        return Err(invalid("Use sign-in to add a subscription credential."));
    }
    let mut result = auth(&app)?
        .create_credential(&input.name, &input.key)
        .await
        .map_err(invalid)?;
    result["registry"] = publish(&app)?;
    Ok(Json(result))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EditCredential {
    name: Option<String>,
    key: Option<String>,
}
async fn edit_credential(
    State(app): State<App>,
    Path(id): Path<String>,
    Json(input): Json<EditCredential>,
) -> ApiResult<Json<Value>> {
    auth(&app)?
        .edit_credential(&id, input.name.as_deref(), input.key.as_deref())
        .await
        .map_err(invalid)?;
    Ok(Json(json!({"registry":publish(&app)?})))
}
async fn delete_credential(
    State(app): State<App>,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    auth(&app)?.delete_credential(&id).await.map_err(invalid)?;
    app.hub.credential_deleted(&id).map_err(invalid)?;
    Ok(Json(json!({"registry":publish(&app)?})))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SignInCredential {
    origin: String,
    credential_id: Option<String>,
    name: Option<String>,
    #[serde(default)]
    consent: bool,
}
async fn sign_in(
    State(app): State<App>,
    Json(input): Json<SignInCredential>,
) -> ApiResult<Json<Value>> {
    let start_url = auth(&app)?
        .begin_credential(
            &input.origin,
            input.credential_id.as_deref(),
            input.name.as_deref(),
            input.consent,
        )
        .map_err(invalid)?;
    Ok(Json(json!({"start_url":start_url})))
}
async fn pane_groups(State(app): State<App>) -> Json<Value> {
    Json(app.hub.pane_groups())
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MoveTab {
    pane: String,
    target_group_id: Option<String>,
    before: Option<String>,
    revision: u64,
}
fn group_error(error: anyhow::Error) -> (StatusCode, Json<Value>) {
    if error.is::<crate::runtime::GroupRevisionConflict>() {
        (
            StatusCode::CONFLICT,
            Json(json!({"error":error.to_string(),"code":"pane_groups_changed"})),
        )
    } else {
        invalid(error)
    }
}
async fn move_tab(State(app): State<App>, Json(input): Json<MoveTab>) -> ApiResult<Json<Value>> {
    tokio::task::spawn_blocking(move || {
        app.hub.move_tab(
            &input.pane,
            input.target_group_id.as_deref(),
            input.before.as_deref(),
            input.revision,
        )
    })
    .await
    .map_err(invalid)?
    .map(Json)
    .map_err(group_error)
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Assignment {
    credential_id: Option<String>,
    revision: u64,
}
async fn assign_credential(
    State(app): State<App>,
    Path(id): Path<String>,
    Json(input): Json<Assignment>,
) -> ApiResult<Json<Value>> {
    tokio::task::spawn_blocking(move || {
        app.hub
            .assign_group_credential(&id, input.credential_id.as_deref(), input.revision)
    })
    .await
    .map_err(invalid)?
    .map(Json)
    .map_err(group_error)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn credential_crud_uses_existing_auth_and_exposes_metadata_only() {
        let dir = tempfile::tempdir().unwrap();
        let auth = Arc::new(
            crate::chatgpt::Auth::open(dir.path().join("auth"), "api_key", "fixture").unwrap(),
        );
        auth.migrate_credentials("").unwrap();
        let mut client = Client::test_client();
        client.auth = Some(auth.clone());
        let hub = Hub::new(client).unwrap();
        let token = "fixture-browser-token-not-a-provider-key".to_owned();
        let app = App {
            hub: hub.clone(),
            auth: Some(auth),
            token: Arc::new(token.clone()),
            api_key_available: false,
            shutdown: Arc::new(Notify::new()),
            browser_auth: crate::browser_auth::BrowserAuth::open(
                dir.path().join("browser"),
                token.clone(),
            )
            .unwrap(),
        };
        let router = routes()
            .with_state(app.clone())
            .layer(middleware::from_fn_with_state(app, authorize));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/api/credentials", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let http = reqwest::Client::new();
        assert_eq!(
            http.get(&url).send().await.unwrap().status(),
            StatusCode::UNAUTHORIZED
        );
        let result: Value = http
            .post(&url)
            .bearer_auth(&token)
            .json(&json!({"type":"api_key","key":"fixture-secret-value"}))
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(
            result["registry"]["credentials"][0]["name"],
            "openai-api-key-1"
        );
        assert!(!result.to_string().contains("fixture-secret-value"));
        let id = result["id"].as_str().unwrap();
        assert!(Uuid::parse_str(id).is_ok());
        let edited: Value = http
            .patch(format!("{url}/{id}"))
            .bearer_auth(&token)
            .json(&json!({"name":"Renamed","key":"fixture-replaced-secret"}))
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(edited["registry"]["credentials"][0]["name"], "Renamed");
        assert!(!edited.to_string().contains("fixture-replaced-secret"));
        let removed: Value = http
            .delete(format!("{url}/{id}"))
            .bearer_auth(&token)
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json()
            .await
            .unwrap();
        assert!(
            removed["registry"]["credentials"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        let conflict = http
            .patch(url.replace("/api/credentials", "/api/pane-groups/missing"))
            .bearer_auth(&token)
            .json(&json!({"credential_id":null,"revision":999}))
            .send()
            .await
            .unwrap();
        assert_eq!(conflict.status(), StatusCode::CONFLICT);
        assert_eq!(
            conflict.json::<Value>().await.unwrap()["code"],
            "pane_groups_changed"
        );
        hub.shutdown().await;
        server.abort();
    }
}
