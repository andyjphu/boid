use super::*;

impl Hub {
    pub fn set_agent_color(&self, from: &str, arguments: &str) -> Result<Value> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Edit {
            color: String,
            #[serde(default)]
            to: Option<String>,
        }
        let edit: Edit = serde_json::from_str(arguments)?;
        if !tool_calls::AGENT_COLORS.contains(&edit.color.as_str()) {
            bail!(
                "Unknown color. Choose {}.",
                tool_calls::AGENT_COLORS.join(", ")
            );
        }
        crate::blocking::run(|| {
            let mut data = self.0.data.lock().unwrap();
            if data.closing {
                bail!("Server is shutting down.");
            }
            let sender = data
                .sessions
                .get(from)
                .filter(|h| !h.record.view.archived)
                .context("Editor is unavailable.")?;
            let editor = sender.record.view.name.clone();
            let target = edit.to.as_deref().unwrap_or(from);
            let id = if data
                .sessions
                .get(target)
                .is_some_and(|h| !h.record.view.archived)
            {
                target.to_owned()
            } else {
                let mut matches = data.sessions.values().filter(|h| {
                    !h.record.view.archived && h.record.view.name.eq_ignore_ascii_case(target)
                });
                let found = matches.next().context("Unknown open agent.")?;
                if matches.next().is_some() {
                    bail!("Agent name is ambiguous; use its session ID.");
                }
                found.record.view.id.clone()
            };
            let color = (edit.color != "grey").then(|| edit.color.clone());
            if data.sessions[&id].record.view.tab_color != color {
                self.update_locked(&mut data, &id, None, |record| {
                    record.view.tab_color = color;
                    record.view.entries.push(Self::make_entry(
                        "notice",
                        format!("Tab color updated by {editor} ({from}): {}", edit.color),
                    ));
                    Ok(())
                })?;
            }
            Ok(json!({"to":id,"color":edit.color}))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn agent_colors_survive_worker_checkpoint_clear_move_and_restart_without_waking() {
        let dir = tempfile::tempdir().unwrap();
        let work = tempfile::tempdir().unwrap();
        let hub = Hub::open(Client::test_client(), dir.path().into()).unwrap();
        let create = |name: &str| {
            hub.create(Create {
                name: name.into(),
                cwd: work.path().to_string_lossy().into(),
                message: String::new(),
            })
            .unwrap()
        };
        let captain = create("captain");
        let peer = create("peer");
        hub.set_agent_color(&captain.id, r#"{"color":"cyan"}"#)
            .unwrap();
        let stale = hub.0.data.lock().unwrap().sessions[&peer.id]
            .record
            .engine
            .clone();
        let (_, mut events) = hub.subscribe();
        hub.set_agent_color(&captain.id, r#"{"to":"PEER","color":"cyan"}"#)
            .unwrap();
        let mut color_event = false;
        while let Ok(event) = events.try_recv() {
            color_event |= event["type"] == "tab_color"
                && event["session_id"] == peer.id
                && event["tab_color"] == "cyan";
        }
        assert!(color_event);
        hub.checkpoint(&peer.id, &stale).unwrap();
        assert_eq!(
            hub.view(&peer.id).unwrap().tab_color.as_deref(),
            Some("cyan")
        );
        assert_eq!(
            hub.request_context(&peer.id).unwrap()["self"]["color"],
            "cyan"
        );
        assert!(
            hub.set_agent_color(
                &captain.id,
                &json!({"to":peer.id,"color":"#ffffff"}).to_string()
            )
            .is_err()
        );
        hub.message(&peer.id, "/clear".into()).unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if hub.view(&peer.id).unwrap().pending.is_empty() {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert_eq!(
            hub.view(&peer.id).unwrap().tab_color.as_deref(),
            Some("cyan")
        );
        hub.stop(&peer.id).unwrap();
        for color in tool_calls::AGENT_COLORS {
            hub.set_agent_color(
                &captain.id,
                &json!({"to":peer.id,"color":color}).to_string(),
            )
            .unwrap();
        }
        hub.set_agent_color(
            &captain.id,
            &json!({"to":peer.id,"color":"blue"}).to_string(),
        )
        .unwrap();
        {
            let data = hub.0.data.lock().unwrap();
            let record = &data.sessions[&peer.id].record;
            assert!(record.paused);
            assert_eq!(record.engine.request_count(), 0);
        }
        hub.add_screen().unwrap();
        let moved = hub.move_pane(&format!("agent:{}", peer.id), 2).unwrap();
        assert_eq!(moved["moved_pane"], format!("agent:{}", peer.id));
        hub.shutdown().await;
        drop(hub);
        let restored = Hub::open(Client::test_client(), dir.path().into()).unwrap();
        assert_eq!(
            restored.view(&captain.id).unwrap().tab_color.as_deref(),
            Some("cyan")
        );
        assert_eq!(
            restored.view(&peer.id).unwrap().tab_color.as_deref(),
            Some("blue")
        );
        assert_eq!(restored.screens()["panes"][format!("agent:{}", peer.id)], 2);
        restored
            .set_agent_color(
                &captain.id,
                &json!({"to":peer.id,"color":"grey"}).to_string(),
            )
            .unwrap();
        assert!(restored.view(&peer.id).unwrap().tab_color.is_none());
        restored.shutdown().await;
    }
}
