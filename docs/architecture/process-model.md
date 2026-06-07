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
