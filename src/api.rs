use anyhow::{Context, Result, bail};
use eventsource_stream::Eventsource;
use futures_util::StreamExt;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::time::Duration;

const REASONING_EFFORT: &str = "medium";

pub struct Client {
    http: reqwest::Client,
    key: String,
    pub model: String,
    cyber_access_program: Option<String>,
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
            Err(std::env::VarError::NotPresent) => "gpt-6-sol".to_owned(),
            Err(error) => return Err(error).context("Cannot read BOID_MODEL."),
        };
        let cyber_access_program = match std::env::var("BOID_CYBER_ACCESS_PROGRAM") {
            Ok(program) => match program.trim() {
                "standard" | "daybreak_blue" | "daybreak_red" => Some(program.trim().to_owned()),
                _ => bail!(
                    "BOID_CYBER_ACCESS_PROGRAM must be standard, daybreak_blue, or daybreak_red."
                ),
            },
            Err(std::env::VarError::NotPresent) => Some("daybreak_blue".to_owned()),
            Err(error) => return Err(error).context("Cannot read BOID_CYBER_ACCESS_PROGRAM."),
        };
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(15))
            .build()
            .context("Cannot create the API client.")?;
        Ok(Self {
            http,
            key,
            model,
            cyber_access_program,
            #[cfg(test)]
            endpoint: None,
        })
    }

    pub fn config(&self) -> Value {
        json!({
            "model": self.model,
            "reasoning_effort": REASONING_EFFORT,
            "cyber_access_program": self.cyber_access_program,
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
        let mut request = json!({
            "model": self.model,
            "instructions": include_str!("PRINCIPLES.md"),
            "input": input,
            "tools": tools,
            "parallel_tool_calls": false,
            "stream": true,
            "store": false,
            "include": ["reasoning.encrypted_content"],
            "reasoning": {"effort": REASONING_EFFORT, "summary": "auto"}
        });
        if let Some(program) = &self.cyber_access_program {
            request["access_programs"] = json!({"cyber": program});
        }
        let response = self
            .http
            .post(endpoint)
            .bearer_auth(&self.key)
            .json(&request)
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
            cyber_access_program: None,
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

    #[tokio::test]
    async fn access_program_selection_is_sent_without_changing_the_model() {
        use axum::{Json, Router, routing::post};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let (sent, mut requests) = tokio::sync::mpsc::channel(4);
        let router = Router::new().route(
            "/",
            post(move |Json(request): Json<Value>| {
                let sent = sent.clone();
                async move {
                    sent.send(request).await.unwrap();
                    (
                        [("content-type", "text/event-stream")],
                        "data: {\"type\":\"response.completed\",\"response\":{\"output\":[]}}\n\n",
                    )
                }
            }),
        );
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        for program in [
            None,
            Some("standard"),
            Some("daybreak_blue"),
            Some("daybreak_red"),
        ] {
            let mut client = Client::test_client_at(endpoint.clone());
            client.model = "gpt-6-sol".into();
            client.cyber_access_program = program.map(str::to_owned);
            assert_eq!(
                client.config(),
                json!({
                    "model":"gpt-6-sol", "reasoning_effort":"medium", "cyber_access_program":program,
                })
            );
            client.respond(&[], json!([]), |_| {}).await.unwrap();
            let request = requests.recv().await.unwrap();
            assert_eq!(request["model"], "gpt-6-sol");
            assert_eq!(
                request["reasoning"],
                json!({"effort":"medium", "summary":"auto"})
            );
            match program {
                Some(program) => assert_eq!(request["access_programs"], json!({"cyber":program})),
                None => assert!(request.get("access_programs").is_none()),
            }
        }
        server.abort();
    }

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
