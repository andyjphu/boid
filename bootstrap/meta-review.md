# Generic and meta-review Boids

Status: Andy's proposed direction, recorded 2026-11-08. This document does not
implement templates, context editing, automatic review, or guidance mutation.

Andy identified case-informed Boid review as Project Marigold's successor. His
full statement is preserved as `catastrophe-15543` in the
[source excerpts](cases/evidence/whole-interaction/excerpts.json):

> yes, this is a crucial case study i want visible to boid agents when i start having boid work on itself; im considering having boid templates; 1. generic - default workers without a specific specialization 2. meta-review boids - review the case studies, and make suggestions to fix context of another agent (RLM style) or fix guidance documents for future agents based on a bank of well-curated case studies; these case studies (im not sure if they're started yet) should include quotes from me (i tend to express my taste densely these days, and i notice agents dont take all N meaning i want them to); as well as preliminary agent analysis and traces, and as we have more case studies we'll have more datapoints for these meta review agents to make better judgements about how to shift guidance docs etc, this is project marigold's true successor (im so proud we got to this point, nice job agents!)

## The proposed roles

| Template | Intended work | Useful input | Candidate output |
| --- | --- | --- | --- |
| Generic | Ordinary workers without a specialization | Task context, appropriate tools, and relevant guidance | Task results; case evidence when assigned |
| Meta-review | Review curated cases and suggest improvements to another agent's context or future agents' guidance | Original user words, visible traces, preliminary analyses, prior repairs, and the target context/guidance | A specific proposed change with evidence, scope, and unresolved uncertainty |

The role distinction comes from Andy. The input/output details are preliminary
maintainer interpretations to make the proposal reviewable. The generic role
can still use a relevant case. The meta-review role does not make every worker
conduct a review on every turn.

Two interventions deserve separate designs. A current-context suggestion may
restore a missed objective, correction, or distinction for one agent's ongoing
task. A guidance revision changes what future agents encounter and needs
evidence that the lesson transfers beyond the triggering example. Andy's
"RLM style" phrase points toward the first intervention; the exact mechanism
and relationship to any particular RLM architecture remain unspecified.

## What the case bank must preserve

The [existing bank](cases/README.md) already contains failure and positive
judgment examples. The new [whole-interaction case](cases/whole-interaction.md)
adds the v0.7 correction sequence and a measured recovery.

Quotes should preserve the density of Andy's feedback. Extracting one lesson
must not discard the other requirements, tentative ideas, praise, or scope
changes in the same turn. Keep the original quote alongside interpretation,
with enough preceding agent behavior and later feedback to recover what the
user was responding to. Preliminary analysis helps a reviewer begin; it must
remain distinguishable from the observations and open to revision.

As the bank grows, reviewers can compare repeated mechanisms, successful
responses, and counterexamples. The intended benefit is better judgment about
what context or guidance to change. Curated examples do not automatically
establish a measured failure rate or prove that a guidance edit worked.

## A practical first version

Catastrophe recommends starting with an assigned review that retrieves a few
relevant cases and examines the actual target context or guidance. The reviewer
should explain the mismatch it sees and offer a concrete change, including
why leaving the guidance unchanged or repairing the implementation alone would
be insufficient. A local implementation defect can have a local repair.

Keep the original guidance, proposed revision, supporting cases, and later
outcome linked. A later reviewer should be able to see whether a prior repair
helped, failed again, or produced a different problem. Measure usefulness through
subsequent work and the operator's corrective burden. A reviewer producing a
persuasive explanation is an intermediate result.

This extends Marigold's preservation of feedback, repairs, and outcomes into
Boid's own work. It does not erase Marigold's earlier records or establish that
every prior policy has been replaced. The current proposal does not activate
an unattended review or rewrite loop.

## Open design decisions

- How templates are selected and what tools or context each receives.
- How a review is triggered, and how cases are selected under a context budget.
- What a reviewer may change directly versus propose, including live-context
  replacement, operator corrections, and edits to shared guidance.
- How to retain original context and source attribution through an accepted
  change, and how the affected worker learns what changed.
- How later work supplies evidence that a change helped, including failed
  repairs, regressions, selection bias, and positive cases.

These choices remain open. The authorized step here is preserving and making
the case bank discoverable for self-development; implemented template behavior
will need a separate product design.
