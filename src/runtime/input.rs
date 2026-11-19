//! Operator input admission representation and execution-boundary dispatch.
//! Metadata edits (name, role, color, credentials) remain Hub-owned and do not
//! enqueue model work. History edits execute only in the recipient's worker.
use super::*;
use crate::session::ContextEdit;

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Input {
    pub(super) id: String,
    pub(super) text: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) images: Vec<crate::images::Image>,
    pub(super) order: u64,
    #[serde(default)]
    pub(super) compact: bool,
    #[serde(default)]
    pub(super) clear: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) take_context: Option<String>,
}

/// Derived from the existing persisted fields. The disk schema and precedence
/// remain unchanged, including commands queued by older releases.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum InputKind {
    Message,
    Clear,
    ClearFirstHalf,
    Compact,
    AutoCompact,
    TakeContext,
    Reload,
}

impl Input {
    pub(super) fn new(
        id: String,
        text: String,
        images: Vec<crate::images::Image>,
        order: u64,
        take_context: Option<String>,
    ) -> Self {
        Self {
            compact: images.is_empty() && text.trim() == "/oai-compact",
            clear: images.is_empty()
                && (take_context.is_some()
                    || matches!(text.trim(), "/clear" | "/clear-first-half")),
            id,
            text,
            images,
            order,
            take_context,
        }
    }

    fn is_auto_compaction(&self) -> bool {
        self.images.is_empty() && self.text.split_whitespace().next() == Some("/auto-oai-compact")
    }

    fn kind(&self) -> InputKind {
        if self.images.is_empty() && self.text.trim() == "/reload" {
            InputKind::Reload
        } else if self.take_context.is_some() {
            InputKind::TakeContext
        } else if self.clear {
            if self.text.trim() == "/clear-first-half" {
                InputKind::ClearFirstHalf
            } else {
                InputKind::Clear
            }
        } else if self.compact {
            InputKind::Compact
        } else if self.is_auto_compaction() {
            InputKind::AutoCompact
        } else {
            InputKind::Message
        }
    }

    pub(super) fn refreshes_skills(&self) -> bool {
        // Honor saved flags as well as current text, including legacy records.
        !self.compact
            && !self.clear
            && !self.is_auto_compaction()
            && !(self.images.is_empty() && self.text.trim() == "/reload")
    }
}

impl Hub {
    pub(super) fn next_input(
        &self,
        id: &str,
        session: &mut Session,
        active: bool,
    ) -> Result<Option<bool>> {
        crate::blocking::run(|| {
            let mut permit = self.reserve_persistence(id, 2)?;
            let mut data = self.0.data.lock().unwrap();
            let mut copied = if data
                .sessions
                .get(id)
                .and_then(|handle| handle.record.inputs.front())
                .is_some_and(|input| input.take_context.is_some())
            {
                // Only context copies need staging. Ordinary input keeps its
                // existing lock path; image work never holds the shared lock.
                drop(data);
                let copied = self.stage_context_copy(id);
                data = self.0.data.lock().unwrap();
                copied
            } else {
                None
            };
            let profile = data.sessions.get(id).and_then(|handle| {
                handle
                    .record
                    .inputs
                    .front()
                    .filter(|input| input.is_auto_compaction())
                    .map(|_| Self::effective_profile(&data, id, &handle.record.view.profile))
            });
            let handle = data.sessions.get_mut(id).context("Unknown session.")?;
            if let Some(input) = handle.record.inputs.front().filter(|i| {
                i.clear
                    && !handle.record.paused
                    && i.order > handle.record.stop_through
                    && (i.take_context.is_none()
                        || copied
                            .as_ref()
                            .is_some_and(|(key, result)| key == &i.id && result.is_ok()))
            }) {
                handle.persistence.enqueue_reserved(
                    Operation::Clear(input.id.clone(), Box::new(handle.record.clone())),
                    &mut permit,
                )?;
            }
            let (event, _) = self.update_locked(&mut data, id, Some(&mut permit), |r| {
            let Some(input) = r.inputs.front() else {
                return Ok(None);
            };
            if r.paused && input.order > r.stop_through {
                return Ok(None);
            };
            let input = r.inputs.pop_front().unwrap();
            let cancelled = input.order <= r.stop_through;
            let mut continue_work = !input.clear;
            if !cancelled {
                let selected = match input.kind() {
                    InputKind::Reload => {
                        session.reload_system_prompt();
                        continue_work = active;
                        r.view.entries.push(Self::make_entry("notice", "System prompt reloaded.".into()));
                        vec![]
                    }
                    InputKind::TakeContext => {
                        let result = copied.take().filter(|(key, _)| key == &input.id)
                            .map(|(_, result)| result)
                            .unwrap_or_else(|| Err(anyhow::anyhow!("Context source is unavailable.")));
                        match result {
                            Ok(copy) => {
                                session.edit_context(ContextEdit::Replace(copy.history));
                                r.view.entries.push(Self::make_entry("notice", format!("Context taken from {}.", copy.name)));
                            }
                            Err(error) => {
                                continue_work = active;
                                r.view.entries.push(Self::make_entry("error", format!("Context unchanged: {error:#}")));
                            }
                        }
                        vec![]
                    }
                    InputKind::ClearFirstHalf => {
                        let change = session.edit_context(ContextEdit::KeepRecentHalf);
                        r.view.entries.push(Self::make_entry("notice", format!("Older context cleared: {} items removed, {} retained. Transcript and role context preserved.", change.removed, change.retained)));
                        vec![]
                    }
                    InputKind::Clear => {
                        session.edit_context(ContextEdit::Clear);
                        r.view.entries.push(Self::make_entry("notice", "Context cleared. The transcript is retained; new messages start a fresh conversation.".into()));
                        vec![]
                    }
                    InputKind::Compact => {
                        session.request_compaction(active);
                        vec![]
                    }
                    InputKind::AutoCompact => {
                        let (notice, resume) = session.auto_compaction_command(&input.text, profile.as_ref().expect("auto-compaction profile staged"), active);
                        continue_work = resume;
                        r.view.entries.push(Self::make_entry("notice", notice));
                        vec![]
                    }
                    InputKind::Message => session.operator_with_images(input.text, id, &input.images),
                };
                for text in selected {
                    r.view
                        .entries
                        .push(Self::make_entry("skill", text.to_string()));
                }
                r.engine = session.clone();
                if let Some(entry) = r.view.entries.iter_mut().find(|entry| entry.id == input.id) {
                    entry.input_batch = Some(session.next_input_batch());
                }
            }
            r.view.pending.retain(|p| p != &input.id);
            Ok((!cancelled).then_some(continue_work))
        })?;
            drop(data);
            Ok(event)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saved_inputs_keep_their_wire_shape_and_legacy_dispatch() {
        let fixtures = [
            (
                json!({"id":"1","text":"ordinary text","order":1,"clear":false,"compact":false}),
                InputKind::Message,
                true,
            ),
            (
                json!({"id":"2","text":"legacy clear marker","order":2,"clear":true,"compact":false}),
                InputKind::Clear,
                false,
            ),
            (
                json!({"id":"3","text":" /clear-first-half ","order":3,"clear":true,"compact":false}),
                InputKind::ClearFirstHalf,
                false,
            ),
            (
                json!({"id":"4","text":"/take-context source","order":4,"clear":true,"compact":false,"take_context":"source"}),
                InputKind::TakeContext,
                false,
            ),
            (
                json!({"id":"5","text":"/oai-compact","order":5,"clear":false,"compact":true}),
                InputKind::Compact,
                false,
            ),
            (
                json!({"id":"6","text":" /auto-oai-compact on ","order":6,"clear":false,"compact":false}),
                InputKind::AutoCompact,
                false,
            ),
            (
                json!({"id":"7","text":" /reload ","order":7,"clear":false,"compact":false}),
                InputKind::Reload,
                false,
            ),
            // Saved flags are authoritative. Do not reinterpret old input as a clear.
            (
                json!({"id":"8","text":"/clear","order":8,"clear":false,"compact":false}),
                InputKind::Message,
                true,
            ),
            (
                json!({"id":"9","text":"/reload","order":9,"clear":false,"compact":false,"images":[{"id":"fixture","mime_type":"image/png","size_bytes":1,"width":1,"height":1}]}),
                InputKind::Message,
                true,
            ),
        ];
        for (stored, kind, refresh) in fixtures {
            let input: Input = serde_json::from_value(stored.clone()).unwrap();
            assert_eq!(input.kind(), kind);
            assert_eq!(input.refreshes_skills(), refresh);
            assert_eq!(serde_json::to_value(input).unwrap(), stored);
        }
    }
}
