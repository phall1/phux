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

- PTY-byte-to-terminal feed and diff computation per pane. Each pane is
  independent; trivial to fan out via `spawn_blocking` or a dedicated
  worker thread per pane.
- Compression of large snapshots before transmission.

We do not parallelize on day one. Premature.

## Error model

Each library crate defines its own error type with `thiserror`. The binary
crate uses `anyhow` at the top level only — never inside library code.

```rust
// crates/phux-server/src/error.rs
#[derive(Debug, thiserror::Error)]
pub enum ServerError {
    #[error("protocol: {0}")]
    Protocol(#[from] phux_protocol::ProtocolError),
    #[error("pty: {0}")]
    Pty(#[from] PtyError),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    // ...
}
```

Errors that cross the IPC boundary are translated to `ERROR` messages
(`SPEC.md` §14) with a `code: ErrorCode` and a `message: str`. The mapping
from internal Rust errors to wire `ErrorCode` is the responsibility of the
server's IPC layer.

## Logging and observability

We use `tracing` for structured logging. Server logs go to
`~/.local/state/phux/log/server.log`, rotated daily, with a `tracing-appender`
file rolling writer. The filter is configured via:

1. Config file (`log_filter = "phux=info,phux_server=debug"`).
2. `PHUX_LOG` environment variable (overrides config).
3. Default: `phux=info`.

Spans we instrument by convention:

- `attach` (client_id, session_id) — wraps an attachment for its lifetime.
- `pane` (pane_id) — wraps PTY pump and diff emission per pane.
- `command` (request_id, kind) — wraps a `COMMAND` dispatch.

The server exposes a `phux server status --json` subcommand for runtime
introspection: number of sessions/windows/panes/clients, per-pane refresh
rate, per-client queue depth, total bytes since start. This becomes the
basis for any future Prometheus/OpenTelemetry exporter; we do not ship one.

## Module structure

A high-level sketch of each crate's intended layout. Concrete module names
will appear as code lands; the shape below should not surprise.

### `phux-protocol`

```
src/
  lib.rs              — re-exports, top-level docs, PROTOCOL_VERSION
  version.rs          — Version, VersionRange, negotiation helpers
  frame.rs            — frame header, length-prefix codec
  wire/               — Appendix A encoding primitives
    mod.rs
    varint.rs
    fields.rs         — field reader/writer, wire types
  msg/                — one module per top-level message
    mod.rs
    hello.rs
    attach.rs
    diff.rs           — PANE_DIFF, PANE_SNAPSHOT, DiffOp, Cell, ...
    input.rs          — INPUT_KEY, INPUT_PASTE, INPUT_MOUSE, INPUT_RAW
    layout.rs         — LayoutTree, LayoutNode
    command.rs        — Command, CommandResult
    event.rs          — events: BELL, OSC_EVENT, ALERT, FOCUS_CHANGED, ...
    error.rs          — wire ERROR message and ErrorCode enum
  caps.rs             — ClientCapabilities, ServerCapabilities, bitsets
  ids.rs              — SessionId, WindowId, PaneId, ClientId, FrameId
```

### `phux-core`

```
src/
  lib.rs              — re-exports
  session.rs          — Session, SessionId
  window.rs           — Window, WindowId
  pane.rs             — Pane (no PTY here; just metadata + state)
  layout/             — layout tree, resize algorithm
    mod.rs
    tree.rs
    resize.rs
  selector.rs         — parse "session:window.pane" / "@id" / "."
  config/             — typed config schema (deserialized in `phux` bin)
    mod.rs
    schema.rs
    defaults.rs
```

### `phux-server`

```
src/
  lib.rs              — `run()` entry point, runtime construction
  server.rs           — top-level Server struct, slotmaps, dispatch
  pty/                — PTY supervision
    mod.rs
    spawn.rs
    pump.rs           — read/write loops feeding into terminal
  terminal.rs         — wraps libghostty_vt::Terminal per pane
  diff/               — diff computation + emission pacing
    mod.rs
    compute.rs        — RenderState → DiffOp[]
    pacer.rs          — per-pane refresh-rate throttle
  ipc/                — IPC over Unix sockets
    mod.rs
    listener.rs
    connection.rs     — per-client connection state machine
    codec.rs          — protocol-layer framing using tokio_util
  client.rs           — Client struct (attached frontend)
  journal/            — crash-recovery journals
    mod.rs
    writer.rs
    replay.rs
  command.rs          — server-side Command handlers
  hooks.rs            — hook dispatch
  error.rs
```

### `phux-client`

```
src/
  lib.rs              — `run()` entry point for TUI client
  attach.rs           — handshake + ATTACH + initial snapshot
  state.rs            — client-side mirror of session/window/pane graph
  renderer/           — composes pane grids + chrome into outer screen
    mod.rs
    chrome.rs         — pane borders, status bar
    pane.rs           — per-pane rendering from diffs
    vt_out.rs         — emits VT to the outer terminal
  input/              — outer-terminal input → INPUT_KEY/MOUSE
    mod.rs
    kbd.rs            — parses KIP / fixterms / legacy
    mouse.rs
  status/             — status bar slot renderer
  keymap.rs           — config-bound action dispatch
  config.rs           — config consumed at startup
  error.rs
```

### `phux` (binary)

```
src/
  main.rs             — runtime construction, subcommand dispatch
  cli.rs              — clap subcommand definitions
  commands/
    mod.rs
    attach.rs
    new.rs
    list.rs
    kill.rs
    send.rs
    capture.rs
    server.rs
    config.rs
```

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
