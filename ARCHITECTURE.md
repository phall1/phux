# Architecture

> **Updated 2026-05-26 for [ADR-0015](./ADR/0015-protocol-layering.md).**
> The wire is now layered (L1 Terminal substrate, L2 Collection
> lifecycle bundle, L3 opaque metadata) with federation and automation
> as orthogonal axes. The implementation in tree still uses the
> pre-layering vocabulary (`Pane`, `Window`, `Session`,
> `LayoutNode` on the wire); the rename to `TerminalId`
> ([ADR-0016](./ADR/0016-terminal-id-as-wire-primary.md)) and the
> demotion of layout/window/focus to TUI-consumer metadata
> ([ADR-0017](./ADR/0017-tui-not-protocol-privileged.md)) are scheduled
> cascades, not yet landed. This document calls out where current
> names map onto layers; the [Protocol layering](#protocol-layering-and-this-implementation)
> section below is the master mapping.

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

## Protocol layering and this implementation

[ADR-0015](./ADR/0015-protocol-layering.md) layers the wire into three
tiers plus two orthogonal cross-cuts. Mapping each onto code currently
in tree:

| Layer | Concept | Implemented in tree as | Status |
|---|---|---|---|
| **L1** | Terminal: PTY + libghostty `Terminal` + identity + I/O + snapshot + event stream | `PaneActor` in `phux-server::pane_actor`; wire `PaneId` and the `PANE_OUTPUT` / `PANE_SNAPSHOT` / `INPUT_*` / `BELL` / `OSC_EVENT` (currently spec-only) messages | shipped under pre-layering vocabulary; rename to `TerminalId` is ADR-0016 |
| **L2** | Collection: named lifecycle bundle of Terminals | `phux-core::Session` plus the session/window registries on `ServerState`; the "session" CLI noun | partial — session lifecycle today bundles windows AND terminals; ADR-0015 splits Collection (bundle of terminals) from the TUI's "windows" presentation |
| **L3** | Opaque metadata KV scoped to Terminal / Collection / global | not yet implemented — closest analog is the in-memory window/layout state on `ServerState` | spec-only |

Cross-cuts:

- **Federation** ([ADR-0007](./ADR/0007-mosh-class-transport-and-satellites.md)) — addressing scheme. The wire's `SessionId` already has a `LOCAL` / `SATELLITE` tag union per the ADR; `TerminalId` (ADR-0016) extends the same shape to every identity. Today's server constructs `LOCAL` only.
- **Automation** — server-side rules subscribing to L1 events. Not yet implemented; an optional service when it lands.

A consumer's tier set is declared at HELLO time. Today's `phux-client`
is an L1+L2+L3-equivalent TUI consumer. A future `phux-client-sdk`
will be L1-only; a future native GUI consumer will be L1+L3 with its
own metadata schema. The reference TUI is **not** protocol-privileged
([ADR-0017](./ADR/0017-tui-not-protocol-privileged.md)) — the wire
carries nothing that exists for it alone.

The cascades that align the in-tree implementation with this layering
are queued, not landed: rename `PaneId` → `TerminalId` workspace-wide;
split `phux-server` so L1 (terminal supervision) is mountable without
the L2/L3 services; reify L3 as a real KV store; demote `LayoutNode`,
`WindowId`, `WINDOW_*`, `LAYOUT_CHANGED`, `FOCUS_CHANGED` from the
wire into the TUI's L3 metadata conventions.

## Wire protocol: bytes on the wire (ADR-0013)

The protocol is asymmetric. Server-to-client *terminal content* is a
stream of VT bytes (`PANE_OUTPUT { pane_id, seq, bytes }` today; under
ADR-0016 the message will be `TERMINAL_OUTPUT { terminal_id, ... }`);
the server forwards what the PTY emitted, after a per-client capability
rewrite. Client-to-server *input* is structured (`INPUT_KEY`,
`INPUT_MOUSE`, `INPUT_FOCUS`, `INPUT_PASTE`, `INPUT_RAW`), built from
libghostty's input atoms per ADR-0006 / ADR-0008. Lifecycle and
commands stay structured. See [SPEC.md](./SPEC.md) §8 for the wire
shape and ADR-0013 for the bytes-on-wire rationale.

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

- Stable IDs are exactly what the wire protocol needs anyway.
- Cross-references ("this client's active terminal") become an ID, not
  a borrowed reference — no aliasing problem.
- Deletion is `O(1)` and slotmap's generational keys catch
  use-after-free in tests.

The shape splits in two: the *domain* (pure data, in `phux-core`) and
the *attached-client + I/O* state (in `phux-server`). The split is
deliberate; see ADR-0008 and the crate-graph note above.

```rust
// phux-core::registry::Registry — domain only, no I/O.
// Pre-ADR-0015 vocabulary; layer mapping in parens.
pub struct Registry {
    sessions: SlotMap<SessionId, Session>,    // L2 Collection (kind of —
                                              //   currently bundles windows too)
    windows:  SlotMap<WindowId,  Window>,     // demotes to TUI L3 metadata
    panes:    SlotMap<PaneId,    Pane>,       // L1 Terminal
}

pub struct Session  { id, name, windows: Vec<WindowId>, active: Option<WindowId> }
pub struct Window   { id, session, panes: Vec<PaneId>, layout: Option<LayoutNode>, active: Option<PaneId> }
pub struct Pane     { id, window, dims, cwd, title }
// LayoutNode is a binary split tree of PaneId leaves. Under ADR-0017
// this whole tree (LayoutNode + Window + active pane focus) demotes
// from a wire-protocol concept to a TUI-consumer convention stored
// in L3 metadata. ADR-0012's "binary split, not n-ary" decision
// continues to apply *to the TUI's tree*, not to the wire.
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

See ADR-0007 for the full forward-compat invariants (URI-shaped
session IDs, hub-and-spoke satellite topology, per-pane encoder
isolation).

## Predictive local echo

> **Status:** Design intent. Not yet implemented as of 2026-05-26.
> The mirror `Terminal` and `RenderState` redraw path landed with
> ADR-0013; the overlay layer below is the next step.

Predictive local echo is the Mosh property users actually feel on a
slow link: the cell paints under your finger, the network round-trip
catches up later. We implement it as a **client feature** layered on
top of the local mirror `Terminal`, not as a transport feature, so it
works uniformly over UDS, SSH-stdio, and a future QUIC transport.

### Mechanism

Three structures inside the client, all per attached pane:

- **Mirror Terminal** — `libghostty_vt::Terminal` fed by `PANE_OUTPUT`.
  Authoritative for the user's visible grid. Predictions never modify it.
- **Prediction overlay** — a sparse `(row, col) -> PredictedCell` map
  drawn on top of the mirror at render time. Cells are styled (dim /
  underline) until the server confirms them.
- **Epoch counter** — monotonic id tagging each prediction with the
  network state at the time it was made. Predictions older than a TTL
  with no confirming `PANE_OUTPUT` are killed (treated as wrong).

phux's structured-input choice (ADR-0006 / ADR-0008) means the client
cannot byte-predict the way Mosh does: the libghostty `Encoder` lives
server-side, so the client doesn't know whether the user's `'a'` will
hit the PTY as `0x61` (insertable text) or be swallowed by the inner
program (vim normal mode, less, etc.). v0.1 therefore predicts at the
**grapheme level** — "if the cursor is plausibly in insertable-text
context, the next visible cell will be this grapheme." Conservative by
default; matches Mosh's safety posture. A future v0.2 enlargement may
add a parallel client-side encoder for richer predictions, with the
extra divergence risk that implies.

### Sequence

The happy path (single keypress, server echoes the same grapheme back):

```
User      Client                                          Server                          PTY/Shell
 │         │                                                │                                │
 │ key 'a' │                                                │                                │
 ├────────►│                                                │                                │
 │         │ 1. predict: paint 'a' at cursor in overlay     │                                │
 │         │    (epoch = N, style = dim/underline)          │                                │
 │         │ 2. INPUT_KEY {pane, KeyEvent('a', …)}          │                                │
 │         ├───────────────────────────────────────────────►│                                │
 │         │                                                │ 3. libghostty Encoder → 0x61   │
 │         │                                                │ 4. write to PTY                │
 │         │                                                ├───────────────────────────────►│
 │         │                                                │                                │ 5. shell
 │         │                                                │                                │    echoes
 │         │                                                │ 6. feed bytes to canonical     │◄─┐
 │         │                                                │    libghostty Terminal         │  │
 │         │                                                │◄───────────────────────────────┘  │
 │         │ 7. PANE_OUTPUT {pane, seq=K, bytes=0x61}       │                                   │
 │         │◄───────────────────────────────────────────────┤                                   │
 │         │ 8. vt_write bytes into mirror Terminal         │                                   │
 │         │ 9. reconcile: prediction at (row,col,'a')      │                                   │
 │         │    matches mirror at (row,col,'a') → CONFIRM,  │                                   │
 │         │    drop overlay entry                          │                                   │
 │         │                                                │                                   │
 │         │ 10. FRAME_ACK {pane, seq=K} ──────────────────►│                                   │
 │         │                                                │                                   │

  Contradiction path (e.g. user is in vim normal mode):
 │         │ 7'. PANE_OUTPUT bytes do NOT place 'a' at      │                                   │
 │         │     cursor (cursor moves instead, no insert)   │                                   │
 │         │ 8'. reconcile: prediction CONTRADICTED         │                                   │
 │         │     drop overlay entry; redraw cell from       │                                   │
 │         │     mirror                                     │                                   │

  Timeout path (server silent, no confirming output ever arrives):
 │         │ -. epoch N has lived > predict_ttl_ms without  │                                   │
 │         │    a confirming PANE_OUTPUT → KILL prediction, │                                   │
 │         │    redraw cell from mirror                     │                                   │
```

Three properties hold:

1. **The mirror is authoritative.** Predictions are an overlay drawn on
   top at render time; they never mutate the mirror. A bug in the
   predictor cannot corrupt the user's visible grid past the next
   redraw.
2. **Reconciliation runs on `PANE_OUTPUT` arrival**, not on
   `FRAME_ACK`. The ack is a server-side flow-control signal (SPEC
   §12.2); it carries no rendering meaning. This means predictive echo
   continues to function correctly even if a future minor version
   reshapes the ack protocol.
3. **Epochs + TTL are the safety net.** If the server is silent
   (network dead, app not echoing, app crashed), predictions don't
   accumulate forever; they age out and the displayed cell falls back
   to the mirror's truth.

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
See `lib.rs` for the docs.rs / crates.io rationale. The pre-ADR-0013
`diff/` module and its companion `wire/diff.rs` have been deleted;
`PANE_OUTPUT` and `PANE_SNAPSHOT` carry VT bytes directly. A small
amount of stale doc-comment text inside `wire/field.rs` still mentions
`DiffOp` and is scheduled for cleanup.

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
  lib.rs              — re-exports of ServerRuntime, ServerState, PaneActor, ...
  runtime.rs          — tokio current-thread + UDS listener + accept loop;
                        spawns per-client tasks on a LocalSet
  state.rs            — ServerState, SharedState, AttachedClient, ClientId,
                        PaneInput, Outbound
  pane_actor.rs       — PaneActor: owns the pane's libghostty Terminal (!Send,
                        in RefCell on the LocalSet), per-pane input encoders,
                        PTY reader/writer threads, broadcast PANE_OUTPUT fanout,
                        snapshot synthesis on demand (ADR-0014)
  grid.rs             — SnapshotSynthesizer: walks the canonical Terminal via
                        RenderState and emits a self-contained vt_replay_bytes
                        sequence for PANE_SNAPSHOT (per-row SGR deltas +
                        graphemes + cursor restore + DECSCUSR)
  downsample.rs       — per-client capability rewrite of outbound VT bytes
                        (truecolor → 256/16, OSC 8 / image / KIP gating)
  id_bridge.rs        — core SessionId <-> wire SessionId (u32)
  telemetry.rs        — tracing setup; opt-in tokio-console behind a feature
  input/              — server-side encoders bridging wire input -> PTY bytes;
                        each pane owns its own PerPane{Key,Mouse,Focus,Paste}
                        encoder, refreshed from Terminal state per encode
    key.rs, mouse.rs, focus.rs, paste.rs, mod.rs
examples/
  one_pane.rs         — end-to-end PTY → Terminal → bytes-on-wire smoke test
                        under ADR-0013 (logs encoded PANE_OUTPUT to stderr)
```

No `pty/`, `journal/`, `command.rs`, or `hooks.rs` yet — these are
future work; their absence here is intentional, not drift. PTY supervision
today lives inside `pane_actor.rs` (two `std::thread`s bridging blocking
`portable_pty` I/O to the async actor over `mpsc` channels).

### `phux-client`

Under ADR-0013 the client owns a `libghostty_vt::Terminal` per
attached pane and uses `RenderState` to drive redraw. The hand-rolled
`mirror/` module from earlier drafts has been deleted.

> **Implementation note (2026-05-26):** the intended per-row dirty
> path is currently bypassed because `libghostty_vt::RenderState::
> dirty()` returns a value outside the modeled `{Clean, Partial, Full}`
> enum, which surfaced as a frozen alt-screen after the first
> `PANE_SNAPSHOT`. The workaround in `attach/render.rs` defaults to
> `Dirty::Full` and unconditionally marks every row as `must_draw`,
> costing a full-screen redraw per frame. Correct visually, off the
> hot path until libghostty is fixed. Tracked as `phux-l0t`.

```
src/
  lib.rs              — re-exports of attach::run
  attach/
    mod.rs            — public run(socket, target); ties everything together
    connection.rs     — UDS transport, length-prefixed frame I/O
    driver.rs         — tokio::select! lifecycle, RawModeGuard RAII for
                        outer terminal state (raw mode + altscreen, restored
                        on any exit)
    render.rs         — PaneRenderer: feeds PANE_OUTPUT bytes into the local
                        Terminal and walks RenderState rows to emit cursor
                        positioning + per-cell SGR deltas + graphemes. Per-row
                        dirty currently bypassed (full redraw per frame);
                        see implementation note above and ticket phux-l0t.
    input.rs          — StdinParser: keyboard + UTF-8 + escape sequences;
                        hardcoded Ctrl-B D detach chord
```

What this tree does NOT contain yet, deliberately:

- Mouse / bracketed-paste parsing on the client (keyboard only in v0).
- Predictive local echo (see "Predictive local echo" above for the
  design that lives on top of the mirror Terminal).
- `VIEWPORT_RESIZE` routing end-to-end (frame exists; SIGWINCH handler
  not yet wired).
- Client-side keybinding dispatch (only the hardcoded detach chord).
- Config loading.

See `research/2026-05-25-libghostty-renderstate.md` for the renderer
contract these modules implement.

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
src/main.rs           — clap subcommand dispatch:
                          `phux attach [SESSION] [--socket PATH]`
                          `phux server  [--session NAME] [--socket PATH]`
                        Auto-spawns a detached `phux server` if the socket
                        doesn't exist when `attach` is invoked (25 ms poll,
                        2 s timeout). Opt-in cargo features: `dhat-heap`
                        (binary), and `tokio-console` via `phux-server`.
```

The wider subcommand surface in DESIGN.md §1 (`new`, `ls`, `windows`,
`panes`, `kill`, `send`, `capture`, `config`, `messages`, `version`,
`help`) is not yet wired.

## State replay & crash recovery

> **Status:** Design intent. Not yet implemented as of 2026-05-26.
> Nothing in the server currently writes to disk; `server.pid`,
> per-pane journals, and `--recover` do not exist. The bytes-on-wire
> shape (ADR-0013) makes the implementation mechanical when its turn
> comes — the PTY byte stream we forward to clients is also exactly
> what a journal would record.

The intended shape: the server journals raw PTY output to disk, per
pane, in `journal/<pane_id>.log`. Journals are append-only, fsync'd
on close, and capped (default: 10 MB ring per pane).

On startup, if `server.pid` is stale, the server can be invoked with
`--recover`. It reads each journal, replays it into a fresh
`libghostty_vt::Terminal`, and reconstitutes sessions from a metadata
file alongside the journals.

Crash recovery is therefore a property of the design, not an
add-on. tmux loses everything on a daemon crash; phux will not.

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
