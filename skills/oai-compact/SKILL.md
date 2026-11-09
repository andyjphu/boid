---
name: oai-compact
description: Compact the current Boid agent's model context when the operator requests manual compaction. Keeps the visible transcript and uses OpenAI's compaction endpoint.
---

Call `oai_compact` with `{}` once. The harness runs compaction after the
current tool batch. Its queued receipt is not confirmation of completion;
report the eventual completion or failure notice accurately.

Compaction replaces the model's working context, preserves the visible
transcript, and archives the preceding context. Do not replace it with a
handwritten summary or prune the returned opaque items. Do not set up automatic
compaction or retry a failed operation unless the operator requests it.

This skill requires Boid's `oai_compact` tool. If unavailable, report that
the running harness needs the compaction feature. The operator can also send
`/oai-compact` directly in a Boid pane, without a model turn to select the tool.
