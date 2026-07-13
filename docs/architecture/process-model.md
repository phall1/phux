---
audience: contributors, agents
stability: evolving
last-reviewed: 2026-06-06
---

# Process model

**TL;DR.** One server per user hosts every session for that user; clients
are separate processes attached over a Unix socket. The single `phux`
binary contains both halves and dispatches by subcommand; an attach
auto-spawns a server if none is listening. Runtime paths live under
`$XDG_RUNTIME_DIR/phux/`, with a per-user state directory still on the
roadmap.

---

The runtime path resolution lives in
[`phux-server/src/runtime/mod.rs`](../../crates/phux-server/src/runtime/mod.rs): the
socket is `$XDG_RUNTIME_DIR/phux/phux.sock` when that variable is set,
otherwise `/tmp/phux-$UID/phux.sock`. The parent directory is created
mode `0o700`.

The persistent per-user state directory below is **design intent, not
yet implemented**. Today the server keeps state only in memory; logs go
to stderr by default; journaling and crash recovery have not landed.

```
$XDG_RUNTIME_DIR/phux/phux.sock     # SOCK_STREAM, perms 0o700 dir
$XDG_STATE_HOME/phux/               # NOT YET IMPLEMENTED
├── server.pid
├── log/
│   └── server.log
└── journal/                        # per-pane PTY output (crash recovery)
    └── <pane_id>.log
```

The single `phux` binary contains both server and client logic; the
subcommand dispatches. `phux server` runs the daemon in the foreground;
`phux` (no args) becomes a client and lazily spawns a server if none is
listening on the socket. The auto-spawn follows tmux's convention so a
user never has to start a daemon by hand.

Inside the server, a PTY-backed terminal actor now runs **two** independent
timers on its `select!`: the state-sync tick that paces output emission to its
consumers, and a second, slower agent-state detector tick
([ADR-0046](../../ADR/0046-server-side-agent-state-detection.md)) that
re-derives the pane's `phux.agent/v1` record from the PTY's foreground process,
the OSC title, and the live screen. The detector timer is the sole driver of
that work — PTY bytes never wake it — so a chatty pane costs no extra
detection. It is constructed only for a PTY-backed actor, only when a rule set
loaded, and it publishes through its own `mpsc` channel to a per-terminal drain
task that owns the metadata write. No new process, no new thread.
