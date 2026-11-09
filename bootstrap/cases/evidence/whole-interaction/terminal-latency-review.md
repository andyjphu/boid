# Terminal cd pause investigation

Scope: isolated Docker only, baseline d312efc. Live boid-v7-ursa-preview and user shells were not controlled or restarted. Baseline executable retained at .ursa/latency-baseline-bin/boid. Root owns deployment decision and concurrent frontend changes.

Measured cause: cwd observation triggered a full replay checkpoint, while holding the PTY child handle and taking output/journal locks inside a Tokio worker. Journal::compact serialized JSON directly to File. Terminal bytes are JSON arrays, so serialization issued one tiny write for each number/delimiter. macOS Docker bind mounts made those writes expensive.

Actual isolated Powerlevel10k replay: 34,747 JSONL bytes, 221 records, 21,986 writes. Unbuffered bind write+sync took 1,178.348ms, including only 0.106ms after serialization for sync_data. Identical serialization on tmpfs took 4.992ms. Single buffered bind write+sync took 1.130ms. These are microbenchmark measurements of actual retained records, not exact production syscall tracing. Evidence: .ursa/latency-evidence/themed-storage.jsonl. Synthetic corroboration is separately labeled synthetic-storage.jsonl.

Independent browser baseline: shell markers/P10k ready within 50–78ms, cwd SSE/title update 597/847/1066/1261ms, HTTP max1163ms. Label updated immediately on receipt; no long tasks/RAF gaps and no repeated resize/control loop. Evidence: .ursa/latency-ui-evidence/cd-latency.json. The fixture recorded a cleanup-only 204 parsing error after measurements; terminal was ended successfully.

Patch: append and sync only Meta records for cwd/name changes; serialize full replay compaction into memory then one file write; move monitor persistence to spawn_blocking. Metadata saves no longer take the PTY output lock. Existing durability and recovery format remain unchanged. Metadata recovery test copies the journal before End and verifies retained cwd/name without relying on End's full checkpoint.

Checks: both terminal recovery tests and binary build pass, .ursa/terminal-latency-build.log. Fixed themed-browser rerun: enter to actual cwd label 101/116/100/102ms, HTTP max13ms, SSE to label0–1ms, no longtasks/RAF gaps/page errors. Same theme/browser/workload, no frontend changes, clean fixture exit. Evidence: .ursa/latency-ui-fixed-evidence/cd-latency.json. No claim of live deployment or a broad load benchmark.
