# Context and agent state

The v1.1.58 refactor separates editing conversation memory from scheduling
execution. It adds no commands and keeps the existing saved-session schema.

| Responsibility | Code |
| --- | --- |
| Queued operator controls and dispatch | `src/runtime/input.rs` |
| Context clear, half-clear and replacement | `src/session/context.rs` |
| Immutable history items and half-selection algorithm | `src/session/history.rs` |
| Saved base prompt, reload and skill catalog | `src/session/prompt.rs` |
| Provider compaction, automatic preference and resume policy | `src/session/compaction.rs` |
| Copying a peer's committed context and image ownership | `src/runtime/context_take.rs` |
| Emergency peer recovery authorization | `src/runtime/peer_context.rs` |
| Role and color edits | `src/runtime/role_context.rs`, `src/runtime/agent_colors.rs` |
| Agent name and request profile | `Hub::rename`, `Hub::select_profile` in `src/runtime.rs` |
| Context export and persistence | `src/runtime/context_export.rs`, `src/runtime/journal.rs`, `src/runtime/persistence.rs` |

## Two owners

The worker owns the live `Session`: model history, partial response, pending
tools and compaction state. Queued context edits run at its existing input
boundary, after pending tools settle. `ContextEdit` provides clear, keep-recent-half
and replace operations. Those operations reset context-related execution state
and measurements together while preserving model, prompt, tools, preferences
and request count. They perform no IO.

`Hub` owns identity and shared metadata in `Record` and `View`: name, role,
color, pane assignment and credential selection. Existing metadata edits use
`update_locked` and coalesced background persistence. They do not wake or
interrupt a worker. Keeping these fields outside `Session` prevents an older
worker checkpoint from undoing a newer peer edit. Request construction reads
the current identity and role; context replacement does not replace them.

## Adding an edit

A future history-editing command belongs in the typed dispatch in
`runtime/input.rs`, with its transformation in `session/context.rs` or
`session/history.rs`. Admission validates the target; the worker rechecks state
at application. The Hub retains ownership of archive ordering, acknowledgments
and persistence. Do not mutate the saved `Record.engine` alone while its worker
is alive, because the next worker checkpoint would overwrite that mutation.

`InputKind` is derived from the existing persisted input fields. This keeps
older queued commands readable without adding another state migration. New
commands that older readers could misinterpret still need an explicit storage
compatibility decision. Image-bearing text retains its existing treatment as
ordinary input.

Cross-agent edits stage source snapshots under the existing mutex, perform
image IO outside it, and apply in the recipient worker. A source snapshot
contains committed history only. Unfinished source calls remain accounted for
without replaying them in the recipient. The emergency half-clear path retains
its own authorization and stale-error checks; sharing a transformation does
not grant permission to invoke it.

A new persistent identity attribute belongs alongside Hub-owned metadata,
with the same checkpoint-survival behavior as role and color. A new operation
on history belongs in the worker. A future plugin or memory backend can use
these boundaries when there is a concrete requirement; no plugin framework or
new memory policy is introduced here.

## Boundaries retained

- Model history contains shared immutable `Arc<Value>` items. Edits retain
  payload identity where possible; they do not deep-copy the whole conversation.
- `take_history` and `restore_history` support persistence serialization. They
  are not user context edits: restoration preserves a valid cache cursor and
  existing usage measurements. `ContextEdit` invalidates those context measurements.
- Provider compaction remains distinct from a clear or copy. It replaces the
  full canonical returned window and preserves its existing continuation and
  failure behavior. A failed compaction retains the prior context.
- Conversation history and the visible transcript are separate. A history edit
  preserves the transcript and archives prior context through the existing lane.
- Prompt reload retains the current catalog and history. It reads the deployed
  base prompt, not an unbuilt checkout file.
- No new disk wait, global mutex, background poll, roster refresh or provider
  request is introduced. Credentials still take effect at the next request.

Existing compaction, image, recovery, role, color, context, journal and input
ordering tests cover these paths. The additional compatibility fixtures protect
saved input dispatch and immutable snapshots across context edits. These are
behavior checks, not a claim about measured production latency.
