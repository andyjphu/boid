# Boid

Boid is an agent swarm harness. It aims to make collaboration useful, preserve
work across context changes, and help agents learn from mistakes.

The goal is useful work, done quickly and with good judgment. Boid should
improve with experience and make effective use of agents working together.
[Performance profiling and the speed mandate](docs/performance.md) guide work
on harness overhead.

Boid's code and intended agent guidance belong in [src/](src/).
[bootstrap/](bootstrap/) holds temporary context for agents developing Boid.

Build and run from the canonical checkout at `~/boid` (Rust 1.89 or newer):

```sh
cd ~/boid
./build.sh
./run.sh
```

`build.sh` builds the release binary and links `~/.local/bin/boid` to it.
Keep `~/.local/bin` on `PATH`. `run.sh` simply executes `boid`, forwarding any
arguments; `boid --help` lists the options. Use `/login` to choose an API key or
[ChatGPT subscription](docs/chatgpt.md); an existing `OPENAI_API_KEY` also works. `boid` starts one background server or
reuses the running server, opens its web workspace, and returns to the shell.
Use `boid --no-open` to print the URL without opening a browser; SSH sessions
also print the URL only. Closing the shell or browser leaves agents running.

`boid stop` shuts down the server and active work while preserving saved
conversations. `boid serve` runs in the foreground for debugging. New agents
start in the server home directory with a random unused bird name; existing
agents keep their directories. Terminals retain the workspace directory selected
by the launcher or `BOID_CWD`. `boid "task"` also starts its agent at home.
State and background server logs live under `~/.local/share/boid` by default.
For browser access without copying a server token, use
[SSH-authorized passkey enrollment](docs/passkeys.md).

After a restart, idle agents return ready for peer messages without a greeting.
Explicitly stopped agents and interrupted or failed work remain paused until
operator input. Already accepted peer messages can run after recovery; simply
restoring an idle agent does not start a model request. `list_agents` reports
whether each open agent is listening. Browser reloads do not change availability.

Streaming text is saved through a bounded background journal queue. Writes
already waiting together can share a disk sync. A crash can lose the unfinished
stream tail that has not reached disk; completed responses, tool claims and
results, peer receipts, and shutdown still wait for their durable checkpoints.

Rapid composer submissions retain their order across pane and screen changes.
A draft clears only when the server acknowledges that exact draft revision.
Unconfirmed submissions remain available for manual review and retry after a
failure or reload; recovery storage does not block sending.

Agent panes retain the complete transcript and load recent history first.
[Earlier history loads on demand](docs/history.md); rendering follows the viewport.
Drag a pane's heading to reorder agents, terminals, and unfinished agent forms.
The insertion line shows the destination. Order and resized positions survive
refresh in that browser window; dragging a live pane heading onto a screen number
moves it to that screen.

![Boid v0.5 with a terminal and two agent panes](docs/images/boid-v0.5.png)

v0.5 preview · [PR #5](https://github.com/andyjphu/boid/pull/5).

In an agent composer, type `/` to select a slash command with the arrow keys and
Enter. `/clear` starts fresh model context while keeping the transcript, agent
identity, and configuration. `/oai-compact` directly runs OpenAI compaction,
using the same operation as the `oai_compact` tool. Both wait for the current
response and tool batch. Clearing makes no model request; queued messages then
use the fresh context. Previous context is saved privately in `sessions/clears/`.

Agents can call `prompt_user` for important questions. Blocking questions open
a reply panel and mark the agent pane in red. Other questions collect behind
the pane's Questions button, with a picker when several are open. Replies appear
in the conversation and reach the agent as user messages.

The agent sets `blocking` and `wait_seconds` (1–86400 seconds; null means five
minutes for a blocking question). Nonblocking questions use null and return
immediately. When a wait expires, the agent receives a timeout result and can
decide what authorized work to continue. The question remains answerable; silence
does not grant approval. Repeating the same open question reuses its ID and
original deadline. Stop and Escape end the wait, while retaining the question.
Questions survive reconnects and server restarts. Restarts preserve the usual
paused recovery behavior; they do not restart a question's timer or agent work.

`/login` changes the connection for its agent pane. `/model` selects a model
from the connected account's catalog.
`/clear` remains available; `/oai-compact` currently requires the API-key route.

### Measuring message admission

Browser input diagnostics are off by default. Open the page with
`?input-timing=1`, or call `boidInputTiming.enable()` in the browser console.
Send a normal message, then inspect
`boidInputTiming.samples()`. Use `boidInputTiming.clear()` to remove retained
samples and `boidInputTiming.disable()` to stop recording. Diagnostics retain
at most 64 samples in memory and do not retain message contents or write disk
logs. Correlation uses the server's message ID and local monotonic durations;
it does not subtract clocks from different machines.

Message responses retain their existing JSON and include `Server-Timing`
durations in milliseconds:

- `boid_lock`: waiting to acquire the server's session-state mutex.
- `boid_mutate`: work while that mutex is held, including history copies,
  mutation, persistence enqueueing, event publication, and worker notification.
- `boid_admit`: runtime method entry through mutex release and the return from
  the synchronous admission wrapper. This does not wait for background saving.
- `boid_handler`: HTTP handler entry after Axum extraction through response-body
  construction. It excludes authentication, request-body extraction, final
  timing-header formatting, socket flush, and client receipt.

Browser `submitToDispatchMs` includes any local wait behind an earlier explicit
submission. Browser submit-to-headers and submit-to-acknowledgment timings
include that wait; server metrics do not. These intervals overlap;
do not add them together. Handler-level failures also
include timing for stages reached. Authentication and extraction failures do
not reach this handler and have no admission metrics. Browser rendering timings
report DOM work, not when pixels became visible. No latency figure in the test
fixtures establishes production speed: inspect samples from actual messages to
measure the current browser and server.
