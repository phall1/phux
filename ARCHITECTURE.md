# Architecture

This document describes phux's internal structure: the process model, the
data model, threading, persistence, and testing strategy. The wire
protocol is described separately in [`SPEC.md`](./SPEC.md) — that is the
contract; this document is implementation guidance.

## Process model

One server per user, hosting all of that user's sessions. Clients are
separate processes that attach to the server over a Unix socket.

```
~/.local/state/phux/
├── socket               # SOCK_STREAM, perms 0600
├── server.pid
└── journal/             # per-pane PTY output (for crash recovery)
    └── <pane_id>.log
```

The single `phux` binary contains both server and client logic; the
subcommand dispatches. `phux server` runs the daemon in the foreground;
`phux` (no args) becomes a client and lazily spawns a server if none is
listening on the socket.

This mirrors tmux's "auto-server" convention because it is correct: users
should never have to think about a daemon.

## Crate dependency graph

```
  ┌──────────────────────────────────────┐
  │                phux                  │   (binary; subcommands)
  └──────┬─────────────────────────┬─────┘
         │                         │
    ┌────▼────┐               ┌────▼────┐
    │ server  │               │ client  │
    └────┬────┘               └────┬────┘
         │                         │
         └────────┐         ┌──────┘
                  │         │
              ┌───▼─────────▼───┐
              │      core       │   (Session, Window, Pane, Layout)
              └────────┬────────┘
                       │
                  ┌────▼─────┐
                  │ protocol │   (wire types, codec)
                  └──────────┘
```

`protocol` has no upward dependencies and never will. It is the foundation;
everything else conforms to it.

`server` depends on `libghostty-vt` (the safe Rust crate from
[libghostty-rs][libghostty-rs]). `client` does not.

[libghostty-rs]: https://github.com/Uzaaft/libghostty-rs

## Data model

The server is a graph of long-lived nodes with stable identity. We use
`SlotMap` (one per node type) rather than `Rc<RefCell<>>` because:

- Stable IDs are exactly what the wire protocol needs anyway
  (`SessionId`, `WindowId`, `PaneId`, `ClientId`).
- Cross-references (e.g. "this client's active pane") become `PaneId`,
  not borrowed references — no aliasing problem.
- Deletion is `O(1)` and doesn't dangle.

```rust
struct Server {
    sessions: SlotMap<SessionId, Session>,
    windows:  SlotMap<WindowId,  Window>,
    panes:    SlotMap<PaneId,    Pane>,
    clients:  SlotMap<ClientId,  Client>,

    by_session_name: HashMap<String, SessionId>,
}

struct Session {
    id: SessionId,
    name: String,
    windows: Vec<WindowId>,
    active_window: WindowId,
}

struct Window {
    id: WindowId,
    session: SessionId,
    name: String,
    layout: LayoutTree,        // tree of splits with PaneId leaves
}

struct Pane {
    id: PaneId,
    window: WindowId,
    pty: PtyHandle,
    terminal: libghostty_vt::Terminal,
    last_emitted_frame: FrameId,
    // ...
}

struct Client {
    id: ClientId,
    socket: ConnectionHandle,
    attached_to: Option<SessionId>,
    viewport_size: (u16, u16),
    capabilities: ClientCapabilities,
    last_acked_frame: HashMap<PaneId, FrameId>,
}
```

## Threading and I/O

**One async runtime, single-threaded by default.** A terminal multiplexer
is I/O-bound, not CPU-bound; the work is poll-many-fds-fanout-bytes.
A single-threaded executor (or a hand-rolled `mio` loop) is simpler and
plenty fast.

We will most likely use `tokio` with `current_thread` flavor. If profiling
ever shows otherwise, we revisit.

Hot paths that *can* go multi-threaded if needed:

- PTY-byte-to-terminal feed and diff computation per pane. Each pane is
  independent; trivial to fan out.
- Compression of large snapshots before transmission.

We do not parallelize on day one. Premature.

## State replay & crash recovery

The server journals raw PTY output to disk, per pane, in `journal/<pane_id>.log`.
Journals are append-only, fsync'd on close, and capped (default: 10 MB
ring per pane).

On startup, if `server.pid` is stale, the server can be invoked with
`--recover`. It reads each journal, replays it into a fresh
`libghostty_vt::Terminal`, and reconstitutes sessions from a metadata
file alongside the journals.

Crash recovery is therefore a property of the design, not an
add-on. tmux loses everything on a daemon crash; phux does not.

## Testing strategy

Three layers:

1. **Unit tests** colocated with code. Standard.
2. **Property tests** (`proptest`) for:
   - Protocol codec roundtrip (encode → decode → equal).
   - State machine invariants (e.g. "after any sequence of Commands, the
     layout tree is well-formed").
   - Diff correctness: applying diffs to a snapshot reproduces the next
     snapshot.
3. **Snapshot tests** (`insta`) for:
   - Wire bytes of representative messages, so accidental format changes
     are loud.
   - Rendered TUI frames (a CellGrid → ASCII art helper).

We will adopt `cargo-mutants` once the codebase is substantial. The bar:
mutation score above 90% on the protocol and core crates.

## Performance discipline

We do not optimize speculatively. We *do* measure:

- Single-pane throughput under a `yes` flood. tmux is the baseline; we
  must not be worse.
- Multi-pane fanout: one server, N clients, M panes.
- Reattach latency for sessions with large scrollback.

Benchmarks live in `benches/` per crate, using `criterion` (added when
there is code to benchmark). The release profile uses fat LTO and a
single codegen unit because final binary perf is a goal.

## Security model

Trust boundary: the operating system user. A phux server trusts every
process running as the same UID that can connect to its Unix socket.

- Local: Unix socket at `~/.local/state/phux/socket` mode 0600.
- Remote: SSH-tunneled `phux serve --stdio` over `ssh host`. Auth is
  SSH's problem.

phux itself does no authentication and no encryption. Crypto in
multiplexers is a tarpit; we delegate.

## When this document is wrong

Code is the implementation; this document is the *intended* design. If
they diverge, file an issue. Either the code drifted or the design did.
Both happen; the response is to reconcile, not to let either rot.
