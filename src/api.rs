use anyhow::{Context, Result, bail};
use eventsource_stream::Eventsource;
use futures_util::StreamExt;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::time::Duration;

pub struct Client {
    http: reqwest::Client,
    key: String,
    pub model: String,
    #[cfg(test)]
    endpoint: Option<String>,
}

impl Client {
    pub fn new() -> Result<Self> {
        let key = std::env::var("OPENAI_API_KEY")
            .context("Set OPENAI_API_KEY before starting Boid.")?
            .trim()
            .to_owned();
        if key.is_empty() {
            bail!("OPENAI_API_KEY must not be empty.");
        }
        let model = match std::env::var("BOID_MODEL") {
            Ok(model) if !model.trim().is_empty() => model,
            Ok(_) => bail!("BOID_MODEL must not be empty."),
            Err(std::env::VarError::NotPresent) => "gpt-6.1-sol".to_owned(),
            Err(error) => return Err(error).context("Cannot read BOID_MODEL."),
        };
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(15))
            .build()
            .context("Cannot create the API client.")?;
        Ok(Self {
            http,
            key,
            model,
            #[cfg(test)]
            endpoint: None,
        })
    }

    /// Stream text and return complete API items for the next request's input.
    /// Dropping the future closes the active request.
    pub async fn respond(
        &self,
        input: &[Value],
        tools: Value,
        mut on_event: impl FnMut(&Value) + Send,
    ) -> Result<Vec<Value>> {
        let endpoint = "https://api.openai.com/v1/responses";
        #[cfg(test)]
        let endpoint = self.endpoint.as_deref().unwrap_or(endpoint);
        let response = self
            .http
            .post(endpoint)
            .bearer_auth(&self.key)
            .json(&json!({
                "model": self.model,
                "instructions": include_str!("PRINCIPLES.md"),
                "input": input,
                "tools": tools,
                "parallel_tool_calls": false,
                "stream": true,
                "store": false,
                "include": ["reasoning.encrypted_content"],
                "reasoning": {"summary": "auto"}
            }))
            .send()
            .await
            .context("Cannot reach the OpenAI API.")?;
        if !response.status().is_success() {
            let status = response.status();
            let error = response.json::<Value>().await.unwrap_or(Value::Null);
            bail!(
                "OpenAI API returned {status}: {}",
                self.redact(error_message(&error))
            );
        }

        let mut stream = response.bytes_stream().eventsource();
        let mut completed = BTreeMap::new();
        while let Some(event) = stream.next().await {
            let event = event.map_err(|error| {
                anyhow::anyhow!(
                    "Cannot read API stream: {}",
                    self.redact(&error.to_string())
                )
            })?;
            let event: Value =
                serde_json::from_str(&event.data).context("Invalid JSON in the API stream.")?;
            on_event(&event);
            match event["type"].as_str() {
                Some("response.output_item.done") => {
                    let index = event["output_index"]
                        .as_u64()
                        .context("Missing completed output index.")?;
                    let item = event
                        .get("item")
                        .context("Missing completed output item.")?;
                    completed.insert(index as usize, item.clone());
                }
                Some("response.completed") => return finish(&event, completed),
                Some("response.failed" | "response.incomplete" | "error") => {
                    bail!(
                        "OpenAI response stopped: {}",
                        self.redact(error_message(&event))
                    );
                }
                _ => {}
            }
        }
        bail!("API stream ended before the response completed.")
    }

    fn redact(&self, message: &str) -> String {
        message.replace(&self.key, "[redacted]")
    }

    #[cfg(test)]
    pub fn test_client() -> Self {
        Self {
            http: reqwest::Client::new(),
            key: "unused-test-key".into(),
            model: "test".into(),
            endpoint: Some("http://127.0.0.1:1".into()),
        }
    }

    #[cfg(test)]
    pub fn test_client_at(endpoint: String) -> Self {
        let mut client = Self::test_client();
        client.endpoint = Some(endpoint);
        client
    }
}

fn error_message(value: &Value) -> &str {
    [
        "/response/error/message",
        "/error/message",
        "/message",
        "/response/incomplete_details/reason",
    ]
    .iter()
    .find_map(|path| value.pointer(path).and_then(Value::as_str))
    .unwrap_or("No error detail was supplied.")
}

fn finish(event: &Value, completed: BTreeMap<usize, Value>) -> Result<Vec<Value>> {
    let mut output = event["response"]["output"]
        .as_array()
        .context("Completed response has no output array.")?
        .clone();
    for (index, item) in completed {
        // output_item.done carries the final encrypted reasoning content.
        *output
            .get_mut(index)
            .context("Completed output index is outside the response.")? = item;
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completed_items_retain_reasoning_and_function_calls() {
        let reasoning = json!({
            "type": "reasoning", "id": "rs_1", "summary": [],
            "encrypted_content": "opaque"
        });
        let call = json!({
            "type": "function_call", "id": "fc_1", "call_id": "call_1",
            "name": "read_file", "arguments": "{\"path\":\"src/lib.rs\"}"
        });
        let event = json!({"response": {"output": [
            {"type": "reasoning", "id": "rs_1"}, call
        ]}});
        let output = finish(&event, BTreeMap::from([(0, reasoning.clone())])).unwrap();
        assert_eq!(output, vec![reasoning, call]);
        assert!(finish(&json!({"response": {}}), BTreeMap::new()).is_err());
    }

    #[test]
    fn failures_and_incomplete_responses_keep_the_reason() {
        assert_eq!(
            error_message(&json!({"response": {"error": {"message": "rate limited"}}})),
            "rate limited"
        );
        assert_eq!(
            error_message(&json!({"response": {"incomplete_details": {
                "reason": "max_output_tokens"
            }}})),
            "max_output_tokens"
        );
    }
}
