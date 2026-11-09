# Session system

A session has a stable server identity and a retained record. Its worker or shell
is an execution resource. A browser pane observes and controls that session.
Disconnecting a browser does not end work, and reconnecting does not start work.

The server sends one sequenced catalog snapshot containing agents and terminals,
then lifecycle updates. PTY bytes use their specialized WebSocket.

An agent or live terminal pane's × ends execution and preserves history.
+ agent opens a local setup pane with native placeholder defaults and Active sessions /
Inactive sessions choices. Name and working directory use the displayed defaults
when left blank. New agents start idle with no task; the first prompt starts work.
The setup can stay open while other panes are used. Creating or
reopening an agent replaces that setup in its position. Reopening leaves the
agent stopped until new input. A setup pane's × only discards the local draft.
The agent chooser contains only agent sessions. Terminal records remain
retained separately. Only running terminals have panes; ending a shell removes
its pane in every window, and refresh does not restore an exited terminal.

+ terminal starts immediately in the server's repository working directory.
The tab follows the observed shell working directory, with the full path in its
tooltip. The native runtime reads the shell process directory through macOS libproc
or Linux procfs; when observation is unavailable, the tooltip identifies the
path as last known.

Each browser tab retains a mixed order of agent, terminal, and setup pane IDs
plus unfinished setup fields in sessionStorage. Refresh/reconnect restores the
order; new sessions append. This storage contains browser layout and drafts,
not the server's authoritative agent record. Another window can have a different
layout. Clearing browser storage discards that view state without ending work.

## Prompt and activity layout

The prompt overlays the transcript. Hovering or focusing it does not move
messages. Typing, submission, or new output while following keeps the latest
message above a visible prompt. This clearance remains while hover, focus, or a
draft keeps the prompt visible. Once hidden, following targets the message end
rather than the extra scroll space. Reading history preserves its position.

A reserved footer line prevents working/idle transitions from resizing the
stream. Working titles shimmer; the footer shows elapsed time and the Escape
hint. The server retains the start time of each working period, so reconnecting
does not reset that counter.

## Usage diagnostics

The compact top-bar controls retain agent and terminal creation and add a small
usage diagnostics popup. Its cumulative request usage is separate from the
latest-response context measurement in agent tabs and prompts. The popup can
show all retained agents or one selected agent, including archived sessions.

Diagnostics preserve provider-reported usage, response identity, model and
service tier. Missing cache fields are unknown, not zero; missing historical
usage is not reconstructed from the last context measurement. Cache reuse is
cached input divided by input tokens. Uncached input alone does not establish
a cache failure. Provider-reported comparison diagnostics are retained when
present; opening the popup does not send model requests or change cache settings.

Dollar amounts are estimates from dated published model rates, not invoices.
Unsupported models, unknown service tiers or incomplete billing components do
not receive a guessed price. Reasoning tokens are already included in output.
The source rates and cache-write accounting are documented in the
[model page](https://developers.openai.com/api/docs/models/gpt-6-sol) and
[prompt caching guide](https://developers.openai.com/api/docs/guides/prompt-caching).

## Continuity follow-up

Andy explicitly added continuity to this session rework. Work should survive
context rollover and server restart while retaining its objective, corrections,
completed work, and unfinished actions. Long-running jobs should remain observable
and controllable.

Durable records and paused restart recovery establish the first foundation.
They do not provide compaction or uninterrupted execution. The later change must
bound model working context without discarding authoritative history, retain the
reason and scope of corrections, and preserve action/job identity and control.
A saved transcript or an unverified summary alone does not meet that outcome.
This remains open work after the initial session-lifecycle change.

Persistence still holds the shared catalog lock while saving or syncing a session.
Per-session asynchronous write ordering and delta-sync batching remain post-v1
work; any change must preserve durable acknowledgement and stream replay order.
Revisit this sooner if measured multi-agent latency requires it. Recovery
isolation for already-corrupted terminal records is also follow-up work; the
server currently reports malformed complete records rather than silently
discarding them.

## Peer messages

New v0.9 sessions advertise regular and emergency messaging. Existing sessions
retain their original model, instructions, tools and skill environment; their
regular sends remain valid. They can receive emergency messages while runnable.
Urgent peer input cancels foreground work or pending skill discovery and uses
the last committed skill snapshot. Cancelled scans finish cleanup without
applying late results or starting overlapping scans.

Records with v0.9 capabilities or message state use version 3. Before upgrading
an older recipient record, Boid retains its original JSON in a `.before-v9` file.
Older binaries reject version 3. Back up the full state directory before a
software rollback; copying a single older record over current state would lose
later work. Restart still restores agents paused. When storage fails, End stops
execution and reports that cancellation and archival were not saved.
