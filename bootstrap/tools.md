# Server, web interface, and tools

Run `boid serve` (or `boid` with no arguments) inside the task container. The
shared server listens on `127.0.0.1:7734` by default and prints a browser URL.
A second server cannot bind the same address. `BOID_LISTEN` changes the listener;
no system service is installed. Set `OPENAI_API_KEY`; `BOID_MODEL` defaults to
`gpt-6-sol`, with medium reasoning effort and Daybreak Blue.

`boid "task"` submits to the running server, prints its session ID, and exits.
`BOID_URL` selects a server. `BOID_CWD` optionally selects a directory as seen
by that server; otherwise the server's working directory is used. `BOID_TOKEN`
supplies its access token, or the client reads the private token file under
`BOID_STATE_DIR` (default `~/.local/share/boid`). The server generates a token
file when needed.

The printed browser URL exchanges a fragment token for an HttpOnly, SameSite
cookie and removes the fragment. API access requires that token or cookie;
cross-origin browser requests are rejected. The browser never receives the
OpenAI credential. Use SSH forwarding or an authenticated HTTPS deployment
for remote access; the default listener is local.

## Sessions and live views

The web workspace shows simultaneous agent traces, per-agent input,
and real terminal panes. Each has a compact tab header beginning with an icon and ending
with a close control. Tabs grow with their title and path, truncating at the
available pane width.
Traces show selected instructions and tools, user and peer messages, text,
tool calls and results, and control events. Provider-exposed reasoning summaries
are hidden in every pane; events remain in the full session record.
Models may return no summary; hidden reasoning is unavailable. Responses are displayed as plain text. Startup context is expandable;
its full-session link retains the original event record, including transport
receipts omitted from the reading view. The view adds no tool-result truncation.

The transcript uses one 11px text size. Tool calls show one command or target
line and use one expansion control for the operation's arguments and result.
Results stay at their chronological position, including when a peer message
intervenes. Successful tool calls have no completion
label; running calls show an ellipsis, and failures retain their status. Streamed
arguments are replaced by their formatted call using entry identity, including
when a peer message arrives between them. The full record retains both events.

An empty message box appears on pane hover or keyboard focus; a draft keeps it
visible. The box overlays the stream, so revealing it does not resize the
transcript. Typing, submission, or followed output makes room above the visible
box; manual history reading keeps its position. Extra scroll space permits
uncovering messages, but following targets the message end when the box is
hidden. Working retains a reserved line below the stream even when idle.
Browser text assistance is disabled on text inputs.
Disconnection mutes the view. Each pane has an × control that ends execution and archives its record; there is no separate
Latest control or title gesture that enlarges a pane.

Adjacent queued messages form a compact block with a lighter background and
full-opacity text. Delivered messages from the same `input_batch` join as well;
separate batches and intervening visible output retain their separation.

Streaming keeps a view at the bottom when it is already there. Reading earlier
messages preserves the visible message and its offset as content changes.
Scrolling back to the bottom resumes following; each pane tracks this separately.

Typing while transcript text is selected focuses that pane's prompt and inserts
the typed character at its saved caret. Copy shortcuts and navigation keep their
normal behavior.

Enter sends; Shift+Enter adds a newline. Input accepted during work waits for
the current model response and all its tool calls to finish, then enters the
next model request without cancelling work. Escape in the selected
agent pane requests interruption and applies queued input. Without queued
input, it leaves execution stopped. Draft text is preserved. Completed results
stay in context; interrupted actions are marked potentially partial. The
headless Stop route retains its separate priority path and cancels older queued
requests. Later user input can resume work. While an agent is working, a bottom
indicator shows Working, elapsed time, and Esc to interrupt; it remains visible
when the message box is hidden. The elapsed time uses the server timestamp and
survives browser refresh.

`list_agents` exposes session identities and status. `send_message` queues an
attributed message for the recipient's next model request. It does not interrupt
an active request. Idle recipients wake; stopped or errored recipients wait for
new user input. Queueing is not acceptance, and peer input does not expand the
operator's permissions.

The server owns execution. Closing a browser, event subscription, or submitting
command leaves work running. Reconnection receives a snapshot followed by live
events; slow event consumers receive a fresh snapshot. Private session records live under BOID_STATE_DIR. They retain model history,
accepted input, and tool execution state. Restart restores conversations stopped,
without issuing model requests or replaying uncertain actions. Terminal records
retain bounded output and return without a running shell. Keep the state directory
on persistent storage when replacing the container. Context compaction and
long-running-job continuity remain follow-up work; see [sessions](sessions.md).

The Responses client streams with `store: false`, explicit conversation input,
and `reasoning.summary: auto`. Completed encrypted reasoning returns as opaque
context. API failures are reported without automatic retries. Runtime guidance
is compiled from `src/PRINCIPLES.md`; parent/bootstrap discovery is not automatic.

The default profile is GPT-6 Sol / medium / Daybreak Blue. `/model` opens a
local keyboard picker for that single profile; Enter confirms and Escape closes
it. The command is not sent to an agent. The picker reads the session's retained profile,
so it does not imply an unavailable model was activated.

Requests send `reasoning.effort: medium` and `access_programs.cyber: daybreak_blue`
by default. `BOID_CYBER_ACCESS_PROGRAM` can explicitly select `standard`,
`daybreak_blue`, or `daybreak_red` for a compatible model/project. An incompatible
selection returns its API error without switching models or falling back.
Daybreak does not select a reasoning stream. See the
[OpenAI Daybreak guide](https://developers.openai.com/api/docs/guides/daybreak).

## Skills in v0.8

New sessions discover local skill folders and include bundled Humanizer for
prose work. `/skills` selects a package; `$name` explicitly requests an
unambiguous skill. `read_skill` loads full text; `list_skills` exposes sources,
policies and warnings. Skill loads have expandable saved bodies; errors stay
visible. Immutable skill snapshots live in the same state directory as sessions.
Packages refresh at operator-message boundaries while
previously loaded text remains in the trace. Earlier saved sessions retain their
original environment. See [local skills](../src/skills/README.md) for directory
layout, configuration, supporting resources, refresh semantics and provenance.
Loading alone does not establish compliance or improved writing.

## Headless routes

The browser and scripts use the same authenticated API:

| Route | Behavior |
| --- | --- |
| `GET /api/config` | Default working directory and model |
| `GET /api/sessions` | Session snapshots |
| `POST /api/sessions` | Create with optional `name`, `cwd`, `message` |
| `GET /api/sessions/{id}` | Inspect one session |
| `GET /api/sessions/{id}/skills` | Inspect skills available for the next operator message |
| `DELETE /api/sessions/{id}` | End the worker and archive its retained record |
| `GET /api/sessions/archived` | Discover ended agent records |
| `POST /api/sessions/{id}/reopen` | Reopen a retained agent, stopped until new input |
| `POST /api/sessions/{id}/messages` | Queue `{ "text": "..." }`; return a message ID |
| `POST /api/sessions/{id}/interrupt` | Interrupt current work and apply queued input; stop if none |
| `POST /api/sessions/{id}/stop` | Request cancellation; later status confirms stopping |
| `GET /api/events` | Shared agent/terminal catalog snapshot and sequenced events |
| `GET /api/terminals` | Terminal identities and status |
| `POST /api/terminals` | Open with optional `cwd`, `cols`, `rows` |
| `GET /api/terminals/{id}/ws` | WebSocket terminal output, input, resize |
| `PATCH /api/terminals/{id}` | Rename with `{ "name": "..." }` |
| `DELETE /api/terminals/{id}` | End and wait for the shell, retaining its archive |

Agent behavior can be checked headlessly. Browser checks establish interface
behavior. Tool unit tests alone do not establish disconnect or coordination behavior.

## Interactive terminals

Terminals start as #1, #2, etc. Double-click a terminal name to edit it; Enter
saves and Escape cancels. Renaming updates other views without replacing the
shell. Names remain with the durable terminal record across server restarts.

Login-shell PTYs and locally vendored xterm.js provide interactive programs, keyboard
input, resize, and reconnect. Disconnecting leaves a shell running; its ×
control ends it. Agent panes use the same control to end and archive the session. The + agent setup pane reopens
an agent stopped; terminal records are retained separately.
Replay retains a bounded 512 KiB history with resize events and marks omitted
older output. Replaying bytes never sends terminal responses back to the shell.
Views share the PTY dimensions; focusing or clicking a terminal transfers input
and resize control to that view. Other views follow its dimensions.
Terminal input is an explicit operator shell action and bypasses the model's
command heuristic. Both terminal and model command children omit API/server
credentials from their environments. Deliberately detached processes still
require container cleanup.

Terminals use `SHELL` when executable, otherwise the account's configured login
shell, and load its startup files. The development image includes zsh and Git.
Install personal themes/plugins and portable shell settings in the container's
home; host-specific paths and credential loaders do not belong in that copy.
The browser terminal prefers locally installed MesloLGS NF, with monospace
fallbacks. Andy's preview uses his Powerlevel10k settings, Oh My Zsh, Git,
autosuggestions, and syntax highlighting. These are operator-provided container
settings, not assets bundled with Boid.

Keep configured shell settings in a persistent container-home volume so replacing
the server container does not remove the theme. For an existing configured
container, copy only the portable theme files into that volume before first use:

```sh
BOID_THEME_SOURCE=existing-configured-container
set -o pipefail
docker exec "$BOID_THEME_SOURCE" tar \
  --exclude=.git --exclude=cache --exclude=log \
  -C /root -cf - .zshrc .p10k.zsh .oh-my-zsh | \
  docker run --rm -i --network none -v boid-shell-home:/root \
    boid-dev tar -C /root -xf -
```

Use a new volume for that initial copy, then reuse it when replacing containers.
The source must already have the desired theme and portable settings; this does
not install Powerlevel10k from scratch. The named paths deliberately omit shell
history and unrelated home files. Keep personal settings out of Git. The launch
example below mounts both durable session state and this separate shell home.

## File and command behavior

The server calls the v3 library through six function tools: `read_file`,
`write_file`, `edit_file`, `glob`, `search`, and `exec_command`.

`Tools::new` sets the working directory. Absolute paths and parent paths use
the process's ordinary permissions. Run the process inside the task container.
The library does not create a container or confine paths itself.

Reads and command output retain the first 32 KiB of each stream and report
truncation. Invalid UTF-8 is displayed with replacement characters. Glob and
search use ripgrep's rules, sorted paths, and a 30-second limit. An explicit
glob overrides ignore rules. Their exit status distinguishes no matches from
a tool error.

Writes replace a file atomically. Edits require exactly one occurrence of the
old text in a UTF-8 file of at most 1 MiB. Both reject final symlinks and require
the parent directory to exist. Existing Unix permissions are preserved; new
files use private temporary-file permissions. Ownership, ACLs, and extended
attributes are not preserved. Coordinate concurrent writers at the caller.

Commands run through Bash without startup profiles, with closed stdin. The
model tool defaults to a 30-second timeout and accepts 1 to 300,000 milliseconds.
Child commands do not inherit `OPENAI_API_KEY` or `BOID_TOKEN`; the server
process still holds its credentials. A command owns its process group. Its background
children end when it finishes, times out, or the caller drops the future.
Processes that deliberately leave the group require container-level cleanup.

The guard adapts the POSIX literal-command detector from Codex at `a90c808`.
It rejects recognized forced `rm` commands and excessive wrapper nesting.
Dynamic command names, script contents, and unsupported shell forms can escape
classification. Passing the guard does not establish safety. It has no
interactive approvals, policy-rule store, or Codex App Server dependency.

## Build and run

Develop locally. Build and execute on `mm-west-2` under Colima Docker. Stage
`Cargo.toml`, `Cargo.lock`, and `src/` in a disposable build directory; keep
the recovery snapshot separate. If the unqualified hostname fails, use
`-o HostName=mm-west-2.local -o HostKeyAlias=mm-west-2` and retain host-key checking.

Build `boid-dev` from `bootstrap/Dockerfile` on stdin. Run `cargo fetch --locked`
with registry access, then `cargo fmt --check`, `cargo test --locked --offline`,
and `cargo build --locked --offline` without network. Share dependency downloads;
keep build artifacts separate for each source workspace. The frontend has no
build step. Pinned terminal/Markdown assets and licenses are under `src/web/vendor/`.

On the Docker host, set `BOID_WORKSPACE` to the task directory. This uses the
existing compiled binary volume and publishes only on host loopback:

```sh
docker run --rm --init \
  --cap-drop=ALL --security-opt no-new-privileges \
  --pids-limit=256 --memory=4g --cpus=4 \
  -p 127.0.0.1:7734:7734 \
  -e OPENAI_API_KEY -e BOID_MODEL -e BOID_LISTEN=0.0.0.0:7734 \
  -e BOID_STATE_DIR=/state -v boid-state:/state \
  -v boid-shell-home:/root \
  -v "$BOID_WORKSPACE:/work" -v boid-v3-target:/opt/boid:ro \
  boid-dev /opt/boid/debug/boid serve
```

Use SSH forwarding to reach the remote host. Run the server under a supervisor
or persistent terminal to survive an SSH logout; browser independence does not
make it survive host shutdown. Bind-mounted fixtures must use Colima's shared
home paths, not Mac `/tmp`. Docker `--env-file` is read by the host client and
does not require a bind mount. Keep credentials out of evidence artifacts.

For self-development, expose a source copy and relevant guidance with the
[bootstrap caution](README.md). Keep the canonical checkout, host home, Docker
socket, and unrelated credentials outside the task mount. Keep Git history out
unless needed and explicitly scoped. Review the resulting diff through a PR.
The container launch supplies the boundary; the binary does not create it.

For development incident recording and Andy's personal task routing, see the
development brief. Boid has no required Codex handoff.

Debug builds read index.html, app.js and style.css from src/web on each request,
so a browser refresh loads UI edits without a Rust rebuild or session restart.
Release builds retain embedded assets.
