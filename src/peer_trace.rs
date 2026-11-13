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
}

impl Hub {
    pub fn read_peer_trace(&self, reader: &str, arguments: &str) -> Value {
        let result = (|| -> Result<Value> {
            let args: ReadTrace = serde_json::from_str(arguments)?;
            let limit = args.limit.unwrap_or(20);
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
            crate::blocking::run(|| {
                // Snapshot the live view under the same lock used by incoming deltas.
                // Never reopen a saved conversation or wake the peer to inspect it.
                let data = self.0.data.lock().unwrap();
                if !data.sessions.contains_key(reader) {
                    bail!("Reader session is unavailable.");
                }
                let target = args.to.trim();
                let handle = if let Some(handle) = data.sessions.get(target) {
                    handle
                } else {
                    let mut matches = data
                        .sessions
                        .values()
                        .filter(|h| h.record.view.name.eq_ignore_ascii_case(target));
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
                let visible = |entry: &Entry| {
                    TRACE_KINDS.contains(&entry.kind.as_str())
                        && !superseded.contains(entry.id.as_str())
                        && kind.is_none_or(|kind| entry.kind == kind)
                };
                let mut entries: Vec<Value> = Vec::new();
                let mut next_before = None;
                if let Some(id) = args.entry_id.as_deref() {
                    let (index, entry) = view
                        .entries
                        .iter()
                        .enumerate()
                        .find(|(_, e)| e.id == id)
                        .context("Trace entry is unavailable.")?;
                    if !visible(entry) {
                        bail!("Entry is hidden, superseded, or does not match kind.");
                    }
                    let start = args.offset.unwrap_or(0);
                    if !entry.text.is_char_boundary(start) {
                        bail!("Invalid UTF-8 byte offset. Use the returned next_offset.");
                    }
                    let mut end = start.saturating_add(TRACE_BYTES).min(entry.text.len());
                    while !entry.text.is_char_boundary(end) {
                        end -= 1;
                    }
                    entries.push(trace_entry(&data, view, entry, index, start, end));
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
                Ok(json!({
                    "peer":{"id":view.id,"name":view.name,"status":view.status,"archived":view.archived},
                    "source":"live_memory","snapshot_seq":data.seq,"stream_seq":handle.record.stream_seq,
                    "observed_at_ms":SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis(),
                    "entries":entries,"next_before":next_before,"text_byte_limit":TRACE_BYTES,
                }))
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
    let mut value = json!({"id":entry.id,"index":index,"kind":entry.kind,"text":&entry.text[start..end],
        "text_start":start,"text_end":end,"text_bytes":entry.text.len(),
        "text_complete":start == 0 && end == entry.text.len(),
        "next_offset":(end < entry.text.len()).then_some(end)});
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
