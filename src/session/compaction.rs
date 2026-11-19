//! Provider compaction and its existing scheduling policy.
use super::*;

impl Session {
    pub fn auto_compaction_command(
        &mut self,
        text: &str,
        profile: &Value,
        active: bool,
    ) -> (String, bool) {
        let words: Vec<_> = text.split_whitespace().collect();
        let enabled = match words.as_slice() {
            ["/auto-oai-compact"] => !self.auto_oai_compact,
            ["/auto-oai-compact", "on"] => true,
            ["/auto-oai-compact", "off"] => false,
            _ => return ("Use /auto-oai-compact [on|off].".into(), active),
        };
        if enabled && profile["billing_route"] == "chatgpt" {
            return ("Automatic OpenAI compaction is unavailable on subscription credentials. No change or API-key request was made.".into(), active);
        }
        self.auto_oai_compact = enabled;
        if !enabled && self.compact_automatic && self.compact_requested {
            self.compact_requested = false;
            self.compact_automatic = false;
            // A queued automatic compaction can be the only reason the worker
            // is active. Turning it off must not start an idle model turn.
            let resume = self.compact_resume;
            self.compact_resume = false;
            return ("Automatic compaction is off.".into(), resume);
        }
        if enabled && self.auto_compaction_due() {
            self.request_compaction(active);
            self.compact_automatic = true;
        }
        (
            if enabled {
                "Automatic compaction is on above 900,000 context tokens.".into()
            } else {
                "Automatic compaction is off.".into()
            },
            active || self.compact_requested,
        )
    }
    fn auto_compaction_due(&self) -> bool {
        self.auto_oai_compact
            && !self.compact_requested
            && self.pending.is_empty()
            && self.context_usage.as_ref().is_some_and(|usage| {
                usage.input_tokens.saturating_add(usage.output_tokens) > 900_000
            })
    }
    pub(super) fn schedule_auto_compaction(
        &mut self,
        hub: &Hub,
        id: &str,
        resume: bool,
    ) -> Result<()> {
        if !self.auto_compaction_due() {
            return Ok(());
        }
        // Resolve the current pane credential only when compaction is due.
        self.refresh_request_profile(hub, id)?;
        if self.profile["billing_route"] == "chatgpt" {
            self.auto_oai_compact = false;
            hub.checkpoint_entries(id, self, vec![("notice".into(),
                "Automatic compaction is off: subscription credentials do not support it. No API-key request was made.".into())])?;
        } else {
            self.request_compaction(resume);
            self.compact_automatic = true;
        }
        Ok(())
    }
    pub fn request_compaction(&mut self, resume: bool) {
        if !self.compact_requested {
            self.compact_resume = resume;
        }
        self.compact_requested = true;
    }
    pub(super) async fn compact(&mut self, client: &Client, hub: &Hub, id: &str) -> Result<bool> {
        let resume = self.compact_resume;
        let automatic = self.compact_automatic;
        self.compact_requested = false;
        self.compact_resume = false;
        self.compact_in_flight = true;
        self.model_requests += 1;
        hub.checkpoint_entries(
            id,
            self,
            vec![("notice".into(), "Compacting context…".into())],
        )?;
        hub.archive_context(id, self.model_requests)?;
        // Compact conversation history, not a temporary roster snapshot. The
        // working note is stored separately and survives the returned window.
        let instructions = format!("{}\n\n{}", self.stable_instructions(), hub.identity(id)?);
        let result = async {
            let materialized = self.provider_input(hub, id).await?;
            let input = materialized.as_deref().unwrap_or(&self.history);
            let mut result = client
                .compact(input, &self.model, &instructions, &self.profile)
                .await?;
            let encoded_images = result["output"]
                .as_array()
                .is_some_and(|items| items.iter().any(crate::images::has_data_images));
            if encoded_images {
                let store = hub.image_store();
                let id = id.to_owned();
                let permit = store.work_permit().await;
                result = tokio::task::spawn_blocking(move || -> Result<Value> {
                    let _permit = permit;
                    store.retain_compaction_images(
                        &id,
                        result["output"]
                            .as_array_mut()
                            .context("Missing compaction window.")?,
                    )?;
                    Ok(result)
                })
                .await
                .context("Cannot retain compacted images.")??;
            }
            Ok::<_, anyhow::Error>(result)
        }
        .await;
        self.compact_in_flight = false;
        self.compact_automatic = false;
        let (kind, notice) = match result {
            Ok(result) => {
                // The whole returned window is canonical, including retained messages.
                self.cache_boundary = None;
                self.history = result["output"].as_array().unwrap().clone().into();
                self.context_usage = None;
                (
                    "notice",
                    "Context compacted. The transcript is unchanged.".to_owned(),
                )
            }
            Err(error) => {
                let suffix = if automatic {
                    self.auto_oai_compact = false;
                    "\nAutomatic compaction is off; use /auto-oai-compact on to retry."
                } else {
                    ""
                };
                (
                    "error",
                    format!("Compaction failed; previous context retained: {error:#}{suffix}"),
                )
            }
        };
        if resume {
            self.history
                .push(json!({"role":"developer","content":notice}));
        }
        hub.checkpoint_entries(id, self, vec![(kind.into(), notice)])?;
        Ok(resume)
    }
}
