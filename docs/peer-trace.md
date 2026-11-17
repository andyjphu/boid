# Reading peer traces

`read_peer_trace` returns a recent mixed view by default: ten entries, oldest
first, with at most 16 KiB of text. `limit` can select 1–100 entries. `kind`
filters by entry type. For the latest assistant text, use `kind:"assistant"`
and `limit:1`; the text can still be streaming and is not necessarily a final
conclusion. A mixed view retains calls, results and attributed peer reports.

Each ordinary entry contains `index`, `kind` and original `text`. The response
identifies the peer by stable ID, name and current status. Peer exchanges keep
sender/recipient attribution, urgency and delivery state; delivery or a queued
tool acknowledgement does not establish completed work. Images remain present.
Routine timestamps, sequence counters, byte totals, completeness flags and
null continuation fields are omitted.

Use `next_before` to read older pages. For an exact citation, pass its index as
a string in `entry_id`, for example `entry_id:"24574"`. Legacy entry UUIDs still
work. Indexes refer to the peer's retained trace and do not shift on append.
A superseded or unavailable entry produces an error rather than a replacement.

Only truncated entries contain `truncated:true` and their starting byte
`offset`. To recover an entire long entry, read its `entry_id` at `offset:0`,
then follow `next_offset` while present. The last chunk may still say truncated
because it is only part of the original entry; absence of `next_offset` means
there is no following chunk. Offsets are UTF-8 bytes, not characters. The text
limit excludes JSON metadata and escaping.

For the matching result of a tool call, pass its `entry_id` and
`related:"result"`. Use `related:"call"` to look backward from a result.
Matching uses the recorded tool name and call ID, never proximity or an
unrelated peer reply. Related lookup overrides `kind`; pagination via `before`
cannot be combined with `entry_id`. An empty `entries` array with
`related_not_found` means no counterpart was found, not proof of failure.
This scan happens only when explicitly requested.

Reads use live memory and neither wake the peer nor mutate its trace. Snapshot
freshness ends at the read. Hidden reasoning and private model context remain
excluded. Repeated reads still add ordinary tool results to the reader's context;
use a narrow filter or exact retrieval when that is enough.

This format follows an operator-supplied interview: an agent used Swift's mixed
trace to correct ownership records and distinguish Angler's interruption report
from Kestrel's pilot outcome. Indexes, attribution and pagination were useful;
routine byte and snapshot metadata were not used in those examples. That is
evidence for this workflow, not a claim that no other consumer ever needs them.
