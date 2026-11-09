use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    time::{SystemTime, UNIX_EPOCH},
};

pub fn pricing() -> Value {
    json!({"as_of":"2026-11-08","currency":"USD","sources":[
        "https://developers.openai.com/api/docs/models/gpt-6-sol",
        "https://developers.openai.com/api/docs/guides/prompt-caching"],
        "basis":"Published token rates, not an invoice. Excludes regional premiums, discounts and non-token fees.",
        "model":"gpt-6-sol","input_per_million":2.0,"cached_input_per_million":0.2,"output_per_million":10.0})
}
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Ledger {
    #[serde(default)]
    pub records: Vec<Report>,
    #[serde(default)]
    pub complete_since_creation: bool,
}
impl Ledger {
    pub fn new() -> Self {
        Self {
            complete_since_creation: true,
            ..Self::default()
        }
    }
    pub fn record(&mut self, report: Report) {
        if !self.records.iter().any(|old| {
            old.attempt == report.attempt
                || (report.response_id.is_some() && old.response_id == report.response_id)
        }) {
            self.records.push(report);
        }
    }
    fn contains_attempt(&self, attempt: u64) -> bool {
        self.records.iter().any(|r| r.attempt == attempt)
    }
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Report {
    pub attempt: u64,
    pub response_id: Option<String>,
    pub status: String,
    pub observed_at_ms: u64,
    pub model: Option<String>,
    pub service_tier: Option<String>,
    pub usage: Value,
    pub prompt_cache_diagnostics: Value,
}
impl Report {
    pub fn from_event(attempt: u64, event: &Value) -> Option<Self> {
        let kind = event["type"].as_str()?;
        if !matches!(
            kind,
            "response.completed" | "response.failed" | "response.incomplete" | "error"
        ) {
            return None;
        }
        let response = event.get("response")?.as_object()?;
        Some(Self {
            attempt,
            response_id: response
                .get("id")
                .and_then(Value::as_str)
                .map(str::to_owned),
            status: kind.strip_prefix("response.").unwrap_or(kind).into(),
            observed_at_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
            model: response
                .get("model")
                .and_then(Value::as_str)
                .map(str::to_owned),
            service_tier: response
                .get("service_tier")
                .and_then(Value::as_str)
                .map(str::to_owned),
            usage: response.get("usage").cloned().unwrap_or(Value::Null),
            prompt_cache_diagnostics: response
                .get("prompt_cache_diagnostics")
                .cloned()
                .unwrap_or(Value::Null),
        })
    }
    pub fn view(&self, id: &str, name: &str) -> Value {
        let mut value = serde_json::to_value(self).unwrap();
        value["session_id"] = json!(id);
        value["session_name"] = json!(name);
        let estimate = self.estimate();
        value["estimated_cost_usd"] = json!(estimate.as_ref().ok());
        value["unknown_reason"] = json!(estimate.err());
        value
    }
    fn count(&self, path: &str) -> Option<u64> {
        self.usage.pointer(path)?.as_u64()
    }
    fn required(&self, path: &str) -> Result<u64, String> {
        self.count(path)
            .ok_or_else(|| format!("Missing or invalid usage {path}"))
    }
    fn tokens(&self) -> Result<(u64, u64, u64, u64), String> {
        let input = self.required("/input_tokens")?;
        let output = self.required("/output_tokens")?;
        if input.checked_add(output) != Some(self.required("/total_tokens")?) {
            return Err("Inconsistent total tokens".into());
        }
        let read = self.required("/input_tokens_details/cached_tokens")?;
        let write = self.required("/input_tokens_details/cache_write_tokens")?;
        if read.checked_add(write).is_none_or(|cache| cache > input) {
            return Err("Cache tokens exceed input tokens".into());
        }
        Ok((input, output, read, write))
    }
    pub fn estimate(&self) -> Result<f64, String> {
        if self.model.as_deref() != Some("gpt-6-sol") {
            return Err("Missing or unsupported reported model".into());
        }
        let tier = match self.service_tier.as_deref() {
            Some("default" | "standard") => 1.0,
            Some("priority" | "fast") => 2.0,
            Some("flex") => 0.5,
            _ => return Err("Missing or unsupported reported service tier".into()),
        };
        let (input, output, read, write) = self.tokens()?;
        let ttl = "/input_tokens_details/cache_write_tokens_details";
        let (short, long) = if write == 0 && self.usage.pointer(ttl).is_none_or(Value::is_null) {
            (0, 0)
        } else {
            (
                self.required(&format!("{ttl}/ttl_30m_tokens"))?,
                self.required(&format!("{ttl}/ttl_12h_tokens"))?,
            )
        };
        if short.checked_add(long) != Some(write) {
            return Err("Cache write lifetimes do not match write tokens".into());
        }
        if let Some(reasoning) = self
            .usage
            .pointer("/output_tokens_details/reasoning_tokens")
        {
            if reasoning.as_u64().is_none_or(|n| n > output) {
                return Err("Invalid reasoning token count".into());
            }
        }
        let (input_rate, read_rate, output_rate) = if input > 272000 {
            (4.0, 0.4, 15.0)
        } else {
            (2.0, 0.2, 10.0)
        };
        Ok(((input - read - write) as f64 * input_rate
            + read as f64 * read_rate
            + short as f64 * input_rate * 1.25
            + long as f64 * input_rate * 2.5
            + output as f64 * output_rate)
            * tier
            / 1_000_000.0)
    }
}
pub fn summarize<'a>(sessions: impl IntoIterator<Item = (&'a Ledger, u64, bool)>) -> Value {
    let mut records = vec![];
    let mut attempts = 0u64;
    let mut inflight = 0u64;
    let mut legacy = false;
    for (ledger, count, active) in sessions {
        attempts = attempts.saturating_add(count);
        legacy |= !ledger.complete_since_creation;
        inflight += u64::from(active && !ledger.contains_attempt(count));
        records.extend(ledger.records.iter());
    }
    let recorded = records.len() as u64;
    let missing = attempts.saturating_sub(recorded).saturating_sub(inflight);
    let mut reasons = BTreeSet::new();
    if legacy {
        reasons.insert("History predates usage recording".to_owned());
    }
    if missing > 0 {
        reasons.insert(format!(
            "{missing} request attempts have no recorded response"
        ));
    }
    if inflight > 0 {
        reasons.insert(format!("{inflight} requests are in flight"));
    }
    let mut priced = 0u64;
    let mut subtotal = 0.0;
    for r in &records {
        match r.estimate() {
            Ok(cost) => {
                priced += 1;
                subtotal += cost
            }
            Err(reason) => {
                reasons.insert(reason);
            }
        }
    }
    let sum = |path: &str| -> Option<u64> {
        if records.is_empty() {
            return None;
        }
        records
            .iter()
            .try_fold(0u64, |sum, r| sum.checked_add(r.count(path)?))
    };
    let input = sum("/input_tokens");
    let output = sum("/output_tokens");
    let cache_valid = |field: &str| {
        records.iter().all(|r| {
            let Some(count) = r.count(field) else {
                return false;
            };
            if r.count("/input_tokens").is_some_and(|input| count > input) {
                return false;
            }
            match (
                r.count("/input_tokens"),
                r.count("/input_tokens_details/cached_tokens"),
                r.count("/input_tokens_details/cache_write_tokens"),
            ) {
                (Some(input), Some(read), Some(write)) => {
                    read.checked_add(write).is_some_and(|sum| sum <= input)
                }
                _ => true,
            }
        })
    };
    let read = cache_valid("/input_tokens_details/cached_tokens")
        .then(|| sum("/input_tokens_details/cached_tokens"))
        .flatten();
    let write = cache_valid("/input_tokens_details/cache_write_tokens")
        .then(|| sum("/input_tokens_details/cache_write_tokens"))
        .flatten();
    let reuse = input.zip(read).and_then(|(i, r)| {
        if i > 0 {
            Some(100.0 * r as f64 / i as f64)
        } else {
            None
        }
    });
    let complete = !legacy && missing == 0 && inflight == 0;
    json!({"request_attempts":attempts,"recorded_requests":recorded,"in_flight_requests":inflight,
        "unrecorded_requests":missing,"usage_known_requests":records.iter().filter(|r|r.tokens().is_ok()).count(),
        "priced_requests":priced,"history_complete":complete,
        "totals":{"input_tokens":input,"output_tokens":output,"cached_read_tokens":read,"cache_write_tokens":write,"reuse_pct":reuse},
        "estimated_cost_usd":if complete && priced==recorded {Some(subtotal)}else{None},
        "priced_cost_usd":subtotal,"unknown_reasons":reasons})
}
#[cfg(test)]
mod tests {
    use super::*;
    fn report() -> Report {
        Report::from_event(1,&json!({"type":"response.completed","response":{
        "id":"r1","model":"gpt-6-sol","service_tier":"default","usage":{"input_tokens":12000,"output_tokens":200,"total_tokens":12200,
        "input_tokens_details":{"cached_tokens":4000,"cache_write_tokens":7000,"cache_write_tokens_details":{"ttl_30m_tokens":3000,"ttl_12h_tokens":4000}},
        "output_tokens_details":{"reasoning_tokens":100}}}})).unwrap()
    }
    #[test]
    fn pricing_disjoint_cache_and_actual_tier() {
        let mut r = report();
        assert!((r.estimate().unwrap() - 0.0323).abs() < 1e-10);
        r.service_tier = Some("priority".into());
        assert!((r.estimate().unwrap() - 0.0646).abs() < 1e-10);
        r.service_tier = None;
        assert!(r.estimate().is_err());
        r.service_tier = Some("default".into());
        r.usage["input_tokens_details"]["cache_write_tokens_details"] = Value::Null;
        assert!(r.estimate().is_err());
    }
    #[test]
    fn long_context_full_request_boundary() {
        let mut r = report();
        r.usage = json!({"input_tokens":272000,"output_tokens":1000,"total_tokens":273000,"input_tokens_details":{"cached_tokens":0,"cache_write_tokens":0}});
        assert!((r.estimate().unwrap() - 0.554).abs() < 1e-10);
        r.usage["input_tokens"] = json!(272001);
        r.usage["total_tokens"] = json!(273001);
        assert!((r.estimate().unwrap() - 1.103004).abs() < 1e-10);
    }
    #[test]
    fn reported_cache_read_survives_missing_write_breakdown() {
        let mut r = report();
        r.usage["input_tokens_details"]
            .as_object_mut()
            .unwrap()
            .remove("cache_write_tokens");
        let mut ledger = Ledger::new();
        ledger.record(r);
        let summary = summarize([(&ledger, 1, false)]);
        assert_eq!(summary["totals"]["cached_read_tokens"], 4000);
        assert!(summary["totals"]["cache_write_tokens"].is_null());
        assert!(summary["totals"]["reuse_pct"].as_f64().unwrap() > 33.0);
        assert!(summary["estimated_cost_usd"].is_null());
    }
    #[test]
    fn coverage_dedup_missing_and_legacy() {
        let mut ledger = Ledger::new();
        let r = report();
        ledger.record(r.clone());
        ledger.record(r.clone());
        let mut duplicate = r.clone();
        duplicate.attempt = 2;
        ledger.record(duplicate);
        assert_eq!(ledger.records.len(), 1);
        let summary = summarize([(&ledger, 2, false)]);
        assert_eq!(summary["unrecorded_requests"], 1);
        assert!(summary["estimated_cost_usd"].is_null());
        let mut missing = r;
        missing.attempt = 2;
        missing.response_id = Some("r2".into());
        missing.usage["input_tokens_details"]
            .as_object_mut()
            .unwrap()
            .remove("cached_tokens");
        ledger.record(missing);
        let summary = summarize([(&ledger, 2, false)]);
        assert!(summary["totals"]["cached_read_tokens"].is_null());
        assert!(summary["estimated_cost_usd"].is_null());
        let old: Ledger = serde_json::from_str("{}").unwrap();
        assert_eq!(summarize([(&old, 0, false)])["history_complete"], false);
    }
}
