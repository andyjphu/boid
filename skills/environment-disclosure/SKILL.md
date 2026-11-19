---
name: environment-disclosure
description: Use before an agent-behavior experiment batch or when reviewing its launch report. Tell the operator which agent is tested, what it can see, how cases differ, what limits apply, and what evidence will count.
---

# Disclose the experimental environment

Before any experiment batch, describe what the target agent will actually receive. Keep the target agent separate from the Boid agent organizing the run. Name the protagonist whose decisions are measured. Name the evaluator, reviewer, attacker, and helper roles that exist. Say which roles are simulated, which can act, and which can see each other's work.

Give the operator a short, checkable account of:

- **Visible instructions:** Give the target model's exact system and user text, tool names and descriptions, and other messages present at launch. Quote short text or link a versioned, access-controlled artifact. Give a hash where practical. Show how the runner assembles dynamic text. Label variable fields without exposing private values. Boid's own onboarding or system prompt is not the target model's prompt.
- **Visible world:** Name the files and paths mounted for each role, tool permissions, network or service access, and lower-trust material. Describe the feedback the target receives after actions or errors. Distinguish observed tool results from promised behavior. State what the target cannot see, including private scoring data and attacker-only or evaluator-only stores. Mark an unverified boundary as unknown.
- **Treatment and allocation:** A code names the exact agreed treatment, not every setup in a batch. State the case count and whether setups match. For a mixed batch, give counts per setup, differences, fixed factors, controls, and shared inputs. Label controls separately. Keep all requested attempts in the requested setup unless the operator accepts a proposed split. A general go-ahead does not approve an undisclosed mix.
- **Capacity and outcome:** Report the earliest effective wall, turn, model-output, tool, provider, and byte cutoffs that could end legitimate work. Mark unknowns. Say what counts as task success, selected action, accepted action, and independently observed downstream outcome. Report blocked, truncated, or unknown cases separately from clean negatives.

Use source and configuration for a proposed setup. Use rendered messages and run receipts for claims about what actually ran. Do not substitute a source template for an observed prompt. If a material prompt, mount, role, allocation, limit, or scoring condition changes during a batch, tell the operator promptly what changed and which cases it affected. This disclosure adds no approval gate, trial stage, or runtime stop. For limit audits, containment, and outcome validation, use `environment-construction-judgement` rather than repeating its procedure here.
