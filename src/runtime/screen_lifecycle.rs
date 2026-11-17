//! Empty-window removal. Browser selection/geometry remains local; IDs are shared.
use super::*;

impl Hub {
    pub fn remove_empty_screen(&self, screen: u32) -> Result<Value> {
        crate::blocking::run(|| {
            let mut data = self.0.data.lock().unwrap();
            if screen == 0 || screen > data.screens.count {
                bail!("Unknown screen.");
            }
            // Idempotent stale-browser calls and the last window are harmless.
            if data.screens.removed.contains(&screen)
                || data.screens.count as usize - data.screens.removed.len() == 1
            {
                return Ok(json!(data.screens));
            }
            let agents = data.sessions.values().any(|handle| {
                !handle.record.view.archived
                    && data
                        .screens
                        .screen(&format!("agent:{}", handle.record.view.id))
                        == screen
            });
            let terminals = data.terminal_summaries.iter().any(|(id, summary)| {
                summary["running"] == true
                    && summary["archived"] != true
                    && data.screens.screen(&format!("terminal:{id}")) == screen
            });
            let creating = data
                .terminal_creating
                .iter()
                .any(|id| data.screens.screen(&format!("terminal:{id}")) == screen);
            if agents || terminals || creating {
                return Ok(json!(data.screens));
            }
            let mut next = data.screens.clone();
            next.removed.insert(screen);
            next.panes.retain(|_, assigned| *assigned != screen);
            let groups_before = next.groups.len();
            next.groups.retain(|_, group| group.screen != screen);
            let groups_changed = next.groups.len() != groups_before;
            if groups_changed {
                next.groups_revision += 1;
            }
            next.revision += 1;
            self.0.store.save_screens(&next)?;
            data.screens = next;
            if groups_changed {
                self.groups_changed(&mut data);
            }
            let result = json!(data.screens);
            self.emit(&mut data, json!({"type":"screens","screens":result}));
            Ok(result)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn input() -> Create {
        Create {
            name: String::new(),
            cwd: String::new(),
            message: String::new(),
        }
    }
    fn removed(catalog: &Value, id: u32) -> bool {
        catalog["removed"].as_array().unwrap().contains(&json!(id))
    }

    #[tokio::test]
    async fn stable_removed_ids_survive_restart_and_defaults_choose_remaining_window() {
        let state = tempfile::tempdir().unwrap();
        let hub = Hub::open(Client::test_client(), state.path().into()).unwrap();
        hub.add_screen().unwrap();
        hub.add_screen().unwrap();
        let retained = hub.create_on_screen(input(), 3).unwrap();
        hub.entry(
            &retained.id,
            "assistant",
            "retained work on stable screen three",
        );
        let before = hub.view(&retained.id).unwrap();
        let (_, mut events) = hub.subscribe();
        assert!(removed(&hub.remove_empty_screen(1).unwrap(), 1));
        assert_eq!(events.try_recv().unwrap()["type"], "screens");
        let default = hub.create(input()).unwrap();
        assert_eq!(
            hub.subscribe_screen(true, Some(2)).0["sessions"][0]["id"],
            default.id
        );
        assert_eq!(
            serde_json::to_value(hub.view(&retained.id).unwrap()).unwrap(),
            serde_json::to_value(before).unwrap()
        );
        assert!(!removed(&hub.remove_empty_screen(2).unwrap(), 2));
        hub.close(&default.id).await.unwrap();
        assert!(removed(&hub.remove_empty_screen(2).unwrap(), 2));
        assert!(!removed(&hub.remove_empty_screen(3).unwrap(), 3)); // last window
        let next = hub.add_screen().unwrap();
        assert_eq!(next["count"], 4);
        assert!(hub.create_on_screen(input(), 1).is_err());
        assert!(hub.move_pane(&format!("agent:{}", retained.id), 2).is_err());
        let catalog = hub.screens();
        assert_eq!(hub.remove_empty_screen(1).unwrap(), catalog); // stale browser
        hub.shutdown().await;
        drop(events);
        drop(hub);
        let restored = Hub::open(Client::test_client(), state.path().into()).unwrap();
        assert_eq!(restored.screens(), catalog);
        let fresh = restored.create(input()).unwrap();
        assert_eq!(
            restored
                .0
                .data
                .lock()
                .unwrap()
                .screens
                .screen(&format!("agent:{}", fresh.id)),
            3
        );
        assert_eq!(
            restored
                .0
                .data
                .lock()
                .unwrap()
                .screens
                .screen(&format!("agent:{}", retained.id)),
            3
        );
        restored.shutdown().await;
    }

    #[tokio::test]
    async fn hidden_drafts_and_terminal_reservations_keep_windows_until_empty() {
        let hub = Hub::new(Client::test_client()).unwrap();
        hub.add_screen().unwrap();
        let visible = hub.create_on_screen(input(), 2).unwrap();
        let group = hub.profile(&visible.id).unwrap()["pane_group_id"]
            .as_str()
            .unwrap()
            .to_owned();
        let hidden = hub.create_in_group(input(), 2, Some(&group)).unwrap();
        hub.close(&visible.id).await.unwrap();
        assert!(!removed(&hub.remove_empty_screen(2).unwrap(), 2));
        assert!(hub.view(&hidden.id).is_some());
        hub.close(&hidden.id).await.unwrap();
        assert!(removed(&hub.remove_empty_screen(2).unwrap(), 2));
        hub.add_screen().unwrap();
        hub.reserve_terminal_screen("pending", Some(3)).unwrap();
        assert!(!removed(&hub.remove_empty_screen(3).unwrap(), 3));
        hub.cancel_terminal_creation("pending");
        assert!(removed(&hub.remove_empty_screen(3).unwrap(), 3));
        hub.add_screen().unwrap();
        hub.reserve_terminal_screen("live", Some(4)).unwrap();
        hub.terminal_upsert(json!({"id":"live","running":true,"archived":false}));
        assert!(!removed(&hub.remove_empty_screen(4).unwrap(), 4));
        hub.terminal_upsert(json!({"id":"live","running":false,"archived":false}));
        assert!(removed(&hub.remove_empty_screen(4).unwrap(), 4));
        assert!(!removed(&hub.remove_empty_screen(1).unwrap(), 1));
        hub.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn deletion_serializes_with_agent_creation_moves_and_terminal_reservations() {
        let hub = Hub::new(Client::test_client()).unwrap();
        let agent = hub.create(input()).unwrap();
        for operation in 0..3 {
            let screen = hub.add_screen().unwrap()["count"].as_u64().unwrap() as u32;
            let gate = Arc::new(std::sync::Barrier::new(2));
            let remover = hub.clone();
            let delete_gate = gate.clone();
            let deletion = tokio::task::spawn_blocking(move || {
                delete_gate.wait();
                remover.remove_empty_screen(screen).unwrap()
            });
            let creator = hub.clone();
            let pane = format!("agent:{}", agent.id);
            let admission = tokio::task::spawn_blocking(move || {
                gate.wait();
                match operation {
                    0 => creator.create_on_screen(input(), screen).map(|_| ()),
                    1 => creator.move_pane(&pane, screen).map(|_| ()),
                    _ => creator
                        .reserve_terminal_screen("race-terminal", Some(screen))
                        .map(|_| ()),
                }
            });
            let result = admission.await.unwrap();
            let catalog = deletion.await.unwrap();
            if result.is_ok() {
                assert!(!removed(&catalog, screen));
                assert!(hub.validate_screen(screen).is_ok());
            } else {
                assert!(removed(&catalog, screen));
                assert!(hub.validate_screen(screen).is_err());
            }
        }
        hub.cancel_terminal_creation("race-terminal");
        hub.shutdown().await;
    }
}
