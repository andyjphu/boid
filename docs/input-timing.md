# Input-path timing (review-only)

This opt-in diagnostic measures the user's message path from the browser submit handler through real server acceptance, visible transcript entry, worker consumption, and the first outbound model HTTP send. It does not measure provider inference or promise zero latency. Do not use a fake transcript entry to improve the number.

## Enable and inspect

Open Boid with `?input-timing=1` in the URL. Send ordinary messages while using the workspace normally. In that tab's browser console:

```js
boidInputTiming.samples()
await boidInputTiming.server()
```

The browser keeps at most 64 samples. Compare a browser sample's `messageId` and `sessionId` with the server record's `message_id` and `session_id`. Browser durations use `performance.now()`; server durations use its monotonic clock. Do not subtract timestamps across the two clocks or add overlapping intervals. A missing server record may have been evicted, cleared, or never admitted with tracing enabled. A missing send milestone may mean the input is still queued, the agent is stopped, a tool turn is underway, or the record was cleared; it is not automatically a provider stall.

The browser reports Enter-key handler to dispatch, acknowledgment, server-delivered entry and DOM completion, plus a separate browser event-queue estimate where the event timestamp is compatible. Programmatic submissions have no Enter timestamp. `submitTo*` values begin in the submit handler. The user entry may arrive by SSE before the HTTP acknowledgment; only actual server admission counts as acceptance. DOM completion is not physical pixel display. Note document visibility and whether the entry was in the viewport when comparing cases.

The server records accepted-to-worker-consumed and accepted-to-first-HTTP-send durations for traced input. A user turn can involve several inputs and tool/model cycles, so compare records by message ID and do not infer one input equals one provider request. The first send is measured at the client dispatch boundary; it does not include provider reasoning. Existing HTTP `Server-Timing` headers separately report lock wait, mutation, admission, and handler time for each message POST. These intervals overlap and must not be summed.

## Stop immediately

In the browser console, run:

```js
boidInputTiming.disable()
boidInputTiming.clear()
await fetch('/api/input-timing', { method: 'DELETE' })
```

`disable()` prevents new tracing headers from this tab. `clear()` drops the tab's local samples. The authenticated DELETE clears the server's volatile buffer; delayed worker marks must not recreate cleared records. Remove `?input-timing=1` from the URL before reloading, because that parameter enables browser timing again. Tracing is off by default, records are bounded and in memory only, and no prompt text, model output, tool arguments, or credential values belong in them. To remove the feature entirely, revert this PR and build/deploy at a separately authorized restart; no live restart is part of this PR.

## Review before any deployment

Run focused Rust timing tests and `tests/input-timing-browser.cjs` against the PR's exact source. Check opt-out POSTs have no trace header, opt-in ones have it, disabled tabs stop adding records, failed/queued/cancelled cases do not claim a send, and DELETE cannot be undone by delayed marks. Compare the same isolated mock with tracing off and on to estimate observer effect, especially with multiple concurrent agents and large histories. The browser's mocked HTTP/SSE and local provider mock are functional checks, not production latency. A reviewer can inspect live ordinary-use measurements only after an authorized deployment that preserves active agents and state.
