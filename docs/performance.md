# Performance investigation

Method revision 1, 2026-11-12. Timing instrumentation entered in v1.1.26;
this guide records the investigation and repairs assembled for v1.1.28.

Speed is a core Boid mandate. Eliminate proven avoidable harness overhead
between inference and tools, including input admission, cancellation, rendering,
and persistence. Immediately report a legitimate feature or correctness tradeoff
when it obstructs that goal. Preserve the useful behavior and resolve the choice
within existing authority; this mandate creates no additional approval gates.
A fast acknowledgment must represent actual server acceptance. Do not disguise
latency with an optimistic message or pending transcript before server receipt.

## Measure the boundary that is slow

Record the source revision, binary version, workload, and exact interval.
A fast health endpoint does not establish fast input or rendering. Distinguish
browser dispatch, server admission, storage, provider latency, and DOM work.
Use ordinary user messages to measure live input. A synthetic fixture, rejected
empty message, or isolated server smoke test cannot establish production speed.
Do not send live agent messages or invoke providers merely to profile the server.

In the browser console, enable `boidInputTiming.enable()`, then inspect
`boidInputTiming.samples()` after ordinary use. `clear()` removes samples;
`disable()` stops recording. Recording is off by default and retains at most 64
samples in memory without message contents. IDs correlate server acknowledgment
and server entry events. Durations use monotonic clocks within each process;
never subtract browser and server timestamps across hosts.

Browser `submitToDispatchMs` captures local dispatch delay; submit-to-headers,
acknowledgment, entry, composer-clear, and DOM intervals overlap. DOM completion
is not physical pixel display. Check viewport/document visibility in the sample.
Server-Timing values are milliseconds and cover these nested intervals:

| Metric | Scope |
| --- | --- |
| `boid_lock` | Waiting for the shared session-state mutex |
| `boid_mutate` | Work holding that mutex, including copying, enqueueing, events and notification |
| `boid_admit` | Runtime admission entry through mutex release and synchronous wrapper return |
| `boid_handler` | Handler entry after Axum extraction through response-body construction |

Admission does not wait for background input saving. Handler timing excludes
authentication, extraction, final header formatting, socket flush and client
receipt. Neither server interval includes waiting in the browser dispatch queue.
Do not add overlapping intervals. Failures report only stages actually reached;
authentication/extraction failures never enter this instrumented handler.

## Safe host measurements

On the assigned host, compare a few authenticated reads without printing tokens
or response contents. This tested pattern uses the local token file; adjust the
port/state path to the verified deployment, not a guessed process name:

```sh
python3 - <<'PY'
import json, time, urllib.request
from pathlib import Path
opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
headers = {'Authorization': 'Bearer ' + (Path.home()/'.local/share/boid/token').read_text().strip()}
for _ in range(4):
    for endpoint in ('server', 'screens'):
        start = time.perf_counter()
        request = urllib.request.Request('http://127.0.0.1:7734/api/'+endpoint, headers=headers)
        with opener.open(request, timeout=5) as response:
            payload = response.read()
        print(endpoint, round((time.perf_counter()-start)*1000, 3), 'ms', len(payload), 'bytes')
        if endpoint == 'server':
            info = json.loads(payload)
            print('identity', info['pid'], info['version'])
PY
```

`/api/server` bypasses the session mutex; `/api/screens` acquires it. A slow
screens read alongside fast server reads supports shared-state contention.
A short probe with no stall does not rule out intermittent contention.
Verify the reported PID's executable and listening port with `ps` and `lsof`
before a bounded macOS profile. Example actually used during the investigation:

```sh
ps -p 51239 -o pid=,etime=,command=
lsof -nP -iTCP:7734 -sTCP:LISTEN
sample 51239 2 10 -file /tmp/ursa-boid-51239-sample.txt
```

51239 is historical, not a reusable target. Inspect the call hierarchy: inclusive
samples of a parent frame cannot all be attributed to its cloning child. Keep
the profile artifact, not just the interpretation. Profiles can expose paths;
retain them privately. Compare `vm_stat` and `sysctl vm.swapusage` before/after
the same bounded interval. Convert page counters using the reported page size.
Allocated swap, cumulative swap counters, high RSS, or free disk space alone do
not establish current thrashing. Avoid loading multiple huge saved sessions to
investigate memory pressure; use bounded metadata reads or synthetic fixtures.

## Repairs and the boundaries they preserve

Baseline source was `27f6032` (v1.1.26). Its profile showed deep copies under
the shared mutex, receipt waits,
and a persistence worker waiting for that mutex. These were distinct costs.
The v1.1.28 work removes empty-inbox/unchanged-status snapshots, indexes peer
comparisons while preserving full equality, shares immutable model history and
transcript text, and serializes borrowed provider input. Sharing must preserve
the same stored and provider JSON values; stored text remains byte-for-byte
unchanged, while HTTP object-field order can differ. Mutable streamed text
needs copy-on-write.

The original streaming repair preserved blocking completed-response and tool
claim checkpoints. Andy explicitly superseded that execution contract on
2026-11-16: saving must proceed independently of model/tool transitions. See
v1.1.39 below. Keep real server acceptance, bounded recording memory, ordered
archives and graceful-shutdown draining. Do not restore disk waits to ordinary
execution under the name of durability or backpressure.

## Reproduce and verify proportionately

Build/test natively in an isolated worktree on mm-west-2. Coordinate concurrent
builds and use `CARGO_BUILD_JOBS=1`. Runtime tests belong to the binary target,
not `--lib`. Actual commands used for the no-op/indexing patch:

```sh
CARGO_BUILD_JOBS=1 ~/.cargo/bin/cargo test --release --bin boid
CARGO_BUILD_JOBS=1 ~/.cargo/bin/cargo test --release --bin boid bounded_runtime_call_benchmark -- --ignored --nocapture
```

That patch passed 94 tests with 5 ignored. Its 3-trial synthetic fixture used
8 MiB model history, 1,493 peer records and memory storage. Median milliseconds:
empty inbox 7.953 -> 0.001; unchanged status 6.783 -> 0.001; real status transition
6.666 -> 1.916. These exclude disk serialization and are not live input timings.
The streaming change also measured the actual Hub path with 128 synthetic
256-byte chunks and a final durability fence: burst median 614.883 -> 72.262 ms.
This is a controlled workload, not a live provider stream or user submission.

Separate 16 MiB history-sharing measurements reported two history copies/drop
2.099 -> 0.0071 ms; source-payload preservation tests passed. Neither fixture
proves all history-dependent costs are gone or predicts production latency.

Choose focused regressions for the changed boundary: no-op behavior with a
blocked writer, unchanged payloads, FIFO replay, uncertain-effect recovery after
recording failure, and shutdown draining. Run the relevant suite once; broaden after new
failures or changes. Validate the exact built binary and embedded assets in an
isolated server without provider calls. Only post-deployment measurements of
actual use can establish the live improvement. Retain unresolved limits and
report any feature tradeoff immediately instead of hiding it behind a speed score.

## Incremental session persistence (v1.1.36)

Session checkpoints use an ordered event journal. A checkpoint stores current
metadata, a model-history suffix, and changed/new transcript entries. Unchanged
model items and transcript text are not serialized again. Streaming text and
checkpoint events share a persistent journal sequence; the stream sequence
remains separate. In v1.1.36, tool claims and completed-work barriers still
waited for the small journal commit. The v1.1.39 contract below supersedes those
waits. Input acceptance remains asynchronous and real.

A single background compactor writes full snapshots. The FIFO writer rotates
its active journal into a closed segment before handing the compactor an
immutable record. New events go to a new active segment. Only closed segments
covered by a successfully published durable snapshot can be pruned. Snapshot publication must preserve journal rotation ordering. Archival and
shutdown must account for the compactor before moving or releasing state.

Version 7 session state consists of the base `<id>.json` plus
`<id>.journal/*.jsonl`. Reading the base JSON alone does not establish the latest
state. Preserve both when making crash-recovery fixtures or backups. Replay
closed segments in numeric order, then the active segment; skip sequences
already covered by the base and require consecutive new events. Startup
preserves any incomplete active tail before truncating it to the last complete
record. Closed-segment corruption is an error. Do not run the repairing replay
function against a live directory as a read-only inspection tool.

The ignored `incremental_disk_checkpoint_benchmark` uses temporary native disk
storage with 4 MiB of synthetic model history, 8 MiB of transcript, and 16
checkpoint/entry pairs. It invokes actual Hub persistence boundaries without
provider calls. Compare the same fixture before/after; these durations are not
browser input or model-response timings. Keep the raw measurements with the
release evidence and state when the workload differs from live use.

On mm-west-2, the 16-pair native fixture measured median checkpoint time
27.516 -> 3.995 ms and transcript-entry commit time 27.933 -> 3.991 ms
(v1.1.35 -> v1.1.36 candidate). Initial post-change iterations overlapped the
seed snapshot compaction and reached about 9 ms. These are synthetic durable
commit measurements, not a claim about inference latency or every live session.

## Nonblocking execution (v1.1.39)

Ordinary execution updates actual server state, queues recording, and proceeds.
Model-response checkpoints, tool-call claims and results, peer delivery, input
consumption and stream updates do not wait for disk receipts or queue capacity.
The recorder coalesces queued ordinary updates into the latest complete state
when needed; it does not discard transcript content or accumulate an unbounded
queue of full records. Context/clear archives retain their ordering and content.
End and graceful shutdown drain pending recording and compaction.

A crash can lose unsaved input, response, tool-result and receipt state. Because a
tool may already have acted after the last saved checkpoint, unclean recovery
must not automatically replay old calls or queued requests. Keep their recorded
text and identify uncertain outcomes. Fresh user input remains usable. A durable
startup marker distinguishes unclean shutdown; only successful graceful shutdown
clears it. Version 8 prevents older binaries from interpreting this state under
the former durability assumptions.

Measure the actual handoffs: completed response observed by the API client to
tool dispatch, and tool return to the next HTTP send. The latter includes request
preparation and serialization; report encoding separately. A local mock endpoint
excludes provider inference but is not a live production latency result. Per-client
clock hooks exist only in test builds. Keep a writer deliberately stalled during
repeated complete cycles to prove execution does not depend on disk progress;
then release it and verify the recorded transcript and model history.

Native mm-west-2 comparison (three trials per size, same fixture):

| Synthetic workload | Completed response → tool dispatch | Tool return → next HTTP send |
| --- | --- | --- |
| Minimal history, v1.1.38 | 15.013 ms | 8.411 ms |
| Minimal history, v1.1.39 | 0.302 ms | 0.273 ms |
| 4 MiB model history + 4,096 transcript entries, v1.1.38 | 11.918 ms | 9.222 ms |
| Same large fixture, v1.1.39 | 1.468 ms | 2.151 ms |

The large fixture includes 8 MiB of transcript text. Its next-request encoding
alone took 1.273 ms in the new implementation. With the writer held for a
500 ms observation window, the baseline made no handoff progress; the new
implementation reached the next request before writer release. A separate
regression completes eight actual model/tool cycles under one held writer,
exceeding the former queue capacity. These are native local mock-provider
measurements, not production response-time or worst-case guarantees.


## Message dispatch and shared state (v1.1.40)

Skill discovery is preloaded once per canonical working directory. A background
watcher checks filesystem metadata every 500 ms and rescans when it changes.
Each operator-input boundary uses the latest completed snapshot without waiting
for filesystem work. Tool continuations retain their existing snapshot. Edits,
renames, deletions, and configuration changes become available after detection
and scan completion; 500 ms is the polling interval, not a freshness guarantee.
Invalid configuration publishes the same disabled catalog and warnings as before.
Sessions in the same directory share the watcher; the last lease or server
shutdown cancels it. Cold initialization still waits for its initial scan.

Skill maps and peer-message text share immutable allocations. Copy-on-write
preserves prior snapshots, and stored JSON remains unchanged. History and SSE
snapshots capture entries, sequence and receiver together under the session
mutex, then preview and serialize outside it. This reduces contention without
replacing the atomic admission and peer-ordering boundary with per-agent locks.
Remaining lock work includes record metadata, entry projections and event
publication; do not claim the shared mutex or all harness latency is gone.

The ignored `actual_worker_message_dispatch_benchmark` measures admission through
actual Hub workers and loopback HTTP, with 256 fixture skill files, one or eight
concurrent agents, the production two-thread Tokio scheduler, and zero or 4 MiB
model history per agent. It reports admission
lock wait/mutation, request encoding, HTTP send, and the first actual assistant
text delta observed through the Hub broadcast. A held background scan must not
prevent a distinct new message from reaching and receiving a mock response.
Run the same fixture on the baseline and candidate using the native commands
above. These timings include local scheduling and exclude browser rendering and
real provider inference. They complement the response-to-tool fixture, which
does not cover a newly submitted message's skill refresh.

Native matched comparison against v1.1.39 (three warm rounds; 24 observations
for each eight-agent case). Median message-admission entry to HTTP-send time:

| Agents | Model history per agent | v1.1.39 | v1.1.40 dispatch change |
| --- | --- | --- | --- |
| 1 | Minimal | 11.652 ms | 0.216 ms |
| 1 | 4 MiB | 13.710 ms | 2.874 ms |
| 8 | Minimal | 47.100 ms | 2.201 ms |
| 8 | 4 MiB | 56.701 ms | 13.451 ms |

Large cases also have 4,096 transcript entries per agent. The held-scan case
blocked progress on the baseline and allowed the proof message to complete on
the candidate. These results isolate the dispatch changes before adding `/fast`;
provider priority processing was never involved. Remaining request encoding,
shared-state work and scheduling are measurable, particularly with large
concurrent histories. This does not establish provider-only production latency.

## Parallel tool execution (v1.1.51)

Responses requests permit multiple tool calls. Within one response, Boid executes
contiguous groups of independent reads, searches, shell commands, skill reads,
roster reads and peer-trace reads with at most eight calls in flight per agent.
A finished call releases its slot immediately; a slow sibling does not stall
admission of the next independent call. There is no cross-agent execution lock.

Native file writes/edits, peer sends/wakes, questions, context exports/changes and
other tools form serial barriers. They wait for preceding calls and finish before
following calls start. Shell commands have arbitrary effects: issuing several in
one response declares them independent. The stable instructions tell agents to
put dependent work in a later response after reading results. Boid does not try
to infer arbitrary shell dependencies. Enabling parallel calls permits batching;
it does not force a model to emit more than one call.

Results enter the transcript and model history as they finish, linked by call ID.
One tool error or timeout is its own result and does not discard sibling results.
The next inference waits until the response's whole tool batch is resolved.
There is no disk-receipt wait on dispatch or completion.

Escape, emergency steering and shutdown drop the concurrent futures. Existing
process-group guards kill unfinished shell/search processes and their children;
deliberately detached processes retain the existing containment limitation.
Completed results stay recorded. Started blocking file reads can finish after
cancellation but cannot publish a late result or mutate a file. Unstarted calls
get a distinct cancellation result. Crash recovery records uncertain outstanding
outcomes and does not replay them. State format10 protects outstanding concurrent
call metadata from older binaries; rollback requires its matching state backup.

Verification uses loopback model responses and actual shell processes on
mm-west-2: mutually blocked starts prove overlap, the ninth/tenth calls prove the
cap and refill behavior, a queued write proves the barrier, and Escape verifies
process-group termination plus completed-result retention. These are harness
checks without external provider calls, not a measured end-to-end provider
speedup or proof that models always choose useful batches.


### Cache reuse with transient role context (1.1.53)

For the API `gpt-6-sol` route, two explicit content boundaries preserve the
latest stable history and the prior successful boundary, while implicit lookup
remains enabled. See [context blocks](context-blocks.md#stable-cache-boundaries-1153).
The previous boundary must survive ordinary journal checkpoints, which borrow
and restore engine history; clearing it there defeats cross-turn reuse.

A request-only tail is not append-only history: `H + role` followed by
`H + new messages + role` no longer contains the earlier cached ending. A
boundary before that tail makes stable history reusable without changing message
roles. The automatic live peer roster was also removed; agents use `list_agents`
on demand. This repair adds no blocking persistence or usage ledger. Cache
savings are unmeasured; Andy will measure provider-reported results.
