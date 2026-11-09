use anyhow::{Context, Result, bail};
use serde::{Serialize, de::DeserializeOwned};
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
