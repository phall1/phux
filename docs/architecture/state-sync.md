---
audience: contributors, agents
stability: evolving
last-reviewed: 2026-06-06
---

# State synchronization

**TL;DR.** How an attaching client catches up to a terminal that already has
history. The server synthesizes a snapshot of current engine state, the
client replays that snapshot into a fresh libghostty `Terminal`, and from
then on both ends stay in step by replaying the same PTY byte stream. A
separate on-disk journal for crash recovery is designed but not built; that
half is marked as such below.

## Lazy state synchronization (built)

Both ends of the wire run libghostty, and the wire carries opaque terminal
bytes rather than a re-encoded grid
([ADR-0013](../../ADR/0013-libghostty-bytes-on-wire.md)). Live output is
therefore just forwarded: the server writes PTY bytes to its `Terminal` and
relays the same bytes, and each client writes them into its own `Terminal`.
Two engines fed identical bytes converge on identical state.

The interesting case is attach, when a client joins a terminal that already
has scrollback and screen contents it never saw. phux handles this lazily, as
specified by [ADR-0018](../../ADR/0018-lazy-state-synchronization.md): rather
than journaling every byte for the lifetime of the terminal, the server
synthesizes a snapshot of the current engine state on demand and sends it
once, at attach time.

The flow on attach:

1. The server reads the current `RenderState` of the terminal's `Terminal`
   and synthesizes a byte sequence that, when replayed into a fresh engine,
   reproduces that state. The synthesis algorithm is described in
   [`research/2026-05-25-libghostty-renderstate.md`](../../research/2026-05-25-libghostty-renderstate.md)
   §7.
2. The client writes that snapshot into its fresh `libghostty_vt::Terminal`.
   The client now holds a faithful mirror of the screen and scrollback as of
   the snapshot point.
3. Live PTY bytes that arrive after the snapshot are written into the same
   `Terminal`, keeping it in step with the server from then on.

Because the snapshot is replayed through the same engine that produced it,
the client's grid matches the server's, up to the documented downsampling
rewrites. That equivalence is a property test, not an assumption; see
[`verification.md`](./verification.md) for how snapshot-on-attach replay is
checked.

This keeps synchronization cheap in the common case: a terminal nobody
attaches to costs nothing beyond its own engine state, and an attach pays a
single snapshot rather than a replay of the terminal's entire history.

## On-disk journal and crash recovery (designed, not built)

> **Status: design intent, not implemented as of 2026-06-06.** Nothing in the
> server writes to disk today. `server.pid`, per-terminal journals, and a
> `--recover` flag do not exist. The notes below describe an intended shape,
> not current behavior.

The same bytes-on-wire shape that makes attach a snapshot-and-replay also
makes crash recovery mechanical, because the PTY byte stream forwarded to
clients is exactly what a journal would need to record. The designed shape:

- The server journals raw PTY output to disk, per terminal, in an
  append-only log. Logs are fsync'd on close and capped (a ring buffer, on
  the order of 10 MB per terminal in the current sketch).
- On startup, if a prior `server.pid` is stale, the server can be invoked
  with `--recover`. It reads each journal, replays it into a fresh
  `libghostty_vt::Terminal`, and reconstitutes grouping from a metadata file
  stored alongside the journals.

In this design crash recovery falls out of the replay model rather than being
a bolt-on: recovery is the attach snapshot path pointed at a journal instead
of a live engine. It is recorded here as a direction; when it is built this
section moves out of the designed-not-built block.
