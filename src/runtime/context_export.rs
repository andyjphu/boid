use super::*;
use serde::Serialize;
use std::io::{BufWriter, Write};
use std::os::unix::fs::PermissionsExt;

impl Hub {
    pub fn eject_context(&self, id: &str) -> Result<Value> {
        crate::blocking::run(|| {
            // Snapshot immutable history references under the metadata lock.
            // Image loading, serialization and disk writes happen after release.
            let (engine, name, status, runtime_context) = {
                let data = self.0.data.lock().unwrap();
                if data.closing {
                    bail!("Server is shutting down.");
                }
                let record = &data
                    .sessions
                    .get(id)
                    .filter(|h| !h.record.view.archived)
                    .context("Unknown open session.")?
                    .record;
                let view = &record.view;
                (
                    record.engine.clone(),
                    view.name.clone(),
                    view.status.clone(),
                    json!({"self":{"id":view.id,"name":view.name,"cwd":view.cwd,"color":view.tab_color.as_deref().unwrap_or("grey")},
                     "peers":peer_roster(&data, true),
                     "working_context":{"text":record.working_context,"editor":record.working_context_editor}}),
                )
            };
            let root = self
                .0
                .store
                .state_directory()
                .context("Context export needs a persistent Boid state directory.")?;
            let directory = root.join("context-exports").join(id);
            std::fs::create_dir_all(&directory)
                .context("Cannot create context export directory")?;
            std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))?;
            let path = directory.join(format!("ejected-context-{}.json", Uuid::new_v4()));
            let history = self.0.images.materialize(id, engine.history_items())?;
            #[derive(Serialize)]
            struct Export<'a> {
                format: &'static str,
                session_id: &'a str,
                agent_name: &'a str,
                status: &'a str,
                scope: &'static str,
                model: &'a str,
                instructions: String,
                tools: &'a Value,
                input: crate::request_context::Input<'a>,
            }
            let export = Export {
                format: "boid-context-v1",
                session_id: id,
                agent_name: &name,
                status: &status,
                scope: "Current committed model context; excludes in-flight partial output and unapplied queued messages. Not a historical API request or full transcript.",
                model: &engine.model,
                instructions: engine.stable_instructions(),
                tools: &engine.tools,
                input: crate::request_context::Input::new(&history, runtime_context),
            };
            let mut temporary = tempfile::NamedTempFile::new_in(&directory)?;
            {
                let mut writer = BufWriter::new(temporary.as_file_mut());
                serde_json::to_writer_pretty(&mut writer, &export)?;
                writer.write_all(b"\n")?;
                writer.flush()?;
            }
            let bytes = temporary.as_file().metadata()?.len();
            temporary
                .persist_noclobber(&path)
                .map_err(|error| error.error)?;
            let notice =
                Self::make_entry("notice", format!("Context exported: {}", path.display()));
            self.update(id, |record| {
                record.view.entries.push(notice.clone());
                Ok(())
            })
            .with_context(|| {
                format!(
                    "Context saved to {}, but its receipt could not be recorded",
                    path.display()
                )
            })?;
            Ok(
                json!({"path":path,"bytes":bytes,"context_items":history.len(),
                "context_cleared":false,"status":"exported","message_id":notice.id}),
            )
        })
    }
}
