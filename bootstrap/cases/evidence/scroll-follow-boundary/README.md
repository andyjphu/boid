# Scroll-follow case evidence

Collected by Agent Cataclysm during the 2026-11-09 repair. These are selected
numeric outputs and visible conversation excerpts. No raw agent transcripts,
credentials, or private reasoning are included.

| Files | What was measured | Limits |
| --- | --- | --- |
| `peer-render-before.json`, `peer-render-after.json` | Local Chromium replay of captured history, before and after targeted rendering; the latter includes a full-visible-history phase | Original history is private and is not packaged. Baseline handler time and subsequent-paint time have different timing boundaries. |
| `bottom-wheel-before.json`, `bottom-wheel-after.json` | Repeated actual wheel gestures at the bottom, idle and during synthetic streaming | Synthetic 600-entry fixture in local Chromium; not every input device or browser. |
| `retained-space-before.json`, `retained-space-after.json` | A tall first entry removed from a 200-entry window while composer space was retained | The relevant result is `retainedCase`. Earlier samples repeat the wheel probe and are retained as collected. |
| `small-up-before.json`, `small-up-after.json` | Small upward wheel gestures during 40 ms token updates, a larger escape gesture, and return to bottom | Full-history rendering. This isolates the follow-mode boundary after culling was removed. |
| `excerpts.json` | Selected verbatim user and assistant excerpts, in order, from the original Codex conversation | Assistant excerpts are portions of longer messages. The file identifies the source thread; it is not a complete transcript. |

`manifest.json` records SHA-256 hashes for the packaged files. JSON numbers and
field names are unchanged from the original measurement files. Initial position
values can differ between fixtures; compare before/after conditions within the
same mechanism rather than combining absolute coordinates across fixtures.

The synthetic browser regression is maintained in
[tests/transcript-browser.cjs](../../../../tests/transcript-browser.cjs).
The original ad hoc instrumentation was used for diagnosis; these outputs are
not a benchmark suite or a claim of long-term performance.
