# Server, web interface, and tools

Run `boid serve` (or `boid` with no arguments) inside the task container. The
shared server listens on `127.0.0.1:7734` by default and prints a browser URL.
A second server cannot bind the same address. `BOID_LISTEN` changes the listener;
no system service is installed. Set `OPENAI_API_KEY`; `BOID_MODEL` defaults to
`gpt-6.1-sol`.

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

The web workspace shows simultaneous agent traces, per-agent input and Stop,
and real terminal panes. Focus enlarges a view while retaining other panes.
Traces show selected instructions and tools, user and peer messages, text,
provider-exposed reasoning summaries, tool calls and results, and control events.
Models may return no summary; hidden reasoning is unavailable. Text is displayed
without interpreting it as HTML. The view adds no extra tool-result truncation.

New user input interrupts an active attempt and steers the next request.
Completed results stay in context; interrupted actions are marked potentially
partial. Stop has a priority path independent of the input queue and cancels
older queued requests. Later user input can resume work.

`list_agents` exposes session identities and status. `send_message` queues an
attributed message for the recipient's next model request. It does not interrupt
an active request. Idle recipients wake; stopped or errored recipients wait for
new user input. Queueing is not acceptance, and peer input does not expand the
operator's permissions.

The server owns execution. Closing a browser, event subscription, or submitting
command leaves work running. Reconnection receives a snapshot followed by live
events; slow event consumers receive a fresh snapshot. Session history currently
lives in server memory and grows with conversation length. Server shutdown or
failure ends sessions; disk persistence and restart recovery are not implemented.

The Responses client streams with `store: false`, explicit conversation input,
and `reasoning.summary: auto`. Completed encrypted reasoning returns as opaque
context. API failures are reported without automatic retries. Runtime guidance
is compiled from `src/PRINCIPLES.md`; parent/bootstrap discovery is not automatic.

## Headless routes

The browser and scripts use the same authenticated API:

| Route | Behavior |
| --- | --- |
| `GET /api/config` | Default working directory and model |
| `GET /api/sessions` | Session snapshots |
| `POST /api/sessions` | Create with optional `name`, `cwd`, `message` |
| `GET /api/sessions/{id}` | Inspect one session |
| `POST /api/sessions/{id}/messages` | Queue `{ "text": "..." }`; return a message ID |
| `POST /api/sessions/{id}/stop` | Request cancellation; later status confirms stopping |
| `GET /api/events` | SSE snapshot, entries, deltas, status |
| `GET /api/terminals` | Terminal identities and status |
| `POST /api/terminals` | Open with optional `cwd`, `cols`, `rows` |
| `GET /api/terminals/{id}/ws` | WebSocket terminal output, input, resize |
| `DELETE /api/terminals/{id}` | End and wait for the shell |

Agent behavior can be checked headlessly. Browser checks establish interface
behavior. Tool unit tests alone do not establish disconnect or coordination behavior.

## Interactive terminals

Bash PTYs and locally vendored xterm.js provide interactive programs, keyboard
input, resize, and reconnect. Disconnecting leaves a shell running; its Close
button ends it. Replay retains the latest 512 KiB and marks omitted older output.
Terminal input is an explicit operator shell action and bypasses the model's
command heuristic. Both terminal and model command children omit API/server
credentials from their environments. Deliberately detached processes still
require container cleanup.

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
build step. Pinned terminal assets and licenses are under `src/web/vendor/`.

On the Docker host, set `BOID_WORKSPACE` to the task directory. This uses the
existing compiled binary volume and publishes only on host loopback:

```sh
docker run --rm --init \
  --cap-drop=ALL --security-opt no-new-privileges \
  --pids-limit=256 --memory=4g --cpus=4 \
  -p 127.0.0.1:7734:7734 \
  -e OPENAI_API_KEY -e BOID_MODEL -e BOID_LISTEN=0.0.0.0:7734 \
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

Record unintended destructive actions even when containment prevents damage.
Preserve the command, target, and result; hand dangerous host/system work to
Codex and continue unaffected work.
