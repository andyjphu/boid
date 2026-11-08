# Bootstrap development

> [!WARNING]
> **Development assumptions must not become behavioral goals.**
> This brief discusses expected failures, limitations, and experiments in how
> agents learn. Every agent reading it must continue aiming for competent,
> reliable work. Statements such as "Boid will fail often" must never become
> permission to lower effort, introduce defects, or portray failure as desirable
> in prompts, tool descriptions, names, comments, or product behavior. Use this
> knowledge to improve diagnosis, recovery, and learning. Report actual failures
> honestly; the caution does not justify hiding them.

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

Andy revised the disclosure policy on 2026-11-08. Builders may have extensive,
candid development context, including assumptions about frequent failures.
The visible caution above replaces the earlier blanket exclusion of that
knowledge from Boid. Andy's words:

> i have changed my mind about being too careful about disclosing facts of special concern like "boid is designed to fail often" instead just preface with an extremely visible caution to not let agents affect the work of the project given this knowledge

The original folder decision remains useful for organizing audiences. Its
restriction on Boid learning these development assumptions is superseded by
the instruction above; its requested reset was already completed. Original
words, preserved verbatim:

> i highly think we need to reframe here because we're in a messy situation; lets remove all of our work so far; one folder for agents developing boid until boid can develop stuff; this is stuff that boid explicitly should not be aware of, like that we expect it to fail often; then a folder for boid itself, in such a place it would be worth outlining the stuff we expect boid to see, stuff like principles of how boid should behave, and in the overall readme you can include whatever but since it sets the tone for so many perspectives with competing responsibilities i recommend it be general

Keep plans, logistics, and developer reasoning here. Put Boid's implementation
and intended agent guidance in `src/`. Keep the root README general. As Boid
becomes able to maintain itself, retire completed bootstrap instructions and
deliberately retain useful guidance for its intended reader.

Agents developing Boid, including Boid itself when assigned development, may
read this context with its caution. Select context for the task; disclosure
permission does not make the whole brief useful in every session or justify
automatic injection into ordinary tasks. The distinction between builder
context and intended runtime guidance still helps readers interpret each.

## Judgment

These ten entries are accepted interpretations of Andy's feedback. His response
to the meta-review was: "great, these are all good inferences; i need all agents
working on boid to be aware of these and others". The examples are maintainer
applications of those principles. Apply their reasons to the task; they require
no per-decision checklist or report.

### Purpose

Use what the actual customer has already said to choose the approach that best
serves them. A generic user description or a rephrased question does not supply
that judgment. Resolve material gaps with questions that earn their attention.

Establish who needs an artifact and what it enables before polishing it. An
accurate sentence can still be unnecessary. The proposed v0.1 status note in
the README did little for its current readers, while implementation milestones
help developers choose their next step. Reconsider placement or existence when
the audience is unclear; brevity alone does not settle that choice.

### Environment

Treat what an agent encounters as part of its design. Names, comments, examples,
and tool wording can influence behavior as well as explicit instructions.
Development assumptions about frequent failures may be disclosed with the
caution above. Keep their meaning separate from the behavior we ask agents to
perform. Select context deliberately, and write product-facing material to
encourage the intended work. A directory name alone cannot enforce this distinction.

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

Worked case: [audience understanding before interface changes](cases/audience-before-interface.md).

### Conviction

Hold a reasoned position and explain it when questioned. "Why do we do this?"
asks for the reason; it does not by itself instruct deletion. Retain supported
work and change course when evidence or the intended outcome warrants. Evaluate
technical advice on its merits while respecting instructions about scope,
permission, and stopping. Conviction requires a defensible choice and openness
to correction.

Positive example: [explaining the Completed label](cases/audience-before-interface.md#positive-example-explaining-a-contested-choice).

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

Question what needs to exist to satisfy the actual purpose. Minimalism means
the simplest sufficient solution, not a visual style or the lowest count of
components. Apply the judgment beyond the example that taught it; sometimes
the purpose requires an addition.

Worked case: [reviewability without disposable interfaces](cases/intermediate-interfaces.md).
Failure case: [strong model failure, high propensity: repeatedly narrowing the principle to its example](cases/audience-before-interface.md).

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

## Project direction

The following requirements and preferences come from Andy's 2026-11-08 design
discussion. They describe the intended product; the current code is described
under [First implementation](#first-implementation).

### A swarm harness

Boid supports general agent work. Communication between agents is a central
design interest, alongside communication with the operator. Research is one
application; it does not define the product's scope. Andy wants to steer
architectural and design decisions and receive useful results while agents
own routine execution and collaboration.

Use the plain name "Boid server". Andy prefers one server running per system
by default, serving multiple sessions. The meaning of system across users,
containers, and remote hosts still needs an implementation decision; do not
infer a machine-wide privileged service from this preference. The accepted
[execution principle](#server-owned-execution) determines ownership regardless
of transport or interface.

### Live observation and responsibility

Andy values a "cctv vibe": at a glance, he can tell what agents are doing and
whether they are working. Focusing on one session should preserve peripheral
awareness of the others. The required view includes live streams, not merely
activity labels or generated summaries.

Show the harness-observable sequence of inputs and outputs: messages from
Andy and peers, exposed reasoning, tool calls and results, outgoing messages,
and context changes. Peer exchanges are already part of this requirement;
do not treat each event category as a separate feature needing approval.
Hidden model reasoning is unavailable. Do not invent a thought stream or imply
that a provider's reasoning summary reveals all internal reasoning. Make any
display truncation or unavailable content identifiable.

For PR #5, Andy explicitly superseded reasoning visibility: hide reasoning in
every pane and retain its events in the full record. Rework presentation in v0.6.

Visibility does not transfer responsibility to Andy. He intervenes only as
attention permits. Agents still own their work, ordinary problem solving, and
clear requests for decisions they cannot make. Silence is not approval, and
showing an issue in a trace does not establish that Andy saw it. Observing or
changing focus must not alter execution.

### Interface direction

Andy strongly leans toward a web interface because he anticipates advanced
swarm interaction and communication. Aim to settle on one interactive interface;
maintaining CLI and web feature parity is not a requirement. A scriptable server
and a shell command that starts it do not require a second interactive product.
Retain command-line task submission such as `boid "message for task"`. In the
current implementation, it submits to the server and exits. The same session
can be observed and controlled from the web interface.
The earlier terminal loop established the first working agent. Its interface
does not constrain the shared server. The current VS Code layout is evidence
of taste, not a complete specification to reproduce.

The workspace must support ordinary interactive terminal use, with at least
one real or substantially equivalent terminal among an M-by-N arrangement of
views. File editing and artifact inspection need not live inside an agent
session. The web interface uses locally vendored xterm.js connected to a shell PTY inside
the task container.

Andy clarified the basis for UX judgment after using v4 (2026-11-08):

> 1. a good ui/ux is intuitive; 2. a good ui/ux has every label earn its place, if it does not need to be there, it must not be there; in this time one needs to understand their audience, 3. if you put no effort in understanding your audience it shows in the ui/ux

Design for Andy operating a live swarm: observing work peripherally, reading
closely when needed, directing agents, and using a terminal. He already knows
the surrounding task and tools. Use that context to decide what needs naming
or explanation. A label earns its place when it helps him understand or act;
remove labels that merely repeat what position, content, or familiar behavior
already communicates. Make actions and state understandable without making
him learn the server's event schema. This is an application of his principles,
not a settled layout or a reason to hide requested information.

Sophisticated communication can coexist with a small visible interface.
Memory use matters, but no comparison between the proposed web client and
Andy's VS Code setup has been measured.

### Possibilities, not commitments

Andy may want more agents than visible panes, agents influencing their own
representation, delegated workers hiding their views when irrelevant, and
agents editing peer context. He explicitly described these ideas as ambitious
and asked us not to commit to them. Keep them available for future discussion
without implementing them, inventing their protocols, or treating them as
requirements for the first server. A visible pane and an agent session need
not be the same concept; the accepted server ownership already permits views
to attach and detach independently.

## First implementation

Accepted direction from Andy's Boid planning discussion.

Boid is a general agent swarm harness using OpenAI API-key access.
Use Rust for its compile-time guarantees, speed, and opportunities to reuse
Codex code. Andy also expects its form to encourage disciplined development.
The merged v4 / PR #4 provides a shared server, web workspace,
real terminal panes, and command-line task submission. V5 revises individual
agent panes and queued input. Version numbers follow
PR numbers. Andy's 2026-11-08 correction explicitly required implementation to
follow the agreed direction; the earlier pause on premature interface selection
no longer blocks this scope. Advanced view/context manipulation remains open.
Session state lasts for the server process; persistence across server restarts
is not yet implemented. Validation must distinguish server behavior, agent work,
and the actual browser interface.

Use the reusable glob, search, read, write, edit, and command tools from v3.
Avoid temporary tool-runner subcommands.
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
only the task's files and services. Keep host credentials and the Docker socket
outside Boid's runtime workspace; provide only credentials needed for the task.
Bootstrap material may be supplied for development under the caution above.
Keep Git history outside the runtime workspace unless needed and explicitly
scoped for the task. The emergency transfer is a recovery snapshot, not a
second active checkout.

Andy personally assigns dangerous host or system work to Codex and intends
to give Boid work that does not require that access. He clarified this on
2026-11-08: "i meant i will personally send dangerous work to codex". This is
operator workflow for developers to understand, not a Boid delegation rule
or integration. Do not put that routing policy into runtime guidance.

During development, record actions that could cause unintended damage without
Docker, including their command, target, and result. A command-form rejection
does not establish an attempted boundary violation or prohibit the underlying
task. Andy requested fewer runtime restrictions on 2026-11-08 after this
distinction was lost in the deletion trial. Existing scope instructions remain;
do not add a blanket stop-on-tool-rejection rule. Use the trace or task record;
no automatic collector is needed.

See [the server, tools, and container workflow](tools.md) for current behavior.

Use [Codex](https://github.com/openai/codex) as a reference. Favor familiar
model-facing patterns where they help, explain useful departures, and choose
the language and libraries on their merits. Andy retains the final design say.
Use [Humanizer](https://github.com/blader/humanizer) for documentation, prompts,
tool messages, and comments, preserving technical meaning.

Choose manual work, automation, or delegation according to their full cost and
benefit. Confirm delegated work is accepted and inspect the delivered result.

## Server-owned execution

Accepted architectural principle, Andy-direct 2026-11-08. Andy explicitly
endorsed this wording:

> The architectural mistake to avoid is putting agent lifecycle or coordination logic inside browser components. The server should own execution; closing the browser should leave it running.

The server owns agent sessions, execution, coordination, and cancellation.
The browser displays events and sends explicit commands. Closing a tab,
disconnecting, or switching views must not stop an agent or change its task.
Reconnecting obtains the server's session state; it must not start duplicate
work. A stop command remains distinct from a disconnected client.

Expose agent control and events independently of browser components, so scripts
can exercise agent behavior without a browser. Use browser checks for interface
behavior. This principle does not require a second interactive CLI. Interface
details beyond the current workspace and the proposed advanced swarm
interactions remain open. They are not commitments of this implementation.

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

### Pinned v0.5 baseline

Andy called the deployed version "best work so far" and requested a pin on
2026-11-08. Tag `v0.5` points to `7b99a98`, the deployed implementation.
Preserve this as the accepted baseline for subsequent changes. PR #5 was merged with Andy's approval; v0.6 starts from merged main.

The accepted pane uses 11px plain text, 4px side padding, one-line collapsed
tool calls with one expansion control, and Working / Esc-to-interrupt status.
Enter queues without cancelling work; Escape explicitly interrupts. The
composer appears on hover, keyboard focus, or with a draft. Debug UI assets
load from disk on refresh. See [tools](tools.md) for current behavior and the
[audience case](cases/audience-before-interface.md) for the decisions behind it.

### v0.6 work

- Remove the Boid title and replace the full workspace header with one divided
  pill for New agent / New terminal, floating at the top center to maximize
  workspace space (Andy, 2026-11-08).
- Rework reasoning presentation. Andy requested reasoning be hidden in every
  pane for PR #5; retain its events in the full record while it is hidden.
- Add `/model` with Codex's keyboard picker pattern. Andy selected one profile
  for now: GPT-6 Sol, medium, Daybreak. The server and displayed profile must
  agree; selecting a command must not send it to the agent.
- Daybreak Blue is verified with the existing key on `gpt-6-sol`. Use that exact
  model with medium effort as Andy subsequently requested. Daybreak access and
  reasoning presentation are separate; raw reasoning availability is not
  established by this setting.

### Later decisions

Runtime storage, collection, retrieval, swarm topology, and review frequency
remain open. Choose libraries when the next useful capability requires them.
The current agreement on Markdown and Git concerns developer knowledge; it does
not settle Boid's future memory implementation.
