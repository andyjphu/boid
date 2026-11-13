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

Bounded asynchronous streaming and consecutive journal batches avoid waiting for
one disk sync per chunk. Preserve FIFO ordering, bounded admission/backpressure,
error propagation, and durable completed-response, tool, peer, stop, and shutdown
barriers. Do not move waits into the global mutex or grow an unbounded queue.
Input dispatch must not wait for a full session save: server memory acceptance
is real, then ordered saving proceeds independently. The accepted crash tradeoff
allows a small recent input window and unfinished stream tail to be lost; it
does not authorize losing completed durable work or replaying tool side effects.

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
blocked writer, unchanged payloads, FIFO replay, failed checkpoints preventing
effects, and shutdown draining. Run the relevant suite once; broaden after new
failures or changes. Validate the exact built binary and embedded assets in an
isolated server without provider calls. Only post-deployment measurements of
actual use can establish the live improvement. Retain unresolved limits and
report any feature tradeoff immediately instead of hiding it behind a speed score.
