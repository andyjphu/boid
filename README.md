# Boid

Boid is an agent swarm harness. It aims to make collaboration useful, preserve
work across context changes, and help agents learn from mistakes.

The goal is useful work, done quickly and with good judgment. Boid should
improve with experience and make effective use of agents working together.

Boid's code and intended agent guidance belong in [src/](src/).
[bootstrap/](bootstrap/) holds temporary context for agents developing Boid.

Build and run from the canonical checkout at `~/boid` (Rust 1.88 or newer):

```sh
cd ~/boid
./build.sh
./run.sh
```

`build.sh` builds the release binary and links `~/.local/bin/boid` to it.
Keep `~/.local/bin` on `PATH`. `run.sh` simply executes `boid`, forwarding any
arguments; `boid --help` lists the options. Set `OPENAI_API_KEY` in the server's
environment before the first launch. `boid` starts one background server or
reuses the running server, opens its web workspace, and returns to the shell.
Use `boid --no-open` to print the URL without opening a browser; SSH sessions
also print the URL only. Closing the shell or browser leaves agents running.

`boid stop` shuts down the server and active work while preserving saved
conversations. `boid serve` runs in the foreground for debugging. New panes use
the directory where `boid` was invoked; existing agents keep their directories.
State and background server logs live under `~/.local/share/boid` by default.

Agent panes initially render the latest 200 entries or approximately 100,000
characters, retaining whole entries and paired tool calls. Use **Show older
entries** to load more and **Latest** to return to the bounded view. Culling
pauses while reading earlier text or selecting it. The full saved transcript
and the agent's context are unchanged. This is a temporary rendering hotfix;
full transcript virtualization remains v2 work.

![Boid v0.5 with a terminal and two agent panes](docs/images/boid-v0.5.png)

v0.5 preview · [PR #5](https://github.com/andyjphu/boid/pull/5).
