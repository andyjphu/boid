//! Immutable model-history payloads and pure selection of retained context.
use super::*;

// Completed model input items are immutable. Cloning a checkpoint shares their
// payloads; appending, clearing, or compacting one history does not change another.
// The serialized form remains the same JSON array used by existing records.
#[derive(Default, Clone, Serialize)]
#[serde(transparent)]
pub(super) struct ModelHistory(pub(super) Vec<Arc<Value>>, #[serde(skip)] pub(super) bool);
impl<'de> Deserialize<'de> for ModelHistory {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        let items = Vec::<Arc<Value>>::deserialize(deserializer)?;
        let images = items.iter().any(|item| crate::images::has_refs(item));
        Ok(Self(items, images))
    }
}
impl ModelHistory {
    pub(super) fn push(&mut self, item: Value) {
        self.1 |= crate::images::has_refs(&item);
        self.0.push(Arc::new(item));
    }
    pub(super) fn clear(&mut self) {
        self.0.clear();
        self.1 = false;
    }
}
impl Extend<Value> for ModelHistory {
    fn extend<T: IntoIterator<Item = Value>>(&mut self, items: T) {
        for item in items {
            self.push(item);
        }
    }
}
impl From<Vec<Value>> for ModelHistory {
    fn from(items: Vec<Value>) -> Self {
        let images = items.iter().any(crate::images::has_refs);
        Self(items.into_iter().map(Arc::new).collect(), images)
    }
}
impl std::ops::Deref for ModelHistory {
    type Target = [Arc<Value>];
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

/// Keep approximately the newer half by JSON text size, without orphan results.
pub(super) fn recent_half(history: Vec<Arc<Value>>) -> Vec<Arc<Value>> {
    fn size(value: &Value) -> usize {
        match value {
            Value::String(text) => text.len(),
            Value::Array(items) => items.iter().map(size).sum(),
            Value::Object(fields) => fields
                .iter()
                .map(|(key, value)| key.len() + size(value))
                .sum(),
            _ => 8,
        }
    }
    let total = history.len();
    let target = history.iter().map(|item| size(item)).sum::<usize>() / 2;
    let mut cut = 0;
    let mut removed_size = 0;
    while cut < total.saturating_sub(1) && removed_size < target {
        removed_size += size(&history[cut]);
        cut += 1;
    }
    // A retained call may need the reasoning from its original model output.
    if history
        .get(cut)
        .is_some_and(|item| item["type"] == "function_call")
    {
        while cut > 0
            && matches!(
                history[cut - 1]["type"].as_str(),
                Some("reasoning" | "function_call")
            )
        {
            cut -= 1;
        }
    }
    let mut calls = std::collections::BTreeSet::new();
    let retained: Vec<_> = history
        .into_iter()
        .skip(cut)
        .filter(|item| {
            if item["type"] == "function_call" {
                if let Some(id) = item["call_id"].as_str() {
                    calls.insert(id.to_owned());
                }
            }
            item["type"] != "function_call_output"
                || item["call_id"]
                    .as_str()
                    .is_some_and(|id| calls.contains(id))
        })
        .collect();
    retained
}
