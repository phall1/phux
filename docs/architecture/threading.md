---
audience: contributors, agents
stability: evolving
last-reviewed: 2026-07-11
---

# Threading and I/O

**TL;DR.** Why the server runs on a single current-thread tokio runtime with
a LocalSet (libghostty's `Terminal` is `!Send`, so it cannot move across
threads), and why server state nonetheless lives behind an
`Arc<Mutex<ServerState>>` using `std::sync` — the lock is never held across
an await, so cross-task, test, and embed paths can share it without a
multi-threaded runtime. A multiplexer is I/O-bound; work-stealing buys
nothing on the hot path.

---

## One current-thread runtime with a LocalSet

A terminal multiplexer is I/O-bound, not CPU-bound: the work is
poll-many-fds-fanout-bytes. A single-threaded executor is simpler and fast
enough. We pick tokio over `mio` or `polling` because the ecosystem we need
(tokio-uds for Unix sockets, signal-hook-tokio for signals, tokio-util frame
codecs) is mature and not worth reinventing. The hot path gains nothing from
work-stealing, so the server does not use the multi-threaded runtime.

The current-thread choice is not only a performance call — it is forced by
the engine. libghostty's `Terminal` is `!Send`: it cannot move across
threads, so the tasks that feed and read it run on a `LocalSet` pinned to the
runtime thread. A multi-threaded runtime would refuse to spawn those tasks
at all.

```rust
fn main() -> std::io::Result<()> {
    tokio::runtime::Builder::new_current_thread()
        .enable_io()
        .enable_time()
        .build()?
        .block_on(phux_server::run())
}
```

## Shared state behind a std::sync Mutex

Server state lives behind an `Arc<Mutex<ServerState>>` from `std::sync` (not
`tokio::sync`). Both facts hold at once and do not contradict: the runtime is
single-threaded for the `!Send` engine, and the state is still wrapped in a
mutex so that multiple tasks — plus the test and embed paths that drive the
server in-process — share one consistent view.

The rule that makes a synchronous mutex safe on an async runtime is that the
lock is **never held across an `.await`**. Each acquisition is a short
critical section: take the lock, read or mutate `ServerState`, drop the lock,
then await any I/O. Holding a `std::sync::Mutex` across a yield point would
risk deadlocking the single thread; the discipline of dropping it first is
what keeps that from happening and what lets group operations such as
`KILL_TERMINALS` apply all-or-nothing under a single acquisition.

This reconciles the earlier server-design sketch, which described the state
as actor-owned: the shared-mutex shape is the one that ships, and it coexists
with the current-thread/LocalSet runtime rather than competing with it.

Because the state is shared with the input lane below, `ServerState` must be
`Send` (so `Arc<Mutex<ServerState>>` is `Send`). That is a real constraint on
what may live in it: message types reachable from a `TerminalHandle` cannot
carry `!Send` payloads (a raw pointer, an `Rc`). The event-unsubscribe request
identifies a subscriber by a `usize` address rather than a
`*const Sender<Outbound>` for exactly this reason.

## The dedicated input lane (ADR-0044)

Input **routing** — resolve the wire pane id, walk the subscription set, check
the input lease (ADR-0033), `try_send` onto the pane actor's mailbox — runs on
its own OS thread, the input lane, not on the LocalSet. Routing touches only
`Send` state (`Arc<Mutex<ServerState>>` and the pane mailbox sender); it never
references the `!Send` `Terminal`, so it can gate and deliver a keystroke on a
second core in parallel with an output-broadcast tick draining on the main
thread. This is the root fix for input/output contention: the actor's fair
`select!` (input biased ahead of output; see below) makes the actor service a
mailboxed keystroke promptly, and the lane makes that mailbox fill without
waiting for the main thread to yield.

The lane is a plain thread with a bounded channel and `blocking_recv`, not a
second tokio runtime — routing is synchronous (a non-blocking `try_send`, no
await), so no runtime is needed. Per-client input order is preserved (read loop
enqueues in wire order; channel, lane, and mailbox are all FIFO), and lease
exclusion is unchanged because the lane runs the same routing function under the
same `Mutex`. Input **encode** (wire event → PTY bytes) reads the pane's live
DEC-mode state and stays on the actor thread.

## Hot paths that could go multi-threaded later

The input lane above is the first realized fan-out off the main thread. Others
can follow the same rule — cross only `Send` state, leave the `!Send` engine
put — if a future profile demands it:

- Moving input **encode** onto the lane, once a `Send` snapshot of the pane's
  encoder-relevant DEC modes is published from the actor (deferred, ADR-0044).
- PTY-byte feed and per-client capability rewriting on outbound terminal
  frames. Each terminal is independent and could move to `spawn_blocking` or
  a dedicated worker thread.
- Compression of large snapshot bodies before transmission.

None but the input lane is parallelized today; the single-thread shape is
sufficient for the rest at the current scale.
