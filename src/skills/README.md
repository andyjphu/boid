# Local skills

New v0.8 sessions include Humanizer and discover local skill folders. The model
sees descriptions first and loads complete guidance with `read_skill`.
`list_skills` returns source identities, policies and discovery warnings.
Explicit `$name` mentions load matching guidance before the model request.
`/skills` opens a searchable browser picker, which inserts `$[source-id]` for an
unambiguous selection. Selection inserts text into the draft; sending it starts
work. Same-name packages stay separate. An ambiguous name reports an error and
does not load either package. Unknown plain `$variables` are left alone;
unknown bracketed selections report an error.

## Adding and configuring skills

Put a package containing `SKILL.md` in `.agents/skills` under the session's
working directory or its ancestors through the nearest Git root. Outside a
Git repository, only the working directory's `.agents/skills` is searched.
Boid also searches the runtime user's `~/.agents/skills`, `/etc/codex/skills`,
and roots in `BOID_SKILL_PATHS` (the platform path-list separator, colon on
Linux). Paths refer to the server/container filesystem. No host directories
are mounted automatically. Symlinked packages are followed and canonical paths
supply identities; aliases to the same file are deduplicated.

Use ordinary YAML frontmatter with a name and description. An omitted name
falls back to the folder name. `agents/openai.yaml` may set
`policy.allow_implicit_invocation: false`. Such skills are omitted from the
implicit catalog but remain explicitly selectable. Dependencies are reported
as unprovisioned; product restrictions disable the package with a warning.
Display metadata, icons and dependency installation are not implemented.
Scripts, references and assets stay in their package directory. `read_skill`
returns that directory; ordinary read and command tools access relevant files.
Scripts never execute merely because a skill was discovered or loaded.

Optional `.boid/skills.json` in the session's working directory controls
availability. Rules match either the exact source id or name; later matches
win. A name rule applies to all packages of that name, including the bundle.
Invalid configuration leaves the skill catalog unavailable with a visible
warning rather than accidentally ignoring disable rules.

```json
{
  "catalog_characters": 8000,
  "rules": [
    {"name": "humanizer", "allow_implicit_invocation": true},
    {"id": "bundled:humanizer", "enabled": true}
  ]
}
```

The implicit catalog has an 8000-character entry budget by default, configurable
up to 40000; fixed instructions and warning notices are additional. Descriptions
are shortened to 500 characters. Bundled skills are listed first. Omitted entries remain available through the
picker, explicit selection and `list_skills`, with an omission notice. Discovery
limits are 8 directory levels, 1024 directories, 256 local packages and 512 KiB
per skill/config file; limits and parse/read failures produce warnings. Full
skill text is never silently truncated.

## Refresh and evidence

At each operator-message boundary, Boid scans on a blocking worker and captures
the current SKILL.md text, policy and catalog for that turn. Tool continuations
and peer messages use the same snapshot. Editing a skill affects the next
operator message, including after restart, without requiring a server restart.
The browser picker scans for the next message and does not mutate active work.
Files can change between selection and sending; the turn uses the send-time
snapshot. Missing or disabled selections produce a trace entry and model-visible
error. Earlier explicit selections and tool loads remain unchanged in history.
Catalog changes have their own visible context entry. This is boundary refresh,
not a filesystem watcher.

Supporting files remain live filesystem content. They are not captured or
versioned with SKILL.md; reads and commands retain their normal tool traces and
output limits. A loaded skill is evidence of loading, not of compliance or better
writing. A policy change cannot remove text the model already saw earlier.

Saved sessions from before this local loader retain their original instructions,
tools and bundle without automatic migration. Create a new session for local
selection and refresh. The original bundled-only draft pinned skill versions
for the entire session; boundary refresh supersedes that behavior for newly
created sessions.

## Modules and future changes

- `parser.rs`: Codex frontmatter parsing, adapted from source at `a90c808`.
  Apache-2.0 attribution and license are in `NOTICE` and `codex-LICENSE`.
- `discovery.rs`: roots, package metadata, file limits and availability rules.
- `catalog.rs` and `instructions.md`: metadata presentation, context budget and
  model guidance. Guidance is compiled into the binary; rebuild to change it.
- `selection.rs`: operator mentions and source disambiguation.
- `skills.rs`: shared saved representation, bundle and complete-text loading.
- `Session::operator`, `Session::refresh_skills` and the runtime worker: selection,
  history and operator-message boundaries. The server owns these behaviors.
- `/skills` in `web/app.js`: browser selection using the server catalog.

Meta-review work can propose changes to any of these, including Rust, browser
code, skill content and guidance. The module boundaries support reviewable
changes; they do not create an automatic review loop or authorize self-deployment.
See [the meta-review proposal](../../bootstrap/meta-review.md) and
[case bank](../../bootstrap/cases/README.md).

## Scope decision, Andy, 2026-11-08

> oh yeah i dont think we'll ever need the full codex eco system, thats like super far backlog item, document that; but i agree that we should consider 1 adn 2, possibly just copying them over now, and then making improvements later; maybe implmenting them modularly that meta review agents can affect all natures of them, down to the code even, super well

The near-term scope is local package compatibility and useful selection/update
behavior. Full Codex ecosystem parity, including plugin/cloud distribution,
marketplaces, installers and cross-environment providers, is distant backlog
and may never be needed. There is no commitment to implement it. This local
implementation reuses Codex's self-contained parser; it does not import Codex's
extension runtime or require Codex App Server. Exact Codex picker syntax,
configuration format, filesystem watchers and token-based budgeting are not
parity goals of this draft.

## Humanizer provenance

Humanizer is bundled from the start of v0.8. It is copied without modification
from Andy's maintained v3.1.0 at
`project-marigold/guidance/skills/humanizer/SKILL.md`. Upstream:
https://github.com/blader/humanizer. The bundled MIT license credits Siqi Chen.
Preserve the license and source/version when updating it. SHA-256:
`0612f1dfb1672b0ea9b97e139bf1f06cabe98d8b27424fe8ff01e1fb4cc99cad`.
The bundle is self-contained and has no package directory or required supporting
files. Editing this compiled-in copy requires a rebuild; local packages can be
edited directly. `bundled:humanizer` distinguishes it from local copies.
