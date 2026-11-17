//! Synthetic message admission through the real Hub worker and loopback HTTP.
//! Never invokes a live provider. Host skills remain part of discovery; local
//! fixture skills and history sizes are reported separately.
use super::*;
use axum::{Router, body::to_bytes, extract::Request, routing::post};

#[derive(Default)]
struct Probe {
    tasks: Mutex<HashMap<tokio::task::Id, String>>,
    events: Mutex<Vec<(String, &'static str, Instant)>>,
}
impl Probe {
    fn mark(&self, id: &str, stage: &'static str, at: Instant) {
        self.events.lock().unwrap().push((id.into(), stage, at));
    }
    fn worker(&self, id: &str, stage: &'static str) {
        if stage == "worker_start" {
            self.tasks
                .lock()
                .unwrap()
                .insert(tokio::task::id(), id.into());
        }
        self.mark(id, stage, Instant::now());
    }
    fn api(&self, stage: &'static str) {
        let id = self.tasks.lock().unwrap()[&tokio::task::id()].clone();
        self.mark(&id, stage, Instant::now());
    }
    fn after(&self, id: &str, stage: &str, start: Instant) -> Option<Instant> {
        self.events
            .lock()
            .unwrap()
            .iter()
            .find(|(session, name, at)| session == id && *name == stage && *at >= start)
            .map(|(_, _, at)| *at)
    }
    async fn finished(&self, id: &str, start: Instant) {
        tokio::time::timeout(Duration::from_secs(15), async {
            while self.after(id, "step_finished", start).is_none()
                || self.after(id, "assistant_delta_observed", start).is_none()
            {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .expect("actual worker did not finish mock response");
    }
}
fn millis(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}
fn elapsed(start: Instant, end: Instant) -> f64 {
    if end >= start {
        millis(end.duration_since(start))
    } else {
        -millis(start.duration_since(end))
    }
}

struct Fixture {
    hub: Hub,
    probe: Arc<Probe>,
    ids: Vec<String>,
    server: tokio::task::JoinHandle<()>,
    observer: tokio::task::JoinHandle<()>,
    _state: tempfile::TempDir,
    _work: tempfile::TempDir,
    history_mib: usize,
    local_skills: usize,
    discovered_skills: usize,
}
impl Fixture {
    async fn new(agents: usize, history_mib: usize) -> Self {
        let probe = Arc::new(Probe::default());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}/", listener.local_addr().unwrap());
        let http_probe = probe.clone();
        let router = Router::new().route("/", post(move |request: Request| {
            let probe = http_probe.clone();
            async move {
                let arrived = Instant::now();
                let bytes = to_bytes(request.into_body(), 16 * 1024 * 1024).await.unwrap();
                let read = Instant::now();
                let body: Value = serde_json::from_slice(&bytes).unwrap();
                let parsed = Instant::now();
                let identity = body["instructions"].as_str().unwrap().lines()
                    .filter_map(|line| serde_json::from_str::<Value>(line).ok())
                    .find(|value| value["id"].is_string() && value["cwd"].is_string()).unwrap();
                let id = identity["id"].as_str().unwrap();
                if body["input"].as_array().unwrap().iter().rev()
                    .find(|item| item["role"] == "user")
                    .is_some_and(|item| item["content"] == "dispatch fixture held proof") {
                    probe.mark(id, "held_proof_http_arrival", arrived);
                }
                assert!(body["input"].as_array().unwrap().iter().any(|item|
                    item["role"] == "user" && item["content"].as_str()
                        .is_some_and(|text| text.starts_with("dispatch fixture "))));
                probe.mark(id, "http_arrival", arrived);
                probe.mark(id, "http_body_read", read);
                probe.mark(id, "http_parsed", parsed);
                probe.mark(id, "mock_reply", Instant::now());
                ([("content-type", "text/event-stream")], format!("data: {}\n\ndata: {}\n\n",
                    json!({"type":"response.output_text.delta","output_index":0,"delta":"ok"}),
                    json!({"type":"response.completed","response":{"output":[{
                        "type":"message","role":"assistant","content":[{"type":"output_text","text":"ok"}]
                    }]}})))
            }
        }));
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let mut client = Client::test_client_at(endpoint);
        let p = probe.clone();
        client.set_test_worker_probe(Arc::new(move |id, stage| p.worker(id, stage)));
        let p = probe.clone();
        client.set_test_handoff_probe(Arc::new(move |stage| p.api(stage)));
        let state = tempfile::tempdir().unwrap();
        let work = tempfile::tempdir().unwrap();
        std::fs::create_dir(work.path().join(".git")).unwrap();
        let local_skills = 256;
        for index in 0..local_skills {
            let directory = work
                .path()
                .join(format!(".agents/skills/dispatch-{index:03}"));
            std::fs::create_dir_all(&directory).unwrap();
            std::fs::write(directory.join("SKILL.md"), format!(
                "---\nname: dispatch-{index:03}\ndescription: Synthetic dispatch timing fixture package {index}.\n---\n{}\n",
                "Fixture source text. ".repeat(100))).unwrap();
        }
        let hub = Hub::open(client, state.path().into()).unwrap();
        let mut ids = Vec::new();
        let mut discovered_skills = 0;
        for index in 0..agents {
            let view = hub
                .create(Create {
                    name: format!("dispatch-{index}"),
                    cwd: work.path().to_string_lossy().into_owned(),
                    message: String::new(),
                })
                .unwrap();
            hub.update(&view.id, |record| {
                discovered_skills = record.engine.skills.len();
                assert!(discovered_skills >= local_skills);
                if history_mib > 0 {
                    let skills = record.engine.skills.clone();
                    let mut encoded = serde_json::to_value(&record.engine)?;
                    encoded["history"] = Value::Array(
                        (0..history_mib * 16)
                            .map(|_| json!({"role":"assistant","content":"h".repeat(64 * 1024)}))
                            .collect(),
                    );
                    record.engine = serde_json::from_value(encoded)?;
                    record.engine.skills = skills;
                }
                record.view.entries.extend(
                    (0..(history_mib * 1024).max(1))
                        .map(|_| Hub::make_entry("assistant", "t".repeat(2048))),
                );
                Ok(())
            })
            .unwrap();
            ids.push(view.id);
        }
        for id in &ids {
            let lane = hub.0.data.lock().unwrap().sessions[id]
                .persistence
                .capacity();
            lane.durable_record().unwrap();
        }
        hub.0.compactions.wait(None);
        // Consume the actual Hub event stream used by SSE. This records delivery
        // to a subscriber, including receiver scheduling, not physical rendering.
        let mut events = hub.0.events.subscribe();
        let event_probe = probe.clone();
        let observer = tokio::spawn(async move {
            let mut assistants = BTreeSet::new();
            while let Ok(event) = events.recv().await {
                let Some(id) = event["session_id"].as_str() else {
                    continue;
                };
                if event["type"] == "entry" && event["entry"]["kind"] == "assistant" {
                    assistants.insert((
                        id.to_owned(),
                        event["entry"]["id"].as_str().unwrap().to_owned(),
                    ));
                }
                if event["type"] == "delta"
                    && event["text"] == "ok"
                    && assistants.contains(&(
                        id.to_owned(),
                        event["entry_id"].as_str().unwrap().to_owned(),
                    ))
                {
                    event_probe.mark(id, "assistant_delta_observed", Instant::now());
                }
            }
        });
        let fixture = Self {
            hub,
            probe,
            ids,
            server,
            observer,
            _state: state,
            _work: work,
            history_mib,
            local_skills,
            discovered_skills,
        };
        // Real first dispatch warms the worker/connection/catalog before measuring
        // ordinary subsequent messages. No worker or Session step is bypassed.
        fixture.round(0, false).await;
        fixture
    }
    async fn round(&self, round: usize, report: bool) {
        let barrier = Arc::new(tokio::sync::Barrier::new(self.ids.len() + 1));
        let mut tasks = Vec::new();
        for id in &self.ids {
            let (id, hub, barrier) = (id.clone(), self.hub.clone(), barrier.clone());
            tasks.push(tokio::spawn(async move {
                barrier.wait().await;
                let start = Instant::now();
                let (result, timings) =
                    hub.message_timed(&id, format!("dispatch fixture round {round}"));
                let acknowledged = Instant::now();
                result.unwrap();
                (id, start, acknowledged, timings)
            }));
        }
        barrier.wait().await;
        for task in tasks {
            let (id, start, acknowledged, timings) = task.await.unwrap();
            self.probe.finished(&id, start).await;
            let at = |stage| self.probe.after(&id, stage, start).unwrap();
            let scan = match (
                self.probe.after(&id, "scan_start", start),
                self.probe.after(&id, "scan_end", start),
            ) {
                (Some(a), Some(b)) => Some(elapsed(a, b)),
                _ => None,
            };
            if report {
                eprintln!(
                    "SYNTHETIC_MESSAGE_DISPATCH {}",
                    json!({
                        "agents":self.ids.len(),"history_mib_per_agent":self.history_mib,
                        "transcript_entries_per_agent":(self.history_mib * 1024).max(1),
                        "local_skill_files":self.local_skills,"discovered_skills":self.discovered_skills,
                        "round":round,"session_id":id,
                        "admission_ms":millis(timings.admission),
                        "lock_wait_ms":timings.lock_wait.map(millis),"mutation_ms":timings.mutation.map(millis),
                        "message_to_http_send_ms":elapsed(start,at("http_send")),
                        "ack_to_http_send_ms":elapsed(acknowledged,at("http_send")),
                        "message_to_response_observed_ms":elapsed(start,at("response_observed")),
                        "message_to_first_assistant_delta_ms":elapsed(start,at("assistant_delta_observed")),
                        "message_to_step_finished_ms":elapsed(start,at("step_finished")),
                        "scan_ms":scan,
                        "encoding_ms":elapsed(at("payload_encode_start"),at("payload_encode_end")),
                        "http_send_to_arrival_ms":elapsed(at("http_send"),at("http_arrival")),
                        "body_read_ms":elapsed(at("http_arrival"),at("http_body_read")),
                        "mock_json_parse_ms":elapsed(at("http_body_read"),at("http_parsed")),
                        "mock_reply_to_response_observed_ms":elapsed(at("mock_reply"),at("response_observed")),
                        "mock_reply_to_first_assistant_delta_ms":elapsed(at("mock_reply"),at("assistant_delta_observed")),
                        "response_observed_to_finished_ms":elapsed(at("response_observed"),at("step_finished")),
                        "scope":"warm actual Hub worker; native temp disk; loopback mock emits real output_text.delta; first delta includes Hub subscriber scheduling; no provider/browser latency; optional scan may finish after response"
                    })
                );
            }
        }
    }
    async fn close(self) {
        self.hub.shutdown().await;
        self.server.abort();
        self.observer.abort();
    }
}

async fn held_scan_case() -> bool {
    let fixture = Fixture::new(1, 0).await;
    let id = &fixture.ids[0];
    let (release, wait) = std::sync::mpsc::channel();
    let gate = Arc::new(SkillScanGate {
        started: Notify::new(),
        release: Mutex::new(wait),
    });
    *fixture.hub.0.skill_scan_gate.lock().unwrap() = Some(gate.clone());
    // Editing triggers candidate background discovery; the trigger message
    // also starts baseline per-message discovery. Send the measured message
    // only after the actual scanner is held, so an earlier request cannot pass.
    let skill = fixture
        ._work
        .path()
        .join(".agents/skills/dispatch-000/SKILL.md");
    let mut text = std::fs::read_to_string(&skill).unwrap();
    text.push_str("\nRefresh trigger fixture edit.\n");
    std::fs::write(skill, text).unwrap();
    fixture
        .hub
        .message(id, "dispatch fixture refresh trigger".into())
        .unwrap();
    let entered = tokio::time::timeout(Duration::from_secs(5), gate.started.notified())
        .await
        .is_ok();
    let start = Instant::now();
    fixture
        .hub
        .message(id, "dispatch fixture held proof".into())
        .unwrap();
    let complete = || async {
        let arrival = loop {
            if let Some(at) = fixture.probe.after(id, "held_proof_http_arrival", start) {
                break at;
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        };
        fixture.probe.finished(id, arrival).await;
    };
    let progressed = entered
        && tokio::time::timeout(Duration::from_millis(500), complete())
            .await
            .is_ok();
    // Always release the synthetic blocked filesystem before assertions/shutdown.
    let _ = release.send(());
    tokio::time::timeout(Duration::from_secs(15), complete())
        .await
        .unwrap();
    eprintln!(
        "SYNTHETIC_MESSAGE_HELD_SCAN {}",
        json!({"gate_entered":entered,
        "progressed_while_scan_held":progressed,"observation_window_ms":500})
    );
    fixture.close().await;
    assert!(entered, "fixture never held a real refresh");
    progressed
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn warm_message_dispatch_does_not_wait_for_skill_refresh() {
    assert!(
        held_scan_case().await,
        "warm message waited for skill refresh before actual provider response"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "Synthetic actual-worker benchmark; assigned native host only"]
async fn actual_worker_message_dispatch_benchmark() {
    for history_mib in [0, 4] {
        for agents in [1, 8] {
            let fixture = Fixture::new(agents, history_mib).await;
            for round in 1..=3 {
                fixture.round(round, true).await;
            }
            fixture.close().await;
        }
    }
    // Baseline deliberately records the blocked path without asserting new policy.
    held_scan_case().await;
}
