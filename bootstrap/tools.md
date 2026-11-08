# CLI and tools

Run `boid` for an interactive conversation, or `boid 'task'` to start with a
request. Piped input runs to completion and exits. Set `OPENAI_API_KEY` in the
container environment. `BOID_MODEL` defaults to `gpt-6.1-sol`.

Text streams as it arrives. Tool calls and results appear between responses;
the terminal shortens long tool displays to 2,000 characters. The conversation
retains the tool's full bounded result. Terminal control characters are removed
from displayed text.

Type a new message to interrupt and steer ongoing work. `/stop` or Ctrl-C stops
the current turn; `/quit` exits. Ctrl-C while idle also exits. An interrupted
command may already have changed files. Its next tool result records that
uncertainty, and earlier completed results remain in the conversation. EOF
lets ongoing work finish. Conversation state lasts for this process; restart
begins a new conversation.

The client uses the [Responses API](https://developers.openai.com/api/docs/guides/function-calling)
with streaming, `store: false`, and explicit conversation input. Completed
reasoning items are carried back with their encrypted content. API failures
are reported without automatic retries. The compiled runtime guidance is
`src/PRINCIPLES.md`; the CLI does not discover parent instruction files.

## File and command behavior

The CLI calls the v3 library through six function tools: `read_file`,
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
CLI tool defaults to a 30-second timeout and accepts 1 to 300,000 milliseconds.
Child commands do not inherit `OPENAI_API_KEY`; the Boid process still holds
that credential. A command owns its process group. Its background
children end when it finishes, times out, or the caller drops the future.
Processes that deliberately leave the group require container-level cleanup.

The guard adapts the POSIX literal-command detector from Codex at `a90c808`.
It rejects recognized forced `rm` commands and excessive wrapper nesting.
Dynamic command names, script contents, and unsupported shell forms can escape
classification. Passing the guard does not establish safety. It has no
interactive approvals, policy-rule store, or Codex App Server dependency.

## Build and run

Develop on the local checkout. Run builds and execution on `mm-west-2` under
Colima Docker. Stage `Cargo.toml`, `Cargo.lock`, and `src/` in a disposable build
directory. Keep the recovery snapshot separate. Do not expose the host home or
Docker socket to the task process. Supply bootstrap guidance when relevant to
a development assignment, with the caution in [the brief](README.md).

Build the toolchain image using the Dockerfile on stdin, so the image receives
no repository context:

```sh
docker build -t boid-dev - < bootstrap/Dockerfile
```

With the staged directory mounted at `/work`, run `cargo fetch --locked` once
with registry network access. Then run `cargo test --locked --offline` and
`cargo build --locked --offline` with network disabled. Share the dependency
download cache, and keep build artifacts separate for each source workspace.

For a model run, allow access to the OpenAI API and supply the API key to the
container. On the Docker host, set `BOID_WORKSPACE` to the staged task directory
and `BOID_TARGET_VOLUME` to a target volume dedicated to that directory. This
command uses the existing compiled binary through a read-only mount:

```sh
docker run --rm -it --init \
  --cap-drop=ALL --security-opt no-new-privileges \
  --pids-limit=256 --memory=4g --cpus=4 \
  -e OPENAI_API_KEY -e BOID_MODEL -e CARGO_NET_OFFLINE=true -e CARGO_BUILD_JOBS=4 \
  -v "$BOID_WORKSPACE:/work" \
  -v boid-v3-cargo:/usr/local/cargo \
  -v boid-v3-target:/opt/boid:ro -v "$BOID_TARGET_VOLUME:/work/target" \
  boid-dev /opt/boid/debug/boid
```

For Boid to work on itself, expose a copy of `Cargo.toml`, `Cargo.lock`, and
`src/`, plus the relevant development guidance. Keep Git history outside the
runtime workspace unless needed and explicitly scoped for the task. Keep the
canonical checkout outside that mount, then review the diff and bring back the
intended changes through a PR. Supply only credentials and services needed for
the task. This launch recipe is the execution boundary; the binary itself does
not create or require a container.

Record any unintended destructive action, even when Docker prevents damage.
Preserve the command, target, and observed result. Hand dangerous host or
system work to Codex; do not retry it outside the task container.
