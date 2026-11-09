# Session system

A session has a stable server identity and a retained record. Its worker or shell
is an execution resource. A browser pane observes and controls that session.
Disconnecting a browser does not end work, and reconnecting does not start work.

The server sends one sequenced catalog snapshot containing agents and terminals,
then lifecycle updates. PTY bytes use their specialized WebSocket.

An agent or live terminal pane's × ends execution and preserves history.
+ agent opens a local setup pane with native placeholder defaults and Active sessions /
Inactive sessions choices. Blank fields use the displayed defaults on creation.
It can stay open while other panes are used. Creating or
reopening an agent replaces that setup in its position. Reopening leaves the
agent stopped until new input. A setup pane's × only discards the local draft.
The agent chooser contains only agent sessions. Terminal records remain
retained separately. Only running terminals have panes; ending a shell removes
its pane in every window, and refresh does not restore an exited terminal.

+ terminal starts immediately in the server's repository working directory.
The tab follows the observed shell working directory, with the full path in its
tooltip. The Linux runtime reads the shell process directory; when observation
is unavailable, the tooltip identifies the path as last known.

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
