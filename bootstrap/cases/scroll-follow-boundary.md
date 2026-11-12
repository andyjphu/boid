# A scrolling bug at the boundary between following and reading

2026-11-09. Boid v1.1.0 to v1.1.4. Prepared by Agent Cataclysm, the agent that implemented and investigated these changes. Andy requested this case after the final repair and explanation. The account separates his feedback, observed behavior, and the author's interpretation.

Boid's transcript kept pulling a small upward scroll back toward the bottom. A large upward scroll worked. The decisive defect was a disagreement between two event handlers: the wheel handler treated upward movement as a request to read, while the scroll handler immediately reenabled automatic following because the viewport was still inside the space reserved for the composer.

That explanation came after several other repairs. The original performance problem was real, and earlier scrolling fixes addressed reproducible defects. Their checks did not cover the small gesture that continued to frustrate Andy.

## The accepted interaction and the first performance repair

Boid displays several agents at once. An agent's pane follows new output until the operator scrolls back to read. The composer appears on hover, focus, or a draft, with space below the transcript so it does not cover the last message. Earlier work had deliberately retained some of that space when the composer disappeared, allowing new output to fill it without a jump. The [whole-interaction case](whole-interaction.md) records that accepted behavior.

The investigation began with Andy's report:

> mm-west-2 appears to be lagging or atleast the web view is when there is a lot of agent to agent communcation try to discern why

A replay of the actual saved history contained 4,172 entries and 330 communications. One peer update took about 1.3 seconds in the browser. The update path scanned both agents' histories and rerendered unrelated tool results. Selection checks interleaved with DOM changes caused repeated style and layout work. One instrumented update triggered 393 layouts and 565 style recalculations. These are browser measurements from the isolated replay, not measurements of every live interaction. [Before-repair evidence](evidence/scroll-follow-boundary/peer-render-before.json).

Version 1.1.1 indexed peer entries and tool pairs, updated affected entries only, and batched rendering with a selection snapshot. With the full visible history loaded, changed peer messages reached the subsequent paint in 23–34 ms in the replay. That measurement includes frame waiting, whereas the earlier 1.3-second figure measured handler time; it establishes that the long stall was removed in this fixture, not a uniform benchmark ratio. [After-repair evidence](evidence/scroll-follow-boundary/peer-render-after.json).

Andy also authorized temporary culling:

> no like add culling now if its easy, but this is a hotfix, culling should not be the final solution, we'd address that fs in v2

The initial view was bounded to 200 renderable entries or approximately 100,000 characters, with whole entries, tool pairs, and access to older history preserved. This reduced initial rendering work, but introduced another interaction with retained scroll space.

## The first scrolling complaint exposed two defects

After the hotfix, Andy reported:

> currently experiencing scrolling rubberband jamming and strange behavior when scrolling while agents are working, thoughts?

He narrowed the location:

> occurs when scrolling past bottom

The existing browser checks covered selection during streaming, programmatic scroll anchors, expanded tool output, and links to older messages. They did not exercise repeated wheel gestures against the bottom boundary.

Actual wheel events exposed a mismatch between the physical scroll range and the automatic target. In this fixture the composer was 37 pixels high, and the reserved space included another 8 pixels of padding:

```text
End of messages                 +0 px
Automatic composer clearance   +37 px
Physical end of scroll range   +45 px
```

The operator could scroll into the last 8 pixels, but restoration pulled the viewport back. The new bottom-scroll handler also scheduled rendering, which restored the position again. Five downward wheel gestures caused 20 programmatic scroll writes in the idle fixture. Correcting the padding accounting and removing unnecessary scroll-triggered rendering eliminated those writes in the corresponding check. [Before](evidence/scroll-follow-boundary/bottom-wheel-before.json), [after](evidence/scroll-follow-boundary/bottom-wheel-after.json).

A separate culling defect affected retained composer space. Its saved endpoint used absolute document coordinates. Removing a tall old entry reduced the document height without moving that endpoint. In the reproduction, the end of the message content moved from 8,276 to 5,021 pixels, while the viewport remained at 8,316. That left 3,295 pixels below the content. Adjusting the retained endpoint by the removed height reduced the gap to the intended 40 pixels. [Before](evidence/scroll-follow-boundary/retained-space-before.json), [after](evidence/scroll-follow-boundary/retained-space-after.json).

Both corrections shipped in v1.1.2. Repeated downward wheel gestures were stable in a fresh live browser. Streaming behavior was checked separately in an isolated fixture. These results supported the two specific repairs, but did not establish that every scrolling path was correct.

## Removing culling did not settle the issue

Andy reported continued trouble and asked whether port 7734 was serving the changes. The running version and served JavaScript matched v1.1.2. An already-open tab could still retain older JavaScript after reconnecting, so a reload was advised. The investigation did not establish that stale JavaScript caused Andy's report.

He then directed:

> remove culling for now, hotfix

Version 1.1.3 removed the limits and history-window controls, restored full transcript rendering, and retained the peer-update and padding fixes. Virtualization remained deferred to v2.

The next description isolated the gesture that earlier checks had missed:

> i scroll to bottom, and then i scroll up a little bit, and then it starts jamming again but if i scroll up a lot it can skip the jamming phase

Earlier checks had included a large upward movement of 250 pixels. That gesture crossed the entire composer region and missed the failure.

## Why small gestures became stuck

The wheel handler cleared `follow` for a negative delta. The subsequent scroll handler recalculated it using `messageBottom`, the end of the message content. A small upward gesture could still leave the viewport below that point, inside the composer space. The scroll handler therefore set `follow` back to true. The next streamed update restored the minimum composer clearance.

In an isolated reproduction with token updates every 40 ms, the message bottom was 46,020 and the physical scroll end was 46,065. Repeated 4-pixel upward gestures became stuck at 46,057, exactly 37 pixels below the message bottom. A 150-pixel gesture crossed above the message bottom and left following off. This reproduced Andy's distinction between a small gesture that jammed and a large one that escaped. [Before-repair trace](evidence/scroll-follow-boundary/small-up-before.json).

The repair distinguished continuing to follow output from resuming after reading:

| Current mode | Event | Result |
| --- | --- | --- |
| Following | Upward wheel gesture or upward navigation key | Leave follow mode immediately. |
| Reading | Further small upward movement or streamed output | Keep following off and preserve the reading position. |
| Reading | Downward movement reaches the physical end | Resume following. |
| Either | Programmatic scroll restoration | Track the resulting position without treating it as a new user gesture. |

The implementation also skips no-op scroll writes. An initial draft inferred user intent from every upward position change, but a layout shrink could also move the viewport upward. The reproduction exposed that unwanted loss of following before the draft was committed. The shipped change uses explicit upward input to leave follow mode and tracks programmatic movement separately.

After the repair, successive 4-pixel gestures moved the viewport from 46,064 to 46,060, 46,056, 46,052, 46,048, and 46,044, with following off throughout. Scrolling down to the end restored following. [After-repair trace](evidence/scroll-follow-boundary/small-up-after.json).

Version 1.1.4 shipped with checks for 1-, 4-, and 10-pixel upward gestures during streaming, return-to-end resumption, and following subsequent output. A fresh browser against the live service also verified those small gestures and resumption, with no browser errors. The live agents were idle during that check; concurrent streaming was verified in the isolated fixture. Culling remained disabled.

Andy responded:

> nice great job! what caused issue?

After the explanation, he requested this case study. His praise is evidence of acceptance at that point. It does not establish long-term reliability or coverage of every browser and input device.

## Interpretation

The common cause across the scrolling defects was an incomplete account of how geometry changes and user intent interact. The composer creates a second bottom position. Retained space, automatic following, live rendering, and culling each changed some part of that geometry. A position test that was useful while already following was insufficient to decide whether an operator who had just scrolled upward wanted to resume following.

The investigation preserved a useful earlier result: the peer-rendering slowdown was measured and repaired. Later scrolling complaints did not invalidate those timings. But preserving that result did not justify treating the user's experience as resolved. The validation needed to follow the new evidence into a different interaction.

The missing distinction in the checks was gesture size near a state boundary. A large upward scroll exercised reading mode after it had already escaped the problematic region. Small deltas exposed the transition between modes. The final checks preserved that distinction and also verified the return path, so preventing the snapback did not silently disable automatic following forever.

These are interpretations of this repair sequence. They support examining state transitions with representative gestures when a report points to a boundary. They do not establish a rule to test every pixel, ban culling, or add a broad UI test framework. Full transcript virtualization remains a separate v2 design task.

## Sources and continuity

The [evidence directory](evidence/scroll-follow-boundary/README.md) contains selected verbatim conversation excerpts, numeric browser outputs, and a hash manifest. The author directly performed the checks and inspected the code; this case has not received independent review. Private transcripts, credentials, and unrelated agent work are excluded.

The implementation history remains available at the exact commits:

- [v1.1.1 rendering and temporary culling](https://github.com/andyjphu/boid/commit/f4b5fe8).
- [v1.1.2 padding and retained-space repair](https://github.com/andyjphu/boid/commit/b9ce04b).
- [v1.1.3 culling removal](https://github.com/andyjphu/boid/commit/3efdba7).
- [v1.1.4 follow-mode repair and gesture checks](https://github.com/andyjphu/boid/commit/1be9212).

The final browser regression is [tests/transcript-browser.cjs](../../tests/transcript-browser.cjs). The related [whole-interaction case](whole-interaction.md) explains why preserving the earlier composer behavior mattered. This case records a task outcome and its limits; it does not close the separately retained review of the missed verification paths.
