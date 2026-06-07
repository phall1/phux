---
audience: contributors, agents
stability: evolving
last-reviewed: 2026-06-06
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

## Hot paths that could go multi-threaded later

If a future profile demands it, two paths can fan out without disturbing the
`!Send` engine on the main thread:

- PTY-byte feed and per-client capability rewriting on outbound terminal
  frames. Each terminal is independent and could move to `spawn_blocking` or
  a dedicated worker thread.
- Compression of large snapshot bodies before transmission.

Neither is parallelized today; the single-thread shape is sufficient at the
current scale.
