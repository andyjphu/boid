use super::*;
use axum::{
    Router,
    body::Body,
    http::{HeaderMap, Response, StatusCode},
    routing::post,
};
use std::{
    collections::VecDeque,
    sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

struct Mock {
    endpoint: String,
    count: Arc<AtomicUsize>,
    requests: Arc<Mutex<Vec<Value>>>,
    server: tokio::task::JoinHandle<()>,
}
impl Drop for Mock {
    fn drop(&mut self) {
        self.server.abort();
    }
}
impl Mock {
    async fn new(replies: Vec<Response<Body>>) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let replies = Arc::new(Mutex::new(VecDeque::from(replies)));
        let count = Arc::new(AtomicUsize::new(0));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let (n, bodies) = (count.clone(), requests.clone());
        let router = Router::new().route(
            "/",
            post(
                move |headers: HeaderMap, axum::Json(body): axum::Json<Value>| {
                    assert_eq!(headers["authorization"], "Bearer unused-test-key");
                    n.fetch_add(1, Ordering::SeqCst);
                    bodies.lock().unwrap().push(body);
                    let response = replies
                        .lock()
                        .unwrap()
                        .pop_front()
                        .expect("Unexpected extra HTTP request");
                    async move { response }
                },
            ),
        );
        let server = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        Self {
            endpoint,
            count,
            requests,
            server,
        }
    }
    fn client(&self) -> Client {
        Client::test_client_at(self.endpoint.clone())
    }
}
fn response(status: u16, body: &str, retry_after: Option<&str>) -> Response<Body> {
    let mut builder = Response::builder()
        .status(StatusCode::from_u16(status).unwrap())
        .header("x-request-id", "fixture-request");
    if let Some(delay) = retry_after {
        builder = builder.header("retry-after", delay);
    }
    builder.body(Body::from(body.to_owned())).unwrap()
}
fn success() -> Response<Body> {
    response(
        200,
        "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_fixture\",\"output\":[]}}\n\n",
        None,
    )
}

#[tokio::test]
async fn transient_failures_recover_with_identical_request_and_timestamped_metadata() {
    let mock = Mock::new(vec![
        response(
            503,
            "upstream connect error or disconnect/reset before headers",
            Some("0"),
        ),
        response(520, "unused-test-key", None),
        success(),
    ])
    .await;
    let client = mock.client();
    let mut events = vec![];
    client
        .respond(
            &[json!({"role":"user","content":"Keep this input"})],
            json!([]),
            |e| events.push(e.clone()),
        )
        .await
        .unwrap();
    assert_eq!(mock.count.load(Ordering::SeqCst), 3);
    let requests = mock.requests.lock().unwrap();
    assert_eq!(requests[0], requests[1]);
    assert_eq!(requests[1], requests[2]);
    let failures: Vec<_> = events
        .iter()
        .filter_map(|e| crate::usage::RequestFailure::from_event(7, e))
        .collect();
    assert_eq!(failures.len(), 2);
    assert_eq!(failures[0].attempt, 7);
    assert_eq!(failures[0].http_status, Some(503));
    assert_eq!(failures[1].http_attempt, 2);
    assert!(failures[0].observed_at_ms > 0);
    assert!(failures[0].retry_delay_ms.unwrap() >= 1000);
    assert_eq!(failures[0].retry_after_ms, Some(0));
    assert!(failures[1].message.contains("[redacted]"));
    assert!(
        !serde_json::to_string(&failures)
            .unwrap()
            .contains("unused-test-key")
    );
    assert_eq!(failures[0].request_id.as_deref(), Some("fixture-request"));
}

#[tokio::test]
async fn exhausted_retries_stop_after_four_attempts() {
    let mock = Mock::new(
        (0..4)
            .map(|_| response(503, "upstream reset", None))
            .collect(),
    )
    .await;
    let mut events = vec![];
    let error = mock
        .client()
        .respond(&[], json!([]), |e| events.push(e.clone()))
        .await
        .unwrap_err();
    assert_eq!(mock.count.load(Ordering::SeqCst), 4);
    assert!(error.to_string().contains("after 4 HTTP attempts"));
    assert!(events.last().unwrap()["failure"]["retry_delay_ms"].is_null());
    assert!(error.to_string().contains("upstream reset"));
    assert!(!error.to_string().contains("No error detail"));
}

#[tokio::test]
async fn permission_quota_and_invalid_requests_fail_without_retry() {
    for (status, body) in [
        (403, r#"{"detail":"Serving region not allowed"}"#),
        (
            429,
            r#"{"error":{"code":"subscription_sharing_usage_limit_exceeded","message":"Limit reached"}}"#,
        ),
        (429, r#"{"error":{"code":"insufficient_quota"}}"#),
        (400, "Invalid request"),
        (401, "Invalid identity"),
    ] {
        let mock = Mock::new(vec![response(status, body, Some("1"))]).await;
        let mut events = vec![];
        assert!(
            mock.client()
                .respond(&[], json!([]), |e| events.push(e.clone()))
                .await
                .is_err()
        );
        assert_eq!(mock.count.load(Ordering::SeqCst), 1);
        assert!(events[0]["failure"]["retry_delay_ms"].is_null());
    }
    let plan = json!({"billing_route":"chatgpt"});
    assert!(
        !provider_error(503, "id", &json!({"non_json_body":"upstream reset"}), &plan)
            .contains("settings/usage")
    );
    assert!(
        provider_error(
            429,
            "id",
            &json!({"error":{"code":"subscription_sharing_usage_limit_exceeded"}}),
            &plan
        )
        .contains("settings/usage")
    );
}

#[tokio::test]
async fn transient_rate_limit_honors_retry_after_and_long_delays_are_not_shortened() {
    let mock = Mock::new(vec![
        response(
            429,
            r#"{"error":{"code":"rate_limit_exceeded"}}"#,
            Some("2"),
        ),
        success(),
    ])
    .await;
    let start = tokio::time::Instant::now();
    mock.client().respond(&[], json!([]), |_| {}).await.unwrap();
    assert!(start.elapsed() >= Duration::from_secs(2));
    assert_eq!(mock.count.load(Ordering::SeqCst), 2);
    let mock = Mock::new(vec![response(503, "unavailable", Some("120"))]).await;
    let mut events = vec![];
    assert!(
        mock.client()
            .respond(&[], json!([]), |e| events.push(e.clone()))
            .await
            .is_err()
    );
    assert_eq!(mock.count.load(Ordering::SeqCst), 1);
    assert_eq!(events[0]["failure"]["retry_after_ms"], 120_000);
    assert!(events[0]["failure"]["retry_delay_ms"].is_null());
}

#[test]
fn retry_after_dates_and_total_wait_budget() {
    let now = UNIX_EPOCH + Duration::from_secs(1_800_000_000);
    let mut headers = HeaderMap::new();
    headers.insert(
        "retry-after",
        httpdate::fmt_http_date(now + Duration::from_secs(20))
            .parse()
            .unwrap(),
    );
    assert_eq!(retry_after(&headers, now), Some(Duration::from_secs(20)));
    assert!(retry_delay(2, Duration::from_secs(20), retry_after(&headers, now)).is_none());
    headers.insert("retry-after", "invalid".parse().unwrap());
    assert!(retry_after(&headers, now).is_none());
    headers.insert("retry-after", "18446744073709551615".parse().unwrap());
    assert!(retry_delay(1, Duration::ZERO, retry_after(&headers, now)).is_none());
}

#[tokio::test]
async fn partial_streams_and_invalid_responses_are_never_replayed() {
    for body in [
        "data: not-json\n\n",
        "data: {\"type\":\"response.output_text.delta\",\"delta\":\"partial\"}\n\n",
    ] {
        let mock = Mock::new(vec![response(200, body, None)]).await;
        let mut events = vec![];
        assert!(
            mock.client()
                .respond(&[], json!([]), |e| events.push(e.clone()))
                .await
                .is_err()
        );
        assert_eq!(mock.count.load(Ordering::SeqCst), 1);
        let last = events.last().unwrap();
        assert_eq!(last["failure"]["phase"], "stream");
        assert!(last["failure"]["retry_delay_ms"].is_null());
    }
}

#[tokio::test]
async fn cancelling_backoff_prevents_further_requests() {
    let mock = Mock::new(vec![response(503, "temporary", Some("2"))]).await;
    let client = mock.client();
    let scheduled = Arc::new(tokio::sync::Notify::new());
    let signal = scheduled.clone();
    let task = tokio::spawn(async move {
        client
            .respond(&[], json!([]), |e| {
                if e["type"] == "boid.request_failure" {
                    signal.notify_one();
                }
            })
            .await
    });
    scheduled.notified().await;
    task.abort();
    let _ = task.await;
    tokio::time::sleep(Duration::from_millis(2100)).await;
    assert_eq!(mock.count.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn connect_failure_is_recorded_and_retried_with_a_bound() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    drop(listener);
    let mut events = vec![];
    assert!(
        Client::test_client_at(endpoint)
            .respond(&[], json!([]), |e| events.push(e.clone()))
            .await
            .is_err()
    );
    assert_eq!(events.len(), 4);
    assert!(events.iter().all(|e| e["failure"]["phase"] == "connect"));
    assert!(events.last().unwrap()["failure"]["retry_delay_ms"].is_null());
}

#[tokio::test]
async fn broad_http_failures_and_empty_stream_share_one_retry_budget() {
    let mock = Mock::new(vec![
        response(408, "Request timeout", None),
        response(409, "Conflict", None),
        response(524, "Proxy timeout", None),
        response(200, "", None),
    ])
    .await;
    let mut events = vec![];
    assert!(
        mock.client()
            .respond(&[], json!([]), |e| events.push(e.clone()))
            .await
            .is_err()
    );
    assert_eq!(mock.count.load(Ordering::SeqCst), 4);
    assert_eq!(events.last().unwrap()["failure"]["phase"], "stream");
    assert!(events.last().unwrap()["failure"]["retry_delay_ms"].is_null());

    let mock = Mock::new(vec![
        response(
            429,
            r#"{"error":{"type":"rate_limit_error","code":"slow_down"}}"#,
            None,
        ),
        response(429, "Proxy rate limit", None),
        success(),
    ])
    .await;
    mock.client().respond(&[], json!([]), |_| {}).await.unwrap();
    assert_eq!(mock.count.load(Ordering::SeqCst), 3);
    assert!(!retryable_status(
        503,
        &json!({"error":{"code":"insufficient_quota"}})
    ));
    assert!(!retryable_status(
        429,
        &json!({"error":{"type":"insufficient_quota"}})
    ));
}

#[tokio::test]
async fn early_stream_failure_recovers_without_leaking_failed_usage_or_changing_input() {
    let mock = Mock::new(vec![
        response(200, "data: {\"type\":\"response.created\"}\n\ndata: {\"type\":\"error\",\"error\":{\"code\":\"server_is_overloaded\",\"type\":\"service_unavailable_error\"}}\n\n", None),
        response(200, "", None),
        success(),
    ]).await;
    let mut events = vec![];
    mock.client()
        .respond(
            &[json!({"role":"user","content":"same task"})],
            json!([]),
            |e| events.push(e.clone()),
        )
        .await
        .unwrap();
    assert_eq!(mock.count.load(Ordering::SeqCst), 3);
    let requests = mock.requests.lock().unwrap();
    assert!(requests.windows(2).all(|r| r[0] == r[1]));
    let failures: Vec<_> = events
        .iter()
        .filter_map(|e| crate::usage::RequestFailure::from_event(1, e))
        .collect();
    assert_eq!(failures.len(), 2);
    assert_eq!(
        failures[0].error_code.as_deref(),
        Some("server_is_overloaded")
    );
    assert!(retryable_stream_error(
        &json!({"type":"error","error":{"type":"service_unavailable_error","code":"future_temporary_failure"}})
    ));
    assert!(retryable_stream_error(
        &json!({"type":"error","code":"server_is_overloaded"})
    ));
    assert!(
        failures
            .iter()
            .all(|f| f.output_started == Some(false) && f.retry_delay_ms.is_some())
    );
    assert_eq!(
        events
            .iter()
            .filter_map(|e| crate::usage::Report::from_event(1, e))
            .count(),
        1
    );
    assert!(!events.iter().any(|e| e["type"] == "error"));
}

#[tokio::test]
async fn output_tools_usage_and_terminal_errors_prevent_stream_replay() {
    let overload =
        json!({"type":"response.failed","response":{"error":{"code":"server_is_overloaded"}}});
    for (prefix, terminal) in [
        (
            Some(json!({"type":"response.output_text.delta","delta":"partial"})),
            overload.clone(),
        ),
        (
            Some(json!({"type":"response.function_call_arguments.delta","delta":"{}"})),
            overload.clone(),
        ),
        (
            Some(json!({"type":"response.output_item.added","item":{"type":"function_call"}})),
            overload.clone(),
        ),
        (
            None,
            json!({"type":"response.failed","response":{"output":[{"type":"function_call"}],"error":{"code":"server_is_overloaded"}}}),
        ),
        (
            None,
            json!({"type":"response.failed","response":{"usage":{"input_tokens":3},"error":{"code":"server_is_overloaded"}}}),
        ),
        (
            None,
            json!({"type":"error","code":"subscription_sharing_usage_limit_exceeded"}),
        ),
        (None, json!({"type":"error","code":"unknown_error"})),
        (
            None,
            json!({"type":"response.incomplete","response":{"incomplete_details":{"reason":"max_output_tokens"}}}),
        ),
    ] {
        let body = format!(
            "{}data: {terminal}\n\n",
            prefix.map(|p| format!("data: {p}\n\n")).unwrap_or_default()
        );
        let mock = Mock::new(vec![response(200, &body, None)]).await;
        let mut events = vec![];
        assert!(
            mock.client()
                .respond(&[], json!([]), |e| events.push(e.clone()))
                .await
                .is_err()
        );
        assert_eq!(mock.count.load(Ordering::SeqCst), 1);
        assert!(events.last().unwrap()["failure"]["retry_delay_ms"].is_null());
        assert!(events.contains(&terminal));
    }
}

#[tokio::test]
async fn reset_and_timeout_before_headers_recover() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    for timeout in [false, true] {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut client =
            Client::test_client_at(format!("http://{}", listener.local_addr().unwrap()));
        client.http = reqwest::Client::builder()
            .timeout(Duration::from_millis(200))
            .build()
            .unwrap();
        let server = tokio::spawn(async move {
            let (mut first, _) = listener.accept().await.unwrap();
            let mut bytes = [0; 8192];
            assert!(first.read(&mut bytes).await.unwrap() > 0);
            if timeout {
                tokio::time::sleep(Duration::from_millis(400)).await;
            }
            drop(first);
            let (mut second, _) = listener.accept().await.unwrap();
            assert!(second.read(&mut bytes).await.unwrap() > 0);
            let body = "data: {\"type\":\"response.completed\",\"response\":{\"output\":[]}}\n\n";
            second
                .write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
        });
        let mut events = vec![];
        client
            .respond(&[], json!([]), |e| events.push(e.clone()))
            .await
            .unwrap();
        server.await.unwrap();
        let failures: Vec<_> = events
            .iter()
            .filter_map(|e| crate::usage::RequestFailure::from_event(1, e))
            .collect();
        assert_eq!(failures.len(), 1);
        assert_eq!(failures[0].phase, "send");
        assert!(failures[0].retry_delay_ms.is_some());
        assert!(!failures[0].message.contains("unused-test-key"));
    }
}
