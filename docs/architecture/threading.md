---
audience: contributors, agents
stability: evolving
last-reviewed: 2026-05-27
---

# Threading and I/O

**TL;DR.** Why phux uses a single-threaded tokio runtime, how
libghostty's `!Send` Terminal lives on a LocalSet, and the actor pattern
that lets us spawn one of them per managed PTY without contention. A
multiplexer is I/O-bound; work-stealing buys nothing on the hot path.

---

**One `tokio` current-thread runtime.** A terminal multiplexer is I/O-bound,
not CPU-bound; the work is poll-many-fds-fanout-bytes. A single-threaded
executor is simpler and plenty fast. We pick tokio specifically over `mio`
or `polling` because the ecosystem we need (tokio-uds for Unix sockets,
signal-hook-tokio for signals, tokio-util frame codecs) is mature and not
worth reinventing. We do not use the multi-threaded runtime: nothing in the
server's hot path benefits from work-stealing.

```rust
fn main() -> std::io::Result<()> {
    tokio::runtime::Builder::new_current_thread()
        .enable_io()
        .enable_time()
        .build()?
        .block_on(phux_server::run())
}
```

Hot paths that *can* go multi-threaded later if needed:

- PTY-byte feed to the canonical `Terminal` per pane plus per-client
  capability rewriting on outbound `PANE_OUTPUT` frames. Each pane is
  independent; trivial to fan out via `spawn_blocking` or a dedicated
  worker thread per pane.
- Compression of large `PANE_SNAPSHOT` bodies before transmission.

We do not parallelize on day one. Premature.
