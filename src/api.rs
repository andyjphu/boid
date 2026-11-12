use anyhow::{Context, Result, bail};
use eventsource_stream::Eventsource;
use futures_util::StreamExt;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const REASONING_EFFORT: &str = "medium";

// API limit, not Codex's smaller default budget. Unknown models have no assumed limit.
// https://developers.openai.com/api/docs/models/gpt-6-sol (2026-11-08)
pub fn context_window(model: &str) -> Option<u64> {
    match model {
        "gpt-6-sol" => Some(1_050_000),
        _ => None,
    }
}

pub struct Client {
    http: reqwest::Client,
    key: String,
    pub auth: Option<Arc<crate::chatgpt::Auth>>,
    pub model: String,
    cyber_access_program: Option<String>,
    #[cfg(test)]
    endpoint: Option<String>,
}

impl Client {
    pub fn new() -> Result<Self> {
        let key = std::env::var("OPENAI_API_KEY")
            .unwrap_or_default()
            .trim()
            .to_owned();
        let route = std::env::var("BOID_AUTH_ROUTE").unwrap_or_else(|_| "api_key".into());
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
        let auth = crate::chatgpt::Auth::open(
            crate::server::token_file()?
                .parent()
                .context("State directory has no parent.")?
                .join("chatgpt"),
            &route,
            &model,
        )?;
        Ok(Self {
            auth: Some(Arc::new(auth)),
            http,
            key,
            model,
            cyber_access_program,
            #[cfg(test)]
            endpoint: None,
        })
    }

    pub fn config(&self) -> Value {
        let selected = match &self.auth {
            Some(auth) => match auth.profile() {
                Ok(profile) => profile,
                Err(_) => {
                    return json!({"billing_route":"unavailable","auth_error":"Cannot read saved billing settings. Repair account storage before creating agents."});
                }
            },
            None => Value::Null,
        };
        let plan = selected["billing_route"] == "chatgpt";
        json!({
            "model": if plan { selected["model"].as_str().unwrap_or(&self.model) } else { &self.model },
            "reasoning_effort": REASONING_EFFORT,
            "cyber_access_program": if plan { None } else { self.cyber_access_program.as_deref() },
            "billing_route": if plan { "chatgpt" } else { "api_key" },
            "chatgpt_account": if plan { selected["chatgpt_account"].clone() } else { Value::Null },
        })
    }
    pub fn api_profile(&self) -> Value {
        json!({"model":self.model,"reasoning_effort":REASONING_EFFORT,
            "cyber_access_program":self.cyber_access_program,"billing_route":"api_key","chatgpt_account":null})
    }
    pub fn key_available(&self) -> bool {
        !self.key.is_empty()
            || self
                .auth
                .as_ref()
                .is_some_and(|auth| auth.api_key().ok().flatten().is_some())
    }
    fn api_key(&self) -> Result<String> {
        if let Some(auth) = &self.auth {
            if let Some(key) = auth.api_key()? {
                return Ok(key);
            }
        }
        if self.key.is_empty() {
            bail!("Use /login to connect an API key before creating API agents.");
        }
        Ok(self.key.clone())
    }
    async fn credential(&self, profile: &Value) -> Result<String> {
        match profile["billing_route"].as_str().unwrap_or("api_key") {
            "api_key" => self.api_key(),
            "chatgpt" => {
                let auth = self
                    .auth
                    .as_ref()
                    .context("ChatGPT account storage is unavailable.")?;
                let id = profile["chatgpt_account"]
                    .as_str()
                    .context("Choose a ChatGPT account for new agents in /login.")?;
                auth.check_inference(id)?;
                auth.token(id).await
            }
            _ => bail!("Unknown agent billing route."),
        }
    }

    /// Stream text and return complete API items for the next request's input.
    /// Dropping the future closes the active request.
    pub async fn respond_with(
        &self,
        input: &[Value],
        tools: Value,
        model: &str,
        instructions: &str,
        profile: &Value,
        mut on_event: impl FnMut(&Value) + Send,
    ) -> Result<Vec<Value>> {
        let endpoint = "https://api.openai.com/v1/responses";
        #[cfg(test)]
        let endpoint = self.endpoint.as_deref().unwrap_or(endpoint);
        let mut request = json!({
            "model": model,
            "instructions": instructions,
            "input": input,
            "tools": tools,
            "parallel_tool_calls": false,
            "stream": true,
            "store": false,
            "include": ["reasoning.encrypted_content"],
            "reasoning": {"summary": "auto"}
        });
        if let Some(effort) = profile["reasoning_effort"].as_str() {
            request["reasoning"]["effort"] = json!(effort);
        }
        if profile["billing_route"] != "chatgpt" {
            if let Some(program) = profile["cyber_access_program"].as_str() {
                request["access_programs"] = json!({"cyber": program});
            }
        } else {
            let functions: Vec<_> = request["tools"]
                .as_array()
                .context("Invalid local tool definitions.")?
                .iter()
                .filter(|tool| tool["name"] != "oai_compact")
                .cloned()
                .collect();
            request["tools"] = json!([{"type":"namespace","name":"boid","description":"Local Boid agent tools","tools":functions}]);
        }
        let mut http_attempt = 0;
        let mut waited = Duration::ZERO;
        let (response, credential, request_id) = loop {
            // Recheck the same account after backoff; another request may have paused it.
            let credential = self.credential(profile).await?;
            http_attempt += 1;
            let sent = self
                .http
                .post(endpoint)
                .bearer_auth(&credential)
                .json(&request)
                .send()
                .await;
            let response = match sent {
                Ok(response) => response,
                Err(error) => {
                    // Only connection establishment failures are safe to retry here.
                    // An ambiguous send/read timeout may already have reached inference.
                    let delay = error
                        .is_connect()
                        .then(|| retry_delay(http_attempt, waited, None))
                        .flatten();
                    let message = self
                        .redact(&format!("Cannot reach the OpenAI API: {error}"))
                        .replace(&credential, "[redacted]");
                    on_event(&failure_event(
                        model,
                        profile,
                        http_attempt,
                        if error.is_connect() {
                            "connect"
                        } else {
                            "send"
                        },
                        None,
                        None,
                        None,
                        &message,
                        None,
                        delay,
                    ));
                    if let Some(delay) = delay {
                        waited += delay;
                        tokio::time::sleep(delay).await;
                        continue;
                    }
                    bail!("{message} (after {http_attempt} HTTP attempts)");
                }
            };
            let request_id = response
                .headers()
                .get("x-request-id")
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned);
            if response.status().is_success() {
                break (response, credential, request_id);
            }
            let status = response.status().as_u16();
            let advised = retry_after(response.headers(), SystemTime::now());
            let body = response
                .text()
                .await
                .unwrap_or_else(|error| format!("Cannot read error body: {error}"));
            let error = serde_json::from_str::<Value>(&body)
                .unwrap_or_else(|_| json!({"non_json_body":body}));
            let delay = retryable_status(status, &error)
                .then(|| retry_delay(http_attempt, waited, advised))
                .flatten();
            let message = self
                .redact(&provider_error(
                    status,
                    request_id.as_deref().unwrap_or("unavailable"),
                    &error,
                    profile,
                ))
                .replace(&credential, "[redacted]");
            // Diagnostics retain timestamps and retries even if the next attempt succeeds.
            on_event(&failure_event(
                model,
                profile,
                http_attempt,
                "http",
                Some(status),
                request_id.as_deref(),
                error_code(&error),
                &message,
                advised,
                delay,
            ));
            self.pause_for_limit(profile, &error).await?;
            if let Some(delay) = delay {
                waited += delay;
                tokio::time::sleep(delay).await;
                continue;
            }
            bail!("{message} (after {http_attempt} HTTP attempts)");
        };
        let status = response.status();
        let request_id = request_id.as_deref().unwrap_or("unavailable");
        let mut stream_error_code = None;
        let result: Result<Vec<Value>> = async {
            let mut stream = response.bytes_stream().eventsource();
            let mut completed = BTreeMap::new();
            while let Some(event) = stream.next().await {
                let event = event.map_err(|error| {
                    anyhow::anyhow!(
                        "Cannot read API stream: {}",
                        self.redact(&error.to_string())
                            .replace(&credential, "[redacted]")
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
                        stream_error_code = error_code(&event).map(str::to_owned);
                        self.pause_for_limit(profile, &event).await?;
                        bail!(
                            "OpenAI response stopped: {}",
                            self.redact(&provider_error(
                                status.as_u16(),
                                &request_id,
                                &event,
                                profile
                            ))
                            .replace(&credential, "[redacted]")
                        );
                    }
                    _ => {}
                }
            }
            bail!("API stream ended before the response completed.")
        }
        .await;
        // Never replay an opened stream, including failures before its first event.
        if let Err(error) = &result {
            let message = self
                .redact(&format!("{error:#}"))
                .replace(&credential, "[redacted]");
            on_event(&failure_event(
                model,
                profile,
                http_attempt,
                "stream",
                Some(status.as_u16()),
                (request_id != "unavailable").then_some(request_id),
                stream_error_code.as_deref(),
                &message,
                None,
                None,
            ));
        }
        result
    }

    async fn pause_for_limit(&self, profile: &Value, error: &Value) -> Result<()> {
        let code = error_code(error);
        if profile["billing_route"] == "chatgpt"
            && code == Some("subscription_sharing_usage_limit_exceeded")
        {
            self.auth
                .as_ref()
                .context("Missing ChatGPT storage.")?
                .pause(
                    profile["chatgpt_account"]
                        .as_str()
                        .context("Missing ChatGPT account.")?,
                    true,
                )
                .await?;
        }
        Ok(())
    }

    pub async fn compact(
        &self,
        input: &[Value],
        model: &str,
        instructions: &str,
        profile: &Value,
    ) -> Result<Value> {
        if profile["billing_route"] == "chatgpt" {
            bail!(
                "/oai-compact is unavailable on the ChatGPT plan route in this integration. Context is retained; no API-key request was made."
            );
        }
        let credential = self.credential(profile).await?;
        let endpoint = "https://api.openai.com/v1/responses/compact".to_owned();
        #[cfg(test)]
        let endpoint = self
            .endpoint
            .as_ref()
            .map(|url| format!("{}/compact", url.trim_end_matches('/')))
            .unwrap_or(endpoint);
        let response = self
            .http
            .post(endpoint)
            .bearer_auth(&credential)
            .json(&json!({"model":model,"input":input,"instructions":instructions}))
            .timeout(Duration::from_secs(300))
            .send()
            .await
            .context("Cannot reach OpenAI compaction.")?;
        let status = response.status();
        let body = response
            .text()
            .await
            .context("Cannot read OpenAI compaction response.")?;
        if !status.is_success() {
            bail!(
                "OpenAI compaction {status}: {}",
                self.redact(&body).replace(&credential, "[redacted]")
            );
        }
        let result: Value =
            serde_json::from_str(&body).context("Invalid compaction response JSON.")?;
        let output = result["output"]
            .as_array()
            .context("Compaction response has no output window.")?;
        if result["object"] != "response.compaction"
            || output.is_empty()
            || !output.iter().any(|item| {
                item["type"] == "compaction"
                    && item["encrypted_content"]
                        .as_str()
                        .is_some_and(|s| !s.is_empty())
            })
        {
            bail!("Compaction response has no valid compacted context.");
        }
        Ok(result)
    }

    #[cfg(test)]
    pub async fn respond(
        &self,
        input: &[Value],
        tools: Value,
        on_event: impl FnMut(&Value) + Send,
    ) -> Result<Vec<Value>> {
        self.respond_with(
            input,
            tools,
            &self.model,
            include_str!("PRINCIPLES.md"),
            &self.config(),
            on_event,
        )
        .await
    }

    fn redact(&self, message: &str) -> String {
        if self.key.is_empty() {
            message.into()
        } else {
            message.replace(&self.key, "[redacted]")
        }
    }

    #[cfg(test)]
    pub fn test_client() -> Self {
        Self {
            http: reqwest::Client::new(),
            key: "unused-test-key".into(),
            auth: None,
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

const MAX_HTTP_ATTEMPTS: u32 = 4;
const MAX_RETRY_WAIT: Duration = Duration::from_secs(30);

fn error_code(value: &Value) -> Option<&str> {
    value
        .pointer("/response/error/code")
        .or_else(|| value.pointer("/error/code"))
        .and_then(Value::as_str)
}
fn retryable_status(status: u16, error: &Value) -> bool {
    if matches!(
        error_code(error),
        Some(
            "subscription_sharing_usage_limit_exceeded"
                | "insufficient_quota"
                | "subscription_sharing_user_not_eligible"
                | "subscription_sharing_route_not_supported"
                | "chatpass_v2_scope_not_authorized"
                | "chatpass_v2_invalid_authorization_context"
                | "subscription_sharing_invalid_user"
                | "subscription_sharing_unsupported_capability"
        )
    ) {
        return false;
    }
    matches!(status, 500 | 502 | 503 | 504 | 520)
        || (status == 429 && error_code(error) == Some("rate_limit_exceeded"))
}
fn retry_after(headers: &reqwest::header::HeaderMap, now: SystemTime) -> Option<Duration> {
    let value = headers
        .get(reqwest::header::RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim();
    if let Ok(seconds) = value.parse::<u64>() {
        return Some(Duration::from_secs(seconds));
    }
    httpdate::parse_http_date(value)
        .ok()
        .map(|date| date.duration_since(now).unwrap_or_default())
}
fn retry_delay(attempt: u32, waited: Duration, advised: Option<Duration>) -> Option<Duration> {
    if attempt >= MAX_HTTP_ATTEMPTS {
        return None;
    }
    let base_ms = 1000u64 << attempt.saturating_sub(1);
    let jitter_ms = (uuid::Uuid::new_v4().as_u128() % (base_ms as u128 / 4 + 1)) as u64;
    let delay = Duration::from_millis(base_ms + jitter_ms).max(advised.unwrap_or_default());
    // Never shorten a server-requested delay to fit the budget.
    (delay <= MAX_RETRY_WAIT.saturating_sub(waited)).then_some(delay)
}
#[allow(clippy::too_many_arguments)]
fn failure_event(
    model: &str,
    profile: &Value,
    http_attempt: u32,
    phase: &str,
    status: Option<u16>,
    request_id: Option<&str>,
    code: Option<&str>,
    message: &str,
    advised: Option<Duration>,
    delay: Option<Duration>,
) -> Value {
    json!({"type":"boid.request_failure","failure":{
        "observed_at_ms":SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as u64,
        "billing_route":profile["billing_route"].as_str().unwrap_or("api_key"),"model":model,
        "http_attempt":http_attempt,"phase":phase,"http_status":status,"request_id":request_id,
        "error_code":code,"message":message,"retry_after_ms":advised.map(|d| d.as_millis().min(u64::MAX as u128) as u64),
        "retry_delay_ms":delay.map(|d| d.as_millis() as u64)
    }})
}

fn error_message(value: &Value) -> &str {
    [
        "/response/error/message",
        "/error/message",
        "/message",
        "/detail",
        "/response/incomplete_details/reason",
        "/non_json_body",
    ]
    .iter()
    .find_map(|path| {
        value
            .pointer(path)
            .and_then(Value::as_str)
            .filter(|text| !text.trim().is_empty())
    })
    .unwrap_or("No error detail was supplied.")
}

fn provider_error(status: u16, request_id: &str, error: &Value, profile: &Value) -> String {
    let route = if profile["billing_route"] == "chatgpt" {
        if error_code(error) == Some("subscription_sharing_usage_limit_exceeded") {
            " ChatGPT plan requests paused after a usage limit. Review https://chatgpt.com/settings/usage. No API-key fallback was used."
        } else {
            " ChatGPT plan request failed; no API-key fallback was used."
        }
    } else {
        ""
    };
    // Retain structured code/param, direct-admission detail, and request ID.
    format!(
        "OpenAI HTTP {status}, request ID {request_id}: {}. Body: {error}.{route}",
        error_message(error).trim().trim_end_matches('.')
    )
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
                    "model":"gpt-6-sol", "reasoning_effort":"medium", "cyber_access_program":program, "billing_route":"api_key", "chatgpt_account":null,
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
            let captured = client.config();
            client.cyber_access_program = Some("changed-server-default".into());
            client
                .respond_with(
                    &[],
                    json!([]),
                    "captured-model",
                    "captured-instructions",
                    &captured,
                    |_| {},
                )
                .await
                .unwrap();
            let replay = requests.recv().await.unwrap();
            assert_eq!(replay["model"], "captured-model");
            assert_eq!(replay["instructions"], "captured-instructions");
            assert_eq!(replay["reasoning"], request["reasoning"]);
            assert_eq!(
                replay.get("access_programs"),
                request.get("access_programs")
            );
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

#[cfg(test)]
#[path = "api_retry_tests.rs"]
mod retry_tests;
