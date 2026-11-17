use super::*;
use std::collections::HashSet;

const TRACE_BYTES: usize = 16 * 1024;
const TRACE_KINDS: &[&str] = &[
    "assistant",
    "user",
    "peer",
    "tool_call",
    "tool_result",
    "tool_arguments",
    "context",
    "skill",
    "notice",
    "error",
];

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReadTrace {
    to: String,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    before: Option<usize>,
    #[serde(default)]
    entry_id: Option<String>,
    #[serde(default)]
    offset: Option<usize>,
    #[serde(default)]
    related: Option<String>,
}

impl Hub {
    pub fn read_peer_trace(&self, reader: &str, arguments: &str) -> Value {
        let result = (|| -> Result<Value> {
            let args: ReadTrace = serde_json::from_str(arguments)?;
            let limit = args.limit.unwrap_or(10);
            if !(1..=100).contains(&limit) {
                bail!("limit must be between 1 and 100.");
            }
            let kind = args.kind.as_deref().filter(|kind| *kind != "all");
            if kind.is_some_and(|kind| !TRACE_KINDS.contains(&kind)) {
                bail!(
                    "Unknown trace kind. Reasoning and private model context are not part of the visible trace."
                );
            }
            if args.entry_id.is_some() && args.before.is_some() {
                bail!("Use entry_id or before, not both.");
            }
            if args.offset.is_some() && args.entry_id.is_none() {
                bail!("offset requires entry_id.");
            }
            if let Some(related) = args.related.as_deref() {
                if args.entry_id.is_none() || !matches!(related, "call" | "result") {
                    bail!("related requires entry_id and must be call or result.");
                }
            }
            crate::blocking::run(|| {
                // Snapshot the live view under the same lock used by incoming deltas.
                // Never reopen a saved conversation or wake the peer to inspect it.
                let data = self.0.data.lock().unwrap();
                if !data
                    .sessions
                    .get(reader)
                    .is_some_and(|h| !h.record.view.archived)
                {
                    bail!("Reader session is unavailable.");
                }
                let target = args.to.trim();
                let handle = if let Some(handle) = data
                    .sessions
                    .get(target)
                    .filter(|h| !h.record.view.archived)
                {
                    handle
                } else {
                    let mut matches = data.sessions.values().filter(|h| {
                        !h.record.view.archived && h.record.view.name.eq_ignore_ascii_case(target)
                    });
                    let first = matches
                        .next()
                        .context("Unknown peer. Use list_agents for open session IDs and names.")?;
                    if matches.next().is_some() {
                        bail!("Peer name is ambiguous. Read by session ID.");
                    }
                    first
                };
                let view = &handle.record.view;
                if view.id == reader {
                    bail!("Address another session.");
                }
                let superseded: HashSet<_> = view
                    .entries
                    .iter()
                    .filter_map(|e| e.replaces.as_deref())
                    .collect();
                let visible_base = |entry: &Entry| {
                    TRACE_KINDS.contains(&entry.kind.as_str())
                        && !superseded.contains(entry.id.as_str())
                };
                let visible = |entry: &Entry| {
                    visible_base(entry) && kind.is_none_or(|kind| entry.kind == kind)
                };
                let mut entries: Vec<Value> = Vec::new();
                let mut next_before = None;
                let mut related_not_found = None;
                if let Some(id) = args.entry_id.as_deref() {
                    let source = id
                        .parse::<usize>()
                        .ok()
                        .and_then(|index| view.entries.get(index).map(|entry| (index, entry)))
                        .or_else(|| view.entries.iter().enumerate().find(|(_, e)| e.id == id))
                        .context("Trace entry is unavailable.")?;
                    if !visible_base(source.1) {
                        bail!("Entry is hidden or superseded.");
                    }
                    let selected = if let Some(related) = args.related.as_deref() {
                        let key = tool_entry_key(source.1)
                            .context("Related lookup requires a recorded tool call or result.")?;
                        let requested_kind = if related == "call" {
                            "tool_call"
                        } else {
                            "tool_result"
                        };
                        let matched = view.entries.iter().enumerate().rev().find(|(_, entry)| {
                            entry.kind == requested_kind
                                && visible_base(entry)
                                && tool_entry_key(entry) == Some(key)
                        });
                        if matched.is_none() {
                            related_not_found = Some(related);
                        }
                        matched
                    } else {
                        if !visible(source.1) {
                            bail!("Entry does not match kind.");
                        }
                        Some(source)
                    };
                    if let Some((index, entry)) = selected {
                        let start = args.offset.unwrap_or(0);
                        if !entry.text.is_char_boundary(start) {
                            bail!("Invalid UTF-8 byte offset. Use the returned next_offset.");
                        }
                        let mut end = start.saturating_add(TRACE_BYTES).min(entry.text.len());
                        while !entry.text.is_char_boundary(end) {
                            end -= 1;
                        }
                        entries.push(trace_entry(&data, view, entry, index, start, end));
                    }
                } else {
                    let end = args.before.unwrap_or(view.entries.len());
                    if end > view.entries.len() {
                        bail!("before is beyond this trace. Use the returned next_before.");
                    }
                    let mut remaining = TRACE_BYTES;
                    for (index, entry) in view.entries[..end]
                        .iter()
                        .enumerate()
                        .rev()
                        .filter(|(_, e)| visible(e))
                    {
                        if entries.len() == limit || remaining == 0 {
                            next_before = entries.last().and_then(|e| e["index"].as_u64());
                            break;
                        }
                        // Prefer the newest text, including the tail of a long streaming message.
                        let mut start = entry.text.len().saturating_sub(remaining);
                        while !entry.text.is_char_boundary(start) {
                            start += 1;
                        }
                        if start == entry.text.len() && !entry.text.is_empty() {
                            next_before = entries.last().and_then(|e| e["index"].as_u64());
                            break;
                        }
                        remaining -= entry.text.len() - start;
                        entries.push(trace_entry(
                            &data,
                            view,
                            entry,
                            index,
                            start,
                            entry.text.len(),
                        ));
                    }
                    entries.reverse();
                }
                let mut result = json!({
                    "peer":{"id":view.id,"name":view.name,"status":view.status},
                    "entries":entries,
                });
                if let Some(before) = next_before {
                    result["next_before"] = json!(before);
                }
                if let Some(related) = related_not_found {
                    result["related_not_found"] = json!(related);
                }
                Ok(result)
            })
        })();
        result.unwrap_or_else(|error| json!({"error":format!("read_peer_trace: {error}")}))
    }
}

fn trace_entry(
    data: &Data,
    view: &View,
    entry: &Entry,
    index: usize,
    start: usize,
    end: usize,
) -> Value {
    let mut value = json!({"index":index,"kind":entry.kind,"text":&entry.text[start..end]});
    if start > 0 || end < entry.text.len() {
        value["truncated"] = json!(true);
        value["offset"] = json!(start);
    }
    if end < entry.text.len() {
        value["next_offset"] = json!(end);
    }
    if !entry.images.is_empty() {
        value["images"] = json!(entry.images);
    }
    // Attribute peer exchanges without duplicating the message body in the result.
    if let Some(message) = data.messages.values().find(|message| {
        (message.recipient_id == view.id && message.id == entry.id)
            || (message.sender_id == view.id && message.sender_entry == entry.id)
    }) {
        value["peer_message"] = json!({"sender_id":message.sender_id,"sender_name":message.sender_name,
            "recipient_id":message.recipient_id,"recipient_name":message.recipient_name,
            "urgency":message.urgency,"state":message.state});
    }
    value
}

fn tool_entry_key(entry: &Entry) -> Option<&str> {
    if !matches!(entry.kind.as_str(), "tool_call" | "tool_result") {
        return None;
    }
    let (header, _) = entry.text.split_once('\n')?;
    (header.ends_with(')') && header.contains(" (")).then_some(header)
}
