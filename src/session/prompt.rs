//! Saved base instructions and skill catalog, independent of conversation edits.
use super::*;

impl Session {
    pub(crate) fn stable_instructions(&self) -> String {
        format!(
            "## Stable context\n{}\n\n{}\n\n{}\n\n{}\n\n{}",
            self.instructions,
            tool_calls::PEER_TRACE_GUIDANCE,
            crate::request_context::GUIDANCE,
            parallel::GUIDANCE,
            include_str!("../TEAMWORK.md")
        )
    }

    pub(crate) fn reload_system_prompt(&mut self) {
        // Refresh the deployed base while retaining the current skill catalog.
        let catalog = self
            .skill_base
            .as_ref()
            .and_then(|base| self.instructions.strip_prefix(base))
            .map(str::to_owned)
            .unwrap_or_else(|| {
                format!(
                    "\n{}",
                    skills::catalog::prompt(&self.skills, 8000, &self.skill_warnings)
                )
            });
        let base = include_str!("../PRINCIPLES.md");
        self.instructions = format!("{base}{catalog}");
        self.skill_base = Some(base.into());
    }
    pub fn refresh_skills(&mut self, snapshot: skills::discovery::Snapshot) {
        let Some(base) = self.skill_base.as_ref() else {
            return;
        };
        let prompt = skills::catalog::prompt(&snapshot.skills, snapshot.budget, &snapshot.warnings);
        let instructions = format!("{base}\n{prompt}");
        if self.skills == snapshot.skills
            && self.skill_warnings == snapshot.warnings
            && self.instructions == instructions
        {
            return;
        }
        self.skills = snapshot.skills;
        self.skill_snapshot = None;
        self.skill_warnings = snapshot.warnings;
        self.instructions = instructions;
        let notice = format!(
            "Skill catalog refreshed for this operator message. This replaces earlier skill availability and policy; previously loaded text is historical.\n{prompt}\nDiscovery warnings: {}",
            json!(self.skill_warnings)
        );
        self.history
            .push(json!({"role":"developer","content":notice}));
    }
}
