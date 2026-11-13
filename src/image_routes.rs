use super::*;
use axum::body::Bytes;
struct ImageBody(Arc<Vec<u8>>);
impl AsRef<[u8]> for ImageBody {
    fn as_ref(&self) -> &[u8] {
        self.0.as_slice()
    }
}

pub(super) fn routes() -> Router<App> {
    Router::new()
        .route(
            "/api/sessions/{id}/images",
            post(upload).layer(DefaultBodyLimit::max(crate::images::MAX_IMAGE_BYTES)),
        )
        .route("/api/sessions/{id}/images/{image}", get(read))
}
async fn upload(
    State(app): State<App>,
    Path(id): Path<String>,
    headers: HeaderMap,
    bytes: Bytes,
) -> ApiResult<impl IntoResponse> {
    let mime = headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .unwrap_or("")
        .trim()
        .to_owned();
    let permit = app.hub.image_store().work_permit().await;
    let image = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        app.hub.upload_image(&id, &mime, &bytes)
    })
    .await
    .map_err(invalid)?
    .map_err(invalid)?;
    Ok((StatusCode::CREATED, Json(image)))
}
async fn read(
    State(app): State<App>,
    Path((id, image)): Path<(String, String)>,
) -> ApiResult<Response> {
    let store = app.hub.image_store();
    let permit = store.work_permit().await;
    let (metadata, bytes) = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        app.hub.ensure_image_target(&id)?;
        let result = store.read(&id, &image)?;
        app.hub.ensure_image_target(&id)?;
        Ok::<_, anyhow::Error>(result)
    })
    .await
    .map_err(invalid)?
    .map_err(invalid)?;
    let mut response = Bytes::from_owner(ImageBody(bytes)).into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(&metadata.mime_type).map_err(invalid)?,
    );
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, max-age=31536000, immutable"),
    );
    response.headers_mut().insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    Ok(response)
}
