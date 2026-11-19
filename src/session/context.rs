//! Worker-owned context edits. Authorization, archives and persistence belong to Hub.
//! These operations perform no IO and cannot change agent identity or preferences.
use super::*;

pub(crate) enum ContextEdit {
    Clear,
    KeepRecentHalf,
    Replace(Vec<Arc<Value>>),
}

pub(crate) struct ContextChange {
    pub removed: usize,
    pub retained: usize,
}

impl Session {
    /// Apply only at an execution boundary, after pending tools have been settled.
    /// The caller archives the previous context before committing the edit.
    pub(crate) fn edit_context(&mut self, edit: ContextEdit) -> ContextChange {
        let previous = self.history.len();
        let (items, removed) = match edit {
            ContextEdit::Clear => (Vec::new(), previous),
            ContextEdit::KeepRecentHalf => {
                let items = history::recent_half(self.take_history());
                let removed = previous - items.len();
                (items, removed)
            }
            ContextEdit::Replace(items) => (items, previous),
        };
        self.reset_context_execution();
        self.restore_history(items);
        ContextChange {
            removed,
            retained: self.history.len(),
        }
    }

    fn reset_context_execution(&mut self) {
        self.history.clear();
        self.cache_boundary = None;
        self.partial_text.clear();
        self.context_usage = None;
        self.compact_requested = false;
        self.compact_resume = false;
        self.compact_in_flight = false;
        self.compact_automatic = false;
        self.request_in_flight = false;
    }

    pub(crate) fn cache_plan(&self) -> crate::request_context::cache::Plan {
        crate::request_context::cache::Plan::new(
            &self.history,
            self.cache_boundary.as_ref(),
            crate::request_context::cache::enabled(&self.model, &self.profile),
        )
    }
    pub(crate) fn history_items(&self) -> &[Arc<Value>] {
        &self.history
    }
    pub(crate) fn take_history(&mut self) -> Vec<Arc<Value>> {
        std::mem::take(&mut self.history).0
    }
    pub(crate) fn restore_history(&mut self, items: Vec<Arc<Value>>) {
        if self
            .cache_boundary
            .as_ref()
            .is_some_and(|b| !b.valid(&items))
        {
            self.cache_boundary = None;
        }
        let images = items.iter().any(|item| crate::images::has_refs(item));
        self.history = ModelHistory(items, images);
    }
    pub fn has_image_refs(&self) -> bool {
        self.history.1
    }
    pub(super) async fn provider_input(
        &self,
        hub: &Hub,
        id: &str,
    ) -> Result<Option<Vec<Arc<Value>>>> {
        if !self.has_image_refs() {
            return Ok(None);
        }
        let history = self.history.0.clone();
        let store = hub.image_store();
        let id = id.to_owned();
        let permit = store.work_permit().await;
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            store.materialize(&id, &history)
        })
        .await
        .context("Image request preparation failed.")?
        .map(Some)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_edits_keep_agent_configuration_and_immutable_checkpoints() {
        let old = Arc::new(json!({"role":"user","content":"old input ".repeat(100)}));
        let recent = Arc::new(json!({"role":"user","content":"recent input"}));
        let replacement =
            Arc::new(json!({"type":"reasoning","encrypted_content":"retained opaque bytes"}));
        let base = Session {
            model: "fixture-model".into(),
            instructions: "unchanged saved instructions".into(),
            tools: json!([{"type":"function","name":"fixture_tool"}]),
            skill_base: Some("unchanged base".into()),
            skill_snapshot: Some("unchanged snapshot".into()),
            profile: json!({"credential_id":"fixture-credential","service_tier":"priority"}),
            auto_oai_compact: true,
            model_requests: 17,
            history: ModelHistory(vec![old.clone(), recent.clone()], false),
            partial_text: "previous partial text".into(),
            context_usage: Some(ContextUsage {
                input_tokens: 100,
                output_tokens: 20,
            }),
            ..Session::default()
        };
        for (edit, expected, removed) in [
            (ContextEdit::Clear, vec![], 2),
            (ContextEdit::KeepRecentHalf, vec![recent.clone()], 1),
            (
                ContextEdit::Replace(vec![replacement.clone()]),
                vec![replacement.clone()],
                2,
            ),
        ] {
            let mut changed = base.clone();
            let change = changed.edit_context(edit);
            assert_eq!((change.removed, change.retained), (removed, expected.len()));
            for (actual, expected) in changed.history_items().iter().zip(&expected) {
                assert!(Arc::ptr_eq(actual, expected));
            }
            assert!(Arc::ptr_eq(&base.history[0], &old));
            assert_eq!(base.history.len(), 2);
            assert!(changed.context_usage.is_none());
            assert!(changed.partial_text.is_empty());
            // Normalize only the three context fields changed by an idle edit.
            // Everything else, including persistent preferences and request count,
            // must retain exactly the existing serialized representation.
            let before = serde_json::to_value(&base).unwrap();
            let mut after = serde_json::to_value(&changed).unwrap();
            for key in ["history", "context_usage", "partial_text"] {
                after[key] = before[key].clone();
            }
            assert_eq!(after, before);
        }
    }
}
