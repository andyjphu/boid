# Reviewability without disposable interfaces

Andy praised the response below and asked that this example be preserved for
developer calibration and possible future research. That is evidence of approval
of this reasoning. No v0.2 implementation outcome has been observed.

Related guidance: [cost and performance](../README.md#cost-and-performance),
[open choices](../README.md#open-choices), and
[conviction](../README.md#conviction).

## Trajectory summary

This section is a maintainer summary. The exchange below preserves the wording
of the messages separately.

Andy wanted a general agent swarm harness, with useful performance as the goal
and code, features, context, and maintenance counted as costs. The first release
could be small and nonfunctional; v1 should work well enough to change its own
code. Basic tools and permissions were the intended starting point. Codex was
an implementation reference, with departures allowed when useful.

The initial guidance PR was merged. Andy then removed the API-key detail from
the general README in commit `ec8fc26`. That correction concerned the audience
of the introduction; it did not change the implementation preference.

Asked what v0.2 should look like, the assistant proposed a read-only tool core
with glob, read, and search, a shared workspace boundary, bounded output, and
interruption. It illustrated the proposal with manual commands such as
`boid --root ./src read PRINCIPLES.md`. The stated aims were reviewability,
enforced access, and reuse by a later agent loop. Go remained a recommendation.

Andy questioned whether dividing work this way would create interfaces already
foreseeable as temporary. The assistant retained the useful objectives and
withdrew the separate tool-runner CLI as a required milestone. It distinguished
implementation order from release boundaries. Andy then praised the decision
and requested this record. No code was written during this discussion, and the
complete v0.2 scope and implementation language remained unsettled.

## Andy's concern, verbatim

> 1. we are meeting multiple concerns here, let me try to understand your concerns, because my concern is that if we attempt to break up work too much, we add intermediate work we can easily forsee will be overwritten later, like terminal flags

## Assistant response, verbatim

> I was combining three concerns:
>
> 1. **Reviewability:** the first code PR should be understandable enough for you to assess its direction.
> 2. **Permission enforcement:** tools should obey access limits in code, before we rely on them inside an agent.
> 3. **Reuse:** the file operations should become the actual agent tools.
>
> Those remain useful concerns. **They don’t require shipping a separate tool-runner CLI.** I added that interface to make the intermediate work demonstrable, without establishing that it would have a lasting purpose.
>
> A workspace option might survive in the final CLI. Dedicated `boid glob` and `boid read` commands are harder to justify: they introduce parsing, output conventions, documentation, and interface decisions that the eventual agent interaction might replace.
>
> I would therefore keep the internal tool functions and permission boundary, and drop the manual subcommands from the proposed milestone. We can exercise internal functions directly. They don’t need a temporary product interface to be reviewable.
>
> “Tools first” can describe implementation order. It needn’t mean a separate release for each layer. We should consider the intended agent interaction far enough ahead to choose durable interfaces, then divide the work where review is useful. If the first coherent change needs to include part of the agent loop, keeping the PR artificially smaller could increase total work.
>
> My concern is preserving your ability to judge consequential choices early. Your concern adds a necessary constraint: that early review should not make us build things we already expect to discard.
>
> You Asked: Explain the concerns behind splitting v0.2 and the risk of disposable intermediate work.
>
> Answer: Keep reviewable, reusable internals; remove the artificial tool-runner CLI milestone.

## Andy's assessment and recording request, verbatim

> this was an advanced decision, one which ive not seen you easily arrive at in similar cases, but please store this example in bootstrap/(you choose name) where we store examples of my taste verbatim by writing prev agent turn, and my decision, and a summary of trajectory heretofore; perhaps they can be useful references for my gripes with harnesses or useful for future research

## Interpretation and scope

Maintainer interpretation: preserve the purposes behind compatible constraints
and reconsider the mechanism chosen to serve them. Here, reviewability and low
implementation cost could coexist once the temporary product interface was
removed. The assistant explained the original concerns instead of abandoning
them when challenged.

This case is relevant when choosing implementation stages, review boundaries,
or scaffolding. It does not establish a ban on CLI flags, prototypes, or small
PRs. Temporary work can be worthwhile when it resolves enough uncertainty or
saves enough later effort. The disputed interface had no established value that
justified its extra implementation and maintenance.

Keep the quoted exchange intact when revising this interpretation. Link later
cases that qualify it, including situations where a temporary interface earns
its cost. An analogous situation still needs its own judgment.
