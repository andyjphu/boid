//! Operator-requested context copies, applied by the recipient's own worker.
use super::*;

pub(super) struct Copy {
    pub name: String,
    pub history: Vec<Arc<Value>>,
}

pub(super) fn command_source(data: &Data, target: &str, text: &str) -> Result<Option<String>> {
    let mut words = text.split_whitespace();
    if words.next() != Some("/take-context") {
        return Ok(None);
    }
    let source = words.next().context(
        "Use /take-context in the web composer to select a peer, or /take-context AGENT_ID.",
    )?;
    if words.next().is_some() {
        bail!("Use /take-context AGENT_ID.");
    }
    let source = if data
        .sessions
        .get(source)
        .is_some_and(|h| !h.record.view.archived)
    {
        source.to_owned()
    } else {
        let mut matches = data
            .sessions
            .values()
            .filter(|h| !h.record.view.archived && h.record.view.name.eq_ignore_ascii_case(source));
        let found = matches.next().context("Unknown open peer.")?;
        if matches.next().is_some() {
            bail!("Peer name is ambiguous; use its agent ID.");
        }
        found.record.view.id.clone()
    };
    if source == target {
        bail!("Select another agent.");
    }
    Ok(Some(source))
}

impl Hub {
    pub fn context_peers(&self, id: &str) -> Result<Value> {
        crate::blocking::run(|| {
            let data = self.0.data.lock().unwrap();
            data.sessions
                .get(id)
                .filter(|h| !h.record.view.archived)
                .context("Unknown open agent.")?;
            let mut peers: Vec<_> = data
                .sessions
                .values()
                .filter(|h| h.record.view.id != id && !h.record.view.archived && !h.archiving)
                .map(|h| json!({"id":h.record.view.id,"name":h.record.view.name}))
                .collect();
            peers.sort_by_key(|peer| {
                (
                    peer["name"].as_str().unwrap().to_lowercase(),
                    peer["id"].as_str().unwrap().to_owned(),
                )
            });
            Ok(json!({"peers":peers}))
        })
    }

    pub(super) fn stage_context_copy(&self, id: &str) -> Option<(String, Result<Copy>)> {
        let (key, snapshot) = {
            let data = self.0.data.lock().unwrap();
            let target = &data.sessions.get(id)?.record;
            let input = target
                .inputs
                .front()
                .filter(|input| !target.paused && input.order > target.stop_through)?;
            let source_id = input.take_context.as_ref()?;
            let snapshot = (|| -> Result<_> {
                let source = data
                    .sessions
                    .get(source_id)
                    .filter(|h| !h.record.view.archived && !h.archiving)
                    .context("Selected peer is no longer open.")?;
                Ok((
                    source_id.clone(),
                    source.record.view.name.clone(),
                    target.view.name.clone(),
                    source.record.engine.history_items().to_vec(),
                ))
            })();
            (input.id.clone(), snapshot)
        };
        let copy = (|| -> Result<Copy> {
            let (source_id, name, target_name, history) = snapshot?;
            let mut history = self.0.images.copy_references(&source_id, id, &history)?;
            // Snapshot only committed history. A source can keep working; its
            // unfinished calls must not be executed or presented as completed here.
            let mut pending = BTreeSet::new();
            for item in &history {
                if let Some(call) = item["call_id"].as_str() {
                    if item["type"] == "function_call" {
                        pending.insert(call.to_owned());
                    } else if item["type"] == "function_call_output" {
                        pending.remove(call);
                    }
                }
            }
            for call in pending {
                history.push(Arc::new(json!({"type":"function_call_output","call_id":call,
                    "output":"Result unavailable in this copied snapshot. The source agent may still be executing this call. This agent did not run it; inspect the source before relying on an outcome or repeating an action."})));
            }
            history.push(Arc::new(json!({"role":"developer","content":format!(
                "The operator replaced your conversation context with a snapshot from {name} ({source_id}). Earlier first-person messages and actions belong to that source agent. You remain {target_name} ({id}), with your own role, tools, configuration and credentials. No source tool call is queued for execution here. Wait for the operator's next instruction."
            )})));
            Ok(Copy { name, history })
        })();
        Some((key, copy))
    }
}
