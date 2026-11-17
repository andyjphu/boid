# Context blocks

Andy requested this vocabulary and behavior on 2026-11-17:

> stable context block (rules, practices etc); status context block (right now agents available and statuses); ephemeral? contextblock (better name?) (for role description right now but agents can put anything in this limited space) -- i.e. have a captain update its own system prompt to encourage/remind to delegate

Andy subsequently clarified that peers may edit each other's working context,
and that it must remain attached to agent identity through memory changes until
explicitly changed or cleared. This supersedes the initial self-only proposal.

Andy superseded the broad “agents can put anything” scope on 2026-11-17 after
observing task histories and result counts filling this space. The third block is
now **role context**. The original wording invited memory-bank use; this change
narrows the contract rather than treating that use as a proven agent violation.

- **Stable context:** operator-independent rules and practices, available-skill
  guidance, and tool-use conventions. This is the request's `instructions` field.
  Stable does not mean immutable: approved guidance and skill changes can update it.
- **Status context:** current session identity and open-agent IDs, names, statuses,
  listening and wake eligibility. The harness reads one in-memory roster snapshot
  for each model request. It excludes archived sessions and includes no transcript
  summaries, wall-clock ticker or speculative task progress. A working status does
  not prove completion. Status can change during inference; tool dispatch checks
  the actual current state.
- **Role context:** role, responsibilities, team, working style, current hard
  constraints and at most one pointer to a task/handoff file.
  Results, counts, completed work, next steps and interaction history belong in
  that file. Preserve those details before replacing an existing mixed note.
  `set_role_context({"text":"...","to":null})`
  replaces the caller's note; `to` can instead name an open peer's stable session
  ID. At most 1024 Unicode characters, including spaces and line breaks; empty text
  clears it. Each edit records the editor's stable ID and name. Changes
  apply to the next model request, persist through compaction, `/clear` and restart,
  and appear in the ordinary tool-call/result record. Earlier receipts retain the
  prior text, allowing a later explicit replacement to restore it.

Example captain note:

> Coordinate the swarm. Do small tasks directly when faster than handing them off.
> Delegate larger independent work, confirm acceptance, and inspect the result.
> Keep ownership clear and surface blockers promptly.

This example is not installed automatically. An agent can edit its own or a peer's
note in response to its assigned work. It should revise the note when the role or hard constraints change;
no periodic rewriting is required.

## Request representation and authority

Status and role context (text plus editor attribution) are serialized as
quoted JSON data in one request-only user-role item after conversation history. They are not new operator messages.
Stable instructions define their meaning: the role note is fallible agent data,
subordinate to stable rules, operator directions, stop orders and permissions.
Putting a reminder here does not grant authority or alter tool permissions.

The harness borrows existing history and appends this item while serializing;
it does not copy the transcript or append runtime snapshots to persisted history.
A changing roster therefore does not rewrite stable instructions or earlier input
items. Only one current runtime-context item is supplied per request. Provider
retries reuse the same encoded request; the next new model request takes a new
snapshot. `list_agents` remains available for an explicit refresh.

Compaction receives conversation history and current identity without the transient
roster/note item. The role note and its editor are stored against the stable
session ID, outside worker-owned model memory and independently of the compacted window,
so compaction, `/clear`, rename, restart and an older worker checkpoint cannot
overwrite a newer peer edit. Only an explicit replacement or clear changes it.
Ending an agent archives its note with that session. Reusing its bird name for a
new session creates a new identity with an empty note.
Concurrent explicit edits use the last accepted value. Editing does not wake,
interrupt or queue a turn for the target. A nonempty note makes an otherwise
empty agent persistent; its name and ID are retained across restart. User edits
and tool receipts remain ordinary history. A request-only snapshot is not a full request trace archive; exact request
capture, if needed, must record the encoded payload separately.

The server updates the target note in memory and uses the normal asynchronous checkpoint
path. No added file scan or synchronous durable write is required between inference
and tool execution. The ordinary crash-loss window still applies.

Records with working-context edits use format version 9. Older binaries reject
these records rather than silently discard the note. Rollback must retain the
upgraded state and restore the matching pre-upgrade state and binary together.

## Exporting context

`/eject-context` saves a private `ejected-context-<unique-id>.json` under
`$BOID_STATE_DIR/context-exports/<session-id>/`, normally
`~/.local/share/boid/context-exports/<session-id>/`. It makes no provider call
and does not clear, compact, wake or interrupt the agent. The `eject-context`
skill uses the equivalent `eject_context` tool.

The JSON contains the latest committed model history, instructions, tool
schemas, model name, and current status/role context. Images are embedded.
In-flight partial output and unapplied queued messages are excluded. This is
a current context snapshot, not a historical API request or full transcript.
Each export gets a new filename. Serialization and disk writes run outside
the global session lock; normal turns do not perform export work.

## Compatibility and existing-note migration

New requests advertise only `set_role_context`. Already-emitted
`set_working_context` calls remain executable; saved historical calls are not
rewritten. The on-disk `working_context` and `working_context_editor` fields keep
their names so existing state and recovery tooling remain compatible.

The authenticated operator endpoint `GET /api/sessions/{id}/role-context` returns
`{text, editor}` without a transcript. `PATCH` accepts `{text, expected}` where
`expected` is the exact prior GET object. It rejects stale text or attribution,
keeps the 1024-character cap, records Operator attribution and does not wake the
agent. The caller must preserve displaced task notes before replacing them.

For this transition, the release migration preserves each inspected nonempty
note verbatim with its editor in a private task-note file under
`$BOID_STATE_DIR/task-notes/<session-id>/`. Curated role notes retain current
constraints and point to that file. The migration compares the live note with
the inspected snapshot before changing it; changed notes require a fresh review.
Empty notes stay empty. Archived sessions are not rewritten. This is an explicit
operator-authorized migration, not an automatic semantic filter on future edits.

The content boundary is guidance. The server enforces length and edit authority,
not whether prose describes a role. Improved agent adherence remains unmeasured.
