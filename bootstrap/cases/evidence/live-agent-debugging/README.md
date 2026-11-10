# Evidence for live-agent debugging

`excerpts.json` contains selected verbatim user messages from Codex thread
`01a1a450-8944-7370-93d7-c57a2a1ee5b6`, with source filename, line, and UTC
timestamp. The final praise arrived on November 10 UTC, November 9 Pacific.
The selection preserves praise and corrections; it is not a complete transcript.

The four numeric browser outputs compare baseline `94c67b6` with the local
Escape/rendering patch later committed as `bfd4d32`. Final guard refinements
were made after timing and covered by the final browser regression. Each run
used the same captured 19,576 entries across seven panes, local Chrome at
1440×960, mocked network routes, no terminals, and 20 streaming rounds.
The repeated-Escape scenario deliberately withheld the stopped status while
returning HTTP 202 promptly. Queued input-task timing used a timer beside
rendering; it is not native device-to-server interruption latency. The before
and after runs are small diagnostic samples, not confidence intervals.

`mountTranscript` and `groupUserEntries` have zero timed calls in the after
streaming sample. Their `max: null` comes from JSON serializing an empty
maximum, not a failed timing measurement. Initial mounting occurs before that
sample. Original production histories, tokens, and model requests are excluded.

`manifest.json` records SHA-256 hashes of the five evidence payloads.
The earlier scrolling measurements remain in
[scroll-follow-boundary](../scroll-follow-boundary/README.md).
