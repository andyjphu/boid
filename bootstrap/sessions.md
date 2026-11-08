# Session system

A session has a stable server identity and a retained record. Its worker or shell
is an execution resource. A browser pane observes and controls that session.
Disconnecting a browser does not end work, and reconnecting does not start work.

The server sends one sequenced catalog snapshot containing agents and terminals,
then lifecycle updates. PTY bytes use their specialized WebSocket.

An agent or live terminal pane's × ends execution and preserves history.
+ agent opens a local setup pane with editable defaults and active/inactive
session choices. It can stay open while other panes are used. Creating or
reopening an agent replaces that setup in its position. Reopening leaves the
agent stopped until new input. A setup pane's × only discards the local draft.
Retained terminals are available in the setup pane's Terminals group; viewing
one replays its output without starting a replacement shell.

+ terminal starts immediately in the server's repository working directory.
The tab follows the observed shell working directory, with the full path in its
tooltip. The Linux runtime reads the shell process directory; when observation
is unavailable, the tooltip identifies the path as last known.

Each browser tab retains a mixed order of agent, terminal, and setup pane IDs
plus unfinished setup fields in sessionStorage. Refresh/reconnect restores the
order; new sessions append. This storage contains browser layout and drafts,
not the server's authoritative agent record. Another window can have a different
layout. Clearing browser storage discards that view state without ending work.

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
