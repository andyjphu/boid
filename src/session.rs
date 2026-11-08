use crate::{api::Client, runtime::Hub, tool_calls};
use anyhow::{Context, Result};
use boid::Tools;
use serde_json::{Value, json};
use std::collections::{BTreeMap, VecDeque};

#[derive(Default)]
pub struct Session {
    history: Vec<Value>,
    pending: VecDeque<Call>,
    running: Option<Call>,
    partial_text: String,
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
struct Call {
    id: String,
    name: String,
    arguments: String,
}
impl Session {
    pub fn user(&mut self, text: String) {
        self.history.push(json!({"role":"user","content":text}));
    }
    pub async fn run(&mut self, client: &Client, tools: &Tools, hub: &Hub, id: &str) -> Result<()> {
        loop {
            for (sender, text) in hub.take_inbox(id) {
                self.user(format!("Peer message from {sender}:\n{text}"));
                hub.entry(
                    id,
                    "notice",
                    format!("Queued message from {sender} applied to this model request."),
                );
            }
            let mut streams: BTreeMap<String, String> = BTreeMap::new();
            let partial = &mut self.partial_text;
            let output = client
                .respond(&self.history, tool_calls::definitions(), |event| {
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
                    if kind == "assistant" {
                        partial.push_str(text);
                    }
                })
                .await?;
            self.partial_text.clear();
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
                    })
                })
                .collect();
            self.pending = calls?;
            self.history.extend(output);
            if self.pending.is_empty() {
                return Ok(());
            }
            while let Some(call) = self.pending.pop_front() {
                hub.entry(
                    id,
                    "tool_call",
                    format!("{} ({})\n{}", call.name, call.id, call.arguments),
                );
                self.running = Some(call);
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
                self.finish(call, result, hub, id);
            }
        }
    }
    pub fn interrupt(&mut self, hub: &Hub, id: &str) {
        if !self.partial_text.is_empty() {
            self.history
                .push(json!({"role":"assistant","content":std::mem::take(&mut self.partial_text)}));
        }
        if let Some(call) = self.running.take() {
            self.finish(call, json!({"error":"Interrupted. The action may have partially completed; inspect state before retrying."}), hub, id);
        }
        while let Some(call) = self.pending.pop_front() {
            self.finish(
                call,
                json!({"error":"Not executed because the turn was interrupted."}),
                hub,
                id,
            );
        }
    }
    fn finish(&mut self, call: Call, output: Value, hub: &Hub, id: &str) {
        hub.entry(
            id,
            "tool_result",
            format!("{} ({})\n{}", call.name, call.id, output),
        );
        self.history.push(
            json!({"type":"function_call_output","call_id":call.id,"output":output.to_string()}),
        );
    }
}
