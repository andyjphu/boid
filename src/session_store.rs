use anyhow::{Context, Result, bail};
use serde::{Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    os::{
        fd::AsRawFd,
        unix::fs::{OpenOptionsExt, PermissionsExt},
    },
    path::PathBuf,
};

/// Private, atomic records. One server owns a state directory at a time.
pub struct SessionStore {
    directory: Option<PathBuf>,
    _lock: Option<File>,
}

impl SessionStore {
    #[cfg(test)]
    pub fn memory() -> Self {
        Self {
            directory: None,
            _lock: None,
        }
    }

    pub fn open(root: PathBuf) -> Result<Self> {
        fs::create_dir_all(&root).context("Cannot create Boid state directory")?;
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(root.join("sessions.lock"))?;
        if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            bail!("Another Boid server owns this state directory.");
        }
        let directory = root.join("sessions");
        fs::create_dir_all(&directory)?;
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))?;
        Ok(Self {
            directory: Some(directory),
            _lock: Some(lock),
        })
    }

    pub fn load<T: DeserializeOwned>(&self) -> Result<Vec<T>> {
        let Some(directory) = &self.directory else {
            return Ok(vec![]);
        };
        let mut records = Vec::new();
        for entry in fs::read_dir(directory)? {
            let path = entry?.path();
            if path.extension().and_then(|s| s.to_str()) != Some("json") {
                continue;
            }
            let record = serde_json::from_slice(&fs::read(&path)?).with_context(|| {
                format!(
                    "Cannot read session record {}; original retained",
                    path.display()
                )
            })?;
            records.push(record);
        }
        Ok(records)
    }

    fn skill_snapshot_path(&self, id: &str) -> Result<PathBuf> {
        if id.len() != 64
            || !id
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        {
            bail!("Invalid skill snapshot identity");
        }
        let sessions = self
            .directory
            .as_ref()
            .context("Skill snapshot requires durable storage")?;
        Ok(sessions
            .parent()
            .context("State directory has no parent")?
            .join("skill-snapshots")
            .join(format!("{id}.json")))
    }

    pub fn save_skills(&self, skills: &crate::skills::Skills) -> Result<Option<String>> {
        if self.directory.is_none() {
            return Ok(None);
        }
        let bytes = serde_json::to_vec(skills)?;
        let id = format!("{:x}", Sha256::digest(&bytes));
        let path = self.skill_snapshot_path(&id)?;
        let directory = path.parent().unwrap();
        fs::create_dir_all(directory)?;
        fs::set_permissions(directory, fs::Permissions::from_mode(0o700))?;
        // Persist the new directory before any session can refer to its contents.
        File::open(directory.parent().unwrap())?.sync_all()?;
        if path.exists() {
            if fs::read(&path)? != bytes {
                bail!("Skill snapshot {id} is corrupt; original retained");
            }
            File::open(directory)?.sync_all()?;
            return Ok(Some(id));
        }
        let mut temporary = tempfile::NamedTempFile::new_in(directory)?;
        temporary.write_all(&bytes)?;
        temporary.as_file().sync_all()?;
        match temporary.persist_noclobber(&path) {
            Ok(_) => (),
            Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {
                if fs::read(&path)? != bytes {
                    bail!("Skill snapshot {id} is corrupt; original retained");
                }
            }
            Err(error) => return Err(error.error.into()),
        }
        File::open(directory)?.sync_all()?;
        Ok(Some(id))
    }

    pub fn load_skills(&self, id: &str) -> Result<crate::skills::Skills> {
        let path = self.skill_snapshot_path(id)?;
        let bytes = fs::read(&path).with_context(|| {
            format!(
                "Cannot read skill snapshot {}; session retained",
                path.display()
            )
        })?;
        if format!("{:x}", Sha256::digest(&bytes)) != id {
            bail!("Skill snapshot {id} is corrupt; original retained");
        }
        serde_json::from_slice(&bytes).context("Invalid skill snapshot; original retained")
    }

    pub fn backup(&self, id: &str) -> Result<()> {
        let Some(directory) = &self.directory else {
            return Ok(());
        };
        if uuid::Uuid::parse_str(id).is_err() {
            bail!("Invalid session ID.");
        }
        let source = directory.join(format!("{id}.json"));
        if !source.exists() {
            return Ok(());
        }
        let backup = directory.join(format!("{id}.before-v9"));
        if backup.exists() {
            return Ok(());
        }
        let mut temporary = tempfile::NamedTempFile::new_in(directory)?;
        temporary.write_all(&fs::read(source)?)?;
        temporary.as_file().sync_all()?;
        temporary.persist_noclobber(&backup).map_err(|e| e.error)?;
        File::open(directory)?.sync_all()?;
        Ok(())
    }
    pub fn archive_context<T: Serialize>(&self, id: &str, attempt: u64, record: &T) -> Result<()> {
        let Some(directory) = &self.directory else {
            return Ok(());
        };
        if uuid::Uuid::parse_str(id).is_err() {
            bail!("Invalid session ID.");
        }
        let archive = directory.join("compactions");
        fs::create_dir_all(&archive)?;
        fs::set_permissions(&archive, fs::Permissions::from_mode(0o700))?;
        File::open(directory)?.sync_all()?;
        let mut temporary = tempfile::NamedTempFile::new_in(&archive)?;
        temporary.write_all(&serde_json::to_vec(record)?)?;
        temporary.as_file().sync_all()?;
        temporary
            .persist_noclobber(archive.join(format!("{id}-{attempt}.json")))
            .map_err(|e| e.error)?;
        File::open(archive)?.sync_all()?;
        Ok(())
    }
    pub fn save<T: Serialize>(&self, id: &str, record: &T) -> Result<()> {
        let Some(directory) = &self.directory else {
            return Ok(());
        };
        if uuid::Uuid::parse_str(id).is_err() {
            bail!("Invalid persisted session ID.");
        }
        let mut temporary = tempfile::NamedTempFile::new_in(directory)?;
        // JSON serialization makes many tiny writes unless buffered, which is
        // costly on cross-VM bind mounts. Keep the same durable commit below.
        let mut bytes = serde_json::to_vec(record)?;
        bytes.push(b'\n');
        temporary.write_all(&bytes)?;
        temporary.as_file().sync_all()?;
        temporary
            .persist(directory.join(format!("{id}.json")))
            .map_err(|e| e.error)
            .context("Cannot commit session record")?;
        File::open(directory)?.sync_all()?;
        // The checkpoint is already durable. Journal cleanup must not turn
        // that committed transition into a reported failure. Recovery skips
        // retained deltas at or below the checkpoint's stream sequence.
        let _ = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(directory.join(format!("{id}.stream")))
            .and_then(|file| file.sync_all());
        Ok(())
    }

    pub fn append<T: Serialize>(&self, id: &str, delta: &T) -> Result<()> {
        let Some(directory) = &self.directory else {
            return Ok(());
        };
        let mut file = OpenOptions::new()
            .append(true)
            .create(true)
            .mode(0o600)
            .open(directory.join(format!("{id}.stream")))?;
        let mut line = serde_json::to_vec(delta)?;
        line.push(b'\n');
        file.write_all(&line)?;
        file.sync_data()?;
        Ok(())
    }

    pub fn deltas<T: DeserializeOwned>(&self, id: &str) -> Result<Vec<T>> {
        let Some(directory) = &self.directory else {
            return Ok(vec![]);
        };
        let path = directory.join(format!("{id}.stream"));
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
            Err(error) => return Err(error.into()),
        };
        let mut deltas = vec![];
        // Only the last incomplete append can be discarded after a crash.
        for line in bytes.split_inclusive(|b| *b == b'\n') {
            if line.last() != Some(&b'\n') {
                break;
            }
            deltas.push(serde_json::from_slice(line).with_context(|| {
                format!(
                    "Invalid stream record {}; original retained",
                    path.display()
                )
            })?);
        }
        Ok(deltas)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backup_failure_does_not_publish_and_retry_preserves_original() {
        let root = tempfile::tempdir().unwrap();
        let store = SessionStore::open(root.path().into()).unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        let source = root.path().join("sessions").join(format!("{id}.json"));
        let backup = source.with_extension("before-v9");
        // A source read failure used to leave an empty, apparently valid backup.
        fs::create_dir(&source).unwrap();
        assert!(store.backup(&id).is_err());
        assert!(!backup.exists());
        fs::remove_dir(&source).unwrap();
        let original = b"{\"version\":2}";
        fs::write(&source, original).unwrap();
        store.backup(&id).unwrap();
        assert_eq!(fs::read(&backup).unwrap(), original);
        assert_eq!(
            fs::metadata(&backup).unwrap().permissions().mode() & 0o777,
            0o600
        );
        fs::write(&source, b"{\"version\":3}").unwrap();
        store.backup(&id).unwrap();
        assert_eq!(fs::read(&backup).unwrap(), original);
    }
}
