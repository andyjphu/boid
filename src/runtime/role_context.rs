use super::*;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RoleContext {
    text: String,
    editor: Option<WorkingContextEditor>,
}
impl RoleContext {
    fn from_record(record: &Record) -> Self {
        Self {
            text: record.working_context.clone(),
            editor: record.working_context_editor.clone(),
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoleContextPatch {
    text: String,
    expected: RoleContext,
}

impl Hub {
    // Legacy dispatch entry point retained for already-emitted tool calls.
    pub fn set_working_context(&self, from: &str, arguments: &str) -> Result<Value> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Edit {
            text: String,
            #[serde(default)]
            to: Option<String>,
        }
        let edit: Edit = serde_json::from_str(arguments)?;
        self.edit_role_context(
            Some(from),
            edit.to.as_deref().unwrap_or(from),
            &edit.text,
            None,
        )
    }

    pub fn role_context(&self, id: &str) -> Result<Value> {
        crate::blocking::run(|| {
            let data = self.0.data.lock().unwrap();
            let target = data
                .sessions
                .get(id)
                .filter(|h| !h.record.view.archived)
                .context("Unknown open session.")?;
            Ok(serde_json::to_value(RoleContext::from_record(
                &target.record,
            ))?)
        })
    }

    // Authenticated operator API: compare the exact note and attribution so an
    // explicit migration cannot overwrite a peer edit made after inspection.
    pub fn patch_role_context(&self, id: &str, edit: RoleContextPatch) -> Result<Value> {
        self.edit_role_context(None, id, &edit.text, Some(&edit.expected))
    }

    fn edit_role_context(
        &self,
        from: Option<&str>,
        to: &str,
        text: &str,
        expected: Option<&RoleContext>,
    ) -> Result<Value> {
        let text = text.trim();
        let characters = text.chars().count();
        if characters > 1024 {
            bail!(
                "Role context must be at most 1024 Unicode characters, including spaces and line breaks. Existing context retained."
            );
        }
        crate::blocking::run(|| {
            let mut data = self.0.data.lock().unwrap();
            if data.closing {
                bail!("Server is shutting down.");
            }
            let editor = if let Some(from) = from {
                let sender = data
                    .sessions
                    .get(from)
                    .filter(|h| {
                        !h.record.view.archived && !h.archiving && h.storage_error.is_none()
                    })
                    .context("Editor is unavailable.")?;
                WorkingContextEditor {
                    id: from.into(),
                    name: sender.record.view.name.clone(),
                }
            } else {
                WorkingContextEditor {
                    id: "operator".into(),
                    name: "Operator".into(),
                }
            };
            let target = data
                .sessions
                .get(to)
                .filter(|h| !h.record.view.archived)
                .context("Unknown open session.")?;
            if expected.is_some_and(|old| *old != RoleContext::from_record(&target.record)) {
                bail!(
                    "Role context changed since inspection. Existing context retained; read it again before replacing it."
                );
            }
            let receipt = json!({"to":to,"text":text,"characters":characters,"max_characters":1024,"editor":editor});
            if target.record.working_context == text
                && target.record.working_context_editor.as_ref() == Some(&editor)
            {
                return Ok(receipt);
            }
            let notice = Self::make_entry(
                "notice",
                format!(
                    "Role context updated by {} ({})\n{}",
                    editor.name,
                    editor.id,
                    if text.is_empty() { "[cleared]" } else { text }
                ),
            );
            // Same coalescing persistence as other metadata. No disk wait,
            // inbox entry, wake, interruption or inference is introduced.
            self.update_locked(&mut data, to, None, |record| {
                record.working_context = text.to_owned();
                record.working_context_editor = Some(editor.clone());
                record.view.entries.push(notice);
                Ok(())
            })?;
            Ok(receipt)
        })
    }
}
