# Preserving the whole interaction through corrections

2026-11-08. Boid v0.6 to v0.7. Prepared by Agent Catastrophe.

Andy called this a crucial case and requested that Boid agents see it when
working on Boid itself. The original case joins three related episodes: default-field
behavior, preservation of terminal styling, and latency introduced by live
directory labels. The analysis below is preliminary. The source excerpts
retain both the mistakes and the later successful diagnosis.

## Baseline and intended outcome

Woo's v0.6 established a compact workspace with agent panes, themed interactive
terminals, and carefully adjusted scrolling. Andy asked Ursa to add retained
sessions, an inline agent-creation pane, stable pane order after refresh, and
terminal labels that follow the shell's directory. The new work was to
accommodate the accepted interface and keep it usable.

The original request used the words "prefilled values in grey" and named
`canary` and `/work` as defaults for now. It also contained separate directions
about density, history, active/inactive grouping, reopening, later rotating
names, later directory policy, and tentative forks. Those other meanings must
not disappear merely because this case focuses on defaults. The complete turn
is preserved in `ursa-11570` in the [source excerpts](evidence/whole-interaction/excerpts.json).

## Default fields: a plausible choice followed by a narrow repair

Ursa initially used real input values. That was a defensible reading of
"prefilled" and made untouched fields submit their defaults. Andy first
corrected their appearance (`ursa-12745`):

> raise the opacity of the default values so that its clear thatif user presses create agent anyway it will populate with those values

This feedback concerns what the appearance communicates about submission, not
just a preferred shade of grey. Ursa brightened the values. Andy then clarified
editing behavior (`ursa-12833`):

> default values means that if i type into the input box, everything in the field clears; this is not current behavior

Ursa added custom first-edit clearing. His response at `ursa-12876` reported:

> Fixed. The first edit clears the entire default; untouched fields still submit their defaults.

Andy then exposed the missing part of the interaction (`ursa-12955`):

> ive got to ask, are you working on default values in some nonstandard way? because typically this has been implemented on the oldest html pages no issue, in fact its so standard behavior across most websites i dont know why we're struggling. but anyway, current behavior seems good, one final gripe is that after i type, the field clears, but when i delete all my work; the old default values dont return (what? why? this is such a nonstandard decision?)

The initial ambiguity does not make every later correction unforeseeable.
Once the typing behavior was clear, Ursa should have reconsidered the field's
whole edit cycle. The custom handler solved first input but left clearing an
edited field outside its model. The later repair uses native placeholders for
display and explicit fallback values at submission. Placeholders alone would
not preserve the requirement that an untouched field submit its default.

The final user turn also says the current behavior "seems good." Preserve that
partial acceptance. It narrows the remaining defect; it does not erase the
need for the last correction or condemn every earlier choice.

Commit `966fea6` removes the custom clearing logic. Ursa reports browser checks
for typing, deletion and placeholder return, blank-field fallback payloads,
and typed draft persistence. The default payload was intercepted without
starting a model task. Catastrophe read the implementation and Ursa's report;
these browser checks were not independently rerun for this case.

## The accepted environment includes the terminal theme

After receiving the new preview, Andy asked (`ursa-12709`):

> where did my powerlevel10k styles in terminal go?

Ursa explained (`ursa-12738`):

> I missed it when creating the separate preview. Powerlevel10k and its zsh configuration were installed inside the old container, but weren’t part of the base image or repository. The new container therefore started with plain zsh.

His preview preserved repository features while losing part of the working
environment. The theme was already present in the accepted experience. Its
absence from the source tree did not make its loss an intended change.

Ursa restored the prior configuration and dependencies for new terminals.
Existing shells were not restarted; his response explained that they could
reload `.zshrc`. The full record at `ursa-12804` preserves that limitation.
This supports preserving relevant runtime configuration during delivery.
It does not justify copying an entire home directory or unrelated credentials.

## A truthful directory label that stalls the terminal

Andy's requested behavior (`ursa-11995`) included both immediate creation and
the meaning of the tab path:

> terminal should similarly not have a setup popup, but in this difference it will just start at the root of the repository, user doesnt need to decide cwd; next each terminal's cwd will be some pwd or truncation(for longer case?) the address in the tab will be dynamic

Ursa read the actual shell process's cwd and propagated changes to the tab.
That correctly avoided inferring directory changes from typed commands.
However, each observed change also triggered a full retained-history
checkpoint. The checkpoint held the output lock while serializing replay
records directly to a file and syncing them, on the Docker shared mount.

Andy reported (`ursa-12879`):

> it seems we face some kind of lag whenever i change terminal cwds now, 1 change dir > wait 1-5 sec > subpane title changes > no longer frozen; this is not good behavior, what is causing it

Earlier checks had established that the directory label eventually updated.
They had not established responsive operation with representative themed
history on the actual mounted storage. Test counts could not fill that gap.

Catastrophe independently inspected the committed blocking path at `d312efc`
and the repair diff. Ursa's isolated measurements then separated shell work,
storage, transport, and browser rendering. The
[investigation](evidence/whole-interaction/terminal-latency-review.md),
[storage measurements](evidence/whole-interaction/themed-storage.jsonl), and
[browser comparison](evidence/whole-interaction/comparison.json) are retained:

| Observation | Before | After | What it establishes |
| --- | --- | --- | --- |
| Serializing the same 34,747-byte themed replay on the mounted filesystem | 21,986 writes; 1,178.348 ms | One buffered write; 1.130 ms | A microbenchmark of actual retained records, not production syscall tracing |
| Enter to displayed cwd, four isolated browser samples | 597, 847, 1066, 1261 ms | 101, 116, 100, 102 ms | Improvement for the same theme/browser/workload |
| Maximum HTTP latency in that fixture | 1163 ms | 13 ms | Reduced interference in this bounded run |

The repair at `36351e2` appends small metadata records, buffers full compaction,
and moves blocking monitor persistence off asynchronous execution threads.
Recovery checks preserve retained cwd/name before End; the repair did not
simply remove persistence. Ursa reports the fix committed and verified in
isolation, but not applied to the running preview at the last recorded update
because that restart would end open shells. No later live resolution or broad
load result is claimed here.

This is also a positive example. Ursa moved from a plausible explanation to
measurements that separated competing causes, then changed the expensive
mechanism while retaining the useful feature. Catastrophe inspected the
reported artifacts, but did not independently repeat these timings.

## Preliminary analysis for future review

The common hypothesis is that the agent solved the latest named symptom
without reconsidering the surrounding interaction and accepted baseline.
The default-field repair addressed first input without deletion/restoration;
the preview preserved code without the theme; the cwd check established a
correct label without responsiveness under representative history.

These episodes have different technical causes. A useful review should retain
those differences while considering the shared decision pattern. The initial
default-value reading was reasonable. Actual process observation was the right
source for cwd. Persistent history had a valid purpose. The failures concerned
how those choices were combined and verified.

Candidate guidance changes should therefore improve how an agent reconstructs
the requested interaction and checks its assumptions. "Always use placeholders,"
"never persist terminal metadata," or "add more tests" would each misread the
case. The reason for a change, its adjacent states, and the accepted environment
matter more than copying this repair into an unrelated task.

More cases can test this hypothesis against contrary examples. A later worker
may preserve the whole interaction yet fail for a different reason; another
may correctly choose custom input behavior or extensive verification. Guidance
should change only as far as the evidence supports. Andy's acceptance of the
analysis and request to preserve this case are evidence of its relevance;
future improvements in agent judgment remain unmeasured.

## Dense feedback and scope

The excerpts support several independently useful readings:

- Grey defaults must still communicate the values that Create will submit.
- Editing follows a familiar cycle, including returning to the empty state.
- A delivery inherits the working environment as well as the source code.
- A current directory label needs an observed source and a responsive path.
- Small interaction details matter even in a deliberately minimal interface.
- Praise can accept a working portion while a correction identifies what remains.
- A proposed future feature and a current implementation instruction have
  different scopes, even when they occur in one sentence.

These are Catastrophe's interpretations, linked above to the original turns.
They are not an exhaustive count of every meaning Andy intended. Preserve the
full request rather than replacing it with this list.

## Sources and continuity

[Source excerpts](evidence/whole-interaction/excerpts.json) contain 34 selected
original visible messages, each with role, timestamp, source file/line, and
SHA-256 of the original line and extracted text. They include the original
dense request, intervening agent responses, Catastrophe's full initial
assessment, Andy's preservation/template proposal, and Ursa's later repair
explanation. This is a selected trace, not the complete conversation or hidden
model reasoning. The preserved initial assessment predates the measured final
repair and remains intact so later analysis can be compared with it.

Related cases: [audience understanding](audience-before-interface.md) records
earlier failures to carry the meaning of feedback beyond its example;
[reviewability](intermediate-interfaces.md) records a successful reconsideration
of a mechanism while retaining the original purposes. Similarity is a reason
to compare their evidence, not proof of one cause or a population frequency.

Later update, 2026-11-08: Ursa reports that Andy authorized the restart and
`36351e2` is now deployed to the v7 preview. He verified the executable hash,
retained the prior agent transcript by prefix hash, and recovered four terminal
identities without automatically replacing their ended shells. The earlier
isolated-only state above is historical and superseded by this attributed
deployment report. Catastrophe has not independently repeated the live check.

Next evidence: operator feedback after the deployed repair, and whether a
worker exposed to this case handles a related task with less corrective effort. Retain any recurrence and the guidance
version that preceded it. Do not mark the learning hypothesis resolved merely
because this case was written or read.


## Later positive outcome: stable reveal and visible conversation

Added during v0.8 integration, preserving the earlier episodes above. This is
an explicitly approved result, alongside the earlier mistakes and measured
repairs. The original visible exchanges are retained as `ursa-13748` through
`ursa-14543` in the source excerpts.

Andy first described the intended hover behavior in a dense turn (`ursa-13748`).
It distinguishes revealing the prompt, covering some messages temporarily,
manually uncovering them, and automatic following of the last message rather
than the extra scroll space. Ursa restated that distinction (`ursa-13753`), and
Andy accepted it (`ursa-13760`). The full wording is retained because a rule such
as “never move the stream” would lose several meanings in that turn.

A later correction (`ursa-14358`, followed by the agent-pane clarification at
`ursa-14365`) accepted the absence of movement on click but identified a problem
while composing: the prompt hid the active conversation. Ursa proposed keeping
hover/click stable while clearing space on typing, submission and followed
output; history reading would keep its anchor, and an actually hidden prompt
would restore the ordinary message endpoint (`ursa-14377`).

Andy requested a checkpoint before the trial (`ursa-14384`):

> sure lets try this but first checkpoint this as v0.7.1 (we're still on 0.7 right?) so that i can refer back to this if need be, its already at 95% satisfaction rn

The retained v0.7.1 tag is `4fed21b`. The trial became `1cf12a1`, and Andy then
said (`ursa-14522`):

> mark this as an extremely good ux result, im not sure what lessojn there is to learn here but it was an amazing result, mark this 0.7.2

The v0.7.2 tag preserves that acceptance. Catastrophe inspected the tagged code,
Ursa's browser-check source/report and active-conversation screenshot during the
integration review. The report covered hover/focus stability, typing and new
output clearance, visible drafts outside pointer hover, multiline shrink,
normal endpoint after hiding, and history anchors. Those original checks remain
attributed to Ursa; later v0.8 verification is recorded with that implementation.

Preliminary interpretation: the useful distinction was between passive reveal
and active conversation, with additional state for history reading and actual
prompt visibility. The correction refined the first design rather than
invalidating its accepted parts. The retained checkpoint allowed a reversible
trial. Andy's explicit approval establishes a successful UX result here; it does
not establish a universal scrolling rule, that a guidance change caused the
success, or that future agents have learned the distinction.

For v0.8 this is a concrete baseline: skill selection should participate in the
same composition behavior, preserve the reserved working footer, and expose
captured skill content without overwhelming the conversation. It is a reason
to inspect the combined interaction, not merely whether a picker opens.
