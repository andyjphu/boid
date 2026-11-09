# v0.8 local skills integration

Agent Catastrophe, 2026-11-08. Implemented on `v8-skills-integration`, starting
from current main `eb35dfde6102bd3082793d7f1e0c1353b34504e4`, the PR7 merge.
Main was fetched again after implementation and remained at that commit.
The earlier `v8-skills` branch and accepted v0.7.1/v0.7.2 tags remain intact.
This is a local implementation, not a deployed release.

## Result

Humanizer is bundled for new sessions. Local packages have discovery, source
identities, explicit selection, policy/configuration, full-text loading and
operator-message-boundary refresh. `/skills` inserts an unambiguous source token
into the draft. The [module and usage guide](../src/skills/README.md) describes
paths, limits, captured content, live supporting files and intentional departures
from Codex. The full plugin/cloud ecosystem remains distant backlog.

The picker preserves v0.7.2's stream/overlay structure, prompt clearance,
reserved activity footer and elapsed timer. Closing a picker or pane invalidates
pending catalog responses. Selection shows the composer before triggering input
layout. Escape closes the picker without interrupting execution.

Explicit loads and catalog updates have compact disclosures containing the exact
saved text. Errors and discovery warnings remain visible. These are records of
loading and availability, not claims that the model followed a skill. The full
model-visible text remains in conversation history.

Discovery runs outside the Hub lock on a blocking worker. Each session owns at
most one scan; controls cancel its result and remain available while it runs.
Checks between filesystem operations stop further work. An already-started read
can finish later, but a cancelled result cannot update the session.

The larger skill bank measurably increased repeated checkpoint cost. Available
skill bodies now live in immutable, hash-verified snapshots, committed before
the referring session checkpoint. Existing snapshots are retained. Version 1
records, including the earlier inline skill draft, remain readable. New
skill-aware records use version 2 so older binaries reject them explicitly.
Back up the whole state directory; binary rollback needs its corresponding
pre-upgrade state. Missing or corrupt snapshot files produce errors while
retaining the original files. Loaded bodies still contribute to history size.

The case bank now includes the positive v0.7.1 to v0.7.2 refinement, exact user
approval, and the original dense request. All 34 selected visible exchanges in
the whole-interaction evidence match their original trace lines and text hashes.
The line hashes include the final newline. The interpretation remains preliminary;
this work does not establish that case exposure improves later agents.

## Ursa consultation and retained behavior

Ursa supplied integration advice and existing browser fixtures. The integration
retains main's buffered/atomic session writes, End after storage failure,
`persisted:false` warning from HTTP and SSE even after pane removal, and terminal
partial-write repair. `src/terminal.rs` is unchanged from main. The picker uses
the existing activity and prompt-clearance functions. Scan cancellation follows
Ursa's warning that `spawn_blocking` alone does not make discovery interruptible.

## Verification

All compilation, runtime checks and browser execution used isolated Docker work
on mm-west-2. No real provider calls were made; tests used a local mock provider
or prevented submission. The test server had its own state and no published port.
The disposable container was stopped and removed after verification.

| Check | Observed result |
| --- | --- |
| Rust formatting, locked offline tests and build | 29 tests passed: 11 library and 18 binary. [Final log](evidence/v08-skills/final-build.log). |
| Skill/runtime integration | Explicit and tool loads, duplicate sources, policy/configuration, supporting-file read, stable continuation, next-message refresh, legacy deserialization, durable recovery, hash/path validation, missing/corrupt snapshots and original-file retention passed. |
| Blocked discovery | Stop, Escape and End remain available before the scan is released; zero provider requests and no owner for a late result. |
| Existing session UI fixture | All 18 checks passed, including real isolated terminal cwd updates, ordering/reconnect, defaults, retained sessions, scrollbar, overlay clearance and reserved footer. [Report](evidence/v08-skills/session-ui-check.json). |
| Combined skills UI | Six checks passed: real catalog/source policy, selection clearance and timer, exact disclosure and visible failures, narrow layout/Escape, history anchor, stale response and removed-pane handling. Zero page errors. [Report](evidence/v08-skills/skills-integration.json). |
| Unsaved End | HTTP warning, SSE warning after pane removal, and absence of a warning on normal close passed using Ursa's fixture with injected responses. Backend storage failures also remain covered by the Rust suite. |
| Real server restart | Three earlier inline-skill records recovered into hash-verified snapshots with identical skill bodies, history, visible entries and identities. [Report](evidence/v08-skills/recovery-check.json). |
| Visual inspection | Inspected the actual [380 px skills view](evidence/v08-skills/skills-narrow.png) and [mixed terminal/agent layout](evidence/v08-skills/session-ui-mixed-layout.png). |

The skills browser fixture uses the real HTTP catalog and renderer, with synthetic
conversation/skill events. It does not show live model compliance. Its initial
history-anchor assertion ran before resize/clearance settled; the fixture was
corrected to settle layout and use actual wheel scrolling. Product code was not
changed to satisfy that failed fixture assumption. The first excerpt verifier
omitted the source line's newline; verification with the recorded convention
matched every original line. Neither mismatch was silently counted as a pass.

## Snapshot cost observations

The [before](evidence/v08-skills/latency-before.json) and
[after](evidence/v08-skills/latency-after.json) probes used the same Docker bind
mount and debug configuration, two Tokio workers, and a gated mock provider.
There is one observation per condition per representation, not a percentile or
latency guarantee. Each retained synthetic user turn adds 1024 padding characters
to both engine history and visible entries. The empty condition clears skills
and disables refresh in the feature binary; it is not a separate main build.

| Catalog | Retained turns | Enter acceptance, inline → snapshot (ms) | Escape to stopped, inline → snapshot (ms) |
| --- | ---: | ---: | ---: |
| Empty | 0 | 1.04 → 1.11 | 4.82 → 4.28 |
| Empty | 100 | 6.49 → 5.93 | 30.76 → 31.22 |
| Humanizer | 0 | 1.63 → 1.13 | 8.90 → 5.50 |
| Humanizer | 100 | 6.37 → 5.87 | 34.75 → 31.69 |
| Local bank + bundle | 0 | 13.41 → 1.49 | 74.10 → 8.00 |
| Local bank + bundle | 100 | 18.75 → 5.86 | 93.25 → 32.68 |

The bank contains 44 local SKILL.md copies plus the bundle: 496,874 raw local
bytes, 576,155 serialized snapshot bytes. Its scan took roughly 12–18 ms in
these observations, separately from Enter acceptance. The prepared zero-history
record fell from 605,930 to 29,787 serialized bytes. `record_bytes` is measured
before the first save assigns the final reference, so it is not the exact final
file size. The result supports removing repeated body serialization; it does
not remove history-dependent cost or prove performance on other storage.

The evidence directory retains the [bank manifest](evidence/v08-skills/bank-manifest.json)
and disposable [before](evidence/v08-skills/latency-probe-before.rs) /
[after](evidence/v08-skills/latency-probe-after.rs) measurement modules. They are
not compiled into the product. The bank copied only SKILL.md text from installed
packages, with canonical-path deduplication; no scripts or package assets ran.
The after probe precedes the final format-version marker and stale-reference
guard; the final functional checks cover those refinements.

Browser fixture sources are retained alongside their reports. They assume an
isolated disposable server, `/work` and `/evidence` mounts, Chromium and
`playwright-core`; the unsaved-End fixture also mounts web assets at `/source`.
The skill fixture uses a deliberately local dummy token. The session UI fixture
requires `BOID_TEST_URL` and `BOID_TEST_TOKEN`. These are targeted development
fixtures, not commands for the live preview.

## Remaining scope

No push, PR, merge, live preview update or deployment was performed. Generic and
meta-review templates remain proposals. Automatic review loops, context editing,
full Codex distribution, and the earlier unrelated truthfulness audit findings
remain open as recorded in the plan. Supporting files remain live, not captured
with SKILL.md. This implementation does not claim to resolve those separate
items or to establish Humanizer compliance from loading alone.
