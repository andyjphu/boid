# Debugging a live agent workspace without losing the accepted behavior

2026-11-09 Pacific. Agent Cataclysm's account of Boid work through v1.1.6 and
the proposed Escape repair. Andy requested this record after saying:

> great! you've been performing so well, please document everything in your mind so this performance can keep happening

This is a record of observable decisions, results, and working hypotheses.
The praise supports retaining the approach for reuse. Its future effectiveness
has not been measured. The [source excerpts](evidence/live-agent-debugging/excerpts.json)
retain the corrections that preceded it.

## Begin with the behavior that matters to the operator

Andy wanted a workspace that stayed usable while agents worked: scrolling back
should remain stable, visible peers should be contactable, Escape should
register promptly, and changes should reach the actual server without losing
history. Those outcomes were more useful than starting from a component or a
preferred optimization.

A short follow-up often supplied the decisive distinction. “occurs when
scrolling past bottom” located one boundary. “i scroll up a little bit” separated
a failing transition from a large gesture that passed the earlier checks.
The [scrolling case](scroll-follow-boundary.md) records both the successful
repairs and the missed small-gesture check. Future investigations should carry
that distinction into the reproduction instead of treating another passing
large-scroll test as an answer.

The accepted baseline included a composer that could reserve space below the
transcript, selection and copying during streaming, expanded tool results,
and links to old messages. A repair had to preserve those behaviors. Temporary
culling improved performance but introduced another geometry interaction.
Andy later explicitly removed it. That correction remained binding during the
Escape investigation; virtualization stayed separate v2 work.

## Separate states that look the same on screen

Several problems came from treating different states as interchangeable:

| Visible signal | Distinction needed for the repair |
| --- | --- |
| A pane exists | The session may be idle and listening, explicitly stopped, interrupted, or archived. |
| The page reloaded | Browser reconnection and server restart have different effects on execution. |
| Interrupt returned HTTP 202 | The server accepted the request; the worker may not yet have stopped. |
| The viewport is near the bottom | Continuing to follow differs from resuming after an upward reading gesture. |
| A command exists in code | The operator still needs to discover and invoke it in the composer. |
| A PR is merged | The running executable and an already-open browser may still be older. |

These distinctions suggest where to inspect, not a new mandatory checklist.
Follow the path relevant to the symptom and find the event that changes state.
For Escape, that meant the key handler, HTTP acceptance, worker interruption,
and the status event. For peer availability, it meant persisted session state,
startup recovery, the worker's wake conditions, and the model-facing roster.

## Preserve the complaint while testing its explanation

Andy described peers as absent from `list_agents` until greeted. Inspection
found that the built-in roster already included every nonarchived session;
reviewed saved results contained all seven agents. Literal omission was not
reproduced. The availability defect was real: startup paused every restored
session, and shutdown used the same flag as explicit Stop.

The repair saved restart-listening intent separately. Eligible idle agents
could receive peer messages after recovery without an invented greeting or an
initial model request. Explicit Stop, errors, and interrupted work retained
their holds. Queued peer messages were allowed to run after the complete roster
had been installed. Tests covered first contact, preserved stops, legacy
records, and messages accepted just before shutdown.

The first test draft assumed that an explicit resume would consume queued
emergency and regular messages in one provider turn. The failure showed the
existing priority behavior required two turns. The test was corrected to
verify both turns; production behavior was not weakened to satisfy the draft.

The deployed v1.1.5 restored all seven eligible agents, and Canary consumed
its previously accepted message. This established the observed recovery result.
It did not establish that every historical report of a missing peer had the
same cause. [Implementation](https://github.com/andyjphu/boid/commit/3b0bf45),
[test correction](https://github.com/andyjphu/boid/commit/b9ce127).

## Measure a representative workload without exercising real agents

The performance investigations replayed captured histories with network routes
mocked. This retained the actual transcript shapes and sizes while avoiding
production messages, interruptions, and paid model calls. Raw histories and
credentials stayed private; published evidence contained selected user excerpts
and numeric outputs.

The first peer-render investigation reproduced a browser handler taking about
1.3 seconds with 4,172 entries. The profile located repeated selection/layout
work and unrelated tool-result rendering. Indexing peer entries and tool pairs
removed that stall in the fixture. Those measurements remain valid evidence
for that repair even though later scrolling reports exposed other defects.

The Escape investigation used a later 19,576-entry capture. Ten presses with
quick HTTP acceptance and deliberately delayed stopped status produced ten
interrupt requests. The browser guard covered only the HTTP request. A local
patch held the guard through status confirmation, ignored held-key repeats,
showed “Interrupt requested,” and retained retry after HTTP failure. A fresh
press after confirmed interruption could still stop the next task.

The same replay exposed whole-transcript mounting and grouping on text-only
streaming updates. Dirty-state tracking skipped that work, and a valid visible
scroll anchor could be reused. All transcript entries remained available.

| Isolated replay observation | Before | Local patch |
| --- | ---: | ---: |
| Interrupt requests from ten rapid presses | 10 | 1 |
| Median queued input-task delay while reading | 33.0 ms | 21.6 ms |
| Median queued input-task delay while following | 24.1 ms | 19.6 ms |

These are timer/task measurements beside simulated rendering, not end-to-end
keyboard-to-worker latency. Layout remained material, and initial rendering
still took about a second. A fast server identity endpoint did not measure
persistence or the interruption path. Source inspection found full-record
saves under a shared lock, but their contribution to the reported live stall
was not causally timed. The [numeric evidence](evidence/live-agent-debugging/README.md)
and [proposed repair, PR #20](https://github.com/andyjphu/boid/pull/20) retain those limits.
The patch was tested and published, not deployed, when Andy requested this record.

## Reuse working mechanisms and test the distinction introduced

When Andy requested explicit `/oai-compact`, the backend already supported it.
The discoverability problem was in a menu that showed one suggestion. Listing
all matching commands with keyboard and mouse selection made the existing
operation available without another model turn or a duplicate compaction path.

`/clear` needed different behavior: discard the model conversation while
retaining the agent's identity, configuration, visible transcript, and lifetime
usage. It ran after the current response and tool batch, archived the previous
context before clearing, and made no provider request by itself. Tests checked
that old conversation content stayed out of the next request after restart,
that tool results completed before clearing, and that archive failure retained
both context and queued input. These were consequences worth checking because
a superficially successful clear could otherwise lose work or leak old context
into the next task. [PR #19](https://github.com/andyjphu/boid/pull/19).

Verification effort followed the changed behavior. Runtime lifecycle changes
received Rust checks with a synthetic provider. Browser changes received real
keyboard, mouse, wheel, selection, and streaming checks in Chrome. Documentation
received source, link, and rendered-reading checks. Passing a focused check did
not justify claiming an untested interaction, and an unrelated full test suite
was not rerun for a prose edit.

## Finish the delivery that was authorized

A deployed fix needed a continuous chain from reviewed source to the experience
Andy opened: commit, executable version, running PID and port, served JavaScript,
and a fresh browser. Source files alone could not establish which code an old
process or tab was using. The live checks preserved session IDs and prior
transcript entry IDs, checked the listener, and inspected the actual interface.
They did not send synthetic instructions to existing agents.

Andy asked for the previous version to be pinned and for restart start and
finish announcements. Those preferences were retained across subsequent Boid
work. Existing publication permission avoided repeated generic sign-offs, while
restart timing was handled according to the active request. For the Escape
investigation he explicitly said not to restart; the server stayed untouched
and the patch remained reviewable on its own branch. The praise did not merge
that PR or authorize a later restart.

## What to carry into the next task

Recover the intended behavior and the latest correction before editing. Use a
small reproduction that retains the detail the operator says changes the
outcome. Locate the state transition and measure the expensive path. Prefer a
repair that preserves the accepted interaction, then check both entering and
leaving the affected state. Report what was reproduced, what was deployed, and
what remains an inference.

This method still made mistakes: early scroll checks missed small gestures;
a test assumed the wrong message-turn count; a deployment helper initially had
a syntax error and was corrected before it ran. Keeping those facts makes the
record more useful than a retrospective containing only successful conclusions.
The next agent should reuse the evidence and judgment, not mechanically repeat
the full investigation or treat this case as an additional approval gate.

The existing [bootstrap judgment entries](../README.md#judgment) already cover
customer intent, cumulative feedback, and proportionate verification. This
case supplies concrete examples rather than replacing them. Relevant unresolved
work remains open: live validation of PR #20 after authorization, isolated
measurement of server interrupt persistence, and the deferred transcript
virtualization design. Creating this document is not evidence that future
performance has improved.

## 2026-11-11 update

Andy subsequently authorized merging PR #20. It merged as `18ad92e` after
Chrome regression checks passed against current main. The merged tree matched
the tested tree. Deployment and live validation remain pending; the original
account above describes the state when the case was written. The original
request prohibited restarting, while leaving the PR unmerged was the agent’s
scope interpretation, not an explicit no-merge instruction.
