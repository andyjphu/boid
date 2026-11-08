# Tool library

The v3 library supplies `Tools::read`, `write`, `edit`, `glob`, `search`, and
`exec`. The next agent loop can call these functions directly. There is no
tool-runner CLI or model connection in this change.

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

Commands run through Bash without startup profiles, with closed stdin and a
caller-supplied timeout. A command owns its process group. Its background
children end when it finishes, times out, or the caller drops the future.
Processes that deliberately leave the group require container-level cleanup.

The guard adapts the POSIX literal-command detector from Codex at `a90c808`.
It rejects recognized forced `rm` commands and excessive wrapper nesting.
Dynamic command names, script contents, and unsupported shell forms can escape
classification. Passing the guard does not establish safety. It has no
interactive approvals, policy-rule store, or Codex App Server dependency.

## Container checks

Develop on the local checkout. Run builds and execution on `mm-west-2` under
Colima Docker. Stage `Cargo.toml`, `Cargo.lock`, and `src/` in a disposable build
directory. Keep the recovery snapshot separate. Do not expose the host home,
Docker socket, Git history, or bootstrap guidance to the task process.

Build the toolchain image using the Dockerfile on stdin, so the image receives
no repository context:

```sh
docker build -t boid-dev - < bootstrap/Dockerfile
```

With the staged directory mounted at `/work`, run `cargo fetch --locked` once
with registry network access. Then run `cargo test --locked --offline` with
network disabled. Keep a Cargo cache and build cache between runs. The tests
exercise guard rejection before side effects, exact edits, output bounds,
search behavior, timeout, and cancellation.

Record any unintended destructive action, even when Docker prevents damage.
Preserve the command, target, and observed result. Hand dangerous host or
system work to Codex; do not retry it outside the task container.
