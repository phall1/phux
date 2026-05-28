---
audience: contributors, agents
stability: evolving
last-reviewed: 2026-05-27
---

# Testing strategy

**TL;DR.** Three layers: unit tests beside code, `proptest` for codec
roundtrips and state-machine invariants and replay equivalence, `insta`
snapshot tests for wire bytes and rendered TUI frames. Mutation testing
with `cargo-mutants` once the codebase is big enough to warrant it;
target is 90% on protocol and core.

---

Three layers:

1. **Unit tests** colocated with code. Standard.
2. **Property tests** (`proptest`) for:
   - Protocol codec roundtrip (encode → decode → equal).
   - State machine invariants (e.g. "after any sequence of Commands, the
     layout tree is well-formed").
   - Replay equivalence: for any PTY byte stream `bs`, writing `bs` to a
     fresh `Terminal` on the client reproduces the same visible grid as
     the server's `Terminal` saw, up to the documented downsampling
     rewrites. The snapshot-on-attach synthesis algorithm
     (research/2026-05-25-libghostty-renderstate.md §7) is checked the
     same way: synthesize, replay into a fresh `Terminal`, compare
     `RenderState` snapshots.
3. **Snapshot tests** (`insta`) for:
   - Wire bytes of representative messages, so accidental format changes
     are loud.
   - Rendered TUI frames (a CellGrid → ASCII art helper).

We will adopt `cargo-mutants` once the codebase is substantial. The bar:
mutation score above 90% on the protocol and core crates.
