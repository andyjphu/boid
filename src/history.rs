use super::*;

const PAGE_ENTRIES: usize = 80;
const PAGE_BYTES: usize = 64 * 1024;
const PREVIEW_BYTES: usize = 8192;

fn preview(value: &mut Value) {
    let Some(text) = value["text"].as_str() else {
        return;
    };
    if text.len() <= PREVIEW_BYTES {
        return;
    }
    let bytes = text.len();
    let mut end = PREVIEW_BYTES;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let text = text[..end].to_owned();
    value["text"] = json!(text);
    value["text_complete"] = json!(false);
    value["text_bytes"] = json!(bytes);
}

// Capture only shared entry payloads and matching metadata while the roster is
// locked. Preview copying and JSON encoding use this owned snapshot afterwards.
pub(super) struct HistoryPage {
    view: View,
    start: usize,
    end: usize,
    total: usize,
    seq: u64,
    superseded: Vec<bool>,
}
impl HistoryPage {
    pub(super) fn into_value(mut self) -> Value {
        let lengths: Vec<_> = self
            .view
            .entries
            .iter_mut()
            .map(|entry| {
                let bytes = entry.text.len();
                if bytes <= PREVIEW_BYTES {
                    return None;
                }
                let mut end = PREVIEW_BYTES;
                while !entry.text.is_char_boundary(end) {
                    end -= 1;
                }
                entry.text = entry.text[..end].to_owned().into();
                Some(bytes)
            })
            .collect();
        // Bound peer text before serializing it, preserving the same preview
        // metadata without first copying an entire large communication.
        let peer_lengths: Vec<_> = self
            .view
            .communications
            .iter_mut()
            .map(|message| {
                let bytes = message.text.len();
                if bytes <= PREVIEW_BYTES {
                    return None;
                }
                let mut end = PREVIEW_BYTES;
                while !message.text.is_char_boundary(end) {
                    end -= 1;
                }
                message.text = message.text[..end].to_owned().into();
                Some(bytes)
            })
            .collect();
        let mut value = json!(self.view);
        for ((entry, bytes), superseded) in value["entries"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .zip(lengths)
            .zip(self.superseded)
        {
            if let Some(bytes) = bytes {
                entry["text_complete"] = json!(false);
                entry["text_bytes"] = json!(bytes);
            }
            entry["superseded"] = json!(superseded);
        }
        for (message, bytes) in value["communications"]
            .as_array_mut()
            .into_iter()
            .flatten()
            .zip(peer_lengths)
        {
            if let Some(bytes) = bytes {
                message["text_complete"] = json!(false);
                message["text_bytes"] = json!(bytes);
            }
        }
        value["history"] =
            json!({"start":self.start,"end":self.end,"total":self.total,"seq":self.seq});
        value
    }
}

impl Hub {
    // Positions are stable: context clearing retains the visible transcript.
    // Whole entries remain intact, including a single entry larger than the byte budget.
    pub(super) fn capture_history_view(
        data: &Data,
        source: &View,
        before: Option<usize>,
        after: Option<usize>,
        around: Option<&str>,
    ) -> Result<HistoryPage> {
        if [before.is_some(), after.is_some(), around.is_some()]
            .into_iter()
            .filter(|v| *v)
            .count()
            > 1
        {
            bail!("Choose one history cursor.");
        }
        let total = source.entries.len();
        let (start, end) = if let Some(id) = around {
            let index = source
                .entries
                .iter()
                .position(|e| e.id == id)
                .context("History entry is unavailable.")?;
            let mut start = index;
            let mut end = index + 1;
            let mut bytes = source.entries[index].text.len().min(PREVIEW_BYTES);
            while start > 0 && index - start < PAGE_ENTRIES / 2 && bytes < PAGE_BYTES / 2 {
                let size = source.entries[start - 1].text.len().min(PREVIEW_BYTES);
                if bytes + size > PAGE_BYTES / 2 {
                    break;
                }
                start -= 1;
                bytes += size;
            }
            while end < total && end - start < PAGE_ENTRIES {
                let size = source.entries[end].text.len().min(PREVIEW_BYTES);
                if bytes + size > PAGE_BYTES {
                    break;
                }
                end += 1;
                bytes += size;
            }
            (start, end)
        } else if let Some(after) = after {
            if after > total {
                bail!("History cursor is out of range.");
            }
            let mut end = after;
            let mut bytes = 0;
            while end < total && end - after < PAGE_ENTRIES {
                let size = source.entries[end].text.len().min(PREVIEW_BYTES);
                if end > after && bytes + size > PAGE_BYTES {
                    break;
                }
                bytes += size;
                end += 1;
            }
            (after, end)
        } else {
            let end = before.unwrap_or(total);
            if end > total {
                bail!("History cursor is out of range.");
            }
            let mut start = end;
            let mut bytes = 0;
            while start > 0 && end - start < PAGE_ENTRIES {
                let size = source.entries[start - 1].text.len().min(PREVIEW_BYTES);
                if start < end && bytes + size > PAGE_BYTES {
                    break;
                }
                bytes += size;
                start -= 1;
            }
            (start, end)
        };
        let view = Self::project_entries(data, source, &source.entries[start..end]);
        let replaced: std::collections::HashSet<&str> = source
            .entries
            .iter()
            .filter_map(|entry| entry.replaces.as_deref())
            .collect();
        let superseded = view
            .entries
            .iter()
            .map(|entry| replaced.contains(entry.id.as_str()))
            .collect();
        Ok(HistoryPage {
            view,
            start,
            end,
            total,
            seq: data.seq,
            superseded,
        })
    }

    pub fn history(
        &self,
        id: &str,
        before: Option<usize>,
        after: Option<usize>,
        around: Option<&str>,
    ) -> Result<Value> {
        crate::blocking::run(|| {
            let data = self.0.data.lock().unwrap();
            let source = &data
                .sessions
                .get(id)
                .filter(|h| !h.record.view.archived)
                .context("Unknown or archived session.")?
                .record
                .view;
            let page = Self::capture_history_view(&data, source, before, after, around)?;
            drop(data);
            Ok(page.into_value())
        })
    }
    pub fn history_list(&self, archived: bool) -> Vec<Value> {
        if archived {
            return vec![];
        }
        crate::blocking::run(|| {
            let data = self.0.data.lock().unwrap();
            let pages: Vec<_> = data
                .sessions
                .values()
                .filter(|h| h.record.view.archived == archived)
                .map(|h| {
                    Self::capture_history_view(&data, &h.record.view, None, None, None)
                        .expect("latest history")
                })
                .collect();
            drop(data);
            pages.into_iter().map(HistoryPage::into_value).collect()
        })
    }
    pub fn history_entry(&self, id: &str, entry: &str) -> Result<Value> {
        crate::blocking::run(|| {
            let data = self.0.data.lock().unwrap();
            let source = &data
                .sessions
                .get(id)
                .filter(|h| !h.record.view.archived)
                .context("Unknown or archived session.")?
                .record
                .view;
            let index = source
                .entries
                .iter()
                .position(|e| e.id == entry)
                .context("History entry is unavailable.")?;
            let projection =
                Self::project_entries(&data, source, &source.entries[index..index + 1]);
            let (total, seq) = (source.entries.len(), data.seq);
            drop(data);
            let mut view = json!(projection);
            view["history"] = json!({"start":index,"end":index+1,"total":total,"seq":seq});
            Ok(view)
        })
    }
    pub fn recent_event(mut event: Value) -> Value {
        if event["type"] == "entry" {
            preview(&mut event["entry"]);
        }
        if event["type"] == "communication" {
            preview(&mut event["message"]);
        }
        if event["type"] != "session" {
            return event;
        }
        let seq = event["seq"].clone();
        let view = &mut event["session"];
        if let Some(entries) = view["entries"].as_array_mut() {
            let total = entries.len();
            let mut start = total;
            let mut bytes = 0;
            while start > 0 && total - start < PAGE_ENTRIES {
                let size = entries[start - 1]["text"]
                    .as_str()
                    .unwrap_or("")
                    .len()
                    .min(PREVIEW_BYTES);
                if start < total && bytes + size > PAGE_BYTES {
                    break;
                }
                bytes += size;
                start -= 1;
            }
            entries.drain(..start);
            for entry in entries.iter_mut() {
                preview(entry);
            }
            let ids: std::collections::HashSet<String> = entries
                .iter()
                .filter_map(|e| e["id"].as_str().map(str::to_owned))
                .collect();
            if let Some(messages) = view["communications"].as_array_mut() {
                messages.retain(|m| {
                    ids.contains(m["id"].as_str().unwrap_or(""))
                        || ids.contains(m["sender_entry"].as_str().unwrap_or(""))
                });
            }
            for message in view["communications"].as_array_mut().into_iter().flatten() {
                preview(message);
            }
            view["history"] = json!({"start":start,"end":total,"total":total,"seq":seq});
        }
        event
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn captured_history_allows_admission_before_encoding_and_retains_its_boundary() {
        let work = tempfile::tempdir().unwrap();
        let hub = Hub::new(Client::test_client()).unwrap();
        let view = hub
            .create(Create {
                name: "snapshot-reader".into(),
                cwd: work.path().to_string_lossy().into_owned(),
                message: String::new(),
            })
            .unwrap();
        // Persisted-like fixture, with no worker/provider dispatch.
        hub.entry(&view.id, "assistant", "existing transcript");
        let task = hub
            .0
            .data
            .lock()
            .unwrap()
            .sessions
            .get_mut(&view.id)
            .unwrap()
            .task
            .take();
        if let Some(task) = task {
            task.abort();
            let _ = task.await;
        }
        let (page, seq) = {
            let data = hub.0.data.lock().unwrap();
            (
                Hub::capture_history_view(
                    &data,
                    &data.sessions[&view.id].record.view,
                    None,
                    None,
                    None,
                )
                .unwrap(),
                data.seq,
            )
        };
        let prior_count = page.view.entries.len();
        // Intentionally hold the unencoded snapshot while another request is
        // admitted. No Data guard or borrow is retained by the captured page.
        let writer = hub.clone();
        let id = view.id.clone();
        let accepted = tokio::time::timeout(
            Duration::from_secs(2),
            tokio::task::spawn_blocking(move || writer.message(&id, "new input".into())),
        )
        .await
        .unwrap()
        .unwrap()
        .unwrap();
        let encoded = page.into_value();
        assert_eq!(encoded["history"]["seq"], seq);
        assert_eq!(encoded["entries"].as_array().unwrap().len(), prior_count);
        assert!(
            !encoded["entries"]
                .as_array()
                .unwrap()
                .iter()
                .any(|entry| entry["id"] == accepted)
        );
        assert!(hub.view(&view.id).unwrap().pending.contains(&accepted));
        hub.shutdown().await;
    }

    #[tokio::test]
    async fn recent_history_pages_preserve_records_and_sequence_boundaries() {
        let state = tempfile::tempdir().unwrap();
        let work = tempfile::tempdir().unwrap();
        let hub = Hub::open(Client::test_client(), state.path().into()).unwrap();
        let view = hub
            .create(Create {
                name: "history".into(),
                cwd: work.path().to_string_lossy().into_owned(),
                message: String::new(),
            })
            .unwrap();
        hub.entry(&view.id, "assistant", "retained fixture work");
        // Populate one durable record, without thousands of artificial fsyncs.
        hub.update(&view.id, |r| {
            r.view.entries = (0..250)
                .map(|i| Entry {
                    id: format!("e{i}"),
                    kind: "assistant".into(),
                    text: format!("Message {i}").into(),
                    images: vec![],
                    replaces: None,
                    input_batch: None,
                })
                .collect();
            r.view.entries[20].text = "界".repeat(100_000).into();
            r.view.entries[249].replaces = Some("e4".into());
            Ok(())
        })
        .unwrap();
        let (snapshot, mut events) = hub.subscribe_history(true);
        assert_eq!(
            snapshot["sessions"][0]["entries"].as_array().unwrap().len(),
            80
        );
        assert_eq!(snapshot["sessions"][0]["history"]["start"], 170);
        assert_eq!(snapshot["sessions"][0]["history"]["total"], 250);
        assert!(snapshot.to_string().len() < 32_000);
        let mut before = 250;
        let mut ids = vec![];
        while before > 0 {
            let page = hub.history(&view.id, Some(before), None, None).unwrap();
            assert!(page.to_string().len() < 100_000);
            ids.extend(
                page["entries"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|e| e["id"].as_str().unwrap().to_owned()),
            );
            before = page["history"]["start"].as_u64().unwrap() as usize;
        }
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), 250);
        let around = hub.history(&view.id, None, None, Some("e20")).unwrap();
        let last = around["entries"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["id"] == "e20")
            .unwrap();
        assert_eq!(last["id"], "e20");
        assert_eq!(last["text_complete"], false);
        assert_eq!(last["text"], "界".repeat(PREVIEW_BYTES / 3));
        assert_eq!(last["text_bytes"], 300_000);
        assert_eq!(
            hub.history_entry(&view.id, "e20").unwrap()["entries"][0]["text"]
                .as_str()
                .unwrap()
                .len(),
            300_000
        );
        assert_eq!(hub.view(&view.id).unwrap().entries.len(), 250);
        assert!(hub.history(&view.id, Some(251), None, None).is_err());
        assert!(hub.history(&view.id, Some(1), Some(2), None).is_err());
        assert!(hub.history(&view.id, None, None, Some("missing")).is_err());
        assert_eq!(
            hub.history(&view.id, None, Some(250), None).unwrap()["entries"],
            json!([])
        );
        assert_eq!(
            hub.history(&view.id, None, None, Some("e4")).unwrap()["entries"]
                .as_array()
                .unwrap()
                .iter()
                .find(|e| e["id"] == "e4")
                .unwrap()["superseded"],
            true
        );
        hub.entry(&view.id, "assistant", "live");
        let event = events.try_recv().unwrap();
        assert_eq!(event["entry_index"], 250);
        assert!(event["seq"].as_u64() > snapshot["seq"].as_u64());
        let latest = hub.history(&view.id, None, None, None).unwrap();
        assert!(latest["history"]["seq"].as_u64() >= event["seq"].as_u64());
        hub.shutdown().await;
    }
}
