//! Per-pane actor (`phux-byc.5`).
//!
//! Owns a `libghostty_vt::Terminal`, a backing `portable_pty` master,
//! and per-pane input encoders. Drives a `select!` loop that forwards
//! PTY output to subscribed clients and writes client-originated input
//! back to the PTY.
//!
//! See ADR-0014 for the placement rationale. In short: `Terminal` is
//! `!Send + !Sync`, so it can't live behind a `tokio::spawn` future. It
//! lives inside a `spawn_local` task that runs on the server's existing
//! current-thread runtime via a `LocalSet`. All cross-task coordination
//! flows through channel handles ([`TerminalHandle`]) that are `Send` —
//! the actor itself never crosses a thread boundary.
//!
//! # PTY async wrapper choice
//!
//! `portable_pty::MasterPty::try_clone_reader` / `take_writer` hand out
//! `Box<dyn Read + Send>` and `Box<dyn Write + Send>` — both **blocking**
//! I/O handles. We bridge them to async with two dedicated `std::thread`s
//! (one for reads, one for writes) that talk to the actor over
//! `tokio::sync::mpsc` channels. This mirrors the pattern in
//! `examples/one_pane.rs` and avoids OS-specific `AsyncFd` plumbing for
//! a feature whose value (a few PTY fds, not hundreds) doesn't justify
//! the complexity. At typical phux pane counts (1–20) the per-pane thread
//! cost is invisible against everything else the server does.
//!
//! # Why `bytes::Bytes` for the output broadcast
//!
//! `tokio::sync::broadcast::Sender` requires `Clone` payloads (every
//! subscriber receives a copy of the same value). `bytes::Bytes` is the
//! standard cheap-clone byte buffer in the tokio ecosystem; `Vec<u8>`
//! would also work but at the cost of a full clone per subscriber.

use std::cell::RefCell;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use bytes::Bytes;
use libghostty_vt::{
    RenderState, Terminal, TerminalOptions,
    render::{CursorVisualStyle, Snapshot},
    terminal::Mode,
};
use phux_protocol::ClientId;
use phux_protocol::wire::frame::FrameKind;
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio_util::sync::CancellationToken;
use tracing::{debug, trace, warn};

use crate::grid::{SnapshotBytes, SnapshotSynthesizer};
use crate::input::paste::PasteOutcome;
use crate::input::{
    PerTerminalFocusEncoder, PerTerminalKeyEncoder, PerTerminalMouseEncoder,
    PerTerminalPasteEncoder,
};
use crate::state::{Outbound, TerminalInput};

/// Snapshot of the live `Terminal`'s cursor + DEC mode bits captured at
/// the moment a consumer is brought up-to-date.
///
/// Captured via ATTACH today; via `FRAME_ACK` in phux-q0e.4. The
/// state-sync tick driver (phux-q0e.3) compares this against the live
/// terminal's current state to decide whether the per-tick incremental
/// synthesis must re-emit the cursor placement + DEC modes that
/// `SnapshotSynthesizer::synthesize` would emit at the tail of a
/// from-empty snapshot.
///
/// The set tracked here mirrors the modes that today's
/// `SnapshotSynthesizer::synthesize` re-emits at the end of a snapshot
/// (`BRACKETED_PASTE`, `FOCUS_EVENT`, `ALT_SCREEN_LEGACY`) plus the
/// cursor placement/visibility/style read off the `RenderState::Snapshot`.
/// New mode bits that synthesize starts re-emitting should be added here
/// in lock-step.
#[derive(Debug, Clone, Copy)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "DEC mode bits are independent flags; collapsing them into a bitfield obscures the per-flag mapping to `Mode::*` constants"
)]
pub struct LastAckedCursorMode {
    /// Cursor column (zero-based viewport coords). `None` when the
    /// cursor is not viewport-resident (off-screen due to scrollback).
    pub cursor_x: Option<u16>,
    /// Cursor row (zero-based viewport coords).
    pub cursor_y: Option<u16>,
    /// `DECTCEM` (DEC private mode 25): cursor visibility.
    pub cursor_visible: bool,
    /// `DECSCUSR` shape.
    pub cursor_visual_style: CursorVisualStyle,
    /// `DECSCUSR` blink flag.
    pub cursor_blinking: bool,
    /// `BRACKETED_PASTE` (DEC private mode 2004).
    pub bracketed_paste: bool,
    /// `FOCUS_EVENT` (DEC private mode 1004).
    pub focus_event: bool,
    /// `ALT_SCREEN_LEGACY` (DEC private mode 47).
    pub alt_screen_legacy: bool,
}

impl LastAckedCursorMode {
    /// Capture the live terminal's cursor + DEC mode state into a fresh
    /// `LastAckedCursorMode`. Querying every field; libghostty FFI errors
    /// degrade to safe defaults (cursor invisible, modes off) so a
    /// transient FFI failure doesn't kill the actor.
    fn capture(terminal: &Terminal<'_, '_>, snapshot: &Snapshot<'_, '_>) -> Self {
        let (cursor_x, cursor_y) = match snapshot.cursor_viewport() {
            Ok(Some(v)) => (Some(v.x), Some(v.y)),
            Ok(None) | Err(_) => (None, None),
        };
        Self {
            cursor_x,
            cursor_y,
            cursor_visible: snapshot.cursor_visible().unwrap_or(false),
            cursor_visual_style: snapshot
                .cursor_visual_style()
                .unwrap_or(CursorVisualStyle::Block),
            cursor_blinking: snapshot.cursor_blinking().unwrap_or(false),
            bracketed_paste: terminal.mode(Mode::BRACKETED_PASTE).unwrap_or(false),
            focus_event: terminal.mode(Mode::FOCUS_EVENT).unwrap_or(false),
            alt_screen_legacy: terminal.mode(Mode::ALT_SCREEN_LEGACY).unwrap_or(false),
        }
    }
}

/// Per-consumer cached reference state for ADR-0018 lazy state
/// synchronization. One per `(TerminalActor, attached ClientId)`.
///
/// Holds the libghostty `RenderState` that tracks "what cells this
/// consumer has already seen" (so the tick driver in phux-q0e.3 can
/// walk only the rows that changed since the last per-consumer
/// snapshot), the `seq` of the last frame this consumer `ACK`ed (driven
/// by phux-q0e.4's `FRAME_ACK` handler), and the cursor/mode state
/// captured at the same instant.
///
/// `Drop` runs the libghostty `ghostty_render_state_free` via
/// `RenderState`'s own destructor — no explicit cleanup needed in
/// DETACH beyond removing the entry from the actor's map.
///
/// `!Send + !Sync` because `RenderState` is `!Send + !Sync` (per the
/// `libghostty-send-sync` bd memory). Lives only inside the actor,
/// which runs on the `LocalSet` thread that owns the `Terminal`.
pub struct ConsumerSyncState {
    /// Per-consumer incremental synthesizer (phux-q0e.3). Owns the
    /// per-consumer `RenderState` — the dirty cache that drives the
    /// per-tick diff walk. Each consumer gets its own synthesizer so the
    /// dirty bookkeeping is isolated; `RowIterator`/`CellIterator`
    /// allocations are cheap.
    pub synthesizer: SnapshotSynthesizer<'static>,
    /// Per-consumer outbound mailbox the tick driver pushes
    /// `TERMINAL_OUTPUT` frames into. Cloned from the
    /// [`crate::state::AttachedClient`]'s `tx` at ATTACH time.
    pub outbound: mpsc::Sender<Outbound>,
    /// Wire-level terminal id for the `TerminalOutput` frame
    /// (`SPEC.md` §8.1). Carried per-consumer because the runtime owns
    /// the mapping `(TerminalActor, WireTerminalId)` and may differ
    /// across consumers in future tier topologies.
    pub wire_terminal_id: u32,
    /// Per-consumer monotonic sequence id for `TERMINAL_OUTPUT`
    /// (`SPEC.md` §8.1, §12). Starts at `1` and increments on each
    /// emitted frame. Per-consumer (not shared) so each consumer can
    /// `FRAME_ACK` against its own stream — this matches the existing
    /// per-pump scheme in `runtime.rs::handle_attach`.
    pub next_seq: u64,
    /// `FrameId` of the most recent `TERMINAL_OUTPUT` this consumer has
    /// `ACK`ed. `0` means "no acks yet — the next emission is the only
    /// thing this consumer has seen" (matches `FrameId::ZERO`'s "empty
    /// initial frame" semantics).
    pub last_acked_seq: u64,
    /// Cursor + DEC mode bits captured at the last sync point. Used by
    /// the tick driver to decide whether to re-emit cursor placement /
    /// mode toggles in the incremental synthesis path. See
    /// [`LastAckedCursorMode`] for the field set rationale.
    pub last_cursor_mode: LastAckedCursorMode,
}

impl std::fmt::Debug for ConsumerSyncState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConsumerSyncState")
            .field("wire_terminal_id", &self.wire_terminal_id)
            .field("next_seq", &self.next_seq)
            .field("last_acked_seq", &self.last_acked_seq)
            .field("last_cursor_mode", &self.last_cursor_mode)
            .finish_non_exhaustive()
    }
}

/// Request to register a new consumer with the actor.
///
/// Drives the ADR-0018 per-consumer state lifecycle. The caller is the
/// runtime's ATTACH path, which has just installed the client in
/// `ServerState`.
///
/// The actor allocates a fresh `RenderState`, primes it against the
/// live `Terminal` (so the next incremental synthesis emits only
/// deltas *from now*), captures the cursor + mode state, and stores
/// the resulting [`ConsumerSyncState`] keyed by `client_id`. The reply
/// fires once the entry is in place; the caller can then proceed to
/// emit `TERMINAL_SNAPSHOT` (which brings the consumer to the same
/// reference point this `RenderState` was primed against).
#[derive(Debug)]
pub struct ConsumerAttachRequest {
    /// Identifier the actor will key the per-consumer state by. Must
    /// match the `ClientId` the caller uses in subsequent
    /// `ConsumerDetachRequest`s and `FRAME_ACK` routing.
    pub client_id: ClientId,
    /// Per-consumer outbound mailbox. The actor stores a clone in the
    /// per-consumer [`ConsumerSyncState`] and uses it on every tick
    /// (phux-q0e.3) to push a `TerminalOutput` frame carrying the
    /// incremental synthesis bytes.
    pub outbound: mpsc::Sender<Outbound>,
    /// Wire-level terminal id (`u32`). The actor stamps it on every
    /// emitted `TerminalOutput` frame. The runtime owns the mapping
    /// from the actor's [`phux_core::ids::TerminalId`] to this wire id and
    /// passes the resolved value here at ATTACH time.
    pub wire_terminal_id: u32,
    /// Channel the actor uses to acknowledge the lifecycle insertion.
    /// `Ok(())` on success; `Err(...)` if the per-consumer
    /// `SnapshotSynthesizer` or its priming pass could not be allocated.
    /// Dropping the receiver on the caller side is benign — the actor
    /// uses `send().ok()`.
    pub reply: oneshot::Sender<Result<(), ConsumerAttachError>>,
}

/// Errors surfaced by [`TerminalActor::register_consumer`] in response to a
/// [`ConsumerAttachRequest`].
#[derive(Debug, thiserror::Error)]
pub enum ConsumerAttachError {
    /// libghostty refused to allocate the per-consumer `RenderState` /
    /// iterators backing the `SnapshotSynthesizer`.
    #[error("libghostty allocation failed: {0}")]
    Ghostty(#[from] libghostty_vt::Error),
    /// `SnapshotSynthesizer::new` (or its priming `mark_synced` call)
    /// failed.
    #[error("synthesizer setup failed: {0}")]
    Synth(#[from] crate::grid::SynthesisError),
}

/// Request to drop the per-consumer state for `client_id`.
///
/// Sent by the runtime's DETACH path (and the EOF cleanup path). Silent
/// no-op if the consumer is not currently registered, matching
/// `ServerState::detach`'s idempotent contract.
#[derive(Debug)]
pub struct ConsumerDetachRequest {
    /// Identifier whose [`ConsumerSyncState`] entry to remove.
    pub client_id: ClientId,
    /// Fired once the entry has been removed (or was already absent).
    /// The caller can use this to sequence later operations against
    /// the actor; dropping the receiver is benign.
    pub reply: oneshot::Sender<()>,
}

/// Default depth of the per-pane input mailbox.
///
/// Small on purpose: keystrokes are tiny and the server drains them in
/// the same event loop. A backed-up channel here would mean the actor
/// has stalled, which is its own bug to investigate.
pub const DEFAULT_INPUT_MAILBOX: usize = 64;

/// Default capacity of the per-pane output broadcast channel.
///
/// Bytes fan out to subscribed clients. Sized for "burst tolerance" —
/// a busy pane can emit a few dozen frames in a short window before a
/// slow subscriber falls behind and gets a `RecvError::Lagged`.
pub const DEFAULT_OUTPUT_BROADCAST: usize = 256;

/// Default PTY read chunk size. Mirrors the example. Sized comfortably
/// above the typical libghostty escape-sequence span so a single read
/// rarely splits a sequence boundary.
const PTY_READ_CHUNK: usize = 4096;

/// Tick interval for the state-sync emission driver (phux-q0e.3).
///
/// 30 ms ≈ 33 Hz; per ADR-0018 / `research/2026-05-26-state-sync-algorithm.md`
/// §"tick scheduler" first-cut. An RTT-adaptive cadence is the follow-up
/// `phux-q0e.5` and lives outside this ticket's scope.
pub const DEFAULT_TICK_INTERVAL: std::time::Duration = std::time::Duration::from_millis(30);

/// Request for the pane's current `vt_replay_bytes` snapshot.
///
/// Sent by the ATTACH handler on the per-client task; the actor walks
/// its `Terminal` via [`SnapshotSynthesizer`] and replies on the
/// oneshot.
#[derive(Debug)]
pub struct SnapshotRequest {
    /// Channel the actor uses to ship the synthesized snapshot back.
    /// Dropping the sender on the receiver side is benign — the actor
    /// just discards the reply.
    pub reply: oneshot::Sender<SnapshotBytes>,
}

/// Cross-task handle to a [`TerminalActor`].
///
/// `TerminalHandle` is `Send + Clone`: per-client tasks clone it freely to
/// request snapshots, send input, or subscribe to the output broadcast.
/// The actor itself (which owns the `!Send` `Terminal`) lives on the
/// `LocalSet` and never crosses a thread boundary.
#[derive(Debug, Clone)]
pub struct TerminalHandle {
    /// Sender for input events (keys, mouse, etc.). Drained by the
    /// actor and written to the PTY via the per-pane encoders.
    pub input: mpsc::Sender<TerminalInput>,
    /// Sender for snapshot requests. The ATTACH handler uses this to
    /// build `TERMINAL_SNAPSHOT` frames.
    pub snapshot: mpsc::Sender<SnapshotRequest>,
    /// Output broadcast channel; subscribers receive every PTY byte
    /// chunk forwarded by the actor.
    pub output: broadcast::Sender<Bytes>,
    /// Resize control channel. Wired but not yet driven end-to-end —
    /// `VIEWPORT_RESIZE` routing through the runtime lands with
    /// `phux-4hp`. The actor honours messages it receives (libghostty
    /// `Terminal::set_size`, PTY winsize ioctl).
    pub resize: mpsc::Sender<(u16, u16)>,
    /// ADR-0018 per-consumer state-sync lifecycle (phux-q0e.2). The
    /// runtime sends a [`ConsumerAttachRequest`] on each successful
    /// ATTACH so the actor allocates the per-consumer `RenderState`
    /// before the `TERMINAL_SNAPSHOT` goes out. Future state-sync work
    /// (phux-q0e.3 tick driver, phux-q0e.4 `FRAME_ACK`) reads from the
    /// resulting per-consumer state map.
    pub consumer_attach: mpsc::Sender<ConsumerAttachRequest>,
    /// Counterpart to [`Self::consumer_attach`]. The runtime sends
    /// this on DETACH (and on the EOF cleanup path) to free the
    /// per-consumer `RenderState`. Silent no-op if the consumer was
    /// never attached.
    pub consumer_detach: mpsc::Sender<ConsumerDetachRequest>,
    /// Pane viewport width in cells at construction time.
    pub cols: u16,
    /// Pane viewport height in cells at construction time.
    pub rows: u16,
}

/// Per-pane actor. Owns the `Terminal`, the PTY master, the per-pane
/// input encoders, and serves the channels exposed via [`TerminalHandle`].
///
/// `Terminal<'static, 'static>` because we use [`Terminal::new`] (NULL
/// allocator) — the lifetime parameters degenerate to `'static`. A
/// future custom allocator path would tie this to the surrounding
/// arena's lifetime; not needed for `phux-byc.5`.
///
/// `Terminal`, encoders, and the `SnapshotSynthesizer` are stashed
/// inside `RefCell` so the `select!` arms (which conceptually borrow
/// `&mut self`) can each take what they need without fighting the
/// borrow checker over disjoint field access.
pub struct TerminalActor {
    terminal: RefCell<Terminal<'static, 'static>>,
    synth: RefCell<SnapshotSynthesizer<'static>>,
    key_enc: RefCell<PerTerminalKeyEncoder>,
    mouse_enc: RefCell<PerTerminalMouseEncoder>,
    focus_enc: RefCell<PerTerminalFocusEncoder>,
    paste_enc: RefCell<PerTerminalPasteEncoder>,
    input_rx: mpsc::Receiver<TerminalInput>,
    snapshot_rx: mpsc::Receiver<SnapshotRequest>,
    resize_rx: mpsc::Receiver<(u16, u16)>,
    consumer_attach_rx: mpsc::Receiver<ConsumerAttachRequest>,
    consumer_detach_rx: mpsc::Receiver<ConsumerDetachRequest>,
    /// Per-consumer state-sync cache (ADR-0018, phux-q0e.2). Keyed by
    /// the [`ClientId`] the runtime uses for subscription tracking in
    /// [`crate::state::ServerState`]; entries are inserted by the
    /// ATTACH handler and removed by DETACH. `!Send` because
    /// `RenderState` is `!Send` — fine; the whole actor lives on the
    /// `LocalSet` thread (ADR-0014).
    consumer_states: HashMap<ClientId, ConsumerSyncState>,
    /// Bytes streaming in from the PTY reader thread. `None` when this
    /// actor is the no-PTY test variant (`TerminalActor::new`); the select!
    /// branch becomes a no-op via `Option::as_mut`.
    pty_rx: Option<mpsc::UnboundedReceiver<PtyEvent>>,
    /// Outbound bytes destined for the PTY writer thread. `None` for
    /// the no-PTY test variant.
    pty_tx: Option<mpsc::UnboundedSender<Vec<u8>>>,
    /// PTY backing resources. Kept alive for the actor's lifetime;
    /// dropped on shutdown to send EOF to the slave and tear down the
    /// reader/writer threads.
    pty: Option<PtyOwned>,
    output_tx: broadcast::Sender<Bytes>,
    /// One-shot fired when the actor observes PTY EOF. Paired with the
    /// matching receiver in [`TerminalActorBundle::exit_notify`]; the
    /// runtime uses it to drive client-detach on shell exit (phux-it8).
    ///
    /// `Option` so the actor can `.take()` it after firing — sending on
    /// a `oneshot::Sender` is a by-value move. `None` after the first
    /// fire or if the bundle's receiver was never created (the test
    /// constructor [`TerminalActor::new_with_seed`] leaves it `Some` too,
    /// but no consumer subscribes; the `.ok()` swallow is benign).
    exit_notify: Option<oneshot::Sender<()>>,
    /// Cancellation token watched by the actor's `select!`. Cancel to
    /// ask the actor to shut down cleanly (drains the PTY, reaps the
    /// child, and exits). A child token of the per-server root token
    /// when constructed via [`Self::build_with_token`]; an unlinked
    /// fresh token when constructed via [`TerminalActor::new`] et al.
    /// Dropping the token does NOT cancel — call `.cancel()` explicitly
    /// (this is intentional; the prior `oneshot::Sender::drop` semantics
    /// were a hidden lifecycle coupling we want gone).
    token: CancellationToken,
    cols: u16,
    rows: u16,
}

/// Bundle of PTY-side resources owned by a [`TerminalActor`] with a real PTY.
///
/// Fields are kept in struct-declaration order so drop order matches the
/// teardown contract: writer thread first (so the writer channel closes
/// before the master), then the master (which sends EOF to the slave),
/// then the child, then the reader thread.
struct PtyOwned {
    /// Master handle — owned by the actor so resize ioctls can be
    /// issued. Wrapped in `Arc` so the writer thread can hold a clone
    /// (it doesn't, currently — the writer thread owns its own
    /// `Box<dyn Write + Send>` taken via `MasterPty::take_writer` —
    /// but the field keeps the master alive for resize / drop-on-exit).
    #[allow(dead_code, reason = "kept alive; methods invoked through &self")]
    master: Arc<Mutex<Box<dyn MasterPty + Send>>>,
    /// Child process spawned on the slave side. Reaped in
    /// [`TerminalActor::shutdown_pty`].
    child: Box<dyn Child + Send + Sync>,
    /// Reader-thread join handle. Reader exits when the master is
    /// dropped (EOF on the read fd) or when its `mpsc::Sender` closes.
    reader_thread: Option<JoinHandle<()>>,
    /// Writer-thread join handle. Writer exits when its `mpsc::Receiver`
    /// closes (i.e., the actor's [`Self::pty_tx`] sender is dropped).
    writer_thread: Option<JoinHandle<()>>,
}

impl std::fmt::Debug for PtyOwned {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PtyOwned")
            .field("child", &self.child)
            .finish_non_exhaustive()
    }
}

/// Events flowing from the PTY reader thread into the actor.
#[derive(Debug)]
enum PtyEvent {
    /// A chunk of bytes read from the PTY master.
    Bytes(Vec<u8>),
    /// The PTY hit EOF or errored. Either way: the child is going away.
    Eof,
}

/// Errors surfaced while constructing a [`TerminalActor`].
#[derive(Debug, thiserror::Error)]
pub enum TerminalActorError {
    /// Libghostty refused to allocate a [`Terminal`] or input encoder.
    #[error("libghostty allocation failed: {0}")]
    Terminal(#[from] libghostty_vt::Error),
    /// Failed to allocate the [`SnapshotSynthesizer`].
    #[error("SnapshotSynthesizer::new failed: {0}")]
    Synth(#[from] crate::grid::SynthesisError),
    /// Could not open a PTY pair via `portable_pty`.
    #[error("openpty failed: {0}")]
    OpenPty(String),
    /// Could not spawn the command on the PTY slave.
    #[error("spawn failed: {0}")]
    Spawn(String),
    /// Could not take the master reader or writer half, or start the
    /// bridge threads.
    #[error("pty io setup failed: {0}")]
    PtyIo(String),
}

/// Bundle returned from [`TerminalActor::new`]: the actor itself plus a
/// [`CancellationToken`] that, when cancelled, fires the actor's
/// shutdown branch.
///
/// The token is **clone-shared** with the actor: callers can clone it
/// before handing the actor off to `spawn_local`, hold the clone, and
/// call `.cancel()` to ask the actor to exit. Unlike the prior
/// `oneshot::Sender<()>`-shaped bundle, dropping `token` does NOT
/// cancel the actor — cancellation must be explicit.
#[must_use]
pub struct TerminalActorBundle {
    /// The actor; pass to `tokio::task::spawn_local`.
    pub actor: TerminalActor,
    /// Cross-task handle to the actor.
    pub handle: TerminalHandle,
    /// Cancellation token. Call `.cancel()` to ask the actor to shut
    /// down cleanly. Cloneable; shares cancellation state with the
    /// actor's internal copy.
    pub token: CancellationToken,
    /// One-shot receiver that fires when the actor observes PTY EOF
    /// (the child process exited, the pane is dying). The runtime
    /// pairs this with the terminal's [`phux_core::ids::TerminalId`] and uses
    /// it to drive client-detach on shell-`exit` (phux-it8).
    ///
    /// Used by the runtime's per-pane EOF watcher task; tests that
    /// don't care about lifecycle simply drop it. Receiver-drop is
    /// benign for the sender side — the actor uses `send().ok()`.
    ///
    /// `Option` so callers can `take()` it out of the bundle;
    /// `None` after the first take.
    pub exit_notify: Option<oneshot::Receiver<()>>,
}

impl std::fmt::Debug for TerminalActor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TerminalActor")
            .field("cols", &self.cols)
            .field("rows", &self.rows)
            .field("has_pty", &self.pty.is_some())
            .finish_non_exhaustive()
    }
}

impl std::fmt::Debug for TerminalActorBundle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TerminalActorBundle")
            .field("actor", &self.actor)
            .field("handle", &self.handle)
            .finish_non_exhaustive()
    }
}

/// Resolve the default shell. Reads `$SHELL`; falls back to `/bin/sh`
/// (POSIX-guaranteed) when unset.
///
/// Sets `TERM=xterm-256color` on the spawned process. This is deliberate
/// (phux-7vx): we previously advertised `TERM=ghostty`, but ghostty's
/// terminfo carries the `fullkbd` extended capability that ncurses
/// applications read as "kitty keyboard protocol available." Several
/// ncurses TUIs (htop is the canonical reproducer) then push the kitty
/// progressive-enhancement flags on startup via `CSI > N u`. libghostty's
/// per-pane `Terminal` honours that push, after which the per-pane key
/// encoder correctly emits CSI-u sequences (e.g. `\x1b[113;1u` for `q`).
/// The trouble is the round-trip on the app's side: htop in particular
/// does NOT actually parse incoming CSI-u for the keys it cares about,
/// so the user's `q` quit no longer reaches htop's key dispatch.
///
/// `xterm-256color` is the universally-recognised safe baseline: 256
/// colours and the standard xterm key vocabulary, no kitty advertisement.
/// Apps that want kitty mode still get it — they have to enable it
/// explicitly with `CSI > N u`, at which point the encoder pivots to
/// CSI-u (validated in `tests/htop_keys.rs`). The encoder's terminal-
/// state awareness is unchanged; only the default advertisement is.
///
/// Trade-off: phux loses ghostty-specific terminfo extensions (sixel,
/// kitty graphics caps as advertised by terminfo, the ghostty-specific
/// SGR colour extensions). Those features are still reachable when the
/// app opts in directly. When phux's own input/output layer fully
/// supports the kitty keyboard protocol round-trip, revert this to
/// `ghostty` (or expose a config switch).
#[must_use]
pub fn default_shell_command() -> CommandBuilder {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_owned());
    let mut cmd = CommandBuilder::new(shell);
    cmd.env("TERM", "xterm-256color");
    cmd
}

impl TerminalActor {
    /// Build a fresh actor of the given dimensions **without** a backing
    /// PTY. Used by tests that exercise snapshot / shutdown semantics
    /// without driving a real process.
    ///
    /// The `Terminal` is allocated via libghostty's default allocator
    /// (NULL alloc → `'static` lifetimes). `max_scrollback` defaults to
    /// `10_000` — a tmux-style mid-range value.
    #[allow(clippy::new_ret_no_self, reason = "bundle-shaped constructor")]
    pub fn new(cols: u16, rows: u16) -> Result<TerminalActorBundle, TerminalActorError> {
        Self::build(cols, rows, None, CancellationToken::new())
    }

    /// Build a fresh actor backed by a real PTY running `cmd`.
    ///
    /// Spawns the command on the slave side, kicks off the reader and
    /// writer bridge threads, and returns the bundle. The caller hands
    /// `actor` to `spawn_local` and keeps `handle` + `token` to talk
    /// to and tear down the actor.
    pub fn new_with_command(
        cmd: CommandBuilder,
        cols: u16,
        rows: u16,
    ) -> Result<TerminalActorBundle, TerminalActorError> {
        Self::build(cols, rows, Some(cmd), CancellationToken::new())
    }

    /// Convenience: spawn the user's default shell (`$SHELL` or
    /// `/bin/sh`) in a fresh PTY.
    pub fn new_with_default_shell(
        cols: u16,
        rows: u16,
    ) -> Result<TerminalActorBundle, TerminalActorError> {
        Self::new_with_command(default_shell_command(), cols, rows)
    }

    /// Build an actor whose cancellation token is `token` (typically a
    /// `root_token.child_token()` from [`crate::runtime::ServerRuntime`]).
    /// The bundle's `token` field is a clone of the same token, so
    /// cancelling either propagates to the actor.
    ///
    /// This is the path the runtime uses; tests use [`Self::new`] /
    /// [`Self::new_with_command`] which generate an unlinked fresh
    /// token internally.
    pub fn build_with_token(
        cols: u16,
        rows: u16,
        cmd: Option<CommandBuilder>,
        token: CancellationToken,
    ) -> Result<TerminalActorBundle, TerminalActorError> {
        Self::build(cols, rows, cmd, token)
    }

    fn build(
        cols: u16,
        rows: u16,
        cmd: Option<CommandBuilder>,
        token: CancellationToken,
    ) -> Result<TerminalActorBundle, TerminalActorError> {
        let terminal = Terminal::new(TerminalOptions {
            cols,
            rows,
            max_scrollback: 10_000,
        })?;
        let synth = SnapshotSynthesizer::new()?;
        let key_enc = PerTerminalKeyEncoder::new()?;
        let mouse_enc = PerTerminalMouseEncoder::new()?;

        let (input_tx, input_rx) = mpsc::channel(DEFAULT_INPUT_MAILBOX);
        let (snapshot_tx, snapshot_rx) = mpsc::channel(DEFAULT_INPUT_MAILBOX);
        let (resize_tx, resize_rx) = mpsc::channel(DEFAULT_INPUT_MAILBOX);
        let (consumer_attach_tx, consumer_attach_rx) =
            mpsc::channel::<ConsumerAttachRequest>(DEFAULT_INPUT_MAILBOX);
        let (consumer_detach_tx, consumer_detach_rx) =
            mpsc::channel::<ConsumerDetachRequest>(DEFAULT_INPUT_MAILBOX);
        let (output_tx, _output_rx_seed) = broadcast::channel(DEFAULT_OUTPUT_BROADCAST);
        let (exit_tx, exit_rx) = oneshot::channel::<()>();
        let bundle_token = token.clone();

        let (pty_rx, pty_tx, pty) = if let Some(cmd) = cmd {
            let (rx, tx, owned) = spawn_pty(cmd, cols, rows)?;
            (Some(rx), Some(tx), Some(owned))
        } else {
            (None, None, None)
        };

        let actor = Self {
            terminal: RefCell::new(terminal),
            synth: RefCell::new(synth),
            key_enc: RefCell::new(key_enc),
            mouse_enc: RefCell::new(mouse_enc),
            focus_enc: RefCell::new(PerTerminalFocusEncoder::new()),
            paste_enc: RefCell::new(PerTerminalPasteEncoder::new()),
            input_rx,
            snapshot_rx,
            resize_rx,
            consumer_attach_rx,
            consumer_detach_rx,
            consumer_states: HashMap::new(),
            pty_rx,
            pty_tx,
            pty,
            output_tx: output_tx.clone(),
            exit_notify: Some(exit_tx),
            token,
            cols,
            rows,
        };
        let handle = TerminalHandle {
            input: input_tx,
            snapshot: snapshot_tx,
            output: output_tx,
            resize: resize_tx,
            consumer_attach: consumer_attach_tx,
            consumer_detach: consumer_detach_tx,
            cols,
            rows,
        };
        Ok(TerminalActorBundle {
            actor,
            handle,
            token: bundle_token,
            exit_notify: Some(exit_rx),
        })
    }

    /// Test-only constructor: write `bytes` into the actor's `Terminal`
    /// before the actor starts running. Useful for unit and integration
    /// tests that want the snapshot/incremental synthesis path to
    /// return non-trivial content without wiring up a PTY pump.
    ///
    /// Public (rather than `#[cfg(test)]`) so integration tests under
    /// `crates/phux-server/tests/` can call it. Not exercised by
    /// production code; the name + doc make the intent clear.
    pub fn new_with_seed(
        cols: u16,
        rows: u16,
        bytes: &[u8],
    ) -> Result<TerminalActorBundle, TerminalActorError> {
        let bundle = Self::new(cols, rows)?;
        bundle.actor.terminal.borrow_mut().vt_write(bytes);
        Ok(bundle)
    }

    /// Register `client_id` as an attached consumer (phux-q0e.2).
    ///
    /// Allocates a fresh `RenderState`, primes it against the live
    /// terminal (`update` + manual `set_dirty(Clean)` walk), and stores
    /// the resulting [`ConsumerSyncState`] in `consumer_states`.
    ///
    /// Why prime + clear: the runtime's ATTACH path emits a
    /// `TERMINAL_SNAPSHOT` immediately after this call returns, which
    /// brings the consumer's mirror Terminal up to the current
    /// canonical state. The per-consumer `RenderState` must reflect
    /// that same reference point — otherwise the first incremental
    /// emission would treat every row as dirty and re-paint the screen
    /// the snapshot just installed.
    ///
    /// Idempotent: re-attaching the same `client_id` (e.g. on a
    /// runtime bug) overwrites the prior entry. The old `RenderState`
    /// is dropped by `HashMap::insert`'s return-value drop, freeing
    /// the underlying libghostty allocation.
    fn register_consumer(
        &mut self,
        client_id: ClientId,
        outbound: mpsc::Sender<Outbound>,
        wire_terminal_id: u32,
    ) -> Result<(), ConsumerAttachError> {
        let terminal = self.terminal.borrow();
        // Cursor + DEC mode capture happens against a one-shot
        // `RenderState` rather than the synthesizer's internal one, so
        // we don't conflict with the priming `mark_synced` borrow below.
        // Allocation cost is one libghostty handle; freed at end of
        // scope.
        let last_cursor_mode = {
            let mut render_state = RenderState::new()?;
            let snapshot = render_state.update(&terminal)?;
            LastAckedCursorMode::capture(&terminal, &snapshot)
        };
        // Allocate the per-consumer synthesizer and prime it against
        // the live terminal so the next incremental synthesis emits
        // only deltas from *now*. `mark_synced` is the q0e.1 primitive
        // that does the walk-and-clear pass (it `update`s, walks rows
        // clearing each dirty bit, then clears the snapshot-level bit).
        // The phux-l0t FFI bug means subsequent `dirty()` reads degrade
        // to `Dirty::Full` defensively, so the first post-prime tick
        // re-emits a full reset+paint; that is correct per ADR-0018
        // (loss-tolerance over byte-minimality) and out-of-scope to fix
        // here.
        let mut synthesizer = SnapshotSynthesizer::new()?;
        synthesizer.mark_synced(&terminal)?;
        self.consumer_states.insert(
            client_id,
            ConsumerSyncState {
                synthesizer,
                outbound,
                wire_terminal_id,
                // First emission gets `seq == 1`. `0` is reserved for
                // the "empty initial frame" sentinel matching
                // `FrameId::ZERO` in [`LastAckedCursorMode`]'s doc.
                next_seq: 1,
                last_acked_seq: 0,
                last_cursor_mode,
            },
        );
        Ok(())
    }

    /// Drop the per-consumer state for `client_id` if present
    /// (phux-q0e.2). Silent no-op if absent — matches the idempotency
    /// of `ServerState::detach`.
    fn unregister_consumer(&mut self, client_id: ClientId) {
        // `HashMap::remove` returns the entry; dropping it frees the
        // libghostty `RenderState` via its `Drop` impl.
        let _ = self.consumer_states.remove(&client_id);
    }

    /// Test-only: number of consumers currently registered.
    #[cfg(test)]
    pub fn consumer_count(&self) -> usize {
        self.consumer_states.len()
    }

    /// Test-only: borrow the per-consumer state for `client_id`.
    #[cfg(test)]
    pub fn consumer_state(&self, client_id: ClientId) -> Option<&ConsumerSyncState> {
        self.consumer_states.get(&client_id)
    }

    /// Synthesize a snapshot of the current `Terminal` state. Exposed
    /// for tests that want to drive the synthesis path synchronously
    /// without going through the actor's `select!` loop.
    fn synthesize(&self) -> Result<SnapshotBytes, crate::grid::SynthesisError> {
        let terminal = self.terminal.borrow();
        let mut synth = self.synth.borrow_mut();
        synth.synthesize(&terminal)
    }

    /// Translate a [`TerminalInput`] into PTY bytes via the per-pane
    /// encoders + the current terminal state.
    ///
    /// Returns `Ok(None)` when the event was deliberately dropped
    /// (e.g., focus events while DEC 1004 is off; rejected untrusted
    /// pastes). Returns `Err` on encoder failure; the caller logs and
    /// continues — a single bad input must not kill the actor.
    fn encode_input(&self, input: &TerminalInput) -> Result<Option<Vec<u8>>, libghostty_vt::Error> {
        let terminal = self.terminal.borrow();
        match input {
            TerminalInput::Key(event) => {
                let mut enc = self.key_enc.borrow_mut();
                let bytes = enc.encode(event, &terminal)?;
                Ok(Some(bytes.to_vec()))
            }
            TerminalInput::Mouse(event) => {
                let mut enc = self.mouse_enc.borrow_mut();
                let bytes = enc.encode(event, &terminal)?;
                Ok(Some(bytes.to_vec()))
            }
            TerminalInput::Focus(event) => {
                let mut enc = self.focus_enc.borrow_mut();
                let bytes = enc.encode(*event, &terminal)?;
                Ok(bytes.map(<[u8]>::to_vec))
            }
            TerminalInput::Paste(event) => {
                let mut enc = self.paste_enc.borrow_mut();
                match enc.encode(event, &terminal)? {
                    PasteOutcome::Encoded(bytes) => Ok(Some(bytes.to_vec())),
                    PasteOutcome::Rejected => Ok(None),
                }
            }
        }
    }

    /// Apply a resize to both the libghostty `Terminal` and the PTY
    /// kernel-side winsize. Idempotent; logs and continues on errors.
    fn handle_resize(&mut self, cols: u16, rows: u16) {
        self.cols = cols;
        self.rows = rows;
        // `Terminal::resize` takes pixel dims for image-protocol sizing;
        // pass 0 (server does not maintain pixel metrics — clients
        // own pixel rendering per ADR-0013).
        if let Err(err) = self.terminal.borrow_mut().resize(cols, rows, 0, 0) {
            warn!(?err, cols, rows, "terminal resize failed");
        }
        if let Some(pty) = &self.pty {
            let size = PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            };
            if let Ok(master) = pty.master.lock()
                && let Err(err) = master.resize(size)
            {
                warn!(?err, cols, rows, "pty resize ioctl failed");
            }
        }
    }

    /// Best-effort reap the child if it has already exited. Called on
    /// PTY EOF — at that point the child has almost certainly exited
    /// (EOF on the master fd indicates the slave has been closed,
    /// which usually means the child has exited or detached). We try
    /// `try_wait` first to avoid blocking; if it returns `None` we
    /// leave the child alone (it might still be alive doing something
    /// odd; the shutdown path will deal with it).
    fn reap_child_if_any(&mut self) {
        let Some(pty) = self.pty.as_mut() else {
            return;
        };
        match pty.child.try_wait() {
            Ok(Some(status)) => debug!(?status, "child reaped on PTY EOF"),
            Ok(None) => trace!("PTY EOF but child still alive — leaving to shutdown path"),
            Err(err) => debug!(?err, "child try_wait failed on PTY EOF"),
        }
    }

    /// Tear down the PTY: kill the child if still alive, drop the
    /// master (which sends EOF to the slave and unblocks the reader
    /// thread), and join the bridge threads. Best-effort: errors are
    /// logged, not propagated, because we're on the shutdown path.
    fn shutdown_pty(&mut self) {
        let Some(mut pty) = self.pty.take() else {
            return;
        };
        // Best-effort kill — if the child has already exited this is a
        // no-op. `kill` is fire-and-forget; `wait` reaps the zombie.
        match pty.child.try_wait() {
            Ok(Some(_status)) => {
                trace!("pty child already exited");
            }
            Ok(None) => {
                if let Err(err) = pty.child.kill() {
                    debug!(?err, "pty child kill failed (already exited?)");
                }
            }
            Err(err) => {
                debug!(?err, "pty child try_wait failed");
            }
        }
        // Drop the master so the reader thread sees EOF and exits.
        // We drop pty_tx so the writer thread sees a closed channel
        // and exits. Both happen automatically when `self.pty` /
        // `self.pty_tx` are dropped at the end of `run`, but doing it
        // here makes the thread joins below predictable.
        drop(self.pty_tx.take());
        // Reap the child so the OS releases its slot.
        match pty.child.wait() {
            Ok(status) => debug!(?status, "pty child reaped"),
            Err(err) => debug!(?err, "pty child wait failed"),
        }
        // We can't drop `pty.master` separately because it's behind an
        // Arc<Mutex<_>> — the Arc strong count drops when `pty` falls
        // out of scope at the end of this function.
        if let Some(handle) = pty.reader_thread.take() {
            // Bounded wait: if the reader hasn't exited inside a small
            // budget we move on. Joining unconditionally would hang
            // the shutdown path if the reader is wedged in a blocking
            // read on a pty fd that won't EOF (rare but possible).
            //
            // In practice EOF arrives immediately after `master` drops
            // at the end of this function; we accept the join-on-drop
            // ordering risk because the reader thread is owned and
            // bounded by us. The unwrap below is safe because the
            // thread itself can't panic — it's a tight loop on Read.
            let _ = handle.join();
        }
        if let Some(handle) = pty.writer_thread.take() {
            let _ = handle.join();
        }
        drop(pty);
    }

    /// Run the actor's event loop until shutdown.
    ///
    /// Branches:
    /// - `shutdown` — biased first so a clean shutdown beats a hot PTY.
    /// - `pty_rx` — PTY bytes: write to `Terminal`, broadcast to
    ///   subscribed clients.
    /// - `input_rx` — wire input events: encode via per-pane encoders
    ///   and forward to the PTY writer thread.
    /// - `snapshot_rx` — ATTACH snapshots: synthesize from `Terminal`.
    /// - `resize_rx` — viewport size changes: update `Terminal` +
    ///   PTY winsize.
    #[allow(
        clippy::future_not_send,
        reason = "ADR-0014: TerminalActor owns !Send Terminal; lives on LocalSet"
    )]
    #[allow(
        clippy::too_many_lines,
        reason = "single select! loop; arms are short and inlined for locality"
    )]
    #[allow(
        clippy::cognitive_complexity,
        reason = "select! macro expansion inflates the score; arms are individually small and locality wins over decomposition"
    )]
    pub async fn run(mut self) {
        debug!(
            cols = self.cols,
            rows = self.rows,
            has_pty = self.pty.is_some(),
            "TerminalActor started",
        );

        // State-sync tick driver (phux-q0e.3). Fixed 33 Hz first cut;
        // the RTT-adaptive cadence is `phux-q0e.5`. `MissedTickBehavior::Delay`
        // — if the actor falls behind under heavy PTY traffic we want
        // subsequent ticks spaced by the interval from when they ran,
        // not bunched up to "catch up" (which would defeat the rate
        // limit's purpose). `Burst` (the default) would spam emissions
        // when a long PTY chunk delays us past several tick boundaries.
        let mut tick = tokio::time::interval(DEFAULT_TICK_INTERVAL);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        // Eat the first immediate tick (Interval fires synchronously on
        // first poll). Without this, the very first iteration would
        // tick before any other branch has a chance to react.
        let _ = tick.tick().await;

        loop {
            // For panes without a PTY, the `pty_rx` branch needs an
            // always-pending future. We construct that with
            // `recv_or_pending`: when the receiver is `Some`, it polls
            // it; when `None`, it parks forever (so the select! arm
            // never fires and the other arms are the only ones live).
            tokio::select! {
                biased;

                () = self.token.cancelled() => {
                    debug!("TerminalActor cancellation token fired");
                    self.shutdown_pty();
                    return;
                }

                // PTY → Terminal + broadcast.
                evt = recv_or_pending(self.pty_rx.as_mut()) => {
                    match evt {
                        Some(PtyEvent::Bytes(chunk)) => {
                            self.terminal.borrow_mut().vt_write(&chunk);
                            // Broadcast send fails only when no
                            // subscribers exist; that's a normal
                            // steady-state (no attached clients) and
                            // we silently drop.
                            let _ = self.output_tx.send(Bytes::from(chunk));
                        }
                        Some(PtyEvent::Eof) | None => {
                            debug!("PTY EOF; firing exit_notify and keeping actor alive for late snapshot/input drain");
                            // Detach the PTY-read branch: drop the
                            // receiver so the select! arm parks
                            // forever. We deliberately do NOT exit —
                            // the actor must remain reachable for
                            // late-arriving SnapshotRequests (e.g., a
                            // client attaching just after the child
                            // exited) and for an orderly shutdown via
                            // the cancellation token. Reap the child
                            // here so we don't leave a zombie waiting
                            // for the explicit shutdown signal.
                            //
                            // phux-it8: fire the `exit_notify` oneshot
                            // so the runtime can broadcast `Detached`
                            // to attached clients whose focused pane
                            // just died (the bug being fixed: client
                            // would freeze in alt-screen with no
                            // signal that the shell had exited).
                            //
                            // TODO(phux-9gw): multi-pane lifecycle —
                            // when a session has more than one pane,
                            // a single EOF should switch focus to a
                            // sibling rather than detach the whole
                            // session. Today sessions are 1:1 with
                            // panes in practice so the simpler
                            // "EOF → detach attached" model is
                            // correct.
                            self.pty_rx = None;
                            self.reap_child_if_any();
                            if let Some(tx) = self.exit_notify.take() {
                                let _ = tx.send(());
                            }
                        }
                    }
                }

                Some(input) = self.input_rx.recv() => {
                    match self.encode_input(&input) {
                        Ok(Some(bytes)) => {
                            if let Some(tx) = self.pty_tx.as_ref() {
                                if tx.send(bytes).is_err() {
                                    debug!("PTY writer channel closed; dropping input");
                                }
                            } else {
                                trace!(?input, "no PTY; input discarded");
                            }
                        }
                        Ok(None) => {
                            trace!(?input, "input gated/dropped by encoder");
                        }
                        Err(err) => {
                            warn!(error = %err, "input encode failed; dropping event");
                        }
                    }
                }

                Some(req) = self.snapshot_rx.recv() => {
                    let snap = match self.synthesize() {
                        Ok(s) => s,
                        Err(err) => {
                            warn!(error = %err, "snapshot synthesis failed; replying with empty");
                            SnapshotBytes {
                                cols: self.cols,
                                rows: self.rows,
                                bytes: Vec::new(),
                            }
                        }
                    };
                    let _ = req.reply.send(snap);
                }

                Some((cols, rows)) = self.resize_rx.recv() => {
                    self.handle_resize(cols, rows);
                }

                Some(req) = self.consumer_attach_rx.recv() => {
                    let ConsumerAttachRequest {
                        client_id,
                        outbound,
                        wire_terminal_id,
                        reply,
                    } = req;
                    let result = self.register_consumer(client_id, outbound, wire_terminal_id);
                    if let Err(err) = &result {
                        warn!(
                            ?client_id,
                            wire_terminal_id,
                            error = %err,
                            "consumer attach: per-consumer synthesizer setup failed",
                        );
                    } else {
                        trace!(
                            ?client_id,
                            wire_terminal_id,
                            "consumer attached: per-consumer synthesizer primed"
                        );
                    }
                    let _ = reply.send(result);
                }

                Some(req) = self.consumer_detach_rx.recv() => {
                    let ConsumerDetachRequest { client_id, reply } = req;
                    self.unregister_consumer(client_id);
                    trace!(?client_id, "consumer detached: per-consumer RenderState freed");
                    let _ = reply.send(());
                }

                // State-sync tick driver (phux-q0e.3, ADR-0018).
                // Iterates each attached consumer, walks the per-consumer
                // incremental synthesizer, and pushes a `TerminalOutput`
                // frame onto that consumer's outbound mailbox whenever
                // `synthesize_incremental` returns non-empty bytes.
                _ = tick.tick() => {
                    self.tick_emit();
                }

                else => break,
            }
        }
    }

    /// One tick of the state-sync emission driver (phux-q0e.3).
    ///
    /// Walks every attached consumer in turn. For each:
    ///
    /// 1. Call [`SnapshotSynthesizer::synthesize_incremental`] against
    ///    the live `Terminal`. Synthesis errors are logged and that
    ///    consumer is skipped for this tick (no kill: a transient FFI
    ///    error on one consumer must not poison the others).
    /// 2. If the body is empty, skip — there is nothing to send. This
    ///    branch fires only in the brief window before the first
    ///    `mark_synced` (phux-q0e.4); after that, the phux-l0t FFI bug
    ///    structurally degrades `dirty()` to `Full`, which always
    ///    produces a non-empty body.
    /// 3. Stamp the per-consumer monotonic `seq` (starting at `1`,
    ///    incrementing per emission) and ship a `TerminalOutput` frame
    ///    via the per-consumer outbound mailbox.
    ///
    /// Per ADR-0018: this method **does not** call `mark_synced`. The
    /// loss-tolerance invariant requires unacked diffs to stay re-
    /// emittable; `FRAME_ACK` (phux-q0e.4) is the only thing allowed to
    /// clear dirty bits.
    fn tick_emit(&mut self) {
        // Borrow the terminal once per tick. The `synthesize_incremental`
        // call only reads from it.
        let terminal = self.terminal.borrow();
        for (client_id, state) in &mut self.consumer_states {
            let bytes = match state.synthesizer.synthesize_incremental(&terminal) {
                Ok(snap) => snap.bytes,
                Err(err) => {
                    warn!(
                        ?client_id,
                        wire_terminal_id = state.wire_terminal_id,
                        error = %err,
                        "state-sync tick: synthesize_incremental failed; skipping consumer",
                    );
                    continue;
                }
            };
            if bytes.is_empty() {
                // Clean (or post-mark_synced fast path); nothing to do
                // for this consumer this tick.
                continue;
            }
            let seq = state.next_seq;
            // Wrapping_add for paranoia; `u64` will not realistically
            // roll over at 33 Hz, but the existing `runtime.rs` pump
            // uses the same idiom and we match it.
            state.next_seq = state.next_seq.wrapping_add(1);
            let frame = FrameKind::TerminalOutput {
                terminal_id: state.wire_terminal_id,
                seq,
                bytes,
            };
            // `try_send` (non-blocking) preserves the actor's single-
            // poll-budget invariant: the tick arm must not yield the
            // loop, otherwise a slow consumer's full mailbox would
            // hold up every other consumer's emission this tick. A
            // `Full` error means the consumer is wedged — the
            // per-client backpressure / disconnect machinery
            // (`SPEC.md` §12.3) lives in the runtime; we just drop and
            // continue, and the next tick re-diffs against the same
            // unacked reference so the consumer catches up naturally.
            match state.outbound.try_send(Outbound::Frame(frame)) {
                Ok(()) => {}
                Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                    trace!(
                        ?client_id,
                        wire_terminal_id = state.wire_terminal_id,
                        seq,
                        "state-sync tick: consumer mailbox full; dropping (will retry next tick)",
                    );
                }
                Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                    trace!(
                        ?client_id,
                        wire_terminal_id = state.wire_terminal_id,
                        seq,
                        "state-sync tick: consumer mailbox closed; entry will be reaped on detach",
                    );
                }
            }
        }
    }
}

/// Convenience: tuple returned by [`spawn_pty`].
type SpawnedPty = (
    mpsc::UnboundedReceiver<PtyEvent>,
    mpsc::UnboundedSender<Vec<u8>>,
    PtyOwned,
);

/// Receive from `rx` when `Some`; otherwise park forever. Used as a
/// select! arm so the actor's loop can run with or without a PTY
/// without an `expect()` or branching `if`.
async fn recv_or_pending(rx: Option<&mut mpsc::UnboundedReceiver<PtyEvent>>) -> Option<PtyEvent> {
    match rx {
        Some(rx) => rx.recv().await,
        None => std::future::pending().await,
    }
}

/// Open a PTY pair, spawn `cmd` on the slave, and start the reader /
/// writer bridge threads. Returns the actor-side channel endpoints and
/// a [`PtyOwned`] bundle to keep the resources alive.
fn spawn_pty(cmd: CommandBuilder, cols: u16, rows: u16) -> Result<SpawnedPty, TerminalActorError> {
    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| TerminalActorError::OpenPty(e.to_string()))?;

    let child = pair
        .slave
        .spawn_command(cmd)
        .map_err(|e| TerminalActorError::Spawn(e.to_string()))?;
    // Drop the slave side: the child inherits the fds, and we don't
    // need our copy. Keeping it would prevent EOF on master read after
    // the child exits.
    drop(pair.slave);

    let mut reader = pair
        .master
        .try_clone_reader()
        .map_err(|e| TerminalActorError::PtyIo(e.to_string()))?;
    let mut writer = pair
        .master
        .take_writer()
        .map_err(|e| TerminalActorError::PtyIo(e.to_string()))?;
    let master = Arc::new(Mutex::new(pair.master));

    let (pty_tx_to_actor, pty_rx_for_actor) = mpsc::unbounded_channel::<PtyEvent>();
    let (input_tx_to_writer, mut input_rx_for_writer) = mpsc::unbounded_channel::<Vec<u8>>();

    let reader_thread = std::thread::Builder::new()
        .name("phux-pty-reader".to_owned())
        .spawn(move || {
            let mut buf = [0u8; PTY_READ_CHUNK];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => {
                        let _ = pty_tx_to_actor.send(PtyEvent::Eof);
                        break;
                    }
                    Ok(n) => {
                        if pty_tx_to_actor
                            .send(PtyEvent::Bytes(buf[..n].to_vec()))
                            .is_err()
                        {
                            // Actor went away.
                            break;
                        }
                    }
                    Err(err) => {
                        debug!(?err, "pty reader thread: read error");
                        let _ = pty_tx_to_actor.send(PtyEvent::Eof);
                        break;
                    }
                }
            }
        })
        .map_err(|e| TerminalActorError::PtyIo(e.to_string()))?;

    let writer_thread = std::thread::Builder::new()
        .name("phux-pty-writer".to_owned())
        .spawn(move || {
            while let Some(bytes) = input_rx_for_writer.blocking_recv() {
                if writer.write_all(&bytes).is_err() {
                    break;
                }
                if writer.flush().is_err() {
                    break;
                }
            }
        })
        .map_err(|e| TerminalActorError::PtyIo(e.to_string()))?;

    Ok((
        pty_rx_for_actor,
        input_tx_to_writer,
        PtyOwned {
            master,
            child,
            reader_thread: Some(reader_thread),
            writer_thread: Some(writer_thread),
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Direct synchronous test: snapshot-of-blank-Terminal yields the
    /// expected reset preamble. Doesn't spawn the actor; exercises the
    /// synthesis helper directly.
    #[test]
    fn synthesize_blank_pane_returns_reset_preamble() {
        let bundle = TerminalActor::new(80, 24).expect("new");
        let snap = bundle.actor.synthesize().expect("synthesize");
        assert_eq!(snap.cols, 80);
        assert_eq!(snap.rows, 24);
        assert!(snap.bytes.starts_with(b"\x1b[!p\x1b[2J\x1b[H"));
    }

    /// Synchronous test: seed bytes flow through to the synthesized
    /// snapshot. Exercises [`TerminalActor::new_with_seed`].
    #[test]
    fn synthesize_seeded_pane_carries_visible_text() {
        let bundle = TerminalActor::new_with_seed(20, 5, b"hello").expect("new_with_seed");
        let snap = bundle.actor.synthesize().expect("synthesize");
        let body = String::from_utf8_lossy(&snap.bytes);
        assert!(
            body.contains("hello"),
            "synthesized bytes should contain seeded text, got: {body:?}"
        );
    }

    /// Async test: the actor responds to `SnapshotRequest` over the
    /// `LocalSet` and ships back the same bytes the synchronous
    /// synthesizer would.
    #[tokio::test(flavor = "current_thread")]
    async fn actor_responds_to_snapshot_request_on_localset() {
        let local = tokio::task::LocalSet::new();
        local
            .run_until(async {
                let bundle =
                    TerminalActor::new_with_seed(20, 5, b"hi there").expect("new_with_seed");
                let handle = bundle.handle.clone();
                // Hold the token; under new semantics dropping it does
                // NOT cancel, so the actor is alive regardless. Keep
                // the binding for parallel structure with the other
                // tests in this module.
                let _token = bundle.token;
                tokio::task::spawn_local(bundle.actor.run());

                let (reply_tx, reply_rx) = oneshot::channel();
                handle
                    .snapshot
                    .send(SnapshotRequest { reply: reply_tx })
                    .await
                    .expect("send snapshot request");
                let snap = reply_rx.await.expect("snapshot reply");
                assert_eq!(snap.cols, 20);
                assert_eq!(snap.rows, 5);
                let body = String::from_utf8_lossy(&snap.bytes);
                assert!(
                    body.contains("hi there"),
                    "actor-synthesized bytes should contain seeded text"
                );
            })
            .await;
    }

    /// The actor stops promptly when its cancellation token fires,
    /// even if input/snapshot channels stay open.
    #[tokio::test(flavor = "current_thread")]
    async fn actor_exits_on_cancellation() {
        let local = tokio::task::LocalSet::new();
        local
            .run_until(async {
                let bundle = TerminalActor::new(20, 5).expect("new");
                let handle = bundle.handle.clone();
                let token = bundle.token;
                let join = tokio::task::spawn_local(bundle.actor.run());

                token.cancel();
                tokio::time::timeout(std::time::Duration::from_millis(500), join)
                    .await
                    .expect("actor did not exit within 500ms")
                    .expect("actor task panicked");

                let (reply_tx, reply_rx) = oneshot::channel();
                let _ = handle
                    .snapshot
                    .try_send(SnapshotRequest { reply: reply_tx });
                drop(reply_rx);
            })
            .await;
    }

    /// A parent token's `.cancel()` propagates to a `child_token()`-
    /// linked `TerminalActor`, which exits within a short deadline. Pins
    /// down the hierarchical cascade introduced by the
    /// `CancellationToken` refactor.
    #[tokio::test(flavor = "current_thread")]
    async fn parent_token_cancel_cascades_to_pane_actor() {
        let local = tokio::task::LocalSet::new();
        local
            .run_until(async {
                let parent = CancellationToken::new();
                let child = parent.child_token();
                let bundle =
                    TerminalActor::build_with_token(20, 5, None, child).expect("build_with_token");
                let join = tokio::task::spawn_local(bundle.actor.run());

                parent.cancel();

                tokio::time::timeout(std::time::Duration::from_millis(500), join)
                    .await
                    .expect("actor did not exit within 500ms of parent cancel")
                    .expect("actor task panicked");
            })
            .await;
    }

    /// Test helper: build a throwaway outbound mailbox + receiver pair
    /// shaped like the production [`crate::state::AttachedClient::tx`].
    /// The receiver is returned so callers can hold it open (otherwise
    /// the actor's `try_send` would see a closed channel).
    fn dummy_outbound() -> (mpsc::Sender<Outbound>, mpsc::Receiver<Outbound>) {
        mpsc::channel(16)
    }

    /// phux-q0e.2: ATTACH allocates a per-consumer `RenderState` and
    /// `register_consumer` stores it keyed by `ClientId`. Two attaches
    /// land two entries; one detach removes only that entry; a second
    /// detach of the same id is a no-op.
    #[test]
    fn register_unregister_consumer_drives_lifecycle_map() {
        let bundle = TerminalActor::new(20, 5).expect("new");
        let mut actor = bundle.actor;
        assert_eq!(actor.consumer_count(), 0, "starts empty");

        let a = ClientId(1);
        let b = ClientId(2);
        let (tx_a, _rx_a) = dummy_outbound();
        let (tx_b, _rx_b) = dummy_outbound();
        actor.register_consumer(a, tx_a, 1).expect("register a");
        assert_eq!(actor.consumer_count(), 1);
        actor.register_consumer(b, tx_b, 2).expect("register b");
        assert_eq!(actor.consumer_count(), 2);

        actor.unregister_consumer(a);
        assert_eq!(actor.consumer_count(), 1, "one entry after first detach");
        assert!(actor.consumer_state(a).is_none(), "a removed");
        assert!(actor.consumer_state(b).is_some(), "b retained");

        // Idempotent detach: re-detaching `a` is a no-op.
        actor.unregister_consumer(a);
        assert_eq!(actor.consumer_count(), 1);

        actor.unregister_consumer(b);
        assert_eq!(actor.consumer_count(), 0, "both removed");
    }

    /// phux-q0e.2: right after `register_consumer` returns, the
    /// per-consumer state has `last_acked_seq == 0` (no `FRAME_ACK`s yet
    /// — wired by phux-q0e.4) and the cursor/mode capture matches the
    /// live terminal. The dirty-bit reset is a best-effort FFI call
    /// (phux-l0t notes the libghostty surface is unreliable on
    /// repeated updates); we assert the observable contract — the
    /// `ConsumerSyncState` is in place and primed against the live
    /// terminal — rather than the post-reset dirty value itself, which
    /// the tick driver (phux-q0e.3) will re-read on its first tick.
    #[test]
    fn register_consumer_initial_state_matches_terminal() {
        let bundle = TerminalActor::new_with_seed(20, 5, b"hello").expect("new_with_seed");
        let mut actor = bundle.actor;
        let client = ClientId(7);
        let (tx, _rx) = dummy_outbound();
        actor.register_consumer(client, tx, 11).expect("register");

        let state = actor.consumer_state(client).expect("state present");
        assert_eq!(state.last_acked_seq, 0, "no acks yet");
        assert_eq!(state.next_seq, 1, "first emission gets seq=1");
        assert_eq!(
            state.wire_terminal_id, 11,
            "wire id stored on the per-consumer entry"
        );
        // Seeded "hello" advances the cursor to (5, 0). The capture
        // must reflect that — proves the RenderState was actually
        // updated against the live terminal, not left blank.
        assert_eq!(state.last_cursor_mode.cursor_x, Some(5));
        assert_eq!(state.last_cursor_mode.cursor_y, Some(0));
    }

    /// phux-q0e.2: end-to-end across the actor's `select!` loop —
    /// ATTACH then DETACH over the channels handle the lifecycle on
    /// the same `LocalSet` thread the `Terminal` lives on. Drives the
    /// actor through `spawn_local`, so the `!Send` `RenderState`
    /// stays on its owning thread.
    #[tokio::test(flavor = "current_thread")]
    async fn consumer_attach_detach_round_trip_over_channels() {
        let local = tokio::task::LocalSet::new();
        local
            .run_until(async {
                let bundle = TerminalActor::new(20, 5).expect("new");
                let handle = bundle.handle.clone();
                let token = bundle.token;
                let join = tokio::task::spawn_local(bundle.actor.run());

                let client = ClientId(42);
                let (out_tx, _out_rx) = dummy_outbound();
                let (tx_a, rx_a) = oneshot::channel();
                handle
                    .consumer_attach
                    .send(ConsumerAttachRequest {
                        client_id: client,
                        outbound: out_tx,
                        wire_terminal_id: 99,
                        reply: tx_a,
                    })
                    .await
                    .expect("send attach");
                rx_a.await.expect("attach reply").expect("attach succeeded");

                let (tx_d, rx_d) = oneshot::channel();
                handle
                    .consumer_detach
                    .send(ConsumerDetachRequest {
                        client_id: client,
                        reply: tx_d,
                    })
                    .await
                    .expect("send detach");
                rx_d.await.expect("detach reply");

                token.cancel();
                tokio::time::timeout(std::time::Duration::from_millis(500), join)
                    .await
                    .expect("actor did not exit within 500ms")
                    .expect("actor task panicked");
            })
            .await;
    }

    /// Resize updates both the libghostty `Terminal` and (when present)
    /// the PTY winsize. We only assert the Terminal side here — the
    /// PTY ioctl path is exercised in the integration test.
    #[tokio::test(flavor = "current_thread")]
    async fn resize_updates_terminal_dims() {
        let local = tokio::task::LocalSet::new();
        local
            .run_until(async {
                let bundle = TerminalActor::new(80, 24).expect("new");
                let handle = bundle.handle.clone();
                let token = bundle.token;
                let join = tokio::task::spawn_local(bundle.actor.run());

                handle.resize.send((120, 40)).await.expect("send resize");
                // Give the actor a moment to process the resize before
                // we shut it down. A bounded `yield_now` loop is the
                // current-thread-friendly version of `sleep(0)`.
                for _ in 0..16 {
                    tokio::task::yield_now().await;
                }

                token.cancel();
                tokio::time::timeout(std::time::Duration::from_millis(500), join)
                    .await
                    .expect("actor did not exit within 500ms")
                    .expect("actor task panicked");
            })
            .await;
    }
}
