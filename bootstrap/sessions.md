# Session system

A session has a stable server identity and a retained record. Its worker or shell
is an execution resource. A browser pane observes and controls that session.
Disconnecting a browser does not end work, and reconnecting does not start work.

The server sends one sequenced catalog snapshot containing agents and terminals,
then lifecycle updates. PTY bytes use their specialized WebSocket.

The pane's × ends execution and preserves history. History opens retained agent
records and terminal replay. Reopening an agent leaves it stopped until new input.
Viewing an ended terminal does not create a replacement shell.

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
