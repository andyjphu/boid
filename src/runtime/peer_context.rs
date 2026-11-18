//! Rare peer recovery, applied by the worker that owns the live context.
use super::*;
use tokio::sync::oneshot;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ClearArgs {
    to: String,
    reason: String,
}

pub(super) struct ClearRequest {
    from: String,
    reason: String,
    error_id: String,
    pub reply: oneshot::Sender<Result<Value>>,
}

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct ClearReceipt {
    error_id: String,
    archive_id: String,
    by: WorkingContextEditor,
    reason: String,
    removed: usize,
    retained: usize,
}

impl ClearReceipt {
    fn result(&self, already_cleared: bool) -> Value {
        json!({"cleared":true, "already_cleared":already_cleared,
            "removed":self.removed, "retained":self.retained,
            "by":self.by, "resumed":false,
            "message":"Transcript and role context preserved. Target remains paused. Inspect retained context before an authorized wake_agent request."})
    }
}

fn eligible_error<'a>(data: &'a Data, from: &str, to: &str) -> Result<&'a str> {
    if data.closing {
        bail!("Server is shutting down.");
    }
    if from == to {
        bail!("Address another session.");
    }
    data.sessions
        .get(from)
        .filter(|h| !h.record.view.archived && !h.archiving && h.storage_error.is_none())
        .context("Sender is unavailable.")?;
    let h = data
        .sessions
        .get(to)
        .context("Unknown recipient session.")?;
    if h.record.view.archived || h.archiving || h.storage_error.is_some() {
        bail!("Recipient is ended, archiving or has a storage failure.");
    }
    let r = &h.record;
    // Clean startup presents held sessions as stopped but preserves the reason
    // for the hold. The persisted model-error flag, not the label, is authority.
    if !matches!(r.view.status.as_str(), "error" | "stopped")
        || !r.paused
        || !r.peer_restartable.unwrap_or(false)
    {
        bail!(
            "Emergency context clearing requires a peer paused by a model-request error. Operator Stop and uncertain recovery cannot be overridden."
        );
    }
    if !r.inputs.is_empty() || r.engine.has_unfinished_execution() {
        bail!("Recipient has pending input or unfinished execution; context was not changed.");
    }
    r.view
        .entries
        .iter()
        .rev()
        .find(|e| e.kind == "error")
        .map(|e| e.id.as_str())
        .context("No recorded model error to recover.")
}

impl Hub {
    pub async fn clear_peer_context_first_half(
        &self,
        from: &str,
        arguments: &str,
    ) -> Result<Value> {
        let args: ClearArgs = serde_json::from_str(arguments)?;
        if args.reason.trim().is_empty() || args.reason.chars().count() > 2048 {
            bail!(
                "Explain the observed error, crucial authorized task and need for context reduction in 1 to 2048 characters."
            );
        }
        let (reply, result) = oneshot::channel();
        crate::blocking::run(|| {
            let data = self.0.data.lock().unwrap();
            let error_id = eligible_error(&data, from, &args.to)?.to_owned();
            let h = &data.sessions[&args.to];
            if h.task.as_ref().is_none_or(|task| task.is_finished()) {
                bail!("Recipient worker is unavailable.");
            }
            h.context_clear
                .try_send(ClearRequest {
                    from: from.into(),
                    reason: args.reason,
                    error_id,
                    reply,
                })
                .map_err(|_| {
                    anyhow::anyhow!(
                        "Recipient recovery is already queued or its worker is unavailable."
                    )
                })?;
            Ok::<_, anyhow::Error>(())
        })?;
        result.await.context("Recipient worker stopped before confirming recovery; inspect its trace before retrying.")?
    }

    pub(super) fn apply_peer_context_clear(
        &self,
        id: &str,
        session: &mut Session,
        request: &ClearRequest,
    ) -> Result<Value> {
        crate::blocking::run(|| {
            let mut permit = self.reserve_persistence(id, 2)?;
            let mut data = self.0.data.lock().unwrap();
            // Recheck at the mutation boundary. Stop, new input, wake and shutdown
            // may have won the race after the caller queued this request.
            let error_id = eligible_error(&data, &request.from, id)?;
            if error_id != request.error_id || session.has_unfinished_execution() {
                bail!("Recipient error or execution state changed; context was not changed.");
            }
            if let Some(receipt) = data.sessions[id]
                .record
                .peer_context_clear
                .as_ref()
                .filter(|receipt| receipt.error_id == error_id)
            {
                return Ok(receipt.result(true));
            }
            let mut staged = session.clone();
            let (removed, retained) = staged.clear_first_half();
            if removed == 0 {
                bail!("No older context can be removed without splitting the retained turn.");
            }
            let receipt = ClearReceipt {
                error_id: error_id.into(),
                archive_id: Uuid::new_v4().to_string(),
                by: WorkingContextEditor {
                    id: request.from.clone(),
                    name: data.sessions[&request.from].record.view.name.clone(),
                },
                reason: request.reason.clone(),
                removed,
                retained,
            };
            let notice = format!(
                "Emergency first-half context clear by {} ({}): {} items removed, {} retained. Transcript and role context preserved; still paused.\nPeer reason: {}",
                receipt.by.name, receipt.by.id, removed, retained, receipt.reason,
            );
            let h = data.sessions.get_mut(id).unwrap();
            h.persistence.enqueue_reserved(
                Operation::Clear(receipt.archive_id.clone(), Box::new(h.record.clone())),
                &mut permit,
            )?;
            self.update_locked(&mut data, id, Some(&mut permit), |r| {
                r.engine = staged.clone();
                r.peer_context_clear = Some(receipt.clone());
                r.view.entries.push(Self::make_entry("notice", notice));
                Ok(())
            })?;
            *session = staged;
            Ok(receipt.result(false))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn emergency_clear_rechecks_stops_errors_and_execution_before_mutation() {
        let work = tempfile::tempdir().unwrap();
        let hub = Hub::new(Client::test_client()).unwrap();
        let mut ids = Vec::new();
        for name in ["rescuer", "target"] {
            ids.push(
                hub.create(Create {
                    name: name.into(),
                    cwd: work.path().to_string_lossy().into(),
                    message: String::new(),
                })
                .unwrap()
                .id,
            );
        }
        let (from, to) = (&ids[0], &ids[1]);
        let error = Hub::make_entry("error", "fixture context failure".into());
        hub.update(to, |r| {
            r.engine.user("old payload ".repeat(100));
            r.engine.user("recent authorized task".into());
            r.view.entries.push(error.clone());
            r.view.status = "error".into();
            r.paused = true;
            r.peer_restartable = Some(true);
            Ok(())
        })
        .unwrap();
        let original = hub.0.data.lock().unwrap().sessions[to].record.clone();
        let (reply, _wait) = oneshot::channel();
        let request = ClearRequest {
            from: from.clone(),
            reason: "Crucial task blocked by context error".into(),
            error_id: error.id,
            reply,
        };
        for condition in [
            "working",
            "idle",
            "operator_stop",
            "uncertain",
            "storage",
            "ended",
            "pending_input",
            "new_error",
            "unfinished",
            "shutdown",
        ] {
            let mut session = original.engine.clone();
            {
                let mut data = hub.0.data.lock().unwrap();
                data.closing = condition == "shutdown";
                let h = data.sessions.get_mut(to).unwrap();
                h.record = original.clone();
                h.storage_error = None;
                match condition {
                    "working" | "idle" => {
                        h.record.view.status = condition.into();
                        h.record.paused = false;
                    }
                    "operator_stop" | "uncertain" => h.record.peer_restartable = Some(false),
                    "storage" => h.storage_error = Some("fixture disk failure".into()),
                    "ended" => h.record.view.archived = true,
                    "pending_input" => h.record.inputs.push_back(Input {
                        id: "pending".into(),
                        text: "new task".into(),
                        images: vec![],
                        order: 1,
                        compact: false,
                        clear: false,
                    }),
                    "new_error" => h
                        .record
                        .view
                        .entries
                        .push(Hub::make_entry("error", "new episode".into())),
                    "unfinished" => {
                        let mut engine = serde_json::to_value(&session).unwrap();
                        engine["request_in_flight"] = json!(true);
                        session = serde_json::from_value(engine).unwrap();
                    }
                    _ => (),
                }
            }
            let before = serde_json::to_value(&session).unwrap();
            assert!(
                hub.apply_peer_context_clear(to, &mut session, &request)
                    .is_err(),
                "{condition}"
            );
            assert_eq!(
                before,
                serde_json::to_value(&session).unwrap(),
                "{condition}"
            );
        }
        {
            let mut data = hub.0.data.lock().unwrap();
            data.closing = false;
            data.sessions.get_mut(to).unwrap().record = original;
        }
        hub.stop(to).unwrap();
        let mut session = hub.0.data.lock().unwrap().sessions[to]
            .record
            .engine
            .clone();
        assert!(
            hub.apply_peer_context_clear(to, &mut session, &request)
                .is_err()
        );
        assert!(
            hub.clear_peer_context_first_half(from, &json!({"to":to,"reason":" "}).to_string())
                .await
                .is_err()
        );
        assert!(
            hub.clear_peer_context_first_half(
                from,
                &json!({"to":from,"reason":"Crucial task"}).to_string()
            )
            .await
            .is_err()
        );
        hub.shutdown().await;
    }
}
