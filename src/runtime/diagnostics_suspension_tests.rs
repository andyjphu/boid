use super::*;
use std::os::unix::fs::PermissionsExt;

#[tokio::test]
async fn diagnostics_suspension_preserves_legacy_bytes_before_rewrite() {
    let state = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let hub = Hub::open(Client::test_client(), state.path().into()).unwrap();
    let view = hub
        .create(Create {
            name: "fixture".into(),
            cwd: work.path().to_string_lossy().into(),
            message: String::new(),
        })
        .unwrap();
    hub.entry(&view.id, "assistant", "Retained transcript and result.");
    hub.shutdown().await;
    let mut effective: Record = hub.0.store.read_session(&view.id).unwrap();
    hub.0
        .store
        .replay_events::<journal::Event>(&view.id, |event| event.apply(&mut effective))
        .unwrap();
    hub.0.store.save(&view.id, &effective).unwrap();
    hub.0
        .store
        .rotate_events(&view.id, effective.journal_seq)
        .unwrap();
    hub.0
        .store
        .prune_events(&view.id, effective.journal_seq)
        .unwrap();
    drop(hub);
    let path = state
        .path()
        .join("sessions")
        .join(format!("{}.json", view.id));
    let backup = path.with_extension("before-diagnostics-suspension");
    let mut legacy: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    legacy["version"] = json!(5);
    legacy["engine"]["history"] = json!([
        {"role":"user","content":"Retained objective"},
        {"type":"reasoning","encrypted_content":"opaque fixture context"},
        {"type":"function_call","call_id":"done","name":"read_file","arguments":"{}"},
        {"type":"function_call_output","call_id":"done","output":"Retained research result"}
    ]);
    // Deliberately not a valid Ledger: loading must skip diagnostics instead of
    // allocating or decoding Report values, while preserving all original bytes.
    legacy["usage"] = json!({"future_diagnostic_shape": "x".repeat(2 * 1024 * 1024)});
    let original = serde_json::to_vec_pretty(&legacy).unwrap();
    std::fs::write(&path, &original).unwrap();
    let restored = Hub::open(Client::test_client(), state.path().into()).unwrap();
    assert_eq!(std::fs::read(&backup).unwrap(), original);
    assert_eq!(
        std::fs::metadata(&backup).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let saved: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(saved["version"], 7);
    assert!(saved.get("usage").is_none());
    assert_eq!(saved["view"]["entries"], legacy["view"]["entries"]);
    assert_eq!(saved["engine"]["history"], legacy["engine"]["history"]);
    {
        let data = restored.0.data.lock().unwrap();
        let record = &data.sessions[&view.id].record;
        assert!(record.usage.records.is_empty() && record.usage.failures.is_empty());
        // Calling the disabled endpoint while holding Data must not acquire it.
        assert_eq!(restored.diagnostics()["enabled"], false);
        assert!(
            serde_json::to_value(record.clone())
                .unwrap()
                .get("usage")
                .is_none()
        );
    }
    restored.shutdown().await;
    drop(restored);
    let reopened = Hub::open(Client::test_client(), state.path().into()).unwrap();
    assert_eq!(std::fs::read(&backup).unwrap(), original);
    reopened.shutdown().await;
    drop(reopened);
    // An existing different backup must never permit losing another old ledger.
    legacy["usage"]["future_diagnostic_shape"] = json!("different retained diagnostics");
    let changed = serde_json::to_vec(&legacy).unwrap();
    std::fs::write(&path, &changed).unwrap();
    assert!(Hub::open(Client::test_client(), state.path().into()).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), changed);
    assert_eq!(std::fs::read(&backup).unwrap(), original);
}
