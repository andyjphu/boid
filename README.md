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
environment before starting it. The server prints its browser URL on startup.

![Boid v0.5 with a terminal and two agent panes](docs/images/boid-v0.5.png)

v0.5 preview · [PR #5](https://github.com/andyjphu/boid/pull/5).
