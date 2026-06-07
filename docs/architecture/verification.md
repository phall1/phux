---
audience: contributors, agents
stability: evolving
last-reviewed: 2026-06-06
---

# Quality bar: testing and performance

**TL;DR.** The quality bar for phux has two halves. Tests run in three
layers (unit tests beside code, property tests for codec and state-machine
invariants, snapshot tests for wire bytes and rendered frames). Performance
is measured, not guessed: a small fixed set of throughput, fanout, and
reattach numbers, plus a release profile tuned for shipped-binary speed. Most
of this describes the intended bar; benches and mutation testing arrive with
the code that needs them.

## Test strategy

Tests are organized in three layers.

1. **Unit tests** colocated with the code they cover.

2. **Property tests** (`proptest`) for invariants that should hold across
   arbitrary inputs:
   - Protocol codec roundtrip: encode, decode, and the result equals the
     input. This is the codec's primary safety net.
   - State-machine invariants, for example that after any sequence of
     commands the layout tree stays well-formed.
   - Replay equivalence: for any PTY byte stream, writing those bytes into a
     fresh `Terminal` on the client reproduces the same visible grid the
     server's `Terminal` saw, up to the documented downsampling rewrites. The
     snapshot-on-attach synthesis algorithm is checked the same way:
     synthesize, replay into a fresh `Terminal`, and compare the resulting
     `RenderState` snapshots. See
     [`state-sync.md`](./state-sync.md) for the synchronization model this
     verifies and
     [`research/2026-05-25-libghostty-renderstate.md`](../../research/2026-05-25-libghostty-renderstate.md)
     §7 for the synthesis algorithm.

3. **Snapshot tests** (`insta`) for outputs that should change only on
   purpose:
   - Wire bytes of representative messages, so an accidental format change is
     loud rather than silent.
   - Rendered TUI frames, via a cell-grid to ASCII-art helper.

Mutation testing with `cargo-mutants` is planned for once the codebase is
substantial enough to warrant it; the intended bar is a mutation score above
90% on the protocol and core crates. It is not running today.

## Performance

phux does not optimize speculatively. A fixed set of numbers is measured so
regressions are visible:

- Single-pane throughput under a `yes` flood, with tmux as the reference
  point.
- Multi-pane fanout: one server, N clients, M panes.
- Reattach latency for sessions with large scrollback.

Benchmarks live in `benches/` per crate using `criterion`, added as the code
under measurement lands. The release profile uses fat LTO and a single
codegen unit, since the speed of the shipped binary is a goal in its own
right.
