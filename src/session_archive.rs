use super::*;
use serde::Deserialize;
use std::io::{BufReader, Read};

#[derive(Serialize, Deserialize)]
struct Manifest<M> {
    record: String,
    sha256: String,
    stream: Option<String>,
    metadata: M,
}
fn digest(path: &std::path::Path) -> Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut bytes = [0; 64 * 1024];
    loop {
        let count = file.read(&mut bytes)?;
        if count == 0 {
            break;
        }
        hasher.update(&bytes[..count]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}
impl SessionStore {
    pub fn session_ids(&self) -> Result<Vec<String>> {
        let Some(directory) = &self.directory else {
            return Ok(vec![]);
        };
        let mut ids = vec![];
        for entry in fs::read_dir(directory)? {
            let path = entry?.path();
            if path.extension().and_then(|x| x.to_str()) == Some("json") {
                let id = path
                    .file_stem()
                    .unwrap()
                    .to_str()
                    .context("Invalid session filename")?;
                uuid::Uuid::parse_str(id).context("Invalid persisted session ID")?;
                ids.push(id.to_owned());
            }
        }
        ids.sort();
        Ok(ids)
    }
    pub fn read_session<T: DeserializeOwned>(&self, id: &str) -> Result<T> {
        uuid::Uuid::parse_str(id).context("Invalid session ID")?;
        let directory = self
            .directory
            .as_ref()
            .context("Session requires durable storage")?;
        serde_json::from_reader(BufReader::new(File::open(
            directory.join(format!("{id}.json")),
        )?))
        .context("Cannot read session record; original retained")
    }
    fn archives_directory(&self) -> Result<PathBuf> {
        Ok(self
            .directory
            .as_ref()
            .context("Archive requires durable storage")?
            .parent()
            .unwrap()
            .join("archives"))
    }
    // The manifest is the commit point. Its full record is copied and verified
    // before publication; only afterward may the old live files be removed.
    pub fn archive_session<M: Serialize>(&self, id: &str, metadata: &M) -> Result<()> {
        uuid::Uuid::parse_str(id).context("Invalid session ID")?;
        let Some(directory) = &self.directory else {
            return Ok(());
        };
        let archives = self.archives_directory()?;
        fs::create_dir_all(&archives)?;
        fs::set_permissions(&archives, fs::Permissions::from_mode(0o700))?;
        File::open(archives.parent().unwrap())?.sync_all()?;
        let generation = uuid::Uuid::new_v4();
        let record = format!("{id}-{generation}.record");
        let source = directory.join(format!("{id}.json"));
        let target = archives.join(&record);
        fs::copy(&source, &target)?;
        fs::set_permissions(&target, fs::Permissions::from_mode(0o600))?;
        File::open(&target)?.sync_all()?;
        let sha256 = digest(&source)?;
        if digest(&target)? != sha256 {
            bail!("Archive verification failed; original retained");
        }
        let journal = directory.join(format!("{id}.stream"));
        let stream = if journal.exists() {
            let name = format!("{id}-{generation}.stream");
            let path = archives.join(&name);
            fs::copy(&journal, &path)?;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
            File::open(&path)?.sync_all()?;
            if digest(&journal)? != digest(&path)? {
                bail!("Archive journal verification failed; original retained");
            }
            Some(name)
        } else {
            None
        };
        File::open(&archives)?.sync_all()?;
        let manifest = Manifest {
            record,
            sha256,
            stream,
            metadata,
        };
        let mut temporary = tempfile::NamedTempFile::new_in(&archives)?;
        serde_json::to_writer(&mut temporary, &manifest)?;
        temporary.as_file().sync_all()?;
        temporary
            .persist(archives.join(format!("{id}.json")))
            .map_err(|e| e.error)?;
        File::open(&archives)?.sync_all()?;
        // A crash here leaves both copies. Startup safely repeats archival of
        // the finalized duplicate; older archive generations remain intact.
        fs::remove_file(source)?;
        if journal.exists() {
            fs::remove_file(journal)?;
        }
        File::open(directory)?.sync_all()?;
        Ok(())
    }
    #[cfg(test)]
    pub fn read_archive<T: DeserializeOwned>(&self, id: &str) -> Result<T> {
        uuid::Uuid::parse_str(id).context("Invalid session ID")?;
        #[cfg(test)]
        if self.directory.is_none() {
            let rows = self.memory_archives.lock().unwrap();
            return serde_json::from_slice(&rows.get(id).context("Unknown archive")?.0)
                .map_err(Into::into);
        }
        let directory = self.archives_directory()?;
        let manifest: Manifest<serde_json::Value> = serde_json::from_reader(BufReader::new(
            File::open(directory.join(format!("{id}.json")))?,
        ))?;
        if std::path::Path::new(&manifest.record).components().count() != 1 {
            bail!("Invalid archive path");
        }
        let path = directory.join(manifest.record);
        if digest(&path)? != manifest.sha256 {
            bail!("Archive checksum mismatch; original retained");
        }
        serde_json::from_reader(BufReader::new(File::open(path)?))
            .context("Cannot read archived session")
    }
    #[cfg(test)]
    pub fn archive_memory<T: Serialize, M: Serialize>(
        &self,
        id: &str,
        record: &T,
        metadata: &M,
    ) -> Result<()> {
        if self.directory.is_none() {
            self.memory_archives.lock().unwrap().insert(
                id.into(),
                (serde_json::to_vec(record)?, serde_json::to_vec(metadata)?),
            );
        }
        Ok(())
    }
}

// Stop the parser as soon as the status field is read. Canonical snapshots put
// view.status before entries and engine. Reordered legacy JSON is still parsed
// correctly, but may require skipping earlier fields before finding status.
impl SessionStore {
    pub fn session_status(&self, id: &str) -> Result<String> {
        use serde::de::{DeserializeSeed, MapAccess, Visitor};
        struct Probe<'a> {
            status: &'a mut Option<String>,
            view: bool,
        }
        impl<'de> DeserializeSeed<'de> for Probe<'_> {
            type Value = ();
            fn deserialize<D: serde::Deserializer<'de>>(
                self,
                de: D,
            ) -> std::result::Result<(), D::Error> {
                de.deserialize_map(self)
            }
        }
        impl<'de> Visitor<'de> for Probe<'_> {
            type Value = ();
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("session status")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> std::result::Result<(), A::Error> {
                while let Some(key) = map.next_key::<String>()? {
                    if self.view && key == "status" {
                        *self.status = Some(map.next_value()?);
                        return Err(serde::de::Error::custom("status found"));
                    }
                    if !self.view && key == "view" {
                        map.next_value_seed(Probe {
                            status: self.status,
                            view: true,
                        })?;
                    } else {
                        map.next_value::<serde::de::IgnoredAny>()?;
                    }
                }
                Ok(())
            }
        }
        uuid::Uuid::parse_str(id).context("Invalid session ID")?;
        let directory = self
            .directory
            .as_ref()
            .context("Session requires durable storage")?;
        let file = File::open(directory.join(format!("{id}.json")))?;
        let mut de = serde_json::Deserializer::from_reader(BufReader::new(file));
        let mut status = None;
        let result = Probe {
            status: &mut status,
            view: false,
        }
        .deserialize(&mut de);
        if let Some(status) = status {
            return Ok(status);
        }
        result.context("Cannot read session header; original retained")?;
        bail!("Session has no status; original retained")
    }
    #[cfg(test)]
    pub fn archived_deltas<T: DeserializeOwned>(&self, id: &str) -> Result<Vec<T>> {
        uuid::Uuid::parse_str(id).context("Invalid session ID")?;
        #[cfg(test)]
        if self.directory.is_none() {
            return Ok(vec![]);
        }
        let directory = self.archives_directory()?;
        let manifest: Manifest<serde_json::Value> = serde_json::from_reader(BufReader::new(
            File::open(directory.join(format!("{id}.json")))?,
        ))?;
        let Some(name) = manifest.stream else {
            return Ok(vec![]);
        };
        if std::path::Path::new(&name).components().count() != 1 {
            bail!("Invalid archive journal path");
        }
        let bytes = fs::read(directory.join(name))?;
        let mut deltas = vec![];
        for line in bytes.split_inclusive(|b| *b == b'\n') {
            if line.last() != Some(&b'\n') {
                break;
            }
            deltas.push(
                serde_json::from_slice(line)
                    .context("Invalid archive stream record; original retained")?,
            );
        }
        Ok(deltas)
    }
}

impl SessionStore {
    fn receipt_path(&self, sender: &str) -> Result<PathBuf> {
        uuid::Uuid::parse_str(sender).context("Invalid receipt sender ID")?;
        Ok(self
            .directory
            .as_ref()
            .context("Receipt index requires storage")?
            .parent()
            .unwrap()
            .join("archived-receipts")
            .join(format!("{sender}.json")))
    }
    pub fn merge_archived_receipts<T: Serialize>(
        &self,
        sender: &str,
        rows: &std::collections::BTreeMap<String, T>,
    ) -> Result<()> {
        let _writer = self.archive_receipt_writer.lock().unwrap();
        let incoming = serde_json::to_value(rows)?.as_object().unwrap().clone();
        #[cfg(test)]
        if self.directory.is_none() {
            let mut indexes = self.memory_receipts.lock().unwrap();
            let mut current: serde_json::Map<String, serde_json::Value> = indexes
                .get(sender)
                .map(|bytes| serde_json::from_slice(bytes))
                .transpose()?
                .unwrap_or_default();
            current.extend(incoming);
            indexes.insert(sender.into(), serde_json::to_vec(&current)?);
            return Ok(());
        }
        let path = self.receipt_path(sender)?;
        let directory = path.parent().unwrap();
        fs::create_dir_all(directory)?;
        fs::set_permissions(directory, fs::Permissions::from_mode(0o700))?;
        File::open(directory.parent().unwrap())?.sync_all()?;
        let mut current: serde_json::Map<String, serde_json::Value> = match File::open(&path) {
            Ok(file) => serde_json::from_reader(BufReader::new(file))?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Default::default(),
            Err(error) => return Err(error.into()),
        };
        current.extend(incoming);
        let mut temporary = tempfile::NamedTempFile::new_in(directory)?;
        serde_json::to_writer(&mut temporary, &current)?;
        temporary.as_file().sync_all()?;
        temporary.persist(&path).map_err(|e| e.error)?;
        File::open(directory)?.sync_all()?;
        Ok(())
    }
    pub fn archived_receipt<T: DeserializeOwned>(
        &self,
        sender: &str,
        call: &str,
    ) -> Result<Option<T>> {
        #[cfg(test)]
        if self.directory.is_none() {
            let indexes = self.memory_receipts.lock().unwrap();
            let Some(bytes) = indexes.get(sender) else {
                return Ok(None);
            };
            let mut rows: serde_json::Map<String, serde_json::Value> =
                serde_json::from_slice(bytes)?;
            return rows
                .remove(call)
                .map(serde_json::from_value)
                .transpose()
                .map_err(Into::into);
        }
        let file = match File::open(self.receipt_path(sender)?) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        // Skip other receipts without retaining their contents in runtime memory.
        struct Lookup<'a, T> {
            call: &'a str,
            value: std::marker::PhantomData<T>,
        }
        impl<'de, T: DeserializeOwned> serde::de::Visitor<'de> for Lookup<'_, T> {
            type Value = Option<T>;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("send receipt index")
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut map: A,
            ) -> std::result::Result<Option<T>, A::Error> {
                let mut found = None;
                while let Some(key) = map.next_key::<String>()? {
                    if key == self.call {
                        found = Some(map.next_value()?);
                    } else {
                        map.next_value::<serde::de::IgnoredAny>()?;
                    }
                }
                Ok(found)
            }
        }
        let mut de = serde_json::Deserializer::from_reader(BufReader::new(file));
        serde::Deserializer::deserialize_map(
            &mut de,
            Lookup {
                call,
                value: std::marker::PhantomData,
            },
        )
        .map_err(Into::into)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn status_probe_stops_before_history_and_archive_keeps_journal() {
        let root = tempfile::tempdir().unwrap();
        let store = SessionStore::open(root.path().into()).unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        let path = root.path().join("sessions").join(format!("{id}.json"));
        let fixture = format!(
            "{{\"view\":{{\"status\":\"idle\",\"entries\":[\"{}\"]}},\"engine\":{{}}}}",
            "x".repeat(8 * 1024 * 1024)
        );
        fs::write(&path, &fixture).unwrap();
        let now = std::time::Instant::now();
        assert_eq!(store.session_status(&id).unwrap(), "idle");
        let probe = now.elapsed();
        let now = std::time::Instant::now();
        let _: serde_json::Value = store.read_session(&id).unwrap();
        eprintln!(
            "SYNTHETIC 8MiB status header {:?}, full parse {:?}",
            probe,
            now.elapsed()
        );
        fs::write(&path, b"{\"view\":{\"status\":\"idle\",BROKEN").unwrap();
        assert_eq!(store.session_status(&id).unwrap(), "idle");
        assert!(store.read_session::<serde_json::Value>(&id).is_err()); // Full load still validates payload.
        fs::write(&path, &fixture).unwrap();
        let delta = serde_json::json!({"seq":4,"entry":"entry","text":"retained tail"});
        store.append(&id, &delta).unwrap();
        store
            .archive_session(&id, &serde_json::json!({"name":"fixture"}))
            .unwrap();
        let record: serde_json::Value = store.read_archive(&id).unwrap();
        assert_eq!(
            record["view"]["entries"][0].as_str().unwrap().len(),
            8 * 1024 * 1024
        );
        assert_eq!(
            store.archived_deltas::<serde_json::Value>(&id).unwrap(),
            vec![delta]
        );
    }
}
