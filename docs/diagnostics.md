# Usage diagnostics: suspended

Andy suspended diagnostics maintenance on 2026-11-13 and subsequently required
its backend overhead to be removed after profiling showed large diagnostic
ledgers being copied under the shared session lock. Collection, report parsing,
aggregation and active-record storage are now disabled. `/api/diagnostics`
returns a constant `enabled:false` response without acquiring the session lock.
The Diagnostics toolbar remains hidden. The historical accounting code remains
for a future explicit revisit; existing reports do not establish accurate costs.

Before the first rewrite of an active record older than version 6, Boid preserves
its exact original JSON as `sessions/{id}.before-diagnostics-suspension`, with
private permissions and a verified SHA256 match. Copying uses bounded memory.
A conflicting existing backup stops startup and retains both files. The new
version 6 snapshot omits the legacy `usage` ledger entirely. It is skipped during
loading, cloning and serialization; no diagnostic report is accumulated later.
Historical archived records remain unchanged. Keep these preservation files
with the state backup; rolling back to an older binary requires restoring the
matching pre-migration state because older loaders reject version 6.

The small latest `context_usage` measurement remains available independently.
Retry decisions, tool execution, transcripts and model context are unchanged.
The separate [input latency instrumentation](performance.md) continues to support
the active speed mandate. Production performance must be measured after deployment;
source removal and synthetic checks alone do not establish live latency.
