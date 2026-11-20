//! Authenticated static wiki, built outside Boid's inference/runtime workers.
use anyhow::{Context, Result};
use axum::{
    body::Body,
    http::{StatusCode, header},
    response::{IntoResponse, Redirect, Response},
};
use std::{
    path::{Component, Path, PathBuf},
    process::Stdio,
};
use tokio::process::{Child, Command};

pub struct Builder {
    child: Child,
}
impl Builder {
    pub fn start(config: &crate::site_config::SiteConfig, state: &Path) -> Result<(Self, PathBuf)> {
        let root = state.join("wiki");
        std::fs::create_dir_all(&root)?;
        let log = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(root.join("wiki-builder.log"))?;
        let child = Command::new("node")
            .arg(config.renderer.join("watch-wiki.mjs"))
            .current_dir(&config.renderer)
            .env("KB_ADDR", &config.kb)
            .env("WIKI_OUTPUT_ROOT", &root)
            .stdin(Stdio::null())
            .stdout(log.try_clone()?)
            .stderr(log)
            .kill_on_drop(true)
            .spawn()
            .context("Cannot start kb-wiki. Install Node.js 22 or later.")?;
        Ok((Self { child }, root))
    }
    pub async fn stop(&mut self) {
        if let Some(pid) = self.child.id() {
            unsafe {
                libc::kill(pid as i32, libc::SIGTERM);
            }
        }
        if tokio::time::timeout(std::time::Duration::from_secs(5), self.child.wait())
            .await
            .is_err()
        {
            let _ = self.child.kill().await;
        }
    }
}

pub async fn serve(root: &Path, path: &str) -> Response {
    if !safe_path(path) {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let current = match tokio::fs::canonicalize(root.join("current")).await {
        Ok(path) => path,
        Err(_) => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                [(header::RETRY_AFTER, "2")],
                "Knowledge base is building. Refresh shortly.",
            )
                .into_response();
        }
    };
    let mut file = current.join(path);
    if tokio::fs::metadata(&file).await.is_ok_and(|m| m.is_dir()) {
        if !path.is_empty() && !path.ends_with('/') {
            return Redirect::temporary(&format!("/wiki/{path}/")).into_response();
        }
        file = file.join("index.html");
    } else if tokio::fs::metadata(&file).await.is_err() && file.extension().is_none() {
        file.set_extension("html");
    }
    let file = match tokio::fs::canonicalize(file).await {
        Ok(path) if path.starts_with(&current) => path,
        _ => return StatusCode::NOT_FOUND.into_response(),
    };
    let mime = match file.extension().and_then(|s| s.to_str()).unwrap_or("") {
        "html" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" => "text/javascript; charset=utf-8",
        "json" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "pdf" => "application/pdf",
        _ => "text/plain; charset=utf-8",
    };
    match tokio::fs::read(file).await {
        Ok(bytes) => Response::builder()
            .header(header::CONTENT_TYPE, mime)
            .body(Body::from(bytes))
            .unwrap(),
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
}
fn safe_path(path: &str) -> bool {
    !path.contains('\\')
        && Path::new(path)
            .components()
            .all(|c| matches!(c, Component::Normal(_)))
}
#[derive(serde::Deserialize)]
pub struct Edit {
    pub original: String,
    pub content: String,
}
type DocumentResult = std::result::Result<serde_json::Value, (StatusCode, String)>;
static EDIT_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
fn document_sync(root: &Path, source: &Path, slug: &str, edit: Option<Edit>) -> DocumentResult {
    use std::io::Write;
    let missing = || (StatusCode::NOT_FOUND, "Article not found.".to_owned());
    let failed = || {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "Could not read or save this article.".to_owned(),
        )
    };
    let _guard = EDIT_LOCK.lock().map_err(|_| failed())?;
    let map: std::collections::BTreeMap<String, String> = serde_json::from_slice(
        &std::fs::read(root.join("current/static/source-map.json")).map_err(|_| missing())?,
    )
    .map_err(|_| failed())?;
    let relative = map.get(slug).ok_or_else(missing)?;
    if !safe_path(relative) || !relative.ends_with(".md") {
        return Err(missing());
    }
    let source = source.canonicalize().map_err(|_| missing())?;
    let file = source
        .join(relative)
        .canonicalize()
        .map_err(|_| missing())?;
    if !file.starts_with(&source) {
        return Err(missing());
    }
    let content = std::fs::read_to_string(&file).map_err(|_| failed())?;
    match edit {
        None => Ok(serde_json::json!({"content":content})),
        Some(edit) => {
            if edit.content.len() > 2 * 1024 * 1024 {
                return Err((
                    StatusCode::PAYLOAD_TOO_LARGE,
                    "Article is too large.".into(),
                ));
            }
            if content != edit.original {
                return Err((
                    StatusCode::CONFLICT,
                    "This article changed on disk. Copy the edits before reloading.".into(),
                ));
            }
            let mut temp = tempfile::NamedTempFile::new_in(file.parent().ok_or_else(missing)?)
                .map_err(|_| failed())?;
            let permissions = std::fs::metadata(&file)
                .map_err(|_| failed())?
                .permissions();
            temp.as_file()
                .set_permissions(permissions)
                .map_err(|_| failed())?;
            temp.write_all(edit.content.as_bytes())
                .map_err(|_| failed())?;
            temp.persist(&file).map_err(|_| failed())?;
            Ok(serde_json::json!({"saved":true}))
        }
    }
}
pub async fn document(
    root: std::sync::Arc<PathBuf>,
    source: std::sync::Arc<PathBuf>,
    slug: String,
    edit: Option<Edit>,
) -> Response {
    match tokio::task::spawn_blocking(move || document_sync(&root, &source, &slug, edit)).await {
        Ok(Ok(value)) => axum::Json(value).into_response(),
        Ok(Err((status, error))) => {
            (status, axum::Json(serde_json::json!({"error":error}))).into_response()
        }
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn editing_preserves_files_and_rejects_stale_or_outside_paths() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("kb");
        let root = dir.path().join("wiki");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::create_dir_all(root.join("current/static")).unwrap();
        std::fs::write(source.join("note.md"), "before").unwrap();
        std::fs::write(dir.path().join("outside.md"), "private").unwrap();
        std::os::unix::fs::symlink(dir.path().join("outside.md"), source.join("escape.md"))
            .unwrap();
        std::fs::write(
            root.join("current/static/source-map.json"),
            r#"{"note":"note.md","escape":"escape.md","traversal":"../outside.md"}"#,
        )
        .unwrap();
        assert_eq!(
            document_sync(&root, &source, "note", None).unwrap()["content"],
            "before"
        );
        document_sync(
            &root,
            &source,
            "note",
            Some(Edit {
                original: "before".into(),
                content: "after".into(),
            }),
        )
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(source.join("note.md")).unwrap(),
            "after"
        );
        assert_eq!(
            document_sync(
                &root,
                &source,
                "note",
                Some(Edit {
                    original: "before".into(),
                    content: "stale".into()
                })
            )
            .unwrap_err()
            .0,
            StatusCode::CONFLICT
        );
        for slug in ["escape", "traversal", "missing"] {
            assert_eq!(
                document_sync(&root, &source, slug, None).unwrap_err().0,
                StatusCode::NOT_FOUND
            );
        }
        assert_eq!(
            std::fs::read_to_string(dir.path().join("outside.md")).unwrap(),
            "private"
        );
    }
    #[tokio::test]
    async fn serves_only_published_files_and_redirects_folders() {
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("build-test");
        std::fs::create_dir_all(output.join("examples")).unwrap();
        std::fs::write(output.join("examples/index.html"), "<h1>Examples</h1>").unwrap();
        std::fs::write(output.join("article.html"), "Article").unwrap();
        std::fs::write(dir.path().join("secret"), "private").unwrap();
        std::os::unix::fs::symlink(&output, dir.path().join("current")).unwrap();
        std::os::unix::fs::symlink(dir.path().join("secret"), output.join("escape.md")).unwrap();
        let response = serve(dir.path(), "examples").await;
        assert_eq!(response.status(), StatusCode::TEMPORARY_REDIRECT);
        assert_eq!(response.headers()[header::LOCATION], "/wiki/examples/");
        assert_eq!(
            serve(dir.path(), "examples/").await.status(),
            StatusCode::OK
        );
        assert_eq!(serve(dir.path(), "article").await.status(), StatusCode::OK);
        assert_eq!(
            serve(dir.path(), "../secret").await.status(),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            serve(dir.path(), "escape.md").await.status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            serve(dir.path(), "missing").await.status(),
            StatusCode::NOT_FOUND
        );
    }
}
