# Architecture

> **Updated 2026-05-25 for [ADR-0013](./ADR/0013-libghostty-bytes-on-wire.md).**
> Pane content moved from a structured cell-level diff to VT bytes on the
> wire. Input remains structured. Both server and client now run
> `libghostty_vt::Terminal`. References to `DiffOp`, `DiffMirror`, and
> "diff stream / diff emission" in this document have been replaced with
> their byte-forwarding equivalents; obsolete modules (`phux-protocol::diff`,
> `phux-client::mirror`) are scheduled for deletion in the Wave 2 refactor
> and are not described here.

This document describes phux's internal structure: the process model, the
data model, threading, persistence, and testing strategy. The wire
protocol is described separately in [`SPEC.md`](./SPEC.md) — that is the
contract; this document is implementation guidance.

## Process model

One server per user, hosting all of that user's sessions. Clients are
separate processes that attach to the server over a Unix socket.

The runtime path resolution lives in
[`phux-server/src/runtime.rs`](./crates/phux-server/src/runtime.rs): the
socket is `$XDG_RUNTIME_DIR/phux/phux.sock` when that variable is set,
otherwise `/tmp/phux-$UID/phux.sock`. The parent directory is created
mode `0o700`. Persistent per-user state (logs, journals) lives under
`$XDG_STATE_HOME/phux/` (default `~/.local/state/phux/`):

```
$XDG_RUNTIME_DIR/phux/phux.sock     # SOCK_STREAM, perms 0o700 dir
$XDG_STATE_HOME/phux/
├── server.pid
├── log/
│   └── server.log
└── journal/                        # per-pane PTY output (crash recovery)
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
                ┌──────────────────────────────────┐
                │              phux                │  binary; subcommands
                └─┬──────────┬───────────┬─────────┘
                  │          │           │
            ┌─────▼───┐  ┌───▼────┐  ┌───▼────┐
            │ server  │  │ client │  │ config │
            └──┬────┬─┘  └─┬──┬───┘  └────────┘
               │    │      │  │
               │    │      │  └─────────────────┐
       ┌───────▼─┐  │   ┌──▼────────────┐       │
       │  core   │  └──►│   protocol    │──► libghostty-vt ◄┘
       └─────────┘      │ (codec, input │       (client also links;
                        │  events, wire │        runs a local Terminal
                        │  envelopes)   │        per attached pane —
                        └───────────────┘        ADR-0013)
```

Two boundaries are load-bearing:

1. **`phux-core` and `phux-protocol` do not depend on each other.** Core
   holds the in-process domain (slotmap keys with generational tags,
   layout tree, registry). Protocol holds the wire shape (`u32`-wide
   IDs, length-prefixed TLV, libghostty-derived input/style atoms).
   The two ID spaces meet in `phux-server::id_bridge::IdBridge` and
   nowhere else; this isolates wire stability from in-process
   refactors and vice versa. See ADR-0011 for the full rationale.
2. **`phux-protocol` depends on `libghostty-vt` directly** (ADR-0008,
   gated by the `server` cargo feature). The protocol crate re-exports
   libghostty's input and style atoms instead of mirroring them. The
   default-features-off shell exists so `crates.io`/`docs.rs` see a
   git-dep-free surface (libghostty-vt is a git-only dep today).

`server` and `client` both depend on `protocol`. Both also depend on
`libghostty-vt` directly: the server's `Terminal` is the canonical
state for each pane and drives the structured-input encoders
(ADR-0006, ADR-0008); the client's `Terminal` is a local replica fed
by `PANE_OUTPUT` bytes for the panes that client has attached, with
`RenderState` providing per-row dirty tracking for efficient redraw.
See ADR-0013 and `research/2026-05-25-libghostty-renderstate.md` for
the renderer-side contract on both ends.

`phux-config` is a sibling of `core` and is consumed by the binary and
the client.

## Wire protocol: bytes on the wire (ADR-0013)

The protocol is asymmetric. Server-to-client *pane content* is a
stream of VT bytes (`PANE_OUTPUT { pane_id, seq, bytes }`); the
server forwards what the PTY emitted, after a per-client capability
rewrite. Client-to-server *input* is structured (`INPUT_KEY`,
`INPUT_MOUSE`, `INPUT_FOCUS`, `INPUT_PASTE`, `INPUT_RAW`), built from
libghostty's input atoms per ADR-0006 / ADR-0008. Session/window/pane
lifecycle and commands stay structured — the session graph is phux's
vocabulary, not libghostty's. See [SPEC.md](./SPEC.md) §8 for the
wire shape and ADR-0013 for the rationale.

The shape follows libghostty's interface: `Terminal::vt_write(&[u8])`
is the **only** way to feed grid content into a `Terminal`, and
structured readout (`grid_ref()`, `mode()`, `cursor_*`, `RenderState`)
is the only way to draw from one. Carrying bytes on the wire means
each end can keep a `Terminal` as its source of truth and the
protocol stops mirroring libghostty's grid model in a parallel
structure. Per-client capability downsampling moves from a per-cell
operation to a server-side VT byte-stream rewriter (SGR rewriting for
truecolor → 256-color → 16-color, OSC 8 stripping, image-protocol
gating, kitty-keyboard gating) sitting between the canonical PTY
stream and each subscribed client's send queue.

## Data model

The server is a graph of long-lived nodes with stable identity. The
domain (`phux-core`) uses one `SlotMap` per node type rather than
`Rc<RefCell<>>` because:

- Stable IDs are exactly what the wire protocol needs anyway
  (`SessionId`, `WindowId`, `PaneId`).
- Cross-references ("this client's active pane") become `PaneId`, not
  borrowed references — no aliasing problem.
- Deletion is `O(1)` and slotmap's generational keys catch
  use-after-free in tests.

The shape splits in two: the *domain* (pure data, in `phux-core`) and
the *attached-client + I/O* state (in `phux-server`). The split is
deliberate; see ADR-0008 and the crate-graph note above.

```rust
// phux-core::registry::Registry — domain only, no I/O.
pub struct Registry {
    sessions: SlotMap<SessionId, Session>,
    windows:  SlotMap<WindowId,  Window>,
    panes:    SlotMap<PaneId,    Pane>,
}

pub struct Session  { id, name, windows: Vec<WindowId>, active: Option<WindowId> }
pub struct Window   { id, session, panes: Vec<PaneId>, layout: Option<LayoutNode>, active: Option<PaneId> }
pub struct Pane     { id, window, dims, cwd, title }
// LayoutNode is a binary split tree of PaneId leaves; see ADR-0010 and
// `phux-core::window`. TABBED is reserved (SPEC §10.3) and absent.
```

The PTY handle and `libghostty_vt::Terminal` for a pane are NOT fields
of `Pane`. They are server-side concerns and will hang off `PaneId` in
side tables in `phux-server` once PTY supervision lands. Keeping `Pane`
free of I/O is what lets `phux-core` stay `forbid(unsafe_code)` and
ship without an async runtime.

```rust
// phux-server::state::ServerState — domain + clients + I/O.
pub struct ServerState {
    pub registry:        Registry,
    pub attached:        HashMap<ClientId, AttachedClient>,
    pub pane_subscribers: HashMap<PaneId, Vec<ClientId>>,
    // Per-pane input log; merge point for multi-client keystrokes.
    pane_inputs:         HashMap<PaneId, Vec<PaneInput>>,
    // Core SessionId (slotmap key, generational) <-> wire SessionId (u32).
    pub session_id_bridge: IdBridge,
    next_client_id:      u64,
}

pub struct AttachedClient {
    pub id:      ClientId,           // server-assigned, monotonic
    pub session: phux_core::SessionId,
    pub tx:      tokio::sync::mpsc::Sender<OutboundFrame>,
}
```

Session name lookup goes through `Registry::sessions()` rather than a
side index — it is O(N) in session count, which is fine: session count
is small (single digits typical, double digits worst-case) and the
extra index would have to be kept consistent across cascading deletes.

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

- PTY-byte feed to the canonical `Terminal` per pane plus per-client
  capability rewriting on outbound `PANE_OUTPUT` frames. Each pane is
  independent; trivial to fan out via `spawn_blocking` or a dedicated
  worker thread per pane.
- Compression of large `PANE_SNAPSHOT` bodies before transmission.

We do not parallelize on day one. Premature.

## Transport abstraction

The wire codec sits behind an `async Transport` trait on both server and
client. v0.1 ships exactly one implementation — `UnixSocketTransport` —
but no domain module in `phux-server` or `phux-client` names a concrete
transport type. All I/O goes through the trait.

This is a load-bearing invariant for ADR-0007 (Mosh-class transport and
satellite forward-compat). It exists to keep two v0.2+ features purely
additive:

- **QUIC transport** (via `quinn`) — provides connection migration,
  0-RTT resumption, and TLS, giving us the UX properties of Mosh
  without reimplementing SSP.
- **SSH-stdio transport** — frames the wire codec over a child SSH
  process's stdin/stdout, used by hub servers to reach satellites over
  existing SSH paths.

Predictive local echo (the Mosh property users actually feel) is
implemented in `phux-client` against the client's local `Terminal`,
not in the transport. The client speculatively `vt_write`s its own
keystrokes into a side `Terminal` (or applies a small overlay on top
of the canonical replica) and reconciles on `FRAME_ACK` (SPEC §6),
when the server's bytes for those keystrokes arrive. It works over
any transport — including the Unix socket. Treating it as a client
feature rather than a transport feature is deliberate: shipping it
in v0.1 unlocks the most visible Mosh-class UX without waiting for
QUIC.

See ADR-0007 for the full forward-compat invariants (URI-shaped
session IDs, hub-and-spoke satellite topology, per-pane encoder
isolation).

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
- `pane` (pane_id) — wraps PTY pump and `PANE_OUTPUT` fanout per pane.
- `command` (request_id, kind) — wraps a `COMMAND` dispatch.

The server exposes a `phux server status --json` subcommand for runtime
introspection: number of sessions/windows/panes/clients, per-pane refresh
rate, per-client queue depth, total bytes since start. This becomes the
basis for any future Prometheus/OpenTelemetry exporter; we do not ship one.

## Module structure

What is in tree today. New modules land in the shape that fits the
crate; do not retrofit older layouts onto new work.

### `phux-protocol`

```
src/
  lib.rs              — re-exports, top-level docs, PROTOCOL_VERSION
  ids.rs              — SessionId, WindowId, PaneId, ClientId, FrameId
  input/              — INPUT_* event types (SPEC §9)
    key.rs, mouse.rs, focus.rs, paste.rs, mod.rs
  wire/               — TLV codec (SPEC Appendix A)
    frame.rs          — FrameKind + length-prefix framing
    encode.rs, decode.rs, field.rs, info.rs, error.rs
```

The `input` and `wire` modules are gated behind the `server` cargo
feature so the no-feature shell compiles without `libghostty-vt`.
See `lib.rs` for the docs.rs / crates.io rationale.

**Pending removal (Wave 2 refactor under ADR-0013):** the `diff/`
module (`cell.rs`, `grid.rs`, `op.rs`, `compute.rs`) and
`wire/diff.rs` are still in tree at the time of writing but no longer
describe the wire — `PANE_OUTPUT` carries raw VT bytes. They will be
deleted alongside the client-side `mirror/` module in the refactor
that lands the bytes-on-wire shape. Do not build new work on top of
them.

### `phux-core`

```
src/
  lib.rs              — re-exports
  ids.rs              — typed slotmap keys
  registry.rs         — Registry: SlotMaps + cascading deletes
  session.rs          — Session
  window.rs           — Window + binary split-tree LayoutNode
  pane.rs             — Pane (pure metadata; no PTY, no Terminal)
```

No `selector.rs` or `config/` here yet — selectors are unimplemented;
config lives in its own crate.

### `phux-server`

```
src/
  lib.rs              — re-exports
  runtime.rs          — tokio current-thread + UDS listener + accept loop
  state.rs            — ServerState, SharedState, attach/detach, subscribers
  id_bridge.rs        — core SessionId <-> wire SessionId (u32)
  grid.rs             — legacy: captures phux_protocol::Grid via RenderState;
                        slated for replacement by a snapshot-bytes synthesizer
                        (see research/2026-05-25-libghostty-renderstate.md §7)
  input/              — server-side encoders bridging wire input -> PTY bytes
    key.rs, mouse.rs, focus.rs, paste.rs, mod.rs
examples/
  one_pane.rs         — PTY child -> Terminal -> PANE_OUTPUT (will be rewritten
                        in the bytes-on-wire refactor; legacy diff stream today)
  diff_spike.rs       — legacy codec smoke; removed in the refactor
benches/capture.rs    — capture iterator throughput
```

No `pty/`, `journal/`, `command.rs`, or `hooks.rs` yet — these are
future work; their absence here is intentional, not drift.

### `phux-client`

Under ADR-0013 the client owns a `libghostty_vt::Terminal` per
attached pane and a `RenderState` per pane for incremental redraw.
The existing `mirror/` module (a hand-rolled Grid that applied
`DiffOp`s) is **legacy** — slated for deletion in the Wave 2
refactor. The new layout will be:

```
src/
  lib.rs              — re-exports
  pane/               — per-pane Terminal + RenderState bookkeeping (pending)
  render/             — RenderState-driven incremental redraw (pending)
  attach/             — frame loop, ATTACHED handling, predictive echo (partial)
```

Today's tree still contains `mirror/` (legacy) and an `attach/render.rs`
that emits VT from a `DiffMirror`. Both go away with the refactor.
See `research/2026-05-25-libghostty-renderstate.md` for the contract
the new modules implement.

### `phux-config`

```
src/
  lib.rs              — parse_str + re-exports
  schema.rs           — typed TOML schema (Config, KeybindingsCfg, ...)
  loader.rs           — XDG resolution + agent round-trip
  keybind.rs          — keybind parser + trie resolver
  error.rs            — ConfigError with line:col spans
  widget/             — StatusWidget trait + registry
    mod.rs
    widgets/time.rs, widgets/session_name.rs, widgets/mod.rs
```

### `phux` (binary)

```
src/main.rs           — prints version stub; subcommand dispatch unimplemented.
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

- Local: Unix socket under `$XDG_RUNTIME_DIR/phux/` (or `/tmp/phux-$UID/`),
  with the parent dir created mode `0o700`. The OS enforces the trust
  boundary on the directory; the socket inherits that boundary.
- Remote: SSH-tunneled `phux server --stdio` over `ssh host`. Auth is
  SSH's problem.

phux itself does no authentication and no encryption. Crypto in
multiplexers is a tarpit; we delegate.

## When this document is wrong

Code is the implementation; this document is the *intended* design. If
they diverge, file an issue. Either the code drifted or the design did.
Both happen; the response is to reconcile, not to let either rot.
