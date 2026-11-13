use crate::{api::Client, runtime::Hub, skills, tool_calls};
use anyhow::{Context, Result};
use boid::Tools;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, VecDeque};
use std::sync::Arc;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
}

impl ContextUsage {
    fn from_event(event: &Value) -> Option<Self> {
        let usage = &event["response"]["usage"];
        Some(Self {
            input_tokens: usage["input_tokens"].as_u64()?,
            output_tokens: usage["output_tokens"].as_u64()?,
        })
    }
}

// Completed model input items are immutable. Cloning a checkpoint shares their
// payloads; appending, clearing, or compacting one history does not change another.
// The serialized form remains the same JSON array used by existing records.
#[derive(Default, Clone, Serialize)]
#[serde(transparent)]
struct ModelHistory(Vec<Arc<Value>>, #[serde(skip)] bool);
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
    fn push(&mut self, item: Value) {
        self.1 |= crate::images::has_refs(&item);
        self.0.push(Arc::new(item));
    }
    fn clear(&mut self) {
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

#[derive(Default, Clone, Serialize, Deserialize)]
pub struct Session {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_usage: Option<ContextUsage>,
    pub model: String,
    pub instructions: String,
    pub tools: Value,
    #[serde(default, skip_serializing)]
    pub skills: skills::Skills,
    #[serde(default)]
    pub skill_snapshot: Option<String>,
    #[serde(default)]
    pub skill_base: Option<String>,
    #[serde(default)]
    pub skill_warnings: Vec<String>,
    #[serde(default)]
    pub profile: Value,
    request_in_flight: bool,
    history: ModelHistory,
    pending: VecDeque<Call>,
    running: Option<Call>,
    pub partial_text: String,
    #[serde(default)]
    model_requests: u64,
    #[serde(default)]
    compact_requested: bool,
    #[serde(default)]
    compact_resume: bool,
    #[serde(default)]
    compact_in_flight: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn checkpoint_histories_share_payloads_and_preserve_storage() {
        let original = json!([
            {"role":"user","content":[{"type":"input_text","text":"retained input"}]},
            {"type":"reasoning","encrypted_content":"opaque retained bytes"},
            {"type":"function_call","call_id":"call-1","name":"exec_command","arguments":"{\"cmd\":\"pwd\"}"},
            {"type":"function_call_output","call_id":"call-1","output":"retained result"}
        ]);
        let mut history: ModelHistory = serde_json::from_value(original.clone()).unwrap();
        let checkpoint = history.clone();
        assert!(Arc::ptr_eq(&history[0], &checkpoint[0]));
        assert_eq!(
            serde_json::to_vec(&checkpoint).unwrap(),
            serde_json::to_vec(&original).unwrap()
        );
        history.push(json!({"role":"user","content":"new input"}));
        assert_eq!(checkpoint.len(), 4);
        assert!(Arc::ptr_eq(&history[3], &checkpoint[3]));
        history.clear();
        assert_eq!(serde_json::to_value(&checkpoint).unwrap(), original);
        let restored: ModelHistory =
            serde_json::from_slice(&serde_json::to_vec(&checkpoint).unwrap()).unwrap();
        assert_eq!(serde_json::to_value(restored).unwrap(), original);
    }

    #[tokio::test]
    async fn interruption_accounts_for_running_and_unstarted_calls() {
        let hub = Hub::new(Client::test_client()).unwrap();
        let call = |id: &str| Call {
            id: id.into(),
            name: "exec_command".into(),
            arguments: "{}".into(),
            argument_entry: None,
        };
        let mut session = Session::default();
        session
            .history
            .push(json!({"type":"reasoning","encrypted_content":"opaque"}));
        session.running = Some(call("running"));
        session.pending.push_back(call("unstarted"));
        session.interrupt(&hub, "test");
        session.user("Keep the earlier result, change direction.".into());
        assert_eq!(session.history[0]["encrypted_content"], "opaque");
        assert_eq!(session.history[1]["call_id"], "running");
        assert!(
            session.history[1]["output"]
                .as_str()
                .unwrap()
                .contains("partially completed")
        );
        assert_eq!(session.history[2]["call_id"], "unstarted");
        assert!(
            session.history[2]["output"]
                .as_str()
                .unwrap()
                .contains("Not executed")
        );
        assert_eq!(session.history[3]["role"], "user");
        assert!(session.running.is_none() && session.pending.is_empty());
    }
}
#[derive(Clone, Serialize, Deserialize)]
struct Call {
    id: String,
    name: String,
    arguments: String,
    argument_entry: Option<String>,
}
impl Session {
    pub fn new(model: String, instructions: String, tools: Value, identity: String) -> Self {
        let mut session = Self {
            model,
            instructions,
            tools,
            ..Self::default()
        };
        session.user(identity);
        session
    }
    pub fn ensure_peer_trace_tool(&mut self) {
        if let Some(tools) = self.tools.as_array_mut() {
            if let Some(tool) = tools
                .iter_mut()
                .find(|tool| tool["name"] == "read_peer_trace")
            {
                *tool = tool_calls::peer_trace_definition();
            } else {
                tools.push(tool_calls::peer_trace_definition());
            }
        }
    }
    pub fn recovering_send_call(&self) -> Option<&str> {
        self.running
            .as_ref()
            .filter(|call| call.name == "send_message")
            .map(|call| call.id.as_str())
    }
    pub fn recover_sends(
        &mut self,
        sender: &str,
        messages: &BTreeMap<(String, String), crate::runtime::Communication>,
    ) -> Vec<(String, String)> {
        let Some(call) = &self.running else {
            return vec![];
        };
        if call.name != "send_message" {
            return vec![];
        }
        let Some(message) = messages.get(&(sender.into(), call.id.clone())) else {
            return vec![];
        };
        let result = message.receipt();
        let call = self.running.take().unwrap();
        self.history.push(
            json!({"type":"function_call_output","call_id":call.id,"output":result.to_string()}),
        );
        vec![(
            "tool_result".into(),
            format!("{} ({})\n{}", call.name, call.id, result),
        )]
    }
    pub fn current_input_batch(&self) -> u64 {
        self.model_requests
    }
    pub fn recover_questions(
        &mut self,
        questions: &mut [crate::runtime::Question],
    ) -> Vec<(String, String)> {
        for q in questions.iter_mut() {
            if q.blocking && q.answer.is_none() && q.wait_outcome.is_none() {
                q.wait_outcome = Some("interrupted".into());
            }
        }
        let Some(call) = &self.running else {
            return vec![];
        };
        if call.name != "prompt_user" {
            return vec![];
        }
        let Some(question) = questions.iter().find(|q| q.call_ids.contains(&call.id)) else {
            return vec![];
        };
        let result = question.receipt();
        let call = self.running.take().unwrap();
        self.history.push(
            json!({"type":"function_call_output","call_id":call.id,"output":result.to_string()}),
        );
        vec![(
            "tool_result".into(),
            format!("{} ({})\n{}", call.name, call.id, result),
        )]
    }
    pub fn recover(&mut self) -> Vec<(String, String)> {
        let mut entries = vec![];
        if self.compact_requested || self.compact_in_flight {
            self.compact_requested = false;
            self.compact_in_flight = false;
            self.compact_resume = false;
            entries.push(("notice".into(), "Compaction interrupted; the previous context is retained. Request compaction again to retry.".into()));
        }
        if !self.partial_text.is_empty() {
            self.history
                .push(json!({"role":"assistant","content":std::mem::take(&mut self.partial_text)}));
        }
        if self.request_in_flight {
            entries.push((
                "notice".into(),
                "The model request was interrupted; no unfinished tool call was executed.".into(),
            ));
            self.request_in_flight = false;
        }
        if let Some(call) = self.running.take() {
            let result = json!({"error":"Interrupted. The action may have partially completed; inspect state before retrying."});
            self.history.push(json!({"type":"function_call_output","call_id":call.id,"output":result.to_string()}));
            entries.push((
                "tool_result".into(),
                format!("{} ({})\n{}", call.name, call.id, result),
            ));
        }
        while let Some(call) = self.pending.pop_front() {
            let result = json!({"error":"Not executed because the turn was interrupted."});
            self.history.push(json!({"type":"function_call_output","call_id":call.id,"output":result.to_string()}));
            entries.push((
                "tool_result".into(),
                format!("{} ({})\n{}", call.name, call.id, result),
            ));
        }
        entries
    }

    pub fn user(&mut self, text: String) {
        self.history.push(json!({"role":"user","content":text}));
    }
    pub fn refresh_tools(&mut self) {
        if let Some(tools) = self.tools.as_array_mut() {
            if !tools.iter().any(|tool| tool["name"] == "oai_compact") {
                tools.push(tool_calls::compaction_definition());
            }
            if !tools.iter().any(|tool| tool["name"] == "prompt_user") {
                tools.push(tool_calls::prompt_user_definition());
            }
        }
    }
    pub fn has_image_refs(&self) -> bool {
        self.history.1
    }
    async fn provider_input(&self, hub: &Hub, id: &str) -> Result<Option<Vec<Arc<Value>>>> {
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
    pub fn operator(&mut self, text: String) -> Vec<Value> {
        self.operator_with_images(text, "", &[])
    }
    pub fn operator_with_images(
        &mut self,
        text: String,
        session: &str,
        images: &[crate::images::Image],
    ) -> Vec<Value> {
        if self.compact_requested {
            self.compact_resume = true;
        }
        self.refresh_tools();
        let selected = if self.skill_base.is_some() {
            skills::selection::mentions(&text, &self.skills)
        } else {
            vec![]
        };
        if images.is_empty() {
            self.user(text);
        } else {
            let mut content = vec![];
            if !text.is_empty() {
                content.push(json!({"type":"input_text", "text":text}));
            }
            for image in images {
                content.push(json!({"type":"input_text", "text":format!("[{}]", image.label)}));
                content.push(json!({"type":"input_image", "image_url":crate::images::ImageStore::reference(session, image), "detail":"auto"}));
            }
            self.history.push(json!({"role":"user", "content":content}));
        }
        let mut entries = vec![];
        for selection in selected {
            let (content, event) = match selection {
                Ok(id) => {
                    let loaded = skills::loaded(&id, &self.skills[&id]);
                    (
                        format!("Operator-selected skill:\n{loaded}"),
                        json!({"event":"loaded", "skill":loaded}),
                    )
                }
                Err(error) => (
                    format!("Skill selection failed: {error}"),
                    json!({"event":"error", "error":error}),
                ),
            };
            self.history
                .push(json!({"role":"developer","content":content}));
            entries.push(event);
        }
        entries
    }
    pub fn refresh_skills(&mut self, snapshot: skills::discovery::Snapshot) {
        let Some(base) = self.skill_base.as_ref() else {
            return;
        };
        let prompt = skills::catalog::prompt(&snapshot.skills, snapshot.budget, &snapshot.warnings);
        let instructions = format!("{base}\n{prompt}");
        if self.skills == snapshot.skills
            && self.skill_warnings == snapshot.warnings
            && self.instructions == instructions
        {
            return;
        }
        self.skills = snapshot.skills;
        self.skill_snapshot = None;
        self.skill_warnings = snapshot.warnings;
        self.instructions = instructions;
        let notice = format!(
            "Skill catalog refreshed for this operator message. This replaces earlier skill availability and policy; previously loaded text is historical.\n{prompt}\nDiscovery warnings: {}",
            json!(self.skill_warnings)
        );
        self.history
            .push(json!({"role":"developer","content":notice}));
    }
    pub fn request_count(&self) -> u64 {
        self.model_requests
    }
    pub fn request_in_flight(&self) -> bool {
        self.request_in_flight
    }
    pub fn next_input_batch(&self) -> u64 {
        self.model_requests + 1
    }
    pub fn has_pending_tools(&self) -> bool {
        !self.pending.is_empty()
    }
    pub fn clear_context(&mut self) {
        self.history.clear();
        self.partial_text.clear();
        self.context_usage = None;
        self.compact_requested = false;
        self.compact_resume = false;
        self.compact_in_flight = false;
        self.request_in_flight = false;
    }
    pub fn request_compaction(&mut self, resume: bool) {
        if !self.compact_requested {
            self.compact_resume = resume;
        }
        self.compact_requested = true;
    }
    async fn compact(&mut self, client: &Client, hub: &Hub, id: &str) -> Result<bool> {
        let resume = self.compact_resume;
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
        let instructions = format!(
            "{}\n\n{}\n\n{}",
            self.instructions,
            hub.identity(id)?,
            tool_calls::PEER_TRACE_GUIDANCE
        );
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
        let (kind, notice) = match result {
            Ok(result) => {
                if let Some(report) = crate::usage::Report::from_event(
                    self.model_requests,
                    &json!({"type":"response.completed","response":result}),
                ) {
                    hub.record_usage(id, report, &self.profile);
                }
                // The whole returned window is canonical, including retained messages.
                self.history = result["output"].as_array().unwrap().clone().into();
                self.context_usage = None;
                (
                    "notice",
                    "Context compacted. The transcript is unchanged.".to_owned(),
                )
            }
            Err(error) => (
                "error",
                format!("Compaction failed; previous context retained: {error:#}"),
            ),
        };
        if resume {
            self.history
                .push(json!({"role":"developer","content":notice}));
        }
        hub.checkpoint_entries(id, self, vec![(kind.into(), notice)])?;
        Ok(resume)
    }
    fn refresh_request_profile(&mut self, hub: &Hub, id: &str) -> Result<()> {
        self.profile = hub.profile(id)?;
        if let Some(model) = self.profile["model"].as_str() {
            self.model = model.to_owned();
        }
        Ok(())
    }
    // Returns whether another step is needed. Queued input is applied before
    // the next model request, after this response's tool calls have finished.
    pub async fn step(
        &mut self,
        client: &Client,
        tools: &Tools,
        hub: &Hub,
        id: &str,
    ) -> Result<bool> {
        self.ensure_peer_trace_tool();
        if self.compact_requested && self.pending.is_empty() {
            self.refresh_request_profile(hub, id)?;
            return self.compact(client, hub, id).await;
        }
        let Some(call) = self.pending.pop_front() else {
            self.model_requests += 1;
            match hub.apply_inbox(id, self) {
                Ok(true) => {}
                Ok(false) => {
                    self.model_requests -= 1;
                    return Ok(false);
                }
                Err(error) => {
                    self.model_requests -= 1;
                    return Err(error);
                }
            }
            self.refresh_request_profile(hub, id)?;
            self.request_in_flight = true;
            hub.checkpoint(id, self)?;
            // Read current metadata at each request; worker checkpoints never overwrite names.
            let instructions = format!(
                "{}\n\n{}\n\n{}",
                self.instructions,
                hub.identity(id)?,
                tool_calls::PEER_TRACE_GUIDANCE
            );
            let materialized = self.provider_input(hub, id).await?;
            let input = materialized.as_deref().unwrap_or(&self.history);
            let mut streams: BTreeMap<String, String> = BTreeMap::new();
            let mut argument_entries = BTreeMap::new();
            let attempt = self.model_requests;
            let partial = &mut self.partial_text;
            let context_usage = &mut self.context_usage;
            let output = client
                .respond_with(
                    input,
                    self.tools.clone(),
                    &self.model,
                    &instructions,
                    &self.profile,
                    |event| {
                        if let Some(failure) =
                            crate::usage::RequestFailure::from_event(attempt, event)
                        {
                            hub.record_request_failure(id, failure);
                        }
                        if let Some(report) = crate::usage::Report::from_event(attempt, event) {
                            hub.record_usage(id, report, &self.profile);
                        }
                        let event_type = event["type"].as_str().unwrap_or("");
                        if matches!(event_type, "response.completed" | "response.incomplete") {
                            *context_usage = ContextUsage::from_event(event);
                        }
                        let kind = match event_type {
                            "response.output_text.delta" | "response.refusal.delta" => "assistant",
                            "response.reasoning_summary_text.delta" => "reasoning",
                            "response.function_call_arguments.delta" => "tool_arguments",
                            _ => return,
                        };
                        let text = event["delta"].as_str().unwrap_or("");
                        let key = format!(
                            "{kind}:{}:{}",
                            event["output_index"], event["summary_index"]
                        );
                        let entry = streams
                            .entry(key)
                            .or_insert_with(|| hub.entry(id, kind, ""));
                        hub.delta(id, entry, text);
                        if kind == "tool_arguments" {
                            if let Some(item_id) = event["item_id"].as_str() {
                                argument_entries.insert(item_id.to_owned(), entry.clone());
                            }
                        }
                        if kind == "assistant" {
                            partial.push_str(text);
                        }
                    },
                )
                .await?;
            self.partial_text.clear();
            self.request_in_flight = false;
            let calls: Result<VecDeque<_>> = output
                .iter()
                .filter(|item| item["type"] == "function_call")
                .map(|item| {
                    let field = |name| -> Result<String> {
                        Ok(item[name]
                            .as_str()
                            .context("Invalid function call from API.")?
                            .into())
                    };
                    Ok(Call {
                        id: field("call_id")?,
                        name: field("name")?,
                        arguments: field("arguments")?,
                        argument_entry: item["id"]
                            .as_str()
                            .and_then(|item_id| argument_entries.remove(item_id)),
                    })
                })
                .collect();
            self.pending = calls?;
            self.history.extend(output);
            hub.checkpoint(id, self)?;
            return Ok(!self.pending.is_empty());
        };
        let sender_entry = hub.entry_replacing(
            id,
            "tool_call",
            format!("{} ({})\n{}", call.name, call.id, call.arguments),
            call.argument_entry.clone(),
        );
        self.running = Some(call);
        if !hub.claim_tool(id, self)? {
            self.pending.push_front(self.running.take().unwrap());
            return Ok(true);
        }
        let call = self.running.as_ref().expect("running call");
        let result = match call.name.as_str() {
            "oai_compact" => match serde_json::from_str::<Value>(&call.arguments) {
                Ok(value) if value == json!({}) => {
                    self.compact_requested = true;
                    self.compact_resume = true;
                    json!({"queued":true,"message":"Compaction will run after the current tool batch."})
                }
                _ => json!({"error":"oai_compact expects an empty JSON object."}),
            },
            "list_skills" => {
                json!({"skills":skills::catalog::list(&self.skills), "warnings":self.skill_warnings})
            }
            "read_skill" => skills::read(&self.skills, &call.arguments),
            "list_agents" => hub.peers(),
            "prompt_user" => {
                match serde_json::from_str::<crate::runtime::PromptArgs>(&call.arguments) {
                    Ok(args) => hub
                        .prompt_user(id, &call.id, args)
                        .await
                        .unwrap_or_else(|error| json!({"error":error.to_string()})),
                    Err(error) => json!({"error":error.to_string()}),
                }
            }
            "read_peer_trace" => hub.read_peer_trace(id, &call.arguments),
            "send_message" => {
                #[derive(serde::Deserialize)]
                #[serde(deny_unknown_fields)]
                struct Message {
                    to: String,
                    text: String,
                    #[serde(default)]
                    urgency: Option<crate::runtime::Urgency>,
                }
                match serde_json::from_str::<Message>(&call.arguments) {
                    Ok(message) => hub
                        .send_communication(
                            id,
                            &message.to,
                            &message.text,
                            message.urgency.unwrap_or_default(),
                            &call.id,
                            &sender_entry,
                        )
                        .unwrap_or_else(|e| json!({"error":e.to_string()})),
                    Err(error) => json!({"error":error.to_string()}),
                }
            }
            _ => tool_calls::execute(tools, &call.name, &call.arguments).await,
        };
        let call = self.running.take().expect("running call");
        self.finish(call, result, hub, id)?;
        Ok(true)
    }
    pub fn interrupt(&mut self, hub: &Hub, id: &str) {
        // Retain a receipt for a question already posted before cancellation.
        if self
            .running
            .as_ref()
            .is_some_and(|call| call.name == "prompt_user")
        {
            if let Err(error) = hub.recover_questions(id, self) {
                hub.persistence_fault(id, error);
            }
            return;
        }
        let entries = self.recover();
        if let Err(error) = hub.checkpoint_entries(id, self, entries) {
            hub.persistence_fault(id, error);
            return;
        }
    }
    fn finish(&mut self, call: Call, output: Value, hub: &Hub, id: &str) -> Result<()> {
        self.history.push(
            json!({"type":"function_call_output","call_id":call.id,"output":output.to_string()}),
        );
        hub.checkpoint_entries(
            id,
            self,
            vec![(
                "tool_result".into(),
                format!("{} ({})\n{}", call.name, call.id, output),
            )],
        )?;
        Ok(())
    }
}
