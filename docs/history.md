# Loading and reading history

The workspace opens with recent messages for each agent. Scrolling toward the
start fetches earlier messages; the earlier/newer buttons also request a page.
Latest messages returns to the current tail. Peer links load the target message
and surrounding history. Full saved transcripts remain available through the
existing full-record link and API.

The browser mounts the viewport with overscan and retains measured spacer
heights for loaded history outside it. Selected text stays mounted during
streaming. Automatic reconnect waits for selection release before replacing a
reading window; an explicit Latest or peer-link navigation clears that selection.
Expanded tool state survives remounting within the loaded window. Initial loading
and rendering do not require fetching the complete transcript.

Pages contain at most 80 entries and a 64 KiB text budget, excluding JSON and
communication metadata. Individual messages initially show at most 8 KiB of
UTF-8 text, with an explicit Load full message action. The complete text remains
saved. Overscan normally mounts at most 100 entries per pane; a selection or
focused entry can temporarily pin a larger range. The client retains pages that
the reader has loaded; Latest replaces that cache with the recent page.

The browser opts in with `/api/events?history=recent`. Full snapshots remain the
default for existing clients. `/api/sessions/{id}/history` returns a recent page;
`before` and `after` are exclusive range-boundary positions, and `around` is an
entry ID. `entry` explicitly fetches the full content of one entry. Pages carry
`history.start`, `end`, `total`, and `seq`. Positions are stable because context
clearing retains transcript entries. Consumers must not use a page's sequence to
advance the global event cursor: it is an entry watermark for reconciling HTTP
reads with live events. A new snapshot invalidates old in-flight history reads.

`/api/sessions?history=recent` and `/api/sessions/archived?history=recent` provide
bounded views for the agent picker. The existing endpoints without this option
and `/api/sessions/{id}` still return full records. No history is removed from
persistent storage, and model context is unchanged by view navigation.
