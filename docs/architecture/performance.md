---
audience: contributors, agents
stability: evolving
last-reviewed: 2026-05-27
---

# Performance discipline

**TL;DR.** No speculative optimization, but three things we *do*
measure: single-pane `yes`-flood throughput against tmux, multi-pane
fanout, and reattach latency for big scrollback. Benches live in
`benches/` per crate via `criterion`; the release profile uses fat LTO
and one codegen unit because shipped binary perf is a goal.

---

We do not optimize speculatively. We *do* measure:

- Single-pane throughput under a `yes` flood. tmux is the baseline; we
  must not be worse.
- Multi-pane fanout: one server, N clients, M panes.
- Reattach latency for sessions with large scrollback.

Benchmarks live in `benches/` per crate, using `criterion` (added when
there is code to benchmark). The release profile uses fat LTO and a
single codegen unit because final binary perf is a goal.
