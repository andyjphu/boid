//! Bounded disk benchmark. Synthetic data only; no model requests.
use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "Synthetic disk persistence benchmark; run only on assigned host"]
async fn incremental_disk_checkpoint_benchmark() {
    let state = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let hub = Hub::open(Client::test_client(), state.path().into()).unwrap();
    let target = hub
        .create(Create {
            name: "disk-benchmark".into(),
            cwd: work.path().to_string_lossy().into(),
            message: String::new(),
        })
        .unwrap();
    hub.entry(&target.id, "assistant", "synthetic seed");
    let worker = hub
        .0
        .data
        .lock()
        .unwrap()
        .sessions
        .get_mut(&target.id)
        .unwrap()
        .task
        .take();
    if let Some(worker) = worker {
        worker.abort();
        let _ = worker.await;
    }
    let mut session = hub.0.data.lock().unwrap().sessions[&target.id]
        .record
        .engine
        .clone();
    let mut encoded = serde_json::to_value(&session).unwrap();
    encoded["history"] = Value::Array(
        (0..64)
            .map(|_| json!({"role":"assistant","content":"x".repeat(64*1024)}))
            .collect(),
    );
    let skills = session.skills.clone();
    session = serde_json::from_value(encoded).unwrap();
    session.skills = skills;
    hub.update(&target.id, |r| {
        r.engine = session.clone();
        r.view
            .entries
            .extend((0..128).map(|_| Hub::make_entry("assistant", "y".repeat(64 * 1024))));
        Ok(())
    })
    .unwrap();
    let mut checkpoint_ms = Vec::new();
    let mut entry_ms = Vec::new();
    for i in 0..16 {
        session.context_usage = Some(crate::session::ContextUsage {
            input_tokens: 10000 + i,
            output_tokens: 100 + i,
        });
        let start = Instant::now();
        hub.checkpoint(&target.id, &session).unwrap();
        checkpoint_ms.push(start.elapsed().as_secs_f64() * 1000.0);
        let start = Instant::now();
        hub.entry(
            &target.id,
            "assistant",
            format!("synthetic update {i}: {}", "z".repeat(256)),
        );
        entry_ms.push(start.elapsed().as_secs_f64() * 1000.0);
    }
    assert_ne!(hub.view(&target.id).unwrap().status, "error");
    let before_shutdown_bytes: u64 = std::fs::read_dir(state.path().join("sessions"))
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().unwrap().is_file())
        .map(|e| e.metadata().unwrap().len())
        .sum();
    eprintln!(
        "SYNTHETIC_DISK {}",
        json!({"history_mib":4,"transcript_mib":8,"iterations":16,
        "checkpoint_ms":checkpoint_ms,"entry_ms":entry_ms,"state_bytes_before_shutdown":before_shutdown_bytes})
    );
    hub.shutdown().await;
}
