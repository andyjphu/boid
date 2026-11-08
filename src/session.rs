use crate::{api::Client, show, tool_calls};
use anyhow::{Context, Result};
use boid::Tools;
use serde_json::{Value, json};
use std::collections::VecDeque;

#[derive(Default)]
pub struct Session {
    history: Vec<Value>,
    pending: VecDeque<Call>,
    running: Option<Call>,
    partial_text: String,
}

struct Call {
    id: String,
    name: String,
    arguments: String,
}

impl Session {
    pub fn user(&mut self, text: String) {
        self.history.push(json!({"role": "user", "content": text}));
    }

    pub async fn run(&mut self, client: &Client, tools: &Tools) -> Result<()> {
        loop {
            let partial = &mut self.partial_text;
            let output = client
                .respond(&self.history, tool_calls::definitions(), |text| {
                    partial.push_str(text);
                    show(text);
                })
                .await?;
            if !self.partial_text.is_empty() {
                show("\n");
            }
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
                show(&format!("\n> {} {}\n", call.name, preview(&call.arguments)));
                self.running = Some(call);
                let call = self.running.as_ref().expect("call was just set");
                let result = tool_calls::execute(tools, &call.name, &call.arguments).await;
                show(&format!("{}\n", preview(&result.to_string())));
                let call = self.running.take().expect("running call is owned here");
                self.finish(call, result);
            }
        }
    }

    pub fn interrupt(&mut self) {
        if !self.partial_text.is_empty() {
            self.history.push(json!({
                "role": "assistant", "content": std::mem::take(&mut self.partial_text)
            }));
        }
        if let Some(call) = self.running.take() {
            self.finish(call, json!({"error": "Interrupted. The action may have partially completed; inspect state before retrying."}));
        }
        while let Some(call) = self.pending.pop_front() {
            self.finish(
                call,
                json!({"error": "Not executed because the turn was interrupted."}),
            );
        }
    }

    fn finish(&mut self, call: Call, output: Value) {
        self.history.push(json!({
            "type": "function_call_output", "call_id": call.id, "output": output.to_string()
        }));
    }
}

fn preview(text: &str) -> String {
    let mut chars = text.chars();
    let prefix: String = chars.by_ref().take(2000).collect();
    if chars.next().is_some() {
        format!("{prefix}\n[Display shortened; full tool result retained for this conversation.]")
    } else {
        prefix
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interruption_accounts_for_running_and_unstarted_calls() {
        let call = |id: &str| Call {
            id: id.into(),
            name: "exec_command".into(),
            arguments: "{}".into(),
        };
        let mut session = Session::default();
        session
            .history
            .push(json!({"type":"reasoning", "encrypted_content":"opaque"}));
        session.running = Some(call("running"));
        session.pending.push_back(call("unstarted"));
        session.interrupt();
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
