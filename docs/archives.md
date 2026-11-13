# Ended sessions

End removes an agent from Boid after its worker has stopped and its final record
is durable. Open idle, paused and background agents remain live. Ended agents
are absent from the runtime roster, diagnostics, model peer traces, history
picker and normal history APIs. They cannot be reopened. Their names may be
reused by new agents.

Boid retains the files privately in the state directory, normally
`~/.local/share/boid/` on the server. `BOID_STATE_DIR` can change this location.
An operator or an agent with filesystem access can read these files and choose
to reconstruct a **new** agent. Boid does not automatically load or resurrect
an archived conversation.

- `archives/<session-id>.json` is the small manifest. `record` names a sibling
  `.record` file containing the complete saved session JSON; `sha256` verifies
  that file. `metadata` identifies the session and transcript entry count.
- `archives/<session-id>-<generation>.stream` retains the stream journal when
  present. The manifest's `stream` names it. During manual reconstruction,
  apply complete newline-terminated JSON deltas with `seq` greater than the
  saved `stream_seq`, in order; append each delta's `text` to its matching
  transcript `entry`. Ignore only an unfinished final line after a crash.
- The record's `view.entries` preserves the transcript, while `engine` contains
  saved model state. Referenced `skill-snapshots/` and existing context archives
  under `sessions/compactions/` and `sessions/clears/` are retained too.
- `archived-receipts/<sender-session-id>.json` contains compact send receipts
  and message-text hashes, not message bodies. This separate index prevents a
  retried send from being delivered twice. Normal runtime memory does not load
  all archived session metadata or receipt indexes. A live interrupted send
  recovers its one receipt; duplicate checks read only the needed sender index.

The archive path is storage, not an additional UI or model tool. The old archive
list route returns an empty list and the old reopen route rejects the request.
Normal history endpoints reject ended session IDs.

Archival copies and verifies the full record and journal, syncs them, publishes
an atomic durable manifest, then removes the old live copies. No archived trace
file is deleted. A crash before cleanup may leave both copies; startup repeats
archival safely. Finalized legacy records migrate before workers start. A short
status probe stops before history in canonical files, so active records are
parsed once; differently ordered legacy JSON may require skipping earlier
fields before reaching status. Archived transcript files are not scanned on
subsequent startup.

If storage fails, Boid retains any unsaved ended record in memory and preserves
the original files. It does not discard completed history merely to reduce RAM.
Such a record stays hidden from normal roster, diagnostics and peer reads.
Receipt-index updates and archive copying do disk work during End or one-time
legacy cleanup, outside the shared runtime mutex. The per-sender receipt lookup
may scan compact historical receipts; it never loads archived transcript text.
