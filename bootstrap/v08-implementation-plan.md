Boid v0.8 implementation plan after v0.7.2

Review by Agent Catastrophe, 2026-11-08. Scope: integrate the existing local-skills
and case-bank work with the newly accepted v0.7.2 baseline. This is a review and
implementation plan as originally written. Execution was subsequently assigned;
see the [implementation notes](v08-integration-notes.md) for the completed work.

The recommendation is to start the integration branch from PR7 head `b18db46`,
which retains the v0.7.2 implementation and adds final documentation, and port
the skills work onto it. Preserve the accepted composer, activity footer and session
persistence behavior. Retain the local skill modules and Humanizer. Adapt the
picker, skill trace presentation and refresh lifecycle before calling v0.8 ready.

**Baseline and evidence**

| Item | Reviewed state | Meaning |
| --- | --- | --- |
| Accepted v0.7.2 | Annotated tag at `1cf12a14fc986e0cea967db49990aa2e81d359fc` | Prompt-clearance result explicitly approved by Andy. |
| Earlier checkpoint | v0.7.1 at `4fed21b` | Retained pre-trial comparison, not the integration target. |
| Skills draft | `98218f3`, branch `v8-skills`, based on `36351e2` | Built and checked against an earlier v7 state; those checks do not establish compatibility with v0.7.2. |
| Case-bank commits | `260d50f`, `e20d5ce` | Existing source evidence and preliminary analysis to preserve. |
| Release head | `b18db46a8acd970009ed795727cdbfa3feb77366` on `origin/v7` | Fetched and verified: only `bootstrap/tools.md` and `bootstrap/sessions.md` differ from the tag. Includes final UX wording and persistent state/theme-volume instructions. |
| Publication at final review | [PR7](https://github.com/andyjphu/boid/pull/7) open, unmerged, targeting main | Andy's instruction to Ursa to make the PR was verified in the original trace. Its existence and exact head were independently checked through GitHub. |

Source review covered all six files changed between `36351e2` and v0.7.2,
relevant skill/runtime/server code, the original recent user turns in Ursa's
trace, the annotated tag, Ursa's task record, and the latency evidence report.
The accepted active-composition screenshot was inspected. Ursa's browser checks
and latency timings are attributed evidence; this review did not rerun them or
claim fresh end-to-end verification. No live session, terminal or deployment was
changed. A detached source worktree was used for the tagged baseline.

Merge-tree probes against both the tag and final PR7 head, without modifying
either worktree, found one conflicted file:
`src/web/app.js`, with four conflict regions. `runtime.rs`, `style.css`, and the
documentation merge textually. A clean textual merge is insufficient evidence
that their behavior remains correct.

**Recent decisions that constrain implementation**

The original trace is
`/Users/andyphu/.codex/sessions/2026/11/04/rollout-2026-11-04T00-23-31-01a1a5a7-fa67-7b01-b37e-befb3dfa996d.jsonl`.
Line numbers below identify original user events, not agent paraphrases.

- Line 13748 requests a prompt over the stream without movement on hover and
  distinguishes the message endpoint from extra scroll room. Line 13760 accepts
  the proposed implementation. Preserve both spatial stability and access to
  covered content.
- Lines 14358 and 14365 clarify the remaining problem in an agent pane: typing
  and followed conversation must remain visible while the prompt is present.
  This refines the earlier hover behavior rather than reversing it.
- Line 14266: "keep the space where the working text would appear, pre padded so there isnt too much jumping around".
- Line 14308 removes the prompt hover tooltip. Do not reintroduce it while
  rebuilding the composer or picker.
- Line 14384 requests the v0.7.1 checkpoint before the trial. Line 14522:
  "mark this as an extremely good ux result, im not sure what lessojn there is to learn here but it was an amazing result, mark this 0.7.2".
- Lines 13595, 13644 and 13686 establish the close button inside the compact tab,
  natural label growth within pane width, and removal of ended terminal panes.
  Terminal history remains stored; it is not a live shell or an active pane.
- Line 14231 authorizes Ursa to restart as necessary in his ongoing preview
  assignment. This is not a new permission gate for that work, nor blanket
  permission for this planning task to mutate the live preview. Line 14550
  authorizes Ursa to prepare his PR; it does not publish the skills/case changes.

The reason to retain v0.7.2 is concrete user acceptance of the combined behavior.
A plausible interpretation is that passive reveal and active conversation need
different scrolling behavior. Treat that as a case-specific inference, not a
universal prohibition on movement or an already-proven agent-guidance repair.

**Review findings and integration consequences**

| Priority | Finding | Evidence and consequence |
| --- | --- | --- |
| Required | The draft picker uses the old pane structure. | v0.7.2 has `.agent-stream`, `.trace-content`, `.composer-overlay`, a resize observer and message-based following. The draft appends trace/composer directly to the pane. Choosing that conflict side would discard the accepted behavior. |
| Required | The draft writes the activity text directly. | v0.7.2 owns it through `updateActivity`, a server timestamp and a one-second update. Its hint currently checks only the model picker. A naive port can erase elapsed time or show “Esc to interrupt” while Escape actually closes the skills picker. The draft's `activity.hidden` assignment would also remove the reserved footer. |
| Required | Selecting a skill currently dispatches input before showing the composer. | `openSkillPicker` dispatches `input`, then calls its close function. v0.7.2's `activatePromptSpace` exits while the composer is hidden. A direct port would miss clearance for the inserted skill draft until a later event. |
| Required | Explicit skill text is an ordinary later Context entry. | `Session::operator` saves the complete JSON/body; `putEntry` collapses only the first Context entry. Humanizer is 32,348 bytes. The code therefore supplies an uncollapsed instruction dump during explicit selection. This was identified by source review, not a browser reproduction in this review. |
| Required | Refresh waits outside the worker's interrupt selection. | The skills draft awaits `spawn_blocking(scan)` before `next_input`. Moving I/O off Tokio threads does not let that session handle Stop/Escape until the awaited scan completes. The previous fix skips cancelled queued messages but does not handle cancellation arriving during discovery. |
| Required preservation; measurement pending | v0.7.2 fixes checkpoint write amplification. | `b60aaeb` serializes to bytes and writes once, retaining atomic rename, file/directory sync and journal cleanup. The old skills branch lacks that commit. It must be retained during integration. Additional skill bodies increase record size; the latency effect of that increase is unmeasured. |
| Documentation | Tagged docs lag several accepted behaviors. | They still describe a collapsing composer, left close control and terminal replay panes. PR7 commit `b18db46` corrects these and documents durable state/theme volumes. Retain those corrections, then add skill-specific behavior. |

The latency report at Ursa's `.ursa/agent-latency-evidence/review.md` compares the
same debug binary and controlled storage paths. It reports Enter acceptance
79.23/337.69 ms before buffering versus 1.02/2.42 ms after, and Escape-to-stopped
662.88/2047.28 ms versus 4.98/12.13 ms. These are two observations per condition,
not percentiles or a universal latency promise. They support preserving the
buffered commit path, not attributing all delay to debug compilation.

**Implementation sequence**

1. **Establish the integration branch and preserve the baseline.**

   Create a new branch from `b18db46`, retaining the v0.7.2 tag as the executable
   baseline. Keep `98218f3` and both accepted tags intact. Apply the four existing
   skills/case commits with manual resolution of `app.js`. Recheck PR7 before
   finalizing the branch. If executable code changes, review that delta before
   treating it as the new base. Once PR7 merges, use its actual merge result as
   the publication base; avoid presenting the entire v7 change as new v8 work.

   Retain v0.7.2's buffered `SessionStore::save`, `working_since_ms` lifecycle,
   stopped recovery, native placeholder defaults, agent-only chooser, live-only
   terminal panes, compact headers, 900 ms scrollbar fade, and reduced-motion
   behavior. Keep new skill state optional on deserialization. Existing v7
   sessions must preserve their original tools/instructions and gain no skills
   through an implicit migration. The new capability applies to new sessions.

   Completion evidence: diff from the accepted baseline contains the intended
   skill additions without reversions of those behaviors. No conflict markers,
   unrelated formatting rewrite or live-data migration. This step establishes
   source integration, not release readiness.

2. **Adapt the skill picker to the accepted composer and footer.**

   In `src/web/app.js`, retain the stream/overlay construction and attach the
   skill picker beside the existing model picker as an explicit command view.
   Passive hover/focus continues to reveal only the composer without moving
   messages. Opening an explicit picker may use the established model-picker
   layout; this need not become a redesign of both pickers.

   Use a small shared picker-state helper for activity hints and Escape routing.
   Route all footer updates through `updateActivity`. Preserve elapsed time
   while suppressing the interrupt hint whenever Escape means closing a picker.
   Keep the footer's reserved space in idle/stopped/error states. Do not set
   `activity.hidden` or replace its text independently.

   Preserve `clearPromptSpaceWhenHidden`, `activatePromptSpace`, history anchors,
   and the overlay resize observer in the four conflict regions. On selection:
   insert the exact source token, restore the composer, focus the input, then
   dispatch input/activate clearance. Do not submit automatically. Preserve
   ordinary drafts, multiline shrink behavior and the tooltip removal.

   Guard asynchronous catalog completion with the current session and picker
   request identity. Closing or reopening a picker, or ending the pane, must
   prevent an older response from replacing the current view or stealing focus.
   Keep errors visible without replacing a newer draft. This follows the v7
   stale-response handling already used for session choices.

   Completion evidence: the existing v0.7.2 clearance sequence still works with
   skill selection inserted into it; Escape closes the picker without sending
   an interrupt; the timer and current status remain accurate after a second
   elapses, after reconnect, and after picker closure.

3. **Make skill events readable while retaining exact evidence.**

   Change the skill-specific event representation in `session.rs`, `runtime.rs`
   and the renderer to distinguish successful explicit loads, catalog refreshes
   and selection failures. Use a compact disclosure for the exact loaded body,
   with name and source available for inspection. Keep discovery errors and
   selection failures visible when collapsed. An ambiguous, missing or disabled
   selection must not look like a successful load.

   Preserve the complete model-visible content and saved history. Render the
   saved event's source/content, not a fresh filesystem read that could show a
   later version. Retain source ids for same-name packages, full tool output,
   and the distinction between a loading event and proof of application.
   Do not conceal earlier correction evidence when a skill refreshes.

   Keep this a skill-specific presentation change. Ordinary user corrections,
   runtime errors and existing context notices should not become collapsed
   merely because they share the generic Context label today. Existing saved
   records continue to render through their current path.

   Completion evidence: explicit Humanizer selection has a compact initial
   display, expands to the exact captured text, leaves the new message visible
   above the composer, and keeps source/load failures readable. Reopening the
   session shows the same historical body after the local file changes.

4. **Protect interruption and measure the added persistence cost.**

   Retain operator-message-boundary discovery, stable snapshots during tool
   continuations and peer input, and no old-session migration. Make discovery
   a pending worker operation that participates in the existing priority control
   path. Stop, interrupt and shutdown must remain observable while it runs.
   Avoid holding the Hub mutex during filesystem scanning.

   A started blocking filesystem task cannot be assumed cancelled by dropping
   its join handle. Use a cancellation signal checked between discovery reads,
   retain at most one scan per session, and reject stale results by generation
   before applying a snapshot or consuming input. If a read itself is slow,
   acknowledge the control transition without launching provider work; allow
   cleanup to finish under the existing shutdown policy. Do not start repeated
   scans on successive interrupts. Cancelled inputs still skip discovery.

   Extend the existing integration check with a gated discovery result: Stop
   and Escape take effect before release of that result; its late completion
   cannot revive work or alter a newer snapshot. Preserve existing queue order,
   checkpoint-failure handling and continuation behavior.

   Then reuse the existing isolated latency method on mm-west-2 Docker with the
   same mount/debug configuration. Compare no-local-skills, Humanizer-only and
   the intended local skill bank under short and retained histories. Record
   catalog size, snapshot bytes, scan time, Enter acceptance and Escape-to-stopped
   separately. These comparisons establish whether the new work erodes the
   observed improvement; do not invent an absolute “0 ms” acceptance threshold.

   Keep the current snapshot representation if the added cost is acceptable.
   If measurements show repeated copying/serialization of unchanged bodies is
   material, separate immutable skill snapshots from mutable session checkpoints
   using content-addressed records. Commit a referenced snapshot durably before
   the session pointer, retain historical versions, report missing snapshots,
   and avoid automatic deletion of evidence. That is a conditional repair,
   not an unmeasured storage redesign required before the first integration.

5. **Update the case, documentation and release evidence.**

   Extend the existing whole-interaction case with the successful v0.7.1 to
   v0.7.2 refinement. Retain the original dense hover request, later visibility
   correction, checkpoint request, exact approval, source pins and observed
   checks. Mark the outcome as explicit approval of this UX result. Keep the
   tentative interpretation separate from proven future agent improvement.

   Integrate Ursa's final documentation corrections, then document the skill
   picker interaction, refresh/control behavior, captured-versus-live resources,
   and old-session compatibility. Preserve the module map so future meta-review
   agents can propose fixes in selection, guidance, discovery, persistence,
   rendering or code. A presentation fault should not automatically become a
   new instruction in every skill.

   Use the existing Rust suite and the expanded skill integration test. Combine
   the existing clearance and skills browser checks rather than create a large
   parallel suite. Run the build/tests on mm-west-2 Docker, inspect the actual
   narrow and wide layouts, and review the final diff against the accepted base.
   Preserve measured failures and fixes in the task record.

   Prepare a focused PR description identifying v7 as the baseline and listing
   actual validation. Publish the skills/case content only within its specific
   authorization. For any authorized preview rollout, preserve histories,
   environment/profile and theme provisioning; report shells interrupted by a
   necessary restart. Verify running binary/assets and recovered records before
   calling it deployed. The existing skill tests are not evidence of a live
   model following Humanizer; record actual compliance only when observed.

**Focused acceptance checks**

| User-visible situation | Required result |
| --- | --- |
| Hover/click an idle prompt | No transcript movement or resize. |
| Type, select a skill, submit, or receive followed output | Latest content clears the visible prompt; multiline shrink leaves no stale large gap. |
| Read history or select transcript text | Anchor/selection remains stable through loads, refresh and incoming output. |
| Leave the pane with a draft or focused input | Clearance remains while the prompt is visible; actual hiding restores the message endpoint. |
| Idle/working/stopped/error transitions | Footer geometry stays fixed; elapsed time follows server state; no invented progress or stopped status. |
| Open skills, wait, close, reopen, or end the pane | Correct Escape meaning, no stale response takeover, no provider call merely from opening or selecting. |
| Duplicate, disabled, missing or changed skill | Source identity and actual load result agree; failures visible; history retains the earlier exact body. |
| Stop during discovery, tool work, or queued input | Existing control priority remains effective; late scan completion cannot resume cancelled work. |
| Restart/reopen old and new sessions | History and identity retained; old tools unchanged; no shell or uncertain action automatically replayed. |

The first four implementation steps should be separate reviewable commits, with
case/docs and validation following. The bounded frontend and runtime work should
remain one integration effort. Storage changes are conditional on the latency
comparison. Ursa confirmed PR7 and the preserved implementation baseline; no
additional worker has been assigned this plan.

Immediate Sending/Interrupting feedback remains an optional refinement suggested
during the latency investigation. Do not replace authoritative saved/stopped
state with optimistic state. Removing the current wait for in-flight submission
would require preserving submit/interrupt ordering, so it is a separate protocol
change rather than a cosmetic shortcut in this integration.

Full Codex plugin/cloud distribution, marketplaces and installer parity remain
distant backlog and may never be needed. Generic/meta-review templates, automatic
review triggers, peer-context editing, context compaction and durable background
jobs remain separate open work. Prior truthfulness audit findings, including
stale disconnected activity and hidden tool-result warnings, remain recorded;
the present source review does not claim they were repaired by v0.7.2.


Implementation update, 2026-11-08: Andy subsequently assigned execution from
current main and consultation with Ursa. The integration baseline is merged
PR7 at `eb35dfd`, including the later storage-failure End path and terminal
partial-write repair. The plan above remains the pre-implementation review.
Implementation evidence and the measured snapshot-storage decision are recorded
in [v0.8 integration notes](v08-integration-notes.md).
