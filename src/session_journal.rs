//! Ordered session events. The lane owns append/rotation; one compactor prunes
//! closed segments only after publishing a durable snapshot that covers them.
use super::*;
use std::io::{BufRead, BufReader};

impl SessionStore {
    fn event_directory(&self, id: &str) -> Result<Option<PathBuf>> {
        if uuid::Uuid::parse_str(id).is_err() {
            bail!("Invalid session journal identity.");
        }
        Ok(self
            .directory
            .as_ref()
            .map(|d| d.join(format!("{id}.journal"))))
    }

    fn ensure_event_directory(&self, id: &str) -> Result<Option<PathBuf>> {
        let Some(directory) = self.event_directory(id)? else {
            return Ok(None);
        };
        match fs::create_dir(&directory) {
            Ok(()) => {
                fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))?;
                File::open(directory.parent().unwrap())?.sync_all()?;
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                if !fs::symlink_metadata(&directory)?.is_dir() {
                    bail!("Session journal is not a directory; original retained.");
                }
            }
            Err(e) => return Err(e.into()),
        }
        Ok(Some(directory))
    }

    pub(crate) fn append_events<T: Serialize>(&self, id: &str, events: &[T]) -> Result<u64> {
        if events.is_empty() {
            return Ok(0);
        }
        let Some(directory) = self.ensure_event_directory(id)? else {
            return Ok(0);
        };
        let path = directory.join("active.jsonl");
        let existed = path.try_exists()?;
        let mut file = OpenOptions::new()
            .append(true)
            .create(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&path)?;
        let mut bytes = Vec::new();
        for event in events {
            serde_json::to_writer(&mut bytes, event)?;
            bytes.push(b'\n');
        }
        file.write_all(&bytes)?;
        file.sync_data()?;
        if !existed {
            File::open(&directory)?.sync_all()?;
        }
        Ok(bytes.len() as u64)
    }

    fn closed_event_segments(directory: &std::path::Path) -> Result<Vec<(u64, PathBuf)>> {
        let mut segments = Vec::new();
        let entries = match fs::read_dir(directory) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(segments),
            Err(e) => return Err(e.into()),
        };
        for entry in entries {
            let path = entry?.path();
            if path.file_name().and_then(|s| s.to_str()) == Some("active.jsonl")
                || path.extension().and_then(|s| s.to_str()) != Some("jsonl")
            {
                continue;
            }
            let sequence: u64 = path
                .file_stem()
                .and_then(|s| s.to_str())
                .context("Invalid session journal segment; original retained.")?
                .parse()
                .context("Invalid session journal sequence; original retained.")?;
            segments.push((sequence, path));
        }
        segments.sort_by_key(|s| s.0);
        if segments.windows(2).any(|w| w[0].0 == w[1].0) {
            bail!("Duplicate journal segment sequence; originals retained.");
        }
        Ok(segments)
    }

    pub(crate) fn replay_events<T: DeserializeOwned>(
        &self,
        id: &str,
        mut apply: impl FnMut(T) -> Result<()>,
    ) -> Result<()> {
        let Some(directory) = self.event_directory(id)? else {
            return Ok(());
        };
        let mut paths: Vec<_> = Self::closed_event_segments(&directory)?
            .into_iter()
            .map(|(_, path)| (path, false))
            .collect();
        let active = directory.join("active.jsonl");
        if active.try_exists()? {
            paths.push((active, true));
        }
        for (path, active) in paths {
            let file = OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NOFOLLOW)
                .open(&path)?;
            let mut reader = BufReader::new(file);
            let mut offset = 0u64;
            let mut line = Vec::new();
            loop {
                line.clear();
                if reader.read_until(b'\n', &mut line)? == 0 {
                    break;
                }
                if line.last() != Some(&b'\n') {
                    if !active {
                        bail!(
                            "Incomplete closed journal segment {}; original retained.",
                            path.display()
                        );
                    }
                    // Preserve incomplete bytes before repairing the append boundary.
                    let retained =
                        directory.join(format!("active.torn-{offset}-{}", uuid::Uuid::new_v4()));
                    let mut backup = OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .mode(0o600)
                        .open(retained)?;
                    backup.write_all(&line)?;
                    backup.sync_all()?;
                    File::open(&directory)?.sync_all()?;
                    let file = OpenOptions::new()
                        .write(true)
                        .custom_flags(libc::O_NOFOLLOW)
                        .open(&path)?;
                    file.set_len(offset)?;
                    file.sync_all()?;
                    break;
                }
                let event = serde_json::from_slice(&line).with_context(|| {
                    format!(
                        "Invalid journal event {}; original retained.",
                        path.display()
                    )
                })?;
                apply(event)?;
                offset += line.len() as u64;
            }
        }
        Ok(())
    }

    pub(crate) fn rotate_events(&self, id: &str, ending_seq: u64) -> Result<()> {
        let Some(directory) = self.event_directory(id)? else {
            return Ok(());
        };
        let active = directory.join("active.jsonl");
        if !active.try_exists()? {
            return Ok(());
        }
        if fs::metadata(&active)?.len() == 0 {
            fs::remove_file(&active)?;
            File::open(&directory)?.sync_all()?;
            return Ok(());
        }
        let closed = directory.join(format!("{ending_seq}.jsonl"));
        if closed.try_exists()? {
            bail!("Journal segment already exists; originals retained.");
        }
        fs::rename(active, closed)?;
        File::open(directory)?.sync_all()?;
        Ok(())
    }

    pub(crate) fn prune_events(&self, id: &str, through_seq: u64) -> Result<()> {
        let Some(directory) = self.event_directory(id)? else {
            return Ok(());
        };
        let mut removed = false;
        for (sequence, path) in Self::closed_event_segments(&directory)? {
            if sequence <= through_seq {
                fs::remove_file(path)?;
                removed = true;
            }
        }
        if removed {
            File::open(directory)?.sync_all()?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    #[test]
    fn event_segments_preserve_order_torn_tail_and_new_appends() {
        let root = tempfile::tempdir().unwrap();
        let store = SessionStore::open(root.path().into()).unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        store
            .append_events(&id, &[json!({"seq":1,"text":"first\n🦉"})])
            .unwrap();
        store.rotate_events(&id, 1).unwrap();
        store.append_events(&id, &[json!({"seq":2})]).unwrap();
        let directory = store.event_directory(&id).unwrap().unwrap();
        let active = directory.join("active.jsonl");
        OpenOptions::new()
            .append(true)
            .open(&active)
            .unwrap()
            .write_all(b"{\"seq\":3")
            .unwrap();
        let mut replayed = vec![];
        store
            .replay_events::<Value>(&id, |e| {
                replayed.push(e);
                Ok(())
            })
            .unwrap();
        assert_eq!(
            replayed,
            vec![json!({"seq":1,"text":"first\n🦉"}), json!({"seq":2})]
        );
        let backup = fs::read_dir(&directory)
            .unwrap()
            .filter_map(|e| e.ok())
            .find(|e| e.file_name().to_string_lossy().starts_with("active.torn-"))
            .unwrap();
        assert_eq!(fs::read(backup.path()).unwrap(), b"{\"seq\":3");
        assert_eq!(
            backup.metadata().unwrap().permissions().mode() & 0o777,
            0o600
        );
        store.append_events(&id, &[json!({"seq":3})]).unwrap();
        store.rotate_events(&id, 3).unwrap();
        store.append_events(&id, &[json!({"seq":4})]).unwrap();
        store.prune_events(&id, 1).unwrap();
        replayed.clear();
        store
            .replay_events::<Value>(&id, |e| {
                replayed.push(e["seq"].clone());
                Ok(())
            })
            .unwrap();
        assert_eq!(replayed, vec![json!(2), json!(3), json!(4)]);
        assert!(directory.join("3.jsonl").exists());
        assert!(active.exists());
    }

    #[test]
    fn empty_recovered_active_does_not_replace_closed_segment() {
        let root = tempfile::tempdir().unwrap();
        let store = SessionStore::open(root.path().into()).unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        store.append_events(&id, &[json!({"seq":1})]).unwrap();
        store.rotate_events(&id, 1).unwrap();
        let directory = store.event_directory(&id).unwrap().unwrap();
        let closed = fs::read(directory.join("1.jsonl")).unwrap();
        fs::write(directory.join("active.jsonl"), b"{\"seq\":2").unwrap();
        store.replay_events::<Value>(&id, |_| Ok(())).unwrap();
        store.rotate_events(&id, 1).unwrap();
        assert!(!directory.join("active.jsonl").exists());
        assert_eq!(fs::read(directory.join("1.jsonl")).unwrap(), closed);
    }

    #[test]
    fn closed_torn_segment_fails_without_destroying_bytes() {
        let root = tempfile::tempdir().unwrap();
        let store = SessionStore::open(root.path().into()).unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        store.append_events(&id, &[json!({"seq":1})]).unwrap();
        store.rotate_events(&id, 1).unwrap();
        let path = store.event_directory(&id).unwrap().unwrap().join("1.jsonl");
        OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b"incomplete")
            .unwrap();
        let before = fs::read(&path).unwrap();
        assert!(store.replay_events::<Value>(&id, |_| Ok(())).is_err());
        assert_eq!(fs::read(path).unwrap(), before);
    }
}
