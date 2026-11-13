# Usage diagnostics: sunset

Andy paused maintenance on 2026-11-13 because the reported accounting is not
currently reliable. The Diagnostics toolbar button is hidden. Keep the existing
code and saved data for a future explicit revisit; do not invest in repairs,
features, rate-table refreshes or related test maintenance now.

This status covers the Diagnostics dialog, its browser rendering and polling,
the `/api/diagnostics` endpoint and runtime aggregation, `src/usage.rs` token,
cache and cost calculations, its rate table, and associated diagnostic tests.
These components remain in place. Existing reports do not establish accurate
usage or costs.

The separate [input latency instrumentation](performance.md) continues to support
the active speed mandate.
