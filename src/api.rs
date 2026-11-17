use anyhow::{Context, Result, bail};
use eventsource_stream::Eventsource;
use futures_util::StreamExt;
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const REASONING_EFFORT: &str = "medium";

fn redact_event(value: &mut Value, redact: &impl Fn(&str) -> String) {
    match value {
        Value::String(text) => *text = redact(text),
        Value::Array(items) => items.iter_mut().for_each(|item| redact_event(item, redact)),
        Value::Object(fields) => fields
            .values_mut()
            .for_each(|item| redact_event(item, redact)),
        _ => (),
    }
}

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
    #[cfg(test)]
    handoff_probe: Option<Arc<dyn Fn(&'static str) + Send + Sync>>,
    #[cfg(test)]
    worker_probe: Option<Arc<dyn Fn(&str, &'static str) + Send + Sync>>,
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
            #[cfg(test)]
            handoff_probe: None,
            #[cfg(test)]
            worker_probe: None,
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
                auth.inference_token(id, false)
                    .await?
                    .context("This pane has no subscription credential.")
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
        if let Some(tier) = profile["service_tier"].as_str() {
            // Subscription sharing uses the account's ordinary tier when off.
            // Its endpoint can reject an explicit default override.
            if profile["billing_route"] != "chatgpt" || tier != "default" {
                request["service_tier"] = json!(tier);
            }
        }
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
        // Local tools execute only after a validated complete response. An
        // abandoned generation can therefore be retried without replaying them.
        let local_tools_only = only_local_tools(&request["tools"]);
        // Encode borrowed input once. Building a Value with json! would copy all
        // historical payloads before serialization; retries can reuse these bytes.
        #[cfg(test)]
        self.test_handoff_mark("payload_encode_start");
        let body = crate::blocking::run(|| encode_input(&request, input))?;
        #[cfg(test)]
        self.test_handoff_mark("payload_encode_end");
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
                if let Some(refreshed) = self
                    .auth
                    .as_ref()
                    .context("Credential storage unavailable.")?
                    .inference_token(id, true)
                    .await?
                {
                    credential = refreshed;
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
            if http_attempt > 1 {
                on_event(&json!({"type":"boid.request_start","http_attempt":http_attempt}));
            }
            #[cfg(test)]
            self.test_handoff_mark("http_send");
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
                // Expose the retry event to callers; session diagnostics collection is suspended.
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
            let mut replay_safe = local_tools_only;
            let mut local_events_only = true;
            let mut stream_advised = advised;
            let result: Result<Vec<Value>> = async {
                let mut stream = response.bytes_stream().eventsource();
                let mut completed = BTreeMap::new();
                let mut observed = BTreeMap::<usize, Option<String>>::new();
                while let Some(event) = stream.next().await {
                    let event = event.map_err(|error| {
                        retryable_stream = true;
                        anyhow::anyhow!("Cannot read API stream: {}", redact(&error.to_string()))
                    })?;
                    let event: Value = serde_json::from_str(&event.data).map_err(|error| {
                        retryable_stream = true;
                        anyhow::anyhow!("Invalid JSON in the API stream: {error}")
                    })?;
                    let kind = event["type"].as_str();
                    local_events_only &= local_stream_event(&event);
                    replay_safe &= local_events_only;
                    if let Some(index) = event["output_index"].as_u64().and_then(|i| usize::try_from(i).ok()) {
                        let id = event["item"]["id"].as_str().or_else(|| event["item_id"].as_str()).map(str::to_owned);
                        let previous = observed.entry(index).or_default();
                        if previous.is_some() && id.is_some() && previous != &id {
                            retryable_stream = true;
                            bail!("Stream output identity changed at index {index}.");
                        }
                        if id.is_some() { *previous = id; }
                    }
                    stream_advised = stream_advised.max(event_retry_after(&event));
                    if matches!(
                        kind,
                        Some("response.failed" | "response.incomplete" | "error")
                    ) {
                        // Generation may have produced text, but none of this
                        // attempt's local tools have been admitted to execution.
                        output_started |= event
                            .pointer("/response/output")
                            .and_then(Value::as_array)
                            .is_some_and(|items| !items.is_empty());
                        retryable_stream = !terminal_provider_error(&event)
                            && !(kind == Some("response.incomplete")
                                && matches!(event.pointer("/response/incomplete_details/reason")
                                    .and_then(Value::as_str), Some("max_output_tokens" | "content_filter")))
                            && (replay_safe || retryable_stream_error(&event));
                        self.pause_for_limit(profile, &event).await?;
                        let message = redact(&provider_error(
                            status.as_u16(),
                            request_id,
                            &event,
                            profile,
                        ));
                        let mut event = event;
                        redact_event(&mut event, &redact);
                        terminal_event = Some(event);
                        bail!("OpenAI response stopped: {message}");
                    }
                    // Unknown event kinds are conservative: only lifecycle notifications
                    // are known to be safe before output, including provider-side tool work.
                    output_started |= !matches!(
                        kind,
                        Some("response.created" | "response.in_progress" | "response.queued")
                    );
                    match kind {
                        Some("response.output_item.done") => {
                            retryable_stream = true;
                            let index = event["output_index"]
                                .as_u64()
                                .and_then(|index| usize::try_from(index).ok())
                                .context("Missing or invalid completed output index.")?;
                            let item = event.get("item")
                                .filter(|item| item.is_object())
                                .context("Missing completed output item.")?;
                            if completed.get(&index).is_some_and(|previous| previous != item) {
                                bail!("Conflicting completed output items at index {index}.");
                            }
                            completed.insert(index, item.clone());
                            retryable_stream = false;
                            on_event(&event);
                        }
                        Some("response.completed") => {
                            retryable_stream = true;
                            let output = finish(&event, completed)?;
                            for (index, id) in &observed {
                                let item = output.get(*index).context("Completed response omits observed stream output.")?;
                                if id.as_deref().is_some_and(|id| item["id"].as_str() != Some(id)) {
                                    bail!("Completed response conflicts with observed stream identity.");
                                }
                            }
                            // Usage and completion belong only to a valid attempt.
                            on_event(&event);
                            return Ok(output);
                        }
                        _ => on_event(&event),
                    }
                }
                retryable_stream = true;
                bail!("API stream ended before the response completed.")
            }
            .await;
            match result {
                Ok(output) => return Ok(output),
                Err(error) => {
                    let delay =
                        (retryable_stream && local_events_only && (!output_started || replay_safe))
                            .then(|| retry_delay(http_attempt, waited, stream_advised))
                            .flatten();
                    // Only a final failure is delivered as the terminal provider event.
                    // A successful replacement owns this model turn.
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
                        stream_advised,
                        delay,
                    );
                    failure["failure"]["output_started"] = json!(output_started);
                    on_event(&failure);
                    // Discard rejected partials even when the retry budget is exhausted.
                    on_event(&json!({"type":"boid.request_retry",
                        "next_attempt":delay.map(|_| http_attempt + 1),
                        "max_attempts":MAX_HTTP_ATTEMPTS,"output_started":output_started}));
                    if let Some(delay) = delay {
                        // Reset before waiting: Escape during backoff must not
                        // recover rejected text into model input.
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
        let body = crate::blocking::run(|| {
            encode_input(&json!({"model":model,"instructions":instructions}), input)
        })?;
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
            handoff_probe: None,
            worker_probe: None,
        }
    }

    #[cfg(test)]
    pub fn test_client_at(endpoint: String) -> Self {
        let mut client = Self::test_client();
        client.endpoint = Some(endpoint);
        client
    }

    #[cfg(test)]
    pub(crate) fn set_test_handoff_probe(
        &mut self,
        probe: Arc<dyn Fn(&'static str) + Send + Sync>,
    ) {
        self.handoff_probe = Some(probe);
    }

    #[cfg(test)]
    pub(crate) fn test_handoff_mark(&self, stage: &'static str) {
        if let Some(probe) = &self.handoff_probe {
            probe(stage);
        }
    }

    #[cfg(test)]
    pub(crate) fn set_test_worker_probe(
        &mut self,
        probe: Arc<dyn Fn(&str, &'static str) + Send + Sync>,
    ) {
        self.worker_probe = Some(probe);
    }

    #[cfg(test)]
    pub(crate) fn test_worker_mark(&self, id: &str, stage: &'static str) {
        if let Some(probe) = &self.worker_probe {
            probe(id, stage);
        }
    }
}

const MAX_HTTP_ATTEMPTS: u32 = 4;
const MAX_RETRY_WAIT: Duration = Duration::from_secs(30 * 60);

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
                | "content_filter"
                | "max_output_tokens"
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
fn only_local_tools(tools: &Value) -> bool {
    tools.as_array().is_some_and(|tools| {
        tools.iter().all(|tool| match tool["type"].as_str() {
            Some("function") => true,
            Some("namespace") => only_local_tools(&tool["tools"]),
            _ => false,
        })
    })
}
fn local_stream_event(event: &Value) -> bool {
    let local_item = |item: &Value| {
        matches!(
            item["type"].as_str(),
            Some("message" | "reasoning" | "function_call")
        )
    };
    match event["type"].as_str().unwrap_or("") {
        "response.created"
        | "response.in_progress"
        | "response.queued"
        | "response.completed"
        | "response.failed"
        | "response.incomplete"
        | "error" => event
            .pointer("/response/output")
            .and_then(Value::as_array)
            .is_none_or(|items| items.iter().all(local_item)),
        "response.output_item.added" | "response.output_item.done" => local_item(&event["item"]),
        "response.content_part.added"
        | "response.content_part.done"
        | "response.output_text.delta"
        | "response.output_text.done"
        | "response.output_text.annotation.added"
        | "response.refusal.delta"
        | "response.refusal.done"
        | "response.reasoning_summary_part.added"
        | "response.reasoning_summary_part.done"
        | "response.reasoning_summary_text.delta"
        | "response.reasoning_summary_text.done"
        | "response.reasoning_text.delta"
        | "response.reasoning_text.done"
        | "response.function_call_arguments.delta"
        | "response.function_call_arguments.done" => true,
        _ => false,
    }
}
// Some stream failures carry HTTP advice inside their error object.
fn event_retry_after(event: &Value) -> Option<Duration> {
    let mut delay = None;
    for path in ["/error/headers", "/response/error/headers", "/headers"] {
        let Some(headers) = event.pointer(path).and_then(Value::as_object) else {
            continue;
        };
        for (key, value) in headers {
            let value = value
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| value.to_string());
            let parsed = if key.eq_ignore_ascii_case("retry-after-ms") {
                value.parse::<u64>().ok().map(Duration::from_millis)
            } else if key.eq_ignore_ascii_case("retry-after") {
                let mut headers = reqwest::header::HeaderMap::new();
                if let Ok(value) = value.parse() {
                    headers.insert(reqwest::header::RETRY_AFTER, value);
                }
                retry_after(&headers, SystemTime::now())
            } else {
                None
            };
            delay = delay.max(parsed);
        }
    }
    delay
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
    let final_output = event["response"]["output"]
        .as_array()
        .context("Completed response has no output array.")?;
    let final_ids = output_ids(final_output.iter())?;
    let stream_ids = output_ids(completed.values())?;

    // A shortened or reordered final array is recoverable only when the stream
    // independently establishes every position and covers every final item.
    // Do not infer missing slots from a sparse set of done events.
    let complete_stream = !completed.is_empty()
        && completed.keys().copied().eq(0..completed.len())
        && final_output.iter().all(|item| {
            item["id"]
                .as_str()
                .is_some_and(|id| stream_ids.contains_key(id))
        });
    let output = if complete_stream {
        completed
            .values()
            .map(|item| {
                let id = item["id"]
                    .as_str()
                    .context("Completed stream item has no stable ID.")?;
                match final_ids.get(id) {
                    Some(index) => merge_completed_item(&final_output[*index], item),
                    None => Ok(item.clone()),
                }
            })
            .collect::<Result<Vec<_>>>()?
    } else {
        let mut output = final_output.clone();
        for (index, item) in completed {
            let id = item["id"]
                .as_str()
                .context("Completed stream item has no stable ID.")?;
            if final_ids.get(id) != Some(&index) {
                bail!("Completed output cannot be reconciled without ambiguous ordering.");
            }
            output[index] = merge_completed_item(&output[index], &item)?;
        }
        output
    };
    validate_completed_calls(&output)?;
    Ok(output)
}

fn output_ids<'a>(items: impl Iterator<Item = &'a Value>) -> Result<BTreeMap<String, usize>> {
    let mut ids = BTreeMap::new();
    for (index, item) in items.enumerate() {
        if !item.is_object() || item["type"].as_str().is_none_or(str::is_empty) {
            bail!("Completed output contains an invalid item.");
        }
        if let Some(id) = item.get("id") {
            let id = id
                .as_str()
                .filter(|id| !id.is_empty())
                .context("Completed output contains an invalid item ID.")?;
            if ids.insert(id.to_owned(), index).is_some() {
                bail!("Completed output contains duplicate item IDs.");
            }
        }
    }
    Ok(ids)
}

fn merge_completed_item(final_item: &Value, done: &Value) -> Result<Value> {
    if final_item["id"] != done["id"] || final_item["type"] != done["type"] {
        bail!("Completed output item identity conflicts with the stream.");
    }
    if done["type"] == "function_call" {
        for field in ["call_id", "name", "arguments"] {
            if let (Some(left), Some(right)) = (final_item.get(field), done.get(field)) {
                if left != right {
                    bail!("Completed function call conflicts with the stream.");
                }
            }
        }
    }
    // Keep final-only fields, but output_item.done owns finalized content,
    // particularly encrypted reasoning needed for the next model request.
    let mut merged = final_item
        .as_object()
        .context("Invalid final output item.")?
        .clone();
    merged.extend(
        done.as_object()
            .context("Invalid completed stream item.")?
            .clone(),
    );
    Ok(Value::Object(merged))
}

fn validate_completed_calls(output: &[Value]) -> Result<()> {
    let mut calls = std::collections::BTreeSet::new();
    for item in output.iter().filter(|item| item["type"] == "function_call") {
        let call_id = item["call_id"]
            .as_str()
            .filter(|id| !id.is_empty())
            .context("Completed function call has no call ID.")?;
        if !calls.insert(call_id) {
            bail!("Completed output contains duplicate function call IDs.");
        }
        if item["name"].as_str().is_none_or(str::is_empty)
            || !item["arguments"].is_string()
            || item
                .get("status")
                .is_some_and(|status| status != "completed")
        {
            bail!("Completed function call has invalid fields or unfinished status.");
        }
    }
    Ok(())
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
    fn complete_stream_recovers_shortened_and_reordered_final_arrays() {
        let reasoning = json!({"id":"r", "type":"reasoning", "encrypted_content":"final-opaque"});
        let call = json!({"id":"f", "type":"function_call", "call_id":"c", "name":"read_file", "arguments":"{}"});
        let done = BTreeMap::from([(0, reasoning.clone()), (1, call.clone())]);
        for final_items in [
            json!([]),
            json!([call.clone()]),
            json!([call.clone(), {"id":"r", "type":"reasoning"}]),
        ] {
            assert_eq!(
                finish(&json!({"response":{"output":final_items}}), done.clone()).unwrap(),
                vec![reasoning.clone(), call.clone()]
            );
        }
    }

    #[test]
    fn aligned_partial_stream_preserves_final_only_items_and_fields() {
        let message = json!({"id":"m", "type":"message", "content":[]});
        let final_reasoning =
            json!({"id":"r", "type":"reasoning", "summary":[], "encrypted_content":"old"});
        let done_reasoning = json!({"id":"r", "type":"reasoning", "encrypted_content":"final"});
        let call = json!({"id":"f", "type":"function_call", "call_id":"c", "name":"read_file", "arguments":"{}"});
        let output = finish(
            &json!({"response":{"output":[message.clone(), final_reasoning, call.clone()]}}),
            BTreeMap::from([(1, done_reasoning)]),
        )
        .unwrap();
        assert_eq!(
            output,
            vec![
                message,
                json!({"id":"r", "type":"reasoning", "summary":[], "encrypted_content":"final"}),
                call
            ]
        );
    }

    #[test]
    fn ambiguous_or_conflicting_output_is_not_silently_repaired() {
        let a = json!({"id":"a", "type":"message", "content":[]});
        let b = json!({"id":"b", "type":"reasoning", "encrypted_content":"opaque"});
        for (final_items, done) in [
            // A missing slot cannot be invented, even if all final IDs are known.
            (json!([b.clone()]), BTreeMap::from([(1, b.clone())])),
            (json!([]), BTreeMap::from([(usize::MAX, b.clone())])),
            // Partial stream evidence cannot reorder a final-only item safely.
            (
                json!([b.clone(), a.clone()]),
                BTreeMap::from([(0, a.clone())]),
            ),
            (json!([a.clone()]), BTreeMap::from([(0, b.clone())])),
            (json!([a.clone(), a.clone()]), BTreeMap::new()),
            (
                json!([a.clone()]),
                BTreeMap::from([(0, a.clone()), (1, a.clone())]),
            ),
            (
                json!([a.clone()]),
                BTreeMap::from([(0, json!({"id":"a", "type":"reasoning"}))]),
            ),
            (
                json!([a.clone()]),
                BTreeMap::from([(0, json!({"type":"message"}))]),
            ),
            (json!([null]), BTreeMap::new()),
        ] {
            assert!(finish(&json!({"response":{"output":final_items}}), done).is_err());
        }
    }

    #[test]
    fn malformed_or_duplicate_function_calls_fail_before_session_admission() {
        let call = json!({"id":"f", "type":"function_call", "call_id":"c", "name":"read_file", "arguments":"{}"});
        for field in ["call_id", "name", "arguments"] {
            let mut invalid = call.clone();
            invalid.as_object_mut().unwrap().remove(field);
            assert!(finish(&json!({"response":{"output":[invalid]}}), BTreeMap::new()).is_err());
        }
        for (field, value) in [
            ("arguments", json!({})),
            ("name", json!("")),
            ("call_id", json!("")),
            ("status", json!("in_progress")),
        ] {
            let mut invalid = call.clone();
            invalid[field] = value;
            assert!(finish(&json!({"response":{"output":[invalid]}}), BTreeMap::new()).is_err());
        }
        let mut duplicate = call.clone();
        duplicate["id"] = json!("other-item");
        assert!(
            finish(
                &json!({"response":{"output":[call.clone(), duplicate]}}),
                BTreeMap::new()
            )
            .is_err()
        );
        for field in ["call_id", "name", "arguments"] {
            let mut conflict = call.clone();
            conflict[field] = json!("different");
            assert!(
                finish(
                    &json!({"response":{"output":[conflict]}}),
                    BTreeMap::from([(0, call.clone())])
                )
                .is_err()
            );
        }
        // Existing nonstreamed function fixtures need no synthetic item ID.
        let mut no_item_id = call.clone();
        no_item_id.as_object_mut().unwrap().remove("id");
        assert_eq!(
            finish(
                &json!({"response":{"output":[no_item_id.clone()]}}),
                BTreeMap::new()
            )
            .unwrap(),
            vec![no_item_id]
        );
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
