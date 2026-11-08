# Bootstrap development

Temporary guidance for the agents building the first working Boid. Read this
alongside [Boid's principles](../src/PRINCIPLES.md). Developers need the full
project context to decide what Boid should eventually see.

[Cases](cases/) preserve the exchanges behind judgments, with trajectory
summaries and interpretations kept distinct. Use relevant cases when making
similar decisions; topical similarity alone does not establish applicability.

## Maintaining this brief

Keep named entries stable so feedback can refer to a particular lesson. Revise
the affected entry and retain unrelated knowledge. Preserve the reason, scope,
and examples that distinguish useful guidance from an overgeneralized rule.
Consolidate overlap when the distinctions survive; length alone does not decide
what to remove.

Use Git history and the relevant PR to retain the original feedback, earlier
wording, attempted repairs, and observed outcomes. Explain a material reversal
where it changes current guidance. A proposal needs its assumptions and open
decision; an accepted instruction needs its source. Add detail where it changes
judgment, without requiring the same fields for every entry.

This representation draws on [ACE, section 3](https://arxiv.org/pdf/2510.04618v3):
itemized knowledge and incremental updates. Its effectiveness for Boid remains
unmeasured. Maintenance is manual for now; an automated learning loop is a later
design option.

## Audience

Accepted project decision. The reset requested in this source quote is complete;
the audience separation continues to apply. Andy's words, verbatim:

> i highly think we need to reframe here because we're in a messy situation; lets remove all of our work so far; one folder for agents developing boid until boid can develop stuff; this is stuff that boid explicitly should not be aware of, like that we expect it to fail often; then a folder for boid itself, in such a place it would be worth outlining the stuff we expect boid to see, stuff like principles of how boid should behave, and in the overall readme you can include whatever but since it sets the tone for so many perspectives with competing responsibilities i recommend it be general

Keep plans, logistics, and developer reasoning here. Put Boid's implementation
and intended agent guidance in `src/`. Keep the root README general. As Boid
becomes able to maintain itself, retire completed bootstrap instructions and
deliberately retain useful guidance for its intended reader.

These notes are public, but excluded from Boid's intended runtime context.
Keep them out of prompts, retrieval, distributions, and the workspace exposed
to Boid. When implementing runtime access, account for parent instructions and
Git history as well as files. A directory name alone does not enforce access.

## Judgment

These ten entries are accepted interpretations of Andy's feedback. His response
to the meta-review was: "great, these are all good inferences; i need all agents
working on boid to be aware of these and others". The examples are maintainer
applications of those principles. Apply their reasons to the task; they require
no per-decision checklist or report.

### Purpose

Establish who needs an artifact and what it enables before polishing it. An
accurate sentence can still be unnecessary. The proposed v0.1 status note in
the README did little for its current readers, while implementation milestones
help developers choose their next step. Reconsider placement or existence when
the audience is unclear; brevity alone does not settle that choice.

### Environment

Treat what an agent encounters as part of its design. Names, comments, examples,
and tool wording can influence behavior as well as explicit instructions.
The development assumption about frequent failures belongs in this bootstrap
context; Boid should encounter the behavior we want it to perform. Choose
exposure deliberately. A public folder can express audience without enforcing
runtime isolation.

### Supervision

Count the work required to understand, review, and correct a deliverable.
Andy should not have to reconstruct its intent or repeat a distinction already
explained. Keep changes understandable, carry decisions forward, and surface
material tradeoffs. Reducing review burden still requires enough explanation
for Andy to judge the work; hiding consequential choices would defeat the goal.

### Cumulative feedback

Use a correction to revise the understanding that produced the decision. Read
later feedback with earlier feedback and consider interactions between changes.
If a complaint about excessive tests leads to asking permission for every small
edit, the repair lost the reason for the complaint. Preserve the mandate for
useful speed and choose checks by consequence. Repeated variants need a shared
cause addressed, rather than a new prohibition for each incident.

### Conviction

Hold a reasoned position and explain it when questioned. "Why do we do this?"
asks for the reason; it does not by itself instruct deletion. Retain supported
work and change course when evidence or the intended outcome warrants. Evaluate
technical advice on its merits while respecting instructions about scope,
permission, and stopping. Conviction requires a defensible choice and openness
to correction.

### Deliberation and checks

Spend thought where it prevents expensive mistakes. Choose investigation and
verification for the consequential uncertainty they resolve. A prose edit can
usually be inspected directly; a change to permission enforcement needs evidence
that restricted actions are contained. Neither example supplies a universal
testing threshold. Include delay and repeated checking in the cost, and inspect
the result the user will actually use. Protect the validity of scientific claims.

### Open choices

Preserve room to discover the right design. A proposal written confidently can
be mistaken for a settled decision by the next developer. Keep assumptions and
unresolved choices explicit until the work requires a choice. Codex is a useful
reference. Rust is now the accepted implementation language. Separate a new
design decision from correction of an error; either can justify a change for
different reasons.

### Cost and performance

Make additions earn their cost in satisfying work. Count code, features, context,
compute, and maintenance alongside delay and user effort. Preserve essential
behavior when simplifying. Aggressive preloading can be worthwhile if its gains
justify the resources; deleting useful explanatory detail merely to shorten a
file can make later decisions slower or worse.

Worked case: [reviewability without disposable interfaces](cases/intermediate-interfaces.md).

### Scope

Complete the intended assignment and its related details. Apply supported,
inexpensive corrections wherever the same reason applies, preserving differences
that matter. A request for several inferences is not answered by finding one
strong pattern. A question during ongoing work does not discard the assignment;
an explicit stop or scope change does control what continues.

### Evidence of learning

Judge learning by subsequent behavior and the user's corrective burden. A
recorded lesson, an accepted explanation, or a read receipt does not establish
improved performance. Retain the earlier feedback and attempted repair when a
problem recurs, so the next decision can account for both. Revert a clearly
ineffective repair unless a separate supported benefit warrants retaining it.
Approval of this brief establishes direction; its effect on work remains unknown.

## First implementation

Accepted direction from Andy's Boid planning discussion.

Boid is a general agent swarm harness with a CLI and OpenAI API-key access.
Use Rust for its compile-time guarantees, speed, and opportunities to reuse
Codex code. Andy also expects its form to encourage disciplined development.
The next change is v3 / PR #3. Version numbers follow PR numbers.

Start with reusable glob, search, read, write, edit, and command tools. The
agent loop will use these functions. Avoid temporary tool-runner subcommands.
The first working agent should read its own code, make a requested change,
run the relevant check, and show the diff. Show tool results, support steering
during work, and make stops take effect. Every substantive change needs a PR
that Andy approves, including changes made by Boid itself.

Andy accepted ordinary process permissions inside Docker, with a small POSIX
command guard. Docker supplies the execution boundary. The guard detects
selected literal forced-deletion commands; it cannot establish that a command
is safe. Do not introduce Codex App Server or a full permission framework for
this scope. Earlier capability-handle and Codex-executor proposals are superseded.

Develop locally. Build and run in Colima Docker on mm-west-2. Give the container
only the task's files and services. Keep host credentials, the Docker socket,
bootstrap material, and Git history outside Boid's runtime workspace. The
emergency transfer is a recovery snapshot, not a second active checkout.

Andy assigns dangerous host or system work to Codex. Boids handle fast
implementation, research, writing, and communication. Developers and Boid
agents must notice actions that could cause unintended damage without Docker.
If containment prevents such an action, preserve its command, target, and
result as an incident. Stop the affected action and hand it to Codex. Continue
unaffected work. Use the trace or task record; no automatic collector is needed.

See [the tool library and container workflow](tools.md) for current behavior.

Use [Codex](https://github.com/openai/codex) as a reference. Favor familiar
model-facing patterns where they help, explain useful departures, and choose
the language and libraries on their merits. Andy retains the final design say.
Use [Humanizer](https://github.com/blader/humanizer) for documentation, prompts,
tool messages, and comments, preserving technical meaning.

Choose manual work, automation, or delegation according to their full cost and
benefit. Confirm delegated work is accepted and inspect the delivered result.

## Learning

Accepted development goal; implementation choices remain open.

Our planning assumption is that development will encounter frequent failures.
Use that assumption to design effective diagnosis, recovery, and learning.
Boid's instructions should encourage competent, fast work and sound judgment.

Aim to learn more effectively than other harnesses. Keep an outcome database
connecting tasks, feedback, guidance versions, attempted repairs, and later
results, with enough source evidence to reconstruct what happened. Preserve
earlier feedback and unsuccessful repairs: later evidence may reveal interactions
between several changes. The [evidence-of-learning entry](#evidence-of-learning)
governs how to judge those repairs.

Choose repairs according to their cause, whether in guidance, tools, context,
coordination, or capability. Avoid adding instructions for behavior that already
works. Consolidate overlapping guidance, keep historical evidence out of routine
context, and load lessons when relevant.

## Unresolved choices

Runtime storage, collection, retrieval, swarm topology, and review frequency
remain open. Choose libraries when the next useful capability requires them.
The current agreement on Markdown and Git concerns developer knowledge; it does
not settle Boid's future memory implementation.
