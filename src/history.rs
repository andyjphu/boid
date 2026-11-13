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

impl Hub {
    // Positions are stable: context clearing retains the visible transcript.
    // Whole entries remain intact, including a single entry larger than the byte budget.
    pub(super) fn history_view(
        data: &Data,
        source: &View,
        before: Option<usize>,
        after: Option<usize>,
        around: Option<&str>,
    ) -> Result<Value> {
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
        let mut view = json!(Self::project_entries(
            data,
            source,
            &source.entries[start..end]
        ));
        let superseded: std::collections::HashSet<&str> = source
            .entries
            .iter()
            .filter_map(|e| e.replaces.as_deref())
            .collect();
        for entry in view["entries"].as_array_mut().unwrap() {
            entry["superseded"] = json!(
                entry["id"]
                    .as_str()
                    .is_some_and(|id| superseded.contains(id))
            );
            preview(entry);
        }
        for message in view["communications"].as_array_mut().into_iter().flatten() {
            preview(message);
        }
        view["history"] = json!({"start":start,"end":end,"total":total,"seq":data.seq});
        Ok(view)
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
                .context("Unknown session.")?
                .record
                .view;
            Self::history_view(&data, source, before, after, around)
        })
    }
    pub fn history_list(&self, archived: bool) -> Vec<Value> {
        crate::blocking::run(|| {
            let data = self.0.data.lock().unwrap();
            data.sessions
                .values()
                .filter(|h| h.record.view.archived == archived)
                .map(|h| {
                    Self::history_view(&data, &h.record.view, None, None, None)
                        .expect("latest history")
                })
                .collect()
        })
    }
    pub fn history_entry(&self, id: &str, entry: &str) -> Result<Value> {
        crate::blocking::run(|| {
            let data = self.0.data.lock().unwrap();
            let source = &data
                .sessions
                .get(id)
                .context("Unknown session.")?
                .record
                .view;
            let index = source
                .entries
                .iter()
                .position(|e| e.id == entry)
                .context("History entry is unavailable.")?;
            let mut view = json!(Self::project_entries(
                &data,
                source,
                &source.entries[index..index + 1]
            ));
            view["history"] =
                json!({"start":index,"end":index+1,"total":source.entries.len(),"seq":data.seq});
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
        // Populate one durable record, without thousands of artificial fsyncs.
        hub.update(&view.id, |r| {
            r.view.entries = (0..250)
                .map(|i| Entry {
                    id: format!("e{i}"),
                    kind: "assistant".into(),
                    text: format!("Message {i}"),
                    replaces: None,
                    input_batch: None,
                })
                .collect();
            r.view.entries[20].text = "界".repeat(100_000);
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
        assert!(last["text"].as_str().unwrap().len() <= PREVIEW_BYTES);
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
