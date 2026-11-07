# Working on Boid

Read [PRINCIPLES.md](PRINCIPLES.md). This file adds instructions for changing
the Boid repository, including when Boid works on its own code. The current
scope and milestones are in [README.md](README.md).

Start with glob, text search, reading, and edits, then command execution and
the OpenAI agent loop. Show tool results and let the operator stop work. Let
users ask about ongoing work without derailing it, and give steering and stop
requests clear effects.

The operator chooses what Boid may read, write, and execute. Show those limits
and enforce them outside the model. Restricted shell commands need OS isolation;
file path checks alone do not contain a process.

Use [Codex](https://github.com/openai/codex) as an implementation reference.
Keep its tool and interaction patterns where they help the model. Reuse parts
that fit and explain proposed departures. Choose the language, libraries, and
swarm structure on their merits; tentative proposals are not settled choices.
The maintainer has the final say on Boid's design.

Count code, features, memory, compute, context load, and maintenance when choosing
an optimization. Aggressive preloading is welcome when the gains justify those
costs. Keep the code readable and changes small enough to understand.

Build learning around outcome records that connect tasks, feedback, guidance
versions, proposed changes, and later results. Preserve enough source evidence
to reconstruct what happened. Keep earlier feedback and revision history so
later decisions can account for interactions between changes. Use observed
results to decide which guidance to retain, revise, or retire.

Give each guidance artifact a clear audience. Consolidate overlap and load
lessons when they are relevant. Keep historical evidence in its record, outside
routine startup context. Keep private planning notes separate from content
shipped with Boid or retrieved by its agents. Prompts, examples, comments, and
names should describe the intended work and behavior.

Use [Humanizer](https://github.com/blader/humanizer) for documentation, prompts,
tool messages, and comments. Write for the actual reader and keep the technical
meaning intact. Prose is part of the environment future agents will encounter.

Every substantive change, including guidance, goes through a PR for maintainer
approval. This also applies when Boid changes its own code. Keep v1 focused on
the small, useful self-edit described in the README, then extend the learning
and swarm capabilities as needed.
