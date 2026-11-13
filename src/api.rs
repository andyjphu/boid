use anyhow::{Context, Result, bail};
use eventsource_stream::Eventsource;
use futures_util::StreamExt;
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const REASONING_EFFORT: &str = "medium";

fn encode_input<I: Serialize + ?Sized>(metadata: &Value, input: &I) -> Result<Vec<u8>> {
    #[derive(Serialize)]
    struct Request<'a, I: Serialize + ?Sized> {
        #[serde(flatten)]
        metadata: &'a Value,
        input: &'a I,
    }
    serde_json::to_vec(&Request { metadata, input }).context("Cannot encode model request")
}

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
        auth.migrate_credentials(&key)?;
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
        let mut profile = json!({
            "model": if plan { selected["model"].as_str().unwrap_or(&self.model) } else { &self.model },
            "reasoning_effort": REASONING_EFFORT,
            "cyber_access_program": if plan { None } else { self.cyber_access_program.as_deref() },
            "billing_route": if self.auth.is_some() && selected["credential_id"].is_null() { "unassigned" } else if plan { "chatgpt" } else { "api_key" },
            "credential_id": selected["credential_id"],
            "chatgpt_account": if plan { selected["chatgpt_account"].clone() } else { Value::Null },
        });
        if self.auth.is_none() {
            profile.as_object_mut().unwrap().remove("credential_id");
        }
        profile
    }
    pub fn api_profile(&self) -> Value {
        json!({"model":self.model,"reasoning_effort":REASONING_EFFORT,
            "cyber_access_program":self.cyber_access_program,"billing_route":"api_key","chatgpt_account":null})
    }
    pub fn key_available(&self) -> bool {
        match &self.auth {
            Some(auth) => auth.credentials().ok().is_some_and(|r| {
                r["credentials"]
                    .as_array()
                    .is_some_and(|rows| rows.iter().any(|r| r["type"] == "api_key"))
            }),
            None => !self.key.is_empty(),
        }
    }
    fn api_key(&self) -> Result<String> {
        if let Some(auth) = &self.auth {
            return auth
                .api_key()?
                .context("Choose a saved API credential. No environment fallback was used.");
        }
        if self.key.is_empty() {
            bail!("Use /login to connect an API key before creating API agents.");
        }
        Ok(self.key.clone())
    }
    async fn credential(&self, profile: &Value) -> Result<String> {
        match profile["billing_route"].as_str().unwrap_or("api_key") {
            "api_key" => {
                if let Some(auth) = &self.auth {
                    let id = profile["credential_id"]
                        .as_str()
                        .context("This pane has no credential. Choose one in the lock view.")?;
                    auth.named_api_key(id)
                } else {
                    self.api_key()
                }
            }
            "chatgpt" => {
                let auth = self
                    .auth
                    .as_ref()
                    .context("ChatGPT account storage is unavailable.")?;
                let id = profile["credential_id"]
                    .as_str()
                    .or_else(|| profile["chatgpt_account"].as_str())
                    .context("This pane has no subscription credential.")?;
                auth.check_inference(id)?;
                auth.token(id).await
            }
            "unassigned" => bail!("This pane has no credential. Choose one in the lock view."),
            _ => bail!("Unknown agent billing route."),
        }
    }

    /// Stream text and return complete API items for the next request's input.
    /// Dropping the future closes the active request.
    pub async fn respond_with<I: Serialize + Sync + ?Sized>(
        &self,
        input: &I,
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
        // Encode borrowed input once. Building a Value with json! would copy all
        // historical payloads before serialization; retries can reuse these bytes.
        let body = encode_input(&request, input)?;
        let request = self
            .http
            .post(endpoint)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(body);
        // Keep the API key bound through retries. OAuth may refresh only this
        // account; deletion keeps the already-issued bearer for this request.
        let mut credential = self.credential(profile).await?;
        let mut request_secrets = vec![credential.clone()];
        let mut http_attempt = 0;
        let mut waited = Duration::ZERO;
        loop {
            if http_attempt > 0 && profile["billing_route"] == "chatgpt" {
                let id = profile["credential_id"]
                    .as_str()
                    .or_else(|| profile["chatgpt_account"].as_str())
                    .context("Missing request account.")?;
                if self
                    .auth
                    .as_ref()
                    .context("Credential storage unavailable.")?
                    .account_exists(id)?
                {
                    credential = self.credential(profile).await?;
                    if !request_secrets.contains(&credential) {
                        request_secrets.push(credential.clone());
                    }
                }
            }
            let redact = |text: &str| {
                request_secrets
                    .iter()
                    .fold(self.redact(text), |text, secret| {
                        text.replace(secret, "[redacted]")
                    })
            };
            http_attempt += 1;
            let sent = request
                .try_clone()
                .expect("In-memory JSON request body can be cloned")
                .bearer_auth(&credential)
                .send()
                .await;
            let response = match sent {
                Ok(response) => response,
                Err(error) => {
                    // No output has been delivered and local tools run only after completion.
                    // A lost response can still mean the provider performed inference twice.
                    let delay = retryable_transport(&error)
                        .then(|| retry_delay(http_attempt, waited, None))
                        .flatten();
                    let phase = if error.is_connect() {
                        "connect"
                    } else {
                        "send"
                    };
                    let message = redact(&format!(
                        "Cannot reach the OpenAI API: {:#}",
                        anyhow::Error::new(error)
                    ));
                    on_event(&failure_event(
                        model,
                        profile,
                        http_attempt,
                        phase,
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
            if !response.status().is_success() {
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
                let message = redact(&provider_error(
                    status,
                    request_id.as_deref().unwrap_or("unavailable"),
                    &error,
                    profile,
                ));
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
            }
            let status = response.status();
            let advised = retry_after(response.headers(), SystemTime::now());
            let request_id = request_id.as_deref().unwrap_or("unavailable");
            let mut terminal_event = None;
            let mut output_started = false;
            let mut retryable_stream = false;
            let result: Result<Vec<Value>> = async {
                let mut stream = response.bytes_stream().eventsource();
                let mut completed = BTreeMap::new();
                while let Some(event) = stream.next().await {
                    let event = event.map_err(|error| {
                        retryable_stream = true;
                        anyhow::anyhow!("Cannot read API stream: {}", redact(&error.to_string()))
                    })?;
                    let event: Value = serde_json::from_str(&event.data)
                        .context("Invalid JSON in the API stream.")?;
                    let kind = event["type"].as_str();
                    if matches!(
                        kind,
                        Some("response.failed" | "response.incomplete" | "error")
                    ) {
                        // A failed event may itself contain output or reported usage.
                        // Preserve that attempt instead of hiding it behind a retry.
                        output_started |= event
                            .pointer("/response/output")
                            .and_then(Value::as_array)
                            .is_some_and(|items| !items.is_empty());
                        let has_usage = event
                            .pointer("/response/usage")
                            .is_some_and(|v| !v.is_null());
                        retryable_stream = kind != Some("response.incomplete")
                            && !has_usage
                            && retryable_stream_error(&event);
                        self.pause_for_limit(profile, &event).await?;
                        let message = redact(&provider_error(
                            status.as_u16(),
                            request_id,
                            &event,
                            profile,
                        ));
                        terminal_event = Some(event);
                        bail!("OpenAI response stopped: {message}");
                    }
                    // Unknown event kinds are conservative: only lifecycle notifications
                    // are known to be safe before output, including provider-side tool work.
                    output_started |= !matches!(
                        kind,
                        Some("response.created" | "response.in_progress" | "response.queued")
                    );
                    on_event(&event);
                    match kind {
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
                        _ => {}
                    }
                }
                retryable_stream = true;
                bail!("API stream ended before the response completed.")
            }
            .await;
            match result {
                Ok(output) => return Ok(output),
                Err(error) => {
                    let delay = (retryable_stream && !output_started)
                        .then(|| retry_delay(http_attempt, waited, advised))
                        .flatten();
                    // Retry diagnostics are durable, but only a final failure reaches
                    // usage accounting. A successful replacement owns this model turn.
                    if delay.is_none() {
                        if let Some(event) = &terminal_event {
                            on_event(event);
                        }
                    }
                    let message = redact(&format!("{error:#}"));
                    let mut failure = failure_event(
                        model,
                        profile,
                        http_attempt,
                        "stream",
                        Some(status.as_u16()),
                        (request_id != "unavailable").then_some(request_id),
                        terminal_event.as_ref().and_then(error_code),
                        &message,
                        advised,
                        delay,
                    );
                    failure["failure"]["output_started"] = json!(output_started);
                    on_event(&failure);
                    if let Some(delay) = delay {
                        waited += delay;
                        tokio::time::sleep(delay).await;
                        continue;
                    }
                    bail!("{message} (after {http_attempt} HTTP attempts)");
                }
            }
        }
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

    pub async fn compact<I: Serialize + Sync + ?Sized>(
        &self,
        input: &I,
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
        let body = encode_input(&json!({"model":model,"instructions":instructions}), input)?;
        let response = self
            .http
            .post(endpoint)
            .bearer_auth(&credential)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(body)
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
        .or_else(|| value.get("code"))
        .and_then(Value::as_str)
}
fn terminal_provider_error(error: &Value) -> bool {
    matches!(
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
                | "invalid_api_key"
                | "invalid_grant"
                | "billing_hard_limit_reached"
                | "context_length_exceeded"
        )
    ) || matches!(
        error_type(error),
        Some(
            "authentication_error"
                | "permission_error"
                | "invalid_request_error"
                | "insufficient_quota"
        )
    )
}
fn error_type(value: &Value) -> Option<&str> {
    value
        .pointer("/response/error/type")
        .or_else(|| value.pointer("/error/type"))
        .and_then(Value::as_str)
}
fn retryable_status(status: u16, error: &Value) -> bool {
    !terminal_provider_error(error)
        && (matches!(status, 408 | 409 | 429) || (500..600).contains(&status))
}
fn retryable_transport(error: &reqwest::Error) -> bool {
    !error.is_builder() && (error.is_connect() || error.is_timeout() || error.is_request())
}
fn retryable_stream_error(error: &Value) -> bool {
    !terminal_provider_error(error)
        && (matches!(
            error_code(error),
            Some(
                "server_error"
                    | "internal_error"
                    | "server_is_overloaded"
                    | "rate_limit_exceeded"
                    | "rate_limit_error"
                    | "slow_down"
                    | "subscription_sharing_usage_unavailable"
                    | "subscription_sharing_user_unavailable"
            )
        ) || matches!(
            error_type(error),
            Some("server_error" | "service_unavailable_error" | "rate_limit_error")
        ))
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

    // Explicitly synthetic and opt-in; never reports production request latency.
    #[test]
    #[ignore = "bounded synthetic history-copy benchmark"]
    fn history_copy_and_encoding_benchmark() {
        use std::{hint::black_box, time::Instant};
        let input: Vec<Value> = (0..1024).map(|i| json!({
            "type":"function_call_output", "call_id":format!("call-{i}"), "output":"x".repeat(16 * 1024)
        })).collect();
        let shared: Vec<Arc<Value>> = input.iter().cloned().map(Arc::new).collect();
        let metadata = json!({"model":"fixture", "store":false, "stream":true});
        let mut old_clone = vec![];
        let mut shared_clone = vec![];
        let mut old_encode = vec![];
        let mut borrowed_encode = vec![];
        for _ in 0..7 {
            let t = Instant::now();
            let first = input.clone();
            let second = input.clone();
            black_box((&first, &second));
            drop(first);
            drop(second);
            old_clone.push(t.elapsed().as_secs_f64() * 1000.0);
            let t = Instant::now();
            let first = shared.clone();
            let second = shared.clone();
            black_box((&first, &second));
            drop(first);
            drop(second);
            shared_clone.push(t.elapsed().as_secs_f64() * 1000.0);
            let t = Instant::now();
            let mut request = metadata.clone();
            request["input"] = json!(&input);
            let bytes = serde_json::to_vec(&request).unwrap();
            black_box(&bytes);
            drop(bytes);
            drop(request);
            old_encode.push(t.elapsed().as_secs_f64() * 1000.0);
            let t = Instant::now();
            let bytes = encode_input(&metadata, &shared).unwrap();
            black_box(&bytes);
            drop(bytes);
            borrowed_encode.push(t.elapsed().as_secs_f64() * 1000.0);
        }
        println!(
            "{}",
            json!({"synthetic":true, "payload_bytes":16*1024*1024,
            "items":1024, "two_history_clones_and_drop_ms":old_clone,
            "two_shared_history_clones_and_drop_ms":shared_clone,
            "old_request_materialize_and_encode_ms":old_encode,
            "borrowed_request_encode_ms":borrowed_encode})
        );
    }

    #[test]
    fn borrowed_input_encoding_preserves_request_fields_and_context() {
        let input = vec![
            Arc::new(json!({"type":"reasoning","encrypted_content":"opaque"})),
            Arc::new(
                json!({"type":"function_call","call_id":"c1","arguments":"{\"cmd\":\"pwd\"}","name":"exec_command"}),
            ),
            Arc::new(
                json!({"type":"function_call_output","call_id":"c1","output":"kept\nverbatim"}),
            ),
        ];
        let metadata = json!({"model":"test", "instructions":"unchanged", "stream":true,
            "store":false,"reasoning":{"summary":"auto","effort":"medium"},
            "include":["reasoning.encrypted_content"],"parallel_tool_calls":false,
            "access_programs":{"cyber":"daybreak_blue"},
            "tools":[{"type":"function","name":"exec_command","strict":true,
                "parameters":{"type":"object","properties":{"cmd":{"type":"string"}},"required":["cmd"],"additionalProperties":false}}]});
        let mut old_request = metadata.clone();
        old_request["input"] = json!(&input);
        let bytes = encode_input(&metadata, &input).unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&bytes).unwrap(),
            old_request
        );
        assert!(metadata.get("input").is_none());
    }

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
                    &[] as &[Value],
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
