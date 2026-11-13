use super::*;

#[tokio::test]
async fn screen_membership_filters_streams_and_survives_restart_without_changing_agents() {
    let dir = tempfile::tempdir().unwrap();
    let hub = Hub::open(Client::test_client(), dir.path().into()).unwrap();
    let input = || Create {
        name: String::new(),
        cwd: String::new(),
        message: String::new(),
    };
    let first = hub.create(input()).unwrap();
    assert_eq!(hub.screens(), json!({"revision":0,"count":1,"panes":{}}));
    assert_eq!(hub.add_screen().unwrap()["count"], 2);
    let (_, mut events) = hub.subscribe();
    let second = hub.create_on_screen(input(), 2).unwrap();
    let creation = events.try_recv().unwrap();
    assert_eq!(creation["session"]["screen"], 2);
    let (one, _) = hub.subscribe_screen(true, Some(1));
    let (two, _) = hub.subscribe_screen(true, Some(2));
    assert_eq!(one["sessions"].as_array().unwrap().len(), 1);
    assert_eq!(one["sessions"][0]["id"], first.id);
    assert_eq!(two["sessions"][0]["id"], second.id);
    assert_eq!(two["catalog"].as_array().unwrap().len(), 2);
    assert!(!hub.screen_event(
        &json!({"type":"delta","session_id":second.id,"screen":2}),
        Some(1)
    ));
    assert!(hub.screen_event(
        &json!({"type":"delta","session_id":second.id,"screen":2}),
        Some(2)
    ));
    assert!(hub.screen_event(&json!({"type":"closed","session_id":second.id}), Some(1)));
    let before = serde_json::to_value(hub.view(&second.id)).unwrap();
    hub.move_pane(&format!("agent:{}", second.id), 1).unwrap();
    assert_eq!(serde_json::to_value(hub.view(&second.id)).unwrap(), before);
    assert!(hub.move_pane(&format!("agent:{}", second.id), 99).is_err());
    assert!(hub.move_pane("agent:missing", 2).is_err());
    assert!(hub.create_on_screen(input(), 99).is_err());
    hub.move_pane(&format!("agent:{}", second.id), 2).unwrap();
    hub.assign_terminal_screen("test", 2).unwrap();
    hub.terminal_upsert(json!({"id":"test","running":true,"archived":false}));
    hub.move_pane("terminal:test", 1).unwrap();
    assert_eq!(hub.add_screen().unwrap()["count"], 3); // Empty screens persist, too.
    let saved = hub.screens();
    hub.shutdown().await;
    drop(events);
    drop(hub);
    let restored = Hub::open(Client::test_client(), dir.path().into()).unwrap();
    assert_eq!(restored.screens(), saved);
    assert_eq!(
        restored.subscribe_screen(true, Some(2)).0["sessions"][0]["id"],
        second.id
    );
    restored.shutdown().await;
}
