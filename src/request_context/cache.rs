//! Cache annotations are a borrowed wire projection, never transcript edits.
use serde::{
    Serialize, Serializer,
    ser::{SerializeMap, SerializeSeq},
};
use serde_json::{Value, json};
use std::sync::Arc;

#[derive(Clone)]
pub(crate) struct Boundary {
    item: usize,
    block: usize,
    anchor: Arc<Value>,
}

impl Boundary {
    pub(crate) fn valid(&self, history: &[Arc<Value>]) -> bool {
        history
            .get(self.item)
            .is_some_and(|value| Arc::ptr_eq(value, &self.anchor))
    }
}

pub(crate) fn enabled(model: &str, profile: &Value) -> bool {
    // Verified Responses API capability for Boid's current model. Other routes
    // and models retain their existing wire representation.
    profile["billing_route"] == "api_key"
        && (model == "gpt-6-sol" || model.starts_with("gpt-6-sol-"))
}
fn field(item: &Value) -> Option<(&'static str, &Value)> {
    let key = if item["type"] == "function_call_output" {
        "output"
    } else if matches!(item["role"].as_str(), Some("user" | "developer"))
        && (item["type"].is_null() || item["type"] == "message")
    {
        "content"
    } else {
        return None;
    };
    item.get(key)
        .filter(|v| v.is_string() || v.is_array())
        .map(|v| (key, v))
}
fn supported(block: &Value) -> bool {
    matches!(
        block["type"].as_str(),
        Some("input_text" | "input_image" | "input_file")
    )
}
pub(crate) struct Plan {
    pub enabled: bool,
    previous: Option<(usize, usize)>,
    current: Option<Boundary>,
}
impl Plan {
    pub fn new(history: &[Arc<Value>], previous: Option<&Boundary>, enabled: bool) -> Self {
        let previous = previous
            .filter(|b| {
                enabled
                    && history
                        .get(b.item)
                        .is_some_and(|v| Arc::ptr_eq(v, &b.anchor))
            })
            .map(|b| (b.item, b.block));
        let current = enabled
            .then(|| {
                history.iter().enumerate().rev().find_map(|(index, item)| {
                    let (_, content) = field(item)?;
                    let block = if content.is_string() {
                        0
                    } else {
                        content.as_array()?.iter().rposition(supported)?
                    };
                    Some(Boundary {
                        item: index,
                        block,
                        anchor: item.clone(),
                    })
                })
            })
            .flatten();
        Self {
            enabled,
            previous,
            current,
        }
    }
    pub fn next(&self) -> Option<Boundary> {
        self.current.clone()
    }
    fn marker(&self, item: usize, block: usize) -> bool {
        self.previous == Some((item, block))
            || self
                .current
                .as_ref()
                .is_some_and(|b| b.item == item && b.block == block)
    }
}
pub(crate) struct Item<'a> {
    pub value: &'a Value,
    pub index: usize,
    pub plan: &'a Plan,
}
impl Serialize for Item<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let Some((key, content)) = field(self.value).filter(|_| self.plan.enabled) else {
            return self.value.serialize(serializer);
        };
        let mut map = serializer.serialize_map(None)?;
        for (name, value) in self.value.as_object().expect("message object") {
            if name == key {
                map.serialize_entry(
                    name,
                    &Content {
                        value: content,
                        item: self.index,
                        plan: self.plan,
                    },
                )?;
            } else {
                map.serialize_entry(name, value)?;
            }
        }
        map.end()
    }
}
struct Content<'a> {
    value: &'a Value,
    item: usize,
    plan: &'a Plan,
}
impl Serialize for Content<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        // Normalize ALL eligible strings every time so earlier prefixes do not
        // flip between string/array shapes when their marker moves forward.
        if let Some(text) = self.value.as_str() {
            let mut seq = serializer.serialize_seq(Some(1))?;
            seq.serialize_element(&Text {
                text,
                marked: self.plan.marker(self.item, 0),
            })?;
            return seq.end();
        }
        let blocks = self.value.as_array().expect("content array");
        let mut seq = serializer.serialize_seq(Some(blocks.len()))?;
        for (index, value) in blocks.iter().enumerate() {
            seq.serialize_element(&Block {
                value,
                marked: self.plan.marker(self.item, index),
            })?;
        }
        seq.end()
    }
}
struct Text<'a> {
    text: &'a str,
    marked: bool,
}
impl Serialize for Text<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("type", "input_text")?;
        map.serialize_entry("text", self.text)?;
        if self.marked {
            map.serialize_entry("prompt_cache_breakpoint", &json!({"mode":"explicit"}))?;
        }
        map.end()
    }
}
struct Block<'a> {
    value: &'a Value,
    marked: bool,
}
impl Serialize for Block<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        if !supported(self.value) {
            return self.value.serialize(serializer);
        }
        let mut map = serializer.serialize_map(None)?;
        for (key, value) in self.value.as_object().expect("content block") {
            // Own annotations: at most two explicit writes plus implicit write.
            if key != "prompt_cache_breakpoint" {
                map.serialize_entry(key, value)?;
            }
        }
        if self.marked {
            map.serialize_entry("prompt_cache_breakpoint", &json!({"mode":"explicit"}))?;
        }
        map.end()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn wire(history: &[Arc<Value>], plan: Plan) -> Value {
        serde_json::to_value(super::super::Input::new(history,
            json!({"self":{"id":"self","name":"captain"},"peers":[{"name":"must not leak"}],"working_context":{"text":"Captain","editor":null}})
        ).with_cache(plan)).unwrap()
    }
    fn marked(v: &Value) -> bool {
        v.get("prompt_cache_breakpoint").is_some()
    }
    fn strip(v: &mut Value) {
        match v {
            Value::Object(m) => {
                m.remove("prompt_cache_breakpoint");
                for v in m.values_mut() {
                    strip(v);
                }
            }
            Value::Array(a) => {
                for v in a {
                    strip(v);
                }
            }
            _ => (),
        }
    }
    #[test]
    fn retains_prior_boundary_in_extended_tool_group_and_borrows_history() {
        let mut history: Vec<Arc<Value>> = vec![
            Arc::new(json!({"role":"user","content":"original question"})),
            Arc::new(json!({"type":"reasoning","encrypted_content":"opaque","id":"reason-1"})),
            Arc::new(
                json!({"type":"function_call","call_id":"a","name":"read_file","arguments":"{}"}),
            ),
            Arc::new(
                json!({"type":"function_call","call_id":"b","name":"read_file","arguments":"{}"}),
            ),
            Arc::new(
                json!({"type":"function_call_output","call_id":"a","output":"exact result A"}),
            ),
        ];
        let original = serde_json::to_value(&history).unwrap();
        let first = Plan::new(&history, None, true);
        let prior = first.next().unwrap();
        let one = wire(&history, first);
        assert!(marked(&one[4]["output"][0]));
        assert_eq!(one[1], original[1]);
        assert_eq!(one[2], original[2]);
        assert_eq!(one[3], original[3]);
        assert!(!one.to_string().contains("must not leak"));
        assert_eq!(serde_json::to_value(&history).unwrap(), original);
        history.push(Arc::new(
            json!({"type":"function_call_output","call_id":"b","output":"exact result B"}),
        ));
        let second = Plan::new(&history, Some(&prior), true);
        let prior2 = second.next().unwrap();
        let two = wire(&history, second);
        // A is no longer the last consecutive tool output: it remains explicitly
        // available, independent of implicit message-ending eligibility.
        assert!(marked(&two[4]["output"][0]));
        assert!(marked(&two[5]["output"][0]));
        assert_eq!(&one.as_array().unwrap()[..5], &two.as_array().unwrap()[..5]);
        history.push(Arc::new(json!({"role":"user","content":"next question"})));
        let three = wire(&history, Plan::new(&history, Some(&prior2), true));
        assert!(!marked(&three[4]["output"][0]));
        assert!(marked(&three[5]["output"][0]));
        assert!(marked(&three[6]["content"][0]));
        let mut old_prefix = two.as_array().unwrap()[..6].to_vec();
        let mut new_prefix = three.as_array().unwrap()[..6].to_vec();
        for v in &mut old_prefix {
            strip(v);
        }
        for v in &mut new_prefix {
            strip(v);
        }
        assert_eq!(
            old_prefix, new_prefix,
            "normalization remains stable as markers move"
        );
        assert_eq!(three[5]["output"][0]["text"], "exact result B");
    }
    #[test]
    fn marks_supported_blocks_preserves_images_and_rejects_stale_anchor() {
        let history = vec![Arc::new(json!({"role":"user","content":[
            {"type":"input_text","text":"describe"},
            {"type":"input_image","image_url":"data:image/png;base64,fixture","detail":"original"}
        ]}))];
        let plan = Plan::new(&history, None, true);
        let previous = plan.next().unwrap();
        let out = wire(&history, plan);
        assert!(marked(&out[0]["content"][1]));
        assert_eq!(
            out[0]["content"][1]["image_url"],
            history[0]["content"][1]["image_url"]
        );
        assert_eq!(out[0]["content"][1]["detail"], "original");
        let reset = vec![
            Arc::new(json!({"role":"user","content":"compacted"})),
            Arc::new(json!({"role":"user","content":"new request"})),
        ];
        let out = wire(&reset, Plan::new(&reset, Some(&previous), true));
        assert!(!marked(&out[0]["content"][0]));
        assert!(marked(&out[1]["content"][0]));
    }
    #[test]
    fn unsupported_routes_and_models_keep_original_history_shape() {
        let history = vec![Arc::new(
            json!({"type":"function_call_output","call_id":"x","output":"raw"}),
        )];
        for (model, route) in [
            ("gpt-6-sol", "chatgpt"),
            ("gpt-5", "api_key"),
            ("other", "api_key"),
        ] {
            assert!(!enabled(model, &json!({"billing_route":route})));
            assert_eq!(
                wire(&history, Plan::new(&history, None, false))[0],
                *history[0]
            );
        }
        assert!(enabled("gpt-6-sol", &json!({"billing_route":"api_key"})));
    }
}
