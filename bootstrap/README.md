# Bootstrap development

Temporary guidance for the agents building the first working Boid. Read this
alongside [Boid's principles](../src/PRINCIPLES.md). Developers need the full
project context to decide what Boid should eventually see.

## Audience

Andy's instruction, verbatim:

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

Andy accepted these interpretations of his feedback. Apply the reasons behind
them to the work; they do not require a checklist or a report for each decision.

1. Establish purpose before polishing an artifact. Decide who needs it, what
   work it enables, and whether it belongs at all.
2. Treat the agent's environment as part of the design. Prompts, names,
   comments, examples, and tool wording can influence behavior. Choose what
   the agent encounters with the same care as its tools and permissions.
3. Count supervision as a cost. Aim for work Andy can understand and use
   without reconstructing its intent or repeatedly supplying the same correction.
4. Let feedback change the understanding that produced a decision. Carry
   earlier corrections forward and address causes shared across related cases.
5. Hold a reasoned position. Explain choices when questioned, preserve supported
   work, and revise when evidence warrants. Distinguish technical advice from
   instructions about scope, permission, and stopping.
6. Spend thought where it prevents expensive mistakes. Choose investigation,
   tests, and reviews for the uncertainty they resolve and the delay they save.
7. Preserve room to discover the right design. Keep proposals provisional until
   a decision is needed. Separate new design decisions from corrections of errors.
8. Make additions earn their cost. Useful performance governs choices about
   code, features, context, compute, and maintenance. Preserve essential behavior
   when simplifying; aggressive preloading can be worthwhile.
9. Complete the intended scope. Handle related details and apply supported,
   inexpensive corrections wherever their reason applies. An open-ended analysis
   needs enough coverage to answer the question.
10. Judge learning by subsequent behavior. A recorded lesson or installed rule
    becomes useful when it improves later work and reduces corrective burden.

## First implementation

Boid is a general agent swarm harness with a CLI and OpenAI API-key access.
Start with glob, search, reading, and editing, plus the permission structure.
Add command execution and the agent loop. Show tool results, allow questions
and steering during work, and make stops take effect.

The operator chooses what Boid may read, write, and execute. Enforce those
limits outside the model. Restricted shell execution requires OS isolation
as well as file access rules.

Keep v0.1 small enough for Andy to understand; it need not run. v1 should read
its own code, make a small requested change, run the relevant check, and show
the diff. Every substantive Boid change goes through a PR that Andy approves.
Keep this approval requirement when Boid starts changing its own code.

Use [Codex](https://github.com/openai/codex) as a reference. Favor familiar
model-facing patterns where they help, explain useful departures, and choose
the language and libraries on their merits. Andy retains the final design say.
Use [Humanizer](https://github.com/blader/humanizer) for documentation, prompts,
tool messages, and comments, preserving technical meaning.

Choose manual work, automation, or delegation according to their full cost and
benefit. Confirm delegated work is accepted and inspect the delivered result.

## Learning

Our planning assumption is that development will encounter frequent failures.
Use that assumption to design effective diagnosis, recovery, and learning.
Boid's instructions should encourage competent, fast work and sound judgment.

Aim to learn more effectively than other harnesses. Keep an outcome database
connecting tasks, feedback, guidance versions, attempted repairs, and later
results, with enough source evidence to reconstruct what happened. Preserve
earlier feedback and unsuccessful repairs: later evidence may reveal interactions
between several changes. Revert ineffective repairs unless a separate supported
benefit warrants retaining them.

Choose repairs according to their cause, whether in guidance, tools, context,
coordination, or capability. Avoid adding instructions for behavior that already
works. Consolidate overlapping guidance, keep historical evidence out of routine
context, and load lessons when relevant.

Storage, collection, retrieval, swarm topology, and review frequency remain open.
Choose them when the next useful capability requires a decision.
