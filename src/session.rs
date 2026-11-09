use crate::{api::Client, runtime::Hub, tool_calls};
use anyhow::{Context, Result};
use boid::Tools;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, VecDeque};

#[derive(Default, Clone, Serialize, Deserialize)]
pub struct Session {
    pub model: String,
    pub instructions: String,
    pub tools: Value,
    #[serde(default)]
    pub profile: Value,
    request_in_flight: bool,
    history: Vec<Value>,
    pending: VecDeque<Call>,
    running: Option<Call>,
    pub partial_text: String,
    #[serde(default)]
    model_requests: u64,
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
    pub fn recover(&mut self) -> Vec<(String, String)> {
        let mut entries = vec![];
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
    pub fn next_input_batch(&self) -> u64 {
        self.model_requests + 1
    }
    pub fn has_pending_tools(&self) -> bool {
        !self.pending.is_empty()
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
        let Some(call) = self.pending.pop_front() else {
            self.model_requests += 1;
            hub.apply_inbox(id, self)?;
            self.request_in_flight = true;
            hub.checkpoint(id, self)?;
            let mut streams: BTreeMap<String, String> = BTreeMap::new();
            let mut argument_entries = BTreeMap::new();
            let partial = &mut self.partial_text;
            let output = client
                .respond_with(
                    &self.history,
                    self.tools.clone(),
                    &self.model,
                    &self.instructions,
                    &self.profile,
                    |event| {
                        let event_type = event["type"].as_str().unwrap_or("");
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
        hub.entry_replacing(
            id,
            "tool_call",
            format!("{} ({})\n{}", call.name, call.id, call.arguments),
            call.argument_entry.clone(),
        );
        self.running = Some(call);
        hub.checkpoint(id, self)?;
        let call = self.running.as_ref().expect("running call");
        let result = match call.name.as_str() {
            "list_agents" => hub.peers(),
            "send_message" => {
                #[derive(serde::Deserialize)]
                #[serde(deny_unknown_fields)]
                struct Message {
                    to: String,
                    text: String,
                }
                match serde_json::from_str::<Message>(&call.arguments) {
                    Ok(message) => hub
                        .send_peer(id, &message.to, &message.text)
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
