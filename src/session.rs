use crate::{api::Client, runtime::Hub, skills, tool_calls};
use anyhow::{Context, Result};
use boid::Tools;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, VecDeque};

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
    history: Vec<Value>,
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
    pub fn operator(&mut self, text: String) -> Vec<Value> {
        if self.compact_requested {
            self.compact_resume = true;
        }
        if let Some(tools) = self.tools.as_array_mut() {
            if !tools.iter().any(|tool| tool["name"] == "oai_compact") {
                tools.push(tool_calls::compaction_definition());
            }
        }
        let selected = if self.skill_base.is_some() {
            skills::selection::mentions(&text, &self.skills)
        } else {
            vec![]
        };
        self.user(text);
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
    pub fn refresh_skills(&mut self, snapshot: skills::discovery::Snapshot) -> Option<Value> {
        let base = self.skill_base.as_ref()?;
        let prompt = skills::catalog::prompt(&snapshot.skills, snapshot.budget, &snapshot.warnings);
        let instructions = format!("{base}\n{prompt}");
        if self.skills == snapshot.skills
            && self.skill_warnings == snapshot.warnings
            && self.instructions == instructions
        {
            return None;
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
        Some(json!({"event":"catalog", "text":notice, "warnings":self.skill_warnings}))
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
        let instructions = format!("{}\n\n{}", self.instructions, hub.identity(id)?);
        let result = client
            .compact(&self.history, &self.model, &instructions, &self.profile)
            .await;
        self.compact_in_flight = false;
        let (kind, notice) = match result {
            Ok(result) => {
                if let Some(report) = crate::usage::Report::from_event(
                    self.model_requests,
                    &json!({"type":"response.completed","response":result}),
                ) {
                    hub.record_usage(id, report);
                }
                // The whole returned window is canonical, including retained messages.
                self.history = result["output"].as_array().unwrap().clone();
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
    // Returns whether another step is needed. Queued input is applied before
    // the next model request, after this response's tool calls have finished.
    pub async fn step(
        &mut self,
        client: &Client,
        tools: &Tools,
        hub: &Hub,
        id: &str,
    ) -> Result<bool> {
        if self.compact_requested && self.pending.is_empty() {
            return self.compact(client, hub, id).await;
        }
        let Some(call) = self.pending.pop_front() else {
            self.model_requests += 1;
            if !hub.apply_inbox(id, self)? {
                self.model_requests -= 1;
                return Ok(false);
            }
            self.request_in_flight = true;
            hub.checkpoint(id, self)?;
            // Read current metadata at each request; worker checkpoints never overwrite names.
            let instructions = format!("{}\n\n{}", self.instructions, hub.identity(id)?);
            let mut streams: BTreeMap<String, String> = BTreeMap::new();
            let mut argument_entries = BTreeMap::new();
            let attempt = self.model_requests;
            let partial = &mut self.partial_text;
            let context_usage = &mut self.context_usage;
            let output = client
                .respond_with(
                    &self.history,
                    self.tools.clone(),
                    &self.model,
                    &instructions,
                    &self.profile,
                    |event| {
                        if let Some(report) = crate::usage::Report::from_event(attempt, event) {
                            hub.record_usage(id, report);
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
