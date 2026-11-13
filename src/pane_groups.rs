//! Shared pane membership and credentials; browser geometry/active tab remain local.
use super::*;

#[derive(Debug)]
pub struct GroupRevisionConflict;
impl std::fmt::Display for GroupRevisionConflict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Pane groups changed. Refresh and try again.")
    }
}
impl std::error::Error for GroupRevisionConflict {}

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct PaneGroup {
    pub id: String,
    pub screen: u32,
    pub tabs: Vec<String>,
    pub credential_id: Option<String>,
    pub credential_type: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn shared_group_binding_covers_background_tabs_and_survives_restart() {
        let dir = tempfile::tempdir().unwrap();
        let work = tempfile::tempdir().unwrap();
        let auth = Arc::new(
            crate::chatgpt::Auth::open(dir.path().join("auth"), "api_key", "fixture").unwrap(),
        );
        auth.migrate_credentials("fixture-first-key").unwrap();
        let first_key = auth.profile().unwrap()["credential_id"]
            .as_str()
            .unwrap()
            .to_owned();
        let second_key = auth
            .create_credential("Second", "fixture-second-key")
            .await
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let client = || {
            let mut client = Client::test_client();
            client.auth = Some(auth.clone());
            client
        };
        let hub = Hub::open(client(), dir.path().join("state")).unwrap();
        let input = || Create {
            name: String::new(),
            cwd: work.path().to_string_lossy().into_owned(),
            message: String::new(),
        };
        let one = hub.create(input()).unwrap();
        let group = one.profile["pane_group_id"].as_str().unwrap().to_owned();
        assert_eq!(group, format!("agent:{}", one.id));
        let two = hub.create_in_group(input(), 1, Some(&group)).unwrap();
        let three = hub.create_in_group(input(), 1, Some(&group)).unwrap();
        let revision = hub.pane_groups()["revision"].as_u64().unwrap();
        hub.assign_group_credential(&group, Some(&second_key), revision)
            .unwrap();
        assert!(
            hub.assign_group_credential(&group, Some(&first_key), revision)
                .is_err()
        );
        for view in [&one, &two, &three] {
            assert_eq!(hub.profile(&view.id).unwrap()["credential_id"], second_key);
            assert_eq!(
                hub.view(&view.id).unwrap().profile["credential_id"],
                second_key
            );
            assert_eq!(
                hub.profile(&view.id).unwrap()["model"],
                view.profile["model"]
            );
        }
        // Individual screen movement detaches only the chosen tab and carries its binding.
        hub.add_screen().unwrap();
        hub.move_pane(&format!("agent:{}", two.id), 2).unwrap();
        assert_eq!(hub.screens()["panes"][format!("agent:{}", two.id)], 2);
        assert_eq!(hub.profile(&one.id).unwrap()["pane_group_id"], group);
        assert_ne!(hub.profile(&two.id).unwrap()["pane_group_id"], group);
        assert_eq!(hub.profile(&two.id).unwrap()["credential_id"], second_key);
        let saved = hub.pane_groups();
        hub.shutdown().await;
        drop(hub);
        let hub = Hub::open(client(), dir.path().join("state")).unwrap();
        assert_eq!(hub.pane_groups(), saved);
        auth.delete_credential(&second_key).await.unwrap();
        hub.credential_deleted(&second_key).unwrap();
        for id in [&one.id, &two.id, &three.id] {
            assert_eq!(hub.profile(id).unwrap()["billing_route"], "unassigned");
            assert!(hub.profile(id).unwrap()["credential_id"].is_null());
        }
        assert!(
            auth.named_api_key(&first_key).is_ok(),
            "Unrelated key remains available, never used as fallback"
        );
        {
            let mut data = hub.0.data.lock().unwrap();
            hub.remove_group_member_locked(&mut data, &format!("agent:{}", one.id))
                .unwrap();
        }
        assert_eq!(
            hub.profile(&one.id).unwrap()["billing_route"],
            "unassigned",
            "A missing authoritative group must not expose the older stored key"
        );
        hub.shutdown().await;
    }
}

impl Hub {
    pub(super) fn validate_groups(
        screens: &mut Screens,
        auth: Option<&crate::chatgpt::Auth>,
    ) -> Result<()> {
        let registry = auth.map(|auth| auth.credentials()).transpose()?;
        let mut seen = BTreeSet::new();
        let mut changed = false;
        for (id, group) in &mut screens.groups {
            if id != &group.id || group.screen == 0 || group.screen > screens.count {
                bail!("Invalid saved pane groups; original retained.");
            }
            for key in &group.tabs {
                if !(key.starts_with("agent:") || key.starts_with("terminal:"))
                    || !seen.insert(key.clone())
                {
                    bail!("Conflicting saved pane membership; original retained.");
                }
                if screens.panes.get(key).copied().unwrap_or(1) != group.screen {
                    bail!("Conflicting saved pane screen; original retained.");
                }
            }
            if let Some(registry) = &registry {
                let kind = group
                    .credential_id
                    .as_deref()
                    .and_then(|id| {
                        registry["credentials"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .find(|row| row["id"] == id)
                    })
                    .and_then(|row| row["type"].as_str())
                    .map(str::to_owned);
                if kind != group.credential_type {
                    group.credential_type = kind;
                    if group.credential_type.is_none() {
                        group.credential_id = None;
                    }
                    changed = true;
                }
            }
        }
        if changed {
            screens.groups_revision += 1;
        }
        Ok(())
    }

    fn binding(&self, profile: &Value) -> Result<(Option<String>, Option<String>)> {
        let Some(auth) = &self.0.client.auth else {
            // Synthetic clients have no registry and never escape their fixture endpoint.
            return Ok((None, Some("api_key".into())));
        };
        let id = auth.legacy_credential(profile)?;
        let kind = id
            .as_deref()
            .map(|id| auth.credential_type(id))
            .transpose()?;
        Ok((id, kind))
    }
    pub(super) fn effective_profile(data: &Data, id: &str, source: &Value) -> Value {
        let key = format!("agent:{id}");
        let mut profile = source.clone();
        let Some(group) = data.screens.groups.values().find(|g| g.tabs.contains(&key)) else {
            profile["credential_id"] = Value::Null;
            profile["billing_route"] = json!("unassigned");
            profile["chatgpt_account"] = Value::Null;
            profile["pane_group_id"] = Value::Null;
            return profile;
        };
        profile["pane_group_id"] = json!(group.id);
        if group.credential_id.is_none() && group.credential_type.as_deref() == Some("api_key") {
            return profile;
        }
        profile["credential_id"] = json!(group.credential_id);
        profile["billing_route"] = json!(group.credential_type.as_deref().unwrap_or("unassigned"));
        profile["chatgpt_account"] = if group.credential_type.as_deref() == Some("chatgpt") {
            json!(group.credential_id)
        } else {
            Value::Null
        };
        profile["cyber_access_program"] = if group.credential_type.as_deref() == Some("api_key") {
            if source["billing_route"].as_str().unwrap_or("api_key") == "api_key" {
                source["cyber_access_program"].clone()
            } else {
                data.api_program.clone()
            }
        } else {
            Value::Null
        };
        profile
    }
    fn insert_singleton_locked(
        &self,
        data: &mut Data,
        key: String,
        screen: u32,
        profile: &Value,
    ) -> Result<()> {
        if data.screens.groups.values().any(|g| g.tabs.contains(&key)) {
            return Ok(());
        }
        let (credential_id, credential_type) = self.binding(profile)?;
        let mut next = data.screens.clone();
        let id = if next.groups.contains_key(&key) {
            format!("group:{}", Uuid::new_v4())
        } else {
            key.clone()
        };
        next.groups.insert(
            id.clone(),
            PaneGroup {
                id,
                screen,
                tabs: vec![key],
                credential_id,
                credential_type,
            },
        );
        next.groups_revision += 1;
        self.0.store.save_screens(&next)?;
        data.screens = next;
        Ok(())
    }
    pub(super) fn prune_absent_agent_groups_locked(&self, data: &mut Data) -> Result<()> {
        let keep = |key: &str| {
            key.strip_prefix("agent:").is_none_or(|id| {
                data.sessions
                    .get(id)
                    .is_some_and(|h| !h.record.view.archived)
            })
        };
        let mut next = data.screens.clone();
        let mut changed = false;
        for group in next.groups.values_mut() {
            let before = group.tabs.len();
            group.tabs.retain(|key| keep(key));
            changed |= before != group.tabs.len();
        }
        next.groups.retain(|_, group| !group.tabs.is_empty());
        let previous = next.panes.len();
        next.panes.retain(|key, _| keep(key));
        changed |= previous != next.panes.len();
        if changed {
            next.groups_revision += 1;
            next.revision += 1;
            self.0.store.save_screens(&next)?;
            data.screens = next;
            self.groups_changed(data);
        }
        Ok(())
    }
    pub(super) fn remove_group_member_locked(&self, data: &mut Data, key: &str) -> Result<()> {
        let mut next = data.screens.clone();
        let mut changed = false;
        for group in next.groups.values_mut() {
            let before = group.tabs.len();
            group.tabs.retain(|member| member != key);
            changed |= before != group.tabs.len();
        }
        if !changed {
            return Ok(());
        }
        next.groups.retain(|_, group| !group.tabs.is_empty());
        next.panes.remove(key);
        next.groups_revision += 1;
        self.0.store.save_screens(&next)?;
        data.screens = next;
        self.groups_changed(data);
        Ok(())
    }
    pub(super) fn ensure_agent_group_locked(&self, data: &mut Data, view: &View) -> Result<()> {
        let key = format!("agent:{}", view.id);
        if view.archived {
            return self.remove_group_member_locked(data, &key);
        }
        self.insert_singleton_locked(data, key.clone(), data.screens.screen(&key), &view.profile)
    }
    pub(super) fn ensure_terminal_group_locked(
        &self,
        data: &mut Data,
        id: &str,
        screen: u32,
    ) -> Result<()> {
        self.insert_singleton_locked(
            data,
            format!("terminal:{id}"),
            screen,
            &self.0.client.config(),
        )
    }
    pub(super) fn live_pane(data: &Data, key: &str) -> bool {
        if let Some(id) = key.strip_prefix("agent:") {
            data.sessions
                .get(id)
                .is_some_and(|h| !h.record.view.archived)
        } else if let Some(id) = key.strip_prefix("terminal:") {
            data.terminal_summaries
                .get(id)
                .is_some_and(|v| v["running"] == true && v["archived"] != true)
        } else {
            false
        }
    }
    pub(super) fn pane_groups_value(data: &Data) -> Value {
        let groups: Vec<_> = data
            .screens
            .groups
            .values()
            .filter_map(|g| {
                let tabs: Vec<_> = g
                    .tabs
                    .iter()
                    .filter(|key| Self::live_pane(data, key))
                    .cloned()
                    .collect();
                if tabs.is_empty() {
                    return None;
                }
                let mut group = g.clone();
                group.tabs = tabs;
                Some(group)
            })
            .collect();
        json!({"revision":data.screens.groups_revision,"groups":groups})
    }
    pub fn pane_groups(&self) -> Value {
        crate::blocking::run(|| Self::pane_groups_value(&self.0.data.lock().unwrap()))
    }
    pub(super) fn groups_changed(&self, data: &mut Data) -> Value {
        let result = Self::pane_groups_value(data);
        self.emit(data, json!({"type":"pane_groups","pane_groups":result}));
        result
    }
    pub fn move_tab(
        &self,
        pane: &str,
        target: Option<&str>,
        before: Option<&str>,
        revision: u64,
    ) -> Result<Value> {
        crate::blocking::run(|| {
            let mut data = self.0.data.lock().unwrap();
            if revision != data.screens.groups_revision {
                return Err(GroupRevisionConflict.into());
            }
            if !Self::live_pane(&data, pane) {
                bail!("Tab is no longer open.");
            }
            let source_id = data
                .screens
                .groups
                .values()
                .find(|g| g.tabs.iter().any(|k| k == pane))
                .context("Tab has no pane group.")?
                .id
                .clone();
            let mut next = data.screens.clone();
            let source = next.groups[&source_id].clone();
            let target_id = if let Some(id) = target {
                let group = next
                    .groups
                    .get(id)
                    .context("Destination pane no longer exists.")?;
                if group.screen != source.screen {
                    bail!("Move the pane to this screen before moving its tabs.");
                }
                id.to_owned()
            } else {
                let id = format!("group:{}", Uuid::new_v4());
                next.groups.insert(
                    id.clone(),
                    PaneGroup {
                        id: id.clone(),
                        tabs: vec![],
                        ..source.clone()
                    },
                );
                id
            };
            next.groups
                .get_mut(&source_id)
                .unwrap()
                .tabs
                .retain(|key| key != pane);
            let group = next.groups.get_mut(&target_id).unwrap();
            let index = match before {
                Some(key) => group
                    .tabs
                    .iter()
                    .position(|k| k == key)
                    .context("Destination tab no longer exists.")?,
                None => group.tabs.len(),
            };
            group.tabs.insert(index, pane.to_owned());
            next.groups.retain(|_, g| !g.tabs.is_empty());
            next.groups_revision += 1;
            self.0.store.save_screens(&next)?;
            data.screens = next;
            Ok(self.groups_changed(&mut data))
        })
    }
    pub fn assign_group_credential(
        &self,
        id: &str,
        credential_id: Option<&str>,
        revision: u64,
    ) -> Result<Value> {
        crate::blocking::run(|| {
            let mut data = self.0.data.lock().unwrap();
            if revision != data.screens.groups_revision {
                return Err(GroupRevisionConflict.into());
            }
            let kind = credential_id
                .map(|id| {
                    self.0
                        .client
                        .auth
                        .as_ref()
                        .context("Credential storage unavailable.")?
                        .credential_type(id)
                })
                .transpose()?;
            let mut next = data.screens.clone();
            let group = next
                .groups
                .get_mut(id)
                .context("Pane group no longer exists.")?;
            group.credential_id = credential_id.map(str::to_owned);
            group.credential_type = kind;
            next.groups_revision += 1;
            self.0.store.save_screens(&next)?;
            data.screens = next;
            Ok(self.groups_changed(&mut data))
        })
    }
    /// Clear even archived membership, so reopening cannot resurrect a deleted binding.
    pub fn credential_deleted(&self, id: &str) -> Result<()> {
        crate::blocking::run(|| {
            let mut data = self.0.data.lock().unwrap();
            let mut next = data.screens.clone();
            let mut changed = false;
            for group in next.groups.values_mut() {
                if group.credential_id.as_deref() == Some(id) {
                    group.credential_id = None;
                    group.credential_type = None;
                    changed = true;
                }
            }
            if changed {
                next.groups_revision += 1;
                let saved = self.0.store.save_screens(&next);
                data.screens = next;
                self.groups_changed(&mut data);
                saved?;
            }
            Ok(())
        })
    }
    pub fn broadcast_credentials(&self, registry: Value) {
        crate::blocking::run(|| {
            let mut data = self.0.data.lock().unwrap();
            self.emit(&mut data, json!({"type":"credentials","registry":registry}));
        });
    }
}
