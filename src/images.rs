//! Session-bound image blobs. History and events contain references, not base64.
//! Uploads and retained archive images are not garbage-collected.
use anyhow::{Context, Result, bail, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use image::{ImageFormat, ImageReader};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::{Cursor, Write},
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

pub const MAX_IMAGE_BYTES: usize = 10 * 1024 * 1024;
pub const MAX_MESSAGE_BYTES: u64 = 20 * 1024 * 1024;
pub const MAX_IMAGES: usize = 8;
const MAX_PIXELS: u64 = 32_000_000;
const REFERENCE: &str = "boid-image://";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Image {
    pub id: String,
    pub mime_type: String,
    pub size_bytes: u64,
    pub width: u32,
    pub height: u32,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub label: String,
}

pub struct ImageStore {
    directory: Option<PathBuf>,
    memory: Mutex<BTreeMap<(String, String), (Image, Arc<Vec<u8>>)>>,
    work: Arc<tokio::sync::Semaphore>,
}
impl ImageStore {
    pub fn new(directory: Option<PathBuf>) -> Self {
        Self {
            directory,
            memory: Mutex::new(BTreeMap::new()),
            work: Arc::new(tokio::sync::Semaphore::new(1)),
        }
    }
    pub async fn work_permit(&self) -> tokio::sync::OwnedSemaphorePermit {
        self.work
            .clone()
            .acquire_owned()
            .await
            .expect("image worker semaphore stays open")
    }
    fn ids(session: &str, id: &str) -> Result<()> {
        uuid::Uuid::parse_str(session).context("Invalid image session ID.")?;
        ensure!(
            id.len() == 64
                && id
                    .bytes()
                    .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c)),
            "Invalid image ID."
        );
        Ok(())
    }
    pub fn put(&self, session: &str, mime: &str, bytes: &[u8]) -> Result<Image> {
        uuid::Uuid::parse_str(session).context("Invalid image session ID.")?;
        let (mime_type, width, height) = validate(mime, bytes)?;
        let id = format!("{:x}", Sha256::digest(bytes));
        let image = Image {
            id: id.clone(),
            mime_type: mime_type.into(),
            size_bytes: bytes.len() as u64,
            width,
            height,
            label: String::new(),
        };
        if let Some(root) = &self.directory {
            private_directory(root)?;
            let directory = root.join(session);
            private_directory(&directory)?;
            write_once(&directory.join(format!("{id}.bin")), bytes)?;
            write_once(
                &directory.join(format!("{id}.json")),
                &serde_json::to_vec(&image)?,
            )?;
        } else {
            self.memory
                .lock()
                .unwrap()
                .entry((session.into(), id))
                .or_insert_with(|| (image.clone(), Arc::new(bytes.to_vec())));
        }
        Ok(image)
    }
    pub fn metadata(&self, session: &str, id: &str) -> Result<Image> {
        Self::ids(session, id)?;
        let image = if let Some(root) = &self.directory {
            let directory = root.join(session);
            let metadata = fs::read(directory.join(format!("{id}.json")))
                .context("Image is unavailable for this session. Paste it again.")?;
            let image: Image =
                serde_json::from_slice(&metadata).context("Invalid saved image metadata.")?;
            ensure!(
                fs::metadata(directory.join(format!("{id}.bin")))?.len() == image.size_bytes,
                "Saved image size differs; image retained for repair."
            );
            image
        } else {
            self.memory
                .lock()
                .unwrap()
                .get(&(session.into(), id.into()))
                .context("Image is unavailable for this session. Paste it again.")?
                .0
                .clone()
        };
        ensure!(
            image.id == id && image.size_bytes <= MAX_IMAGE_BYTES as u64,
            "Invalid saved image metadata."
        );
        Ok(image)
    }
    pub fn read(&self, session: &str, id: &str) -> Result<(Image, Arc<Vec<u8>>)> {
        let image = self.metadata(session, id)?;
        let bytes = if let Some(root) = &self.directory {
            Arc::new(
                fs::read(root.join(session).join(format!("{id}.bin")))
                    .context("Cannot read saved image.")?,
            )
        } else {
            self.memory
                .lock()
                .unwrap()
                .get(&(session.into(), id.into()))
                .context("Image is unavailable.")?
                .1
                .clone()
        };
        ensure!(
            bytes.len() as u64 == image.size_bytes
                && format!("{:x}", Sha256::digest(bytes.as_slice())) == id,
            "Saved image content differs; image retained for repair."
        );
        Ok((image, bytes))
    }
    pub fn for_message(&self, session: &str, ids: &[String]) -> Result<Vec<Image>> {
        ensure!(
            ids.len() <= MAX_IMAGES,
            "A message can contain at most 8 images."
        );
        let mut total = 0;
        ids.iter()
            .enumerate()
            .map(|(index, id)| {
                let mut image = self.metadata(session, id)?;
                total += image.size_bytes;
                ensure!(
                    total <= MAX_MESSAGE_BYTES,
                    "Images in one message must total at most 20 MiB."
                );
                image.label = format!("Image #{}", index + 1);
                Ok(image)
            })
            .collect()
    }
    pub fn reference(session: &str, image: &Image) -> String {
        format!("{REFERENCE}{session}/{}", image.id)
    }
    // Clone only image-bearing messages; keep all other immutable items shared.
    // Caller runs this on a blocking worker, outside the session-state mutex.
    pub fn materialize(&self, session: &str, input: &[Arc<Value>]) -> Result<Vec<Arc<Value>>> {
        let mut urls = BTreeMap::new();
        input
            .iter()
            .map(|item| {
                if !has_refs(item) {
                    return Ok(item.clone());
                }
                let mut item = (**item).clone();
                for part in item
                    .get_mut("content")
                    .and_then(Value::as_array_mut)
                    .into_iter()
                    .flatten()
                {
                    if part["type"] != "input_image" {
                        continue;
                    }
                    let Some(reference) = part["image_url"]
                        .as_str()
                        .and_then(|url| url.strip_prefix(REFERENCE))
                    else {
                        continue;
                    };
                    let (owner, id) = reference
                        .split_once('/')
                        .context("Invalid saved image reference.")?;
                    ensure!(
                        owner == session,
                        "Image reference belongs to another session."
                    );
                    if !urls.contains_key(id) {
                        let (metadata, bytes) = self.read(session, id)?;
                        urls.insert(
                            id.to_owned(),
                            format!(
                                "data:{};base64,{}",
                                metadata.mime_type,
                                STANDARD.encode(bytes.as_slice())
                            ),
                        );
                    }
                    part["image_url"] = json!(urls[id]);
                }
                Ok(Arc::new(item))
            })
            .collect()
    }
    // Replace only the transport encoding in the provider's canonical window.
    pub fn retain_compaction_images(&self, session: &str, output: &mut [Value]) -> Result<()> {
        for item in output {
            for part in item
                .get_mut("content")
                .and_then(Value::as_array_mut)
                .into_iter()
                .flatten()
            {
                if part["type"] != "input_image" {
                    continue;
                }
                let Some(url) = part["image_url"]
                    .as_str()
                    .filter(|s| s.starts_with("data:"))
                else {
                    continue;
                };
                let (header, encoded) = url
                    .split_once(',')
                    .context("Invalid retained image data URL.")?;
                let mime = header
                    .strip_prefix("data:")
                    .and_then(|s| s.strip_suffix(";base64"))
                    .context("Invalid retained image encoding.")?;
                ensure!(
                    encoded.len() <= (MAX_IMAGE_BYTES + 2) / 3 * 4,
                    "Retained image exceeds 10 MiB."
                );
                let bytes = STANDARD
                    .decode(encoded)
                    .context("Invalid retained image base64.")?;
                let image = self.put(session, mime, &bytes)?;
                part["image_url"] = json!(Self::reference(session, &image));
            }
        }
        Ok(())
    }
}
pub fn has_data_images(item: &Value) -> bool {
    item["content"].as_array().is_some_and(|content| {
        content.iter().any(|part| {
            part["type"] == "input_image"
                && part["image_url"]
                    .as_str()
                    .is_some_and(|url| url.starts_with("data:"))
        })
    })
}
pub fn has_refs(item: &Value) -> bool {
    item["content"].as_array().is_some_and(|content| {
        content.iter().any(|part| {
            part["type"] == "input_image"
                && part["image_url"]
                    .as_str()
                    .is_some_and(|url| url.starts_with(REFERENCE))
        })
    })
}
fn private_directory(path: &Path) -> Result<()> {
    fs::create_dir_all(path)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    if let Some(parent) = path.parent() {
        File::open(parent)?.sync_all()?;
    }
    Ok(())
}
fn write_once(path: &Path, bytes: &[u8]) -> Result<()> {
    if path.exists() {
        return Ok(());
    }
    let directory = path.parent().unwrap();
    let mut file = tempfile::NamedTempFile::new_in(directory)?;
    file.write_all(bytes)?;
    file.as_file().sync_all()?;
    match file.persist_noclobber(path) {
        Ok(_) => {}
        Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error.error.into()),
    }
    File::open(directory)?.sync_all()?;
    Ok(())
}
fn validate<'a>(mime: &'a str, bytes: &[u8]) -> Result<(&'a str, u32, u32)> {
    ensure!(
        !bytes.is_empty() && bytes.len() <= MAX_IMAGE_BYTES,
        "Images must contain 1 byte to 10 MiB."
    );
    let expected = match mime {
        "image/png" => ImageFormat::Png,
        "image/jpeg" => ImageFormat::Jpeg,
        "image/webp" => ImageFormat::WebP,
        _ => bail!("Paste a PNG, JPEG, or WebP image."),
    };
    ensure!(
        image::guess_format(bytes)? == expected,
        "Image content does not match its Content-Type."
    );
    if expected == ImageFormat::Png {
        let mut offset = 8usize;
        while offset + 12 <= bytes.len() {
            let length = u32::from_be_bytes(bytes[offset..offset + 4].try_into().unwrap()) as usize;
            let end = offset
                .checked_add(length)
                .and_then(|n| n.checked_add(12))
                .context("Invalid PNG chunk size.")?;
            ensure!(end <= bytes.len(), "Truncated PNG image.");
            ensure!(
                &bytes[offset + 4..offset + 8] != b"acTL",
                "Animated images are not supported."
            );
            offset = end;
        }
    }
    if expected == ImageFormat::WebP && bytes.get(12..16) == Some(b"VP8X".as_slice()) {
        ensure!(
            bytes.get(20).is_none_or(|flags| flags & 2 == 0),
            "Animated images are not supported."
        );
    }
    let (width, height) = ImageReader::with_format(Cursor::new(bytes), expected)
        .into_dimensions()
        .context("Cannot read image dimensions.")?;
    ensure!(
        width > 0 && height > 0 && u64::from(width) * u64::from(height) <= MAX_PIXELS,
        "Images must contain at most 32 million pixels."
    );
    let mut reader = ImageReader::with_format(Cursor::new(bytes), expected);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(width);
    limits.max_image_height = Some(height);
    limits.max_alloc = Some(256 * 1024 * 1024);
    reader.limits(limits);
    reader.decode().context("Invalid image data.")?;
    Ok((mime, width, height))
}

#[cfg(test)]
pub(crate) fn fixture_png() -> Vec<u8> {
    let mut bytes = Cursor::new(Vec::new());
    image::DynamicImage::new_rgba8(2, 3)
        .write_to(&mut bytes, ImageFormat::Png)
        .unwrap();
    bytes.into_inner()
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn image_blobs_validate_scope_limits_and_original_bytes() {
        let root = tempfile::tempdir().unwrap();
        let store = ImageStore::new(Some(root.path().join("images")));
        let session = uuid::Uuid::new_v4().to_string();
        let other = uuid::Uuid::new_v4().to_string();
        let png = fixture_png();
        let saved = store.put(&session, "image/png", &png).unwrap();
        assert_eq!((saved.width, saved.height), (2, 3));
        assert_eq!(store.read(&session, &saved.id).unwrap().1.as_slice(), png);
        assert!(store.metadata(&other, &saved.id).is_err());
        assert!(store.metadata(&session, "../outside").is_err());
        assert!(store.put(&session, "image/jpeg", &png).is_err());
        assert!(store.put(&session, "image/png", &png[..20]).is_err());
        assert!(store.put(&session, "image/gif", b"GIF89a").is_err());
        assert!(
            store
                .put(&session, "image/png", &vec![0; MAX_IMAGE_BYTES + 1])
                .is_err()
        );
        assert!(
            store
                .for_message(&session, &vec![saved.id.clone(); MAX_IMAGES + 1])
                .is_err()
        );
        let original = Arc::new(json!({"role":"user","content":"same text"}));
        let image = Arc::new(
            json!({"role":"user","content":[{"type":"input_image","detail":"auto","image_url":ImageStore::reference(&session, &saved)}]}),
        );
        let materialized = store
            .materialize(&session, &[original.clone(), image.clone()])
            .unwrap();
        assert!(Arc::ptr_eq(&original, &materialized[0]));
        let url = materialized[1]["content"][0]["image_url"].as_str().unwrap();
        assert_eq!(
            STANDARD.decode(url.split_once(',').unwrap().1).unwrap(),
            png
        );
        assert!(
            image["content"][0]["image_url"]
                .as_str()
                .unwrap()
                .starts_with(REFERENCE)
        );
        let mut canonical = vec![
            (*materialized[1]).clone(),
            json!({"type":"compaction","encrypted_content":"opaque fixture"}),
        ];
        store
            .retain_compaction_images(&session, &mut canonical)
            .unwrap();
        assert_eq!(canonical[0], *image);
        assert_eq!(
            canonical[1],
            json!({"type":"compaction","encrypted_content":"opaque fixture"})
        );
    }
}
