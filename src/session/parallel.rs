use super::*;
use futures_util::{StreamExt, stream::FuturesUnordered};

// Limits process fan-out per agent without making other agents wait for its tools.
const LIMIT: usize = 8;
pub(super) const GUIDANCE: &str = "## Tool concurrency\nIssue independent tool calls together in one response; Boid runs up to eight concurrently, including shell commands. Calls in the same batch must not depend on each other's results or mutate shared resources incompatibly. Put dependent work in a later response after inspecting the results. File writes/edits, peer-control actions and context changes execute in order and separate concurrent groups. Shell commands can have arbitrary effects: batching them declares they are independent. Results are attributed by call ID and may arrive out of order.";

pub(super) fn eligible(name: &str) -> bool {
    matches!(
        name,
        "read_file"
            | "glob"
            | "search"
            | "exec_command"
            | "list_skills"
            | "read_skill"
            | "list_agents"
            | "read_peer_trace"
    )
}

impl Session {
    pub(super) async fn run_parallel(
        &mut self,
        _client: &Client,
        tools: &Tools,
        hub: &Hub,
        id: &str,
    ) -> Result<bool> {
        let mut admission_closed = false;
        // Own these snapshots so completed calls can checkpoint mutable session state.
        let skills = self.skills.clone();
        let warnings = self.skill_warnings.clone();
        let mut work = FuturesUnordered::new();
        loop {
            while !admission_closed
                && work.len() < LIMIT
                && self
                    .pending
                    .front()
                    .is_some_and(|call| eligible(&call.name))
            {
                let call = self.pending.pop_front().unwrap();
                self.parallel_running.push(call.clone());
                if !hub.claim_tool(id, self)? {
                    self.parallel_running.pop();
                    self.pending.push_front(call);
                    admission_closed = true;
                    break;
                }
                hub.entry_replacing(
                    id,
                    "tool_call",
                    format!("{} ({})\n{}", call.name, call.id, call.arguments),
                    call.argument_entry.clone(),
                );
                let skills = &skills;
                let warnings = &warnings;
                work.push(async move {
                    #[cfg(test)]
                    _client.test_handoff_mark("tool_dispatch");
                    let result = match call.name.as_str() {
                        "list_skills" => {
                            json!({"skills":skills::catalog::list(skills),"warnings":warnings})
                        }
                        "read_skill" => skills::read(skills, &call.arguments),
                        "list_agents" => hub.peers(),
                        "read_peer_trace" => hub.read_peer_trace(id, &call.arguments),
                        _ => tool_calls::execute(tools, &call.name, &call.arguments).await,
                    };
                    #[cfg(test)]
                    _client.test_handoff_mark("tool_return");
                    (call, result)
                });
            }
            let Some((call, result)) = work.next().await else {
                break;
            };
            let position = self
                .parallel_running
                .iter()
                .position(|running| running.id == call.id)
                .expect("completed call must be tracked");
            self.parallel_running.remove(position);
            // Save each completion immediately. Escape drops the remaining futures,
            // kills their process groups, and recovers only outstanding call IDs.
            self.finish(call, result, hub, id)?;
        }
        Ok(true)
    }
}
