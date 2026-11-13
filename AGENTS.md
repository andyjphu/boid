# Developing Boid

Before working on Boid, read [bootstrap/README.md](bootstrap/README.md) and
[src/PRINCIPLES.md](src/PRINCIPLES.md). Developers need both the development
context and the behavior expected of Boid. Carry relevant instructions into
delegated work.

The [case bank](bootstrap/cases/README.md) is available to developers, including
Boid when working on itself. Read its index and retrieve cases relevant to the
task. Preserve the distinction between Andy's words, agent interpretation, and
observed outcomes. The [meta-review template proposal](bootstrap/meta-review.md)
records a future direction; it does not establish implemented template behavior.

This entry point and `bootstrap/` are for developers, including Boid when it
works on its own implementation. Read the prominent caution in the bootstrap
brief: development assumptions about failures must not become behavioral goals.
Select runtime guidance and developer context for the actual task; do not
automatically inject this whole brief or parent instructions into every session.

Speed is a core mandate: remove proven avoidable harness overhead between tools
and inference while preserving correct server behavior. Report legitimate
feature tradeoffs immediately. Read [the performance guide](docs/performance.md)
when investigating latency or changing admission, persistence, history, or
rendering; distinguish live measurements from synthetic benchmarks.
