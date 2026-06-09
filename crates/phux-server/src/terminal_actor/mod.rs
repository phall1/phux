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
//! `tokio::sync::mpsc` channels. This avoids OS-specific `AsyncFd`
//! plumbing for a feature whose value (a few PTY fds, not hundreds)
//! doesn't justify the complexity. At typical phux pane counts (1–20)
//! the per-pane thread cost is invisible against everything else the
//! server does.
//!
//! # Why `bytes::Bytes` for the output broadcast
//!
//! `tokio::sync::broadcast::Sender` requires `Clone` payloads (every
//! subscriber receives a copy of the same value). `bytes::Bytes` is the
//! standard cheap-clone byte buffer in the tokio ecosystem; `Vec<u8>`
//! would also work but at the cost of a full clone per subscriber.

use std::cell::RefCell;
use std::collections::HashMap;

use bytes::Bytes;
use libghostty_vt::{RenderState, Terminal as GhosttyTerminal, TerminalOptions};
use phux_protocol::ClientId;
use phux_protocol::wire::frame::{AgentEvent, FrameKind, TerminalEventType};
use portable_pty::{CommandBuilder, PtySize};
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio_util::sync::CancellationToken;
use tracing::{debug, trace, warn};

use crate::grid::{ConsumerReference, SnapshotBytes, SnapshotSynthesizer};
use crate::input::paste::PasteOutcome;
use crate::input::{
    PerTerminalFocusEncoder, PerTerminalKeyEncoder, PerTerminalMouseEncoder,
    PerTerminalPasteEncoder,
};
use crate::state::{Outbound, TerminalInput};

pub mod requests;
pub mod spawn;
pub mod sync;
pub mod tick;

pub use requests::*;
pub use spawn::*;
pub use sync::*;
pub use tick::*;

/// Per-Terminal scrollback cap used by the no-config convenience
/// constructors ([`TerminalActor::new`] / [`TerminalActor::new_with_command`]).
/// A tmux-style mid-range value; the runtime path overrides it with
/// `defaults.history-limit` via [`TerminalActor::build_with_token`].
const DEFAULT_MAX_SCROLLBACK: u32 = 10_000;

/// Upper bound on consecutive ready PTY chunks coalesced into a single
/// `vt_write` + broadcast frame per pump wakeup (phux-ahk burst path). A
/// heavy neovim redraw or p10k repaint arrives as many ~4KB reads; coalescing
/// collapses the per-chunk Terminal write, broadcast frame, and downstream
/// socket write into one. Bounded so a process emitting an unbroken stream
/// can't monopolize the actor's `select!` loop (starving input / snapshot
/// requests) — at 4KB/chunk this caps one drain at ~256KB.
const MAX_PTY_COALESCE: usize = 64;

/// Byte cap on one coalesced `vt_write` payload. A heavy redraw arrives
/// as many ~4KB reads; coalescing still collapses them into one frame,
/// but a single `vt_write` is a synchronous libghostty parse that blocks
/// the actor loop (and thus the input arm polled before it) for its full
/// duration. Capping at 48KB keeps a typical neovim / p10k repaint in one
/// frame while bounding the worst-case parse, so a queued keystroke
/// interleaves after at most one capped parse. libghostty's VT parser is
/// a streaming state machine, so splitting the byte stream on this
/// boundary loses no escape sequence — bytes are never reordered. Paired
/// with `MAX_INPUT_COALESCE`, this is the load-bearing bound on the output
/// arm: the two consts together keep either direction from monopolizing
/// the single-thread actor loop.
const MAX_PTY_COALESCE_BYTES: usize = 48 * 1024;

/// Upper bound on input events drained in a single `input_rx` wakeup
/// before returning to the `select!`. Input events are tiny (one encode +
/// channel send each) and `input_rx` is a bounded, low-rate single-client
/// mailbox that empties in microseconds, so in steady state the PTY-output
/// arm wins as soon as the mailbox drains. This cap bounds a single
/// pathological batch (a paste that the encoder expands, or a burst of
/// queued keys) so it cannot inflate one `input_rx` turn without limit; it
/// does not by itself force a yield to output. The structural output bound
/// is `MAX_PTY_COALESCE_BYTES`.
const MAX_INPUT_COALESCE: usize = 16;

/// Per-pane actor. Owns the `Terminal`, the PTY master, the per-pane
/// input encoders, and serves the channels exposed via [`TerminalHandle`].
///
/// `GhosttyTerminal<'static, 'static>` because we use [`GhosttyTerminal::new`] (NULL
/// allocator) — the lifetime parameters degenerate to `'static`. A
/// future custom allocator path would tie this to the surrounding
/// arena's lifetime; not needed for `phux-byc.5`.
///
/// `Terminal`, encoders, and the `SnapshotSynthesizer` are stashed
/// inside `RefCell` so the `select!` arms (which conceptually borrow
/// `&mut self`) can each take what they need without fighting the
/// borrow checker over disjoint field access.
#[allow(
    clippy::struct_excessive_bools,
    reason = "DEC mode bits and internal state flags are independent; collapsing them would obscure individual semantics"
)]
pub struct TerminalActor {
    terminal: RefCell<GhosttyTerminal<'static, 'static>>,
    synth: RefCell<SnapshotSynthesizer<'static>>,
    /// Cheap idle short-circuit for [`Self::tick_emit`] (phux-4l0).
    ///
    /// `true` whenever the canonical [`libghostty_vt::Terminal`] has been mutated
    /// (`vt_write`, resize) since the last `tick_emit`. Set at every
    /// mutation point, cleared at the top of each `tick_emit`. When this
    /// is `false` AND no consumer is awaiting its first emission, the
    /// per-consumer row walk is skipped entirely — an idle pane with N
    /// consumers then costs O(1) per tick instead of O(N * rows) row
    /// renders + allocations.
    ///
    /// Deliberately independent of libghostty's `RenderState`/`Snapshot`
    /// dirty bits: those are *consumed* (cleared) by ANY `RenderState::update`
    /// on the shared terminal (see
    /// [`crate::grid::SnapshotSynthesizer::synthesize_against_reference`]),
    /// including the one-shot updates in snapshot/screen/attach handling,
    /// so probing them here could miss a write a sibling handler already
    /// consumed. A self-owned flag cannot be clobbered that way.
    terminal_dirty_since_tick: bool,
    key_enc: RefCell<PerTerminalKeyEncoder>,
    mouse_enc: RefCell<PerTerminalMouseEncoder>,
    focus_enc: RefCell<PerTerminalFocusEncoder>,
    paste_enc: RefCell<PerTerminalPasteEncoder>,
    input_rx: mpsc::Receiver<TerminalInput>,
    snapshot_rx: mpsc::Receiver<SnapshotRequest>,
    screen_rx: mpsc::Receiver<ScreenRequest>,
    pwd_rx: mpsc::Receiver<PwdRequest>,
    resize_rx: mpsc::Receiver<ResizeRequest>,
    consumer_attach_rx: mpsc::Receiver<ConsumerAttachRequest>,
    consumer_detach_rx: mpsc::Receiver<ConsumerDetachRequest>,
    /// Per-consumer state-sync `FRAME_ACK` channel (phux-q0e.4). Drained
    /// by a select! arm that walks `consumer_states[client_id]` and
    /// advances `last_acked_seq` (the reference itself advances on emit).
    consumer_ack_rx: mpsc::Receiver<ConsumerAckRequest>,
    /// Per-consumer state-sync cache (ADR-0018, phux-q0e.2). Keyed by
    /// the [`ClientId`] the runtime uses for subscription tracking in
    /// [`crate::state::ServerState`]; entries are inserted by the
    /// ATTACH handler and removed by DETACH. `!Send` because the actor
    /// holds the `!Send` `Terminal` — fine; the whole actor lives on the
    /// `LocalSet` thread (ADR-0014).
    consumer_states: HashMap<ClientId, ConsumerSyncState>,
    /// Whether the per-consumer state-sync tick (phux-q0e.3) is the live
    /// emitter of `TerminalOutput` frames (ADR-0018).
    ///
    /// `false` in production for human TUI attach (phux-yeca). Raw PTY
    /// bytes are the byte-faithful, low-latency human path; synthesized
    /// per-consumer ticks are reserved for explicitly negotiated
    /// state-sync consumers. When `true`, the tick is the live
    /// server->client emission path: per attached consumer it diffs the
    /// live `Terminal` against that consumer's own
    /// [`crate::grid::ConsumerReference`] (via the actor's shared
    /// [`SnapshotSynthesizer`]) and pushes only the delta with a
    /// per-consumer monotonic `seq`. The reference advances on emit
    /// (emit-once); the runtime suppresses its broadcast pump for any
    /// tick-managed consumer so exactly one emitter serves each consumer.
    ///
    /// Three prerequisites had to land before this can be enabled for a
    /// negotiated consumer: all are met mechanically, but human attach stays
    /// raw until phux-fseo adds an explicit mode boundary.
    ///
    /// 1. **Single emitter (phux-3uv).** The runtime's `handle_attach`
    ///    suppresses its raw PTY-byte broadcast pump for any consumer this
    ///    actor reports as tick-managed (via
    ///    [`ConsumerAttachOutcome::tick_managed`]). Without that a
    ///    tick-emitted `TerminalOutput` and the broadcast pump's would both
    ///    land on the same consumer mailbox with independent `seq` —
    ///    double-paint, non-monotonic `seq` (proto.md §8.2).
    /// 2. **Client `FRAME_ACK` loop (phux-3uv).** The client drives
    ///    `FRAME_ACK`, advancing the server's `last_acked_seq` for
    ///    backpressure accounting (proto.md §8.2).
    /// 3. **Per-consumer dirty isolation (phux-ia4).** `RenderState::update`
    ///    *consumes* the shared `Terminal` dirty state on first read each
    ///    tick (libghostty `render.zig`), which starved all-but-one
    ///    consumer on a shared pane under the old per-consumer-`RenderState`
    ///    dirty model. Resolved by diffing each consumer against its own
    ///    [`crate::grid::ConsumerReference`] (rendered row bodies), which
    ///    never reads the shared dirty bits — full per-consumer isolation
    ///    regardless of attach/ack divergence.
    ///
    /// Tests may set it either way via the test-only setters; production
    /// leaves it `false` until output mode negotiation exists.
    consumer_tick_emits: bool,
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
    output_tx: broadcast::Sender<PaneOutput>,
    /// One-shot fired when the actor observes PTY EOF. Paired with the
    /// matching receiver in [`TerminalActorBundle::exit_notify`]; the
    /// runtime uses it to drive client-detach on shell exit (phux-it8).
    ///
    /// `Option` so the actor can `.take()` it after firing — sending on
    /// a `oneshot::Sender` is a by-value move. `None` after the first
    /// fire or if the bundle's receiver was never created (the test
    /// constructor [`TerminalActor::new_with_seed`] leaves it `Some` too,
    /// but no consumer subscribes; the `.ok()` swallow is benign).
    ///
    /// Carries the child's exit status when known: `Some(code)` for a
    /// normal `_exit(n)`, `None` for signal-killed children or
    /// otherwise-unknown exits (phux-4li.11; the structured exit code
    /// flows into the `TERMINAL_CLOSED` wire frame the runtime emits on
    /// PTY EOF).
    exit_notify: Option<oneshot::Sender<Option<i32>>>,
    /// Cancellation token watched by the actor's `select!`. Cancel to
    /// ask the actor to shut down cleanly (drains the PTY, reaps the
    /// child, and exits). A child token of the per-server root token
    /// when constructed via [`Self::build_with_token`]; an unlinked
    /// fresh token when constructed via [`TerminalActor::new`] et al.
    /// Dropping the token does NOT cancel — call `.cancel()` explicitly
    /// (this is intentional; the prior `oneshot::Sender::drop` semantics
    /// were a hidden lifecycle coupling we want gone).
    token: CancellationToken,
    /// Optional sink for agent events the actor sources from the PTY
    /// stream (SPEC §7.5, phux-y2t): `bell`, `title_changed`, `dirty`,
    /// `idle`, and the OSC-133-sourced `command_started` / `command_finished`.
    /// `None` for actors that no one watches (most tests); set by the
    /// runtime's spawn path via [`Self::set_event_sink`]. The runtime
    /// drains this channel and fans each event out to event-stream
    /// subscribers scoped to this pane (it owns the wire `TerminalId`,
    /// which the actor does not know).
    ///
    /// `try_send` semantics: a full sink drops the event rather than
    /// stalling the hot PTY-pump loop — the event stream is an
    /// accelerator, not a guarantee (a dropped event just falls back to
    /// the CLI poll floor).
    event_sink: Option<mpsc::Sender<AgentEvent>>,
    /// Last terminal title observed (OSC 0 / OSC 2), for change detection.
    /// `title_changed` fires only when the polled title differs from this.
    last_title: String,
    /// Whether the pane is currently in an active output "burst": a
    /// `dirty` event has been emitted and no settling `idle` has followed.
    /// Drives the dirty/idle coalescing — at most one `dirty` per burst,
    /// then one `idle` when a tick observes the grid has settled.
    in_output_burst: bool,
    /// Event subscribers for this pane. When semantic state changes occur
    /// (command started, grid changed, etc.), broadcast to all subscribers
    /// whose `event_types` filter matches. `Vec` guarded by `RefCell` for
    /// interior mutability (single-threaded actor, no lock contention).
    /// Subscribers added by `handle_subscribe_terminal_events` and removed
    /// implicitly on detach.
    event_subscribers: RefCell<Vec<TerminalEventSubscriber>>,
    /// Last known working directory for this pane. Used to detect CWD
    /// changes and emit `CwdChanged` events. Queried lazily on prompt via
    /// `process_cwd` (kernel fcntl `F_GETPATH` on macOS, /proc/PID/cwd on Linux).
    #[allow(dead_code, reason = "reserved for future CwdChanged event emission")]
    last_known_cwd: RefCell<String>,
    /// Whether we've already emitted a Dirty event in the current output
    /// burst. Coalesces multiple grid mutations into one event per burst
    /// (matching the `in_output_burst` coalescing for `AgentEvent`).
    dirty_event_emitted_this_burst: bool,
    /// Inbound subscription request channel. Drained by a select! arm
    /// that calls `subscribe_to_events`.
    subscribe_to_events_rx: mpsc::Receiver<SubscribeToEventsRequest>,
    /// Inbound unsubscription request channel. Drained by a select! arm
    /// that calls `unsubscribe_from_events`.
    unsubscribe_from_events_rx: mpsc::Receiver<UnsubscribeFromEventsRequest>,
    /// Wire-level terminal id (for Event frames). Set by the runtime
    /// during subscription registration. `0` until a subscriber arrives.
    wire_terminal_id: u32,
    cols: u16,
    rows: u16,
}

/// Errors surfaced while constructing a [`TerminalActor`].
#[derive(Debug, thiserror::Error)]
pub enum TerminalActorError {
    /// Libghostty refused to allocate a Terminal or input encoder.
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
    ///
    /// The payload is the child's exit status: `Some(code)` on a normal
    /// `_exit(n)` (or where the kernel reports a code at all), `None`
    /// for signal-killed children or unknown-cause exits. Mirrors the
    /// `TERMINAL_CLOSED.exit_status` wire field exactly (phux-4li.11).
    pub exit_notify: Option<oneshot::Receiver<Option<i32>>>,
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
impl TerminalActor {
    /// Build a fresh actor of the given dimensions **without** a backing
    /// PTY. Used by tests that exercise snapshot / shutdown semantics
    /// without driving a real process.
    ///
    /// The `GhosttyTerminal` is allocated via libghostty's default allocator
    /// (NULL alloc → `'static` lifetimes). `max_scrollback` is
    /// `DEFAULT_MAX_SCROLLBACK` — a tmux-style mid-range value the
    /// runtime overrides with `defaults.history-limit` via
    /// [`Self::build_with_token`].
    #[allow(clippy::new_ret_no_self, reason = "bundle-shaped constructor")]
    pub fn new(cols: u16, rows: u16) -> Result<TerminalActorBundle, TerminalActorError> {
        Self::build(
            cols,
            rows,
            None,
            DEFAULT_MAX_SCROLLBACK,
            CancellationToken::new(),
        )
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
        Self::build(
            cols,
            rows,
            Some(cmd),
            DEFAULT_MAX_SCROLLBACK,
            CancellationToken::new(),
        )
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
        max_scrollback: u32,
        token: CancellationToken,
    ) -> Result<TerminalActorBundle, TerminalActorError> {
        Self::build(cols, rows, cmd, max_scrollback, token)
    }

    fn build(
        cols: u16,
        rows: u16,
        cmd: Option<CommandBuilder>,
        max_scrollback: u32,
        token: CancellationToken,
    ) -> Result<TerminalActorBundle, TerminalActorError> {
        let terminal = GhosttyTerminal::new(TerminalOptions {
            cols,
            rows,
            // `defaults.history-limit` is a `u32` on the wire/config; the
            // libghostty option is `usize`. The widen is lossless on all
            // supported targets.
            max_scrollback: max_scrollback as usize,
        })?;
        let synth = SnapshotSynthesizer::new()?;
        let key_enc = PerTerminalKeyEncoder::new()?;
        let mouse_enc = PerTerminalMouseEncoder::new()?;

        let (input_tx, input_rx) = mpsc::channel(DEFAULT_INPUT_MAILBOX);
        let (snapshot_tx, snapshot_rx) = mpsc::channel(DEFAULT_INPUT_MAILBOX);
        let (screen_tx, screen_rx) = mpsc::channel(DEFAULT_INPUT_MAILBOX);
        let (pwd_tx, pwd_rx) = mpsc::channel(DEFAULT_INPUT_MAILBOX);
        let (resize_tx, resize_rx) = mpsc::channel(DEFAULT_INPUT_MAILBOX);
        let (consumer_attach_tx, consumer_attach_rx) =
            mpsc::channel::<ConsumerAttachRequest>(DEFAULT_INPUT_MAILBOX);
        let (consumer_detach_tx, consumer_detach_rx) =
            mpsc::channel::<ConsumerDetachRequest>(DEFAULT_INPUT_MAILBOX);
        let (consumer_ack_tx, consumer_ack_rx) =
            mpsc::channel::<ConsumerAckRequest>(DEFAULT_INPUT_MAILBOX);
        let (subscribe_to_events_tx, subscribe_to_events_rx) =
            mpsc::channel::<SubscribeToEventsRequest>(DEFAULT_INPUT_MAILBOX);
        let (unsubscribe_from_events_tx, unsubscribe_from_events_rx) =
            mpsc::channel::<UnsubscribeFromEventsRequest>(DEFAULT_INPUT_MAILBOX);
        let (output_tx, _output_rx_seed) = broadcast::channel(DEFAULT_OUTPUT_BROADCAST);
        let (exit_tx, exit_rx) = oneshot::channel::<Option<i32>>();
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
            // A pane may carry initial content (PTY banner, restored
            // scrollback); start dirty so the first tick always emits.
            terminal_dirty_since_tick: true,
            key_enc: RefCell::new(key_enc),
            mouse_enc: RefCell::new(mouse_enc),
            focus_enc: RefCell::new(PerTerminalFocusEncoder::new()),
            paste_enc: RefCell::new(PerTerminalPasteEncoder::new()),
            input_rx,
            snapshot_rx,
            screen_rx,
            pwd_rx,
            resize_rx,
            consumer_attach_rx,
            consumer_detach_rx,
            consumer_ack_rx,
            consumer_states: HashMap::new(),
            // phux-yeca: keep the human attach path on the raw PTY
            // broadcast pump by default. The per-consumer synthesized-VT
            // tick path is correct for state-sync experiments, but as the
            // sole emitter it adds a visible 20-30 ms floor to local typing
            // and can lose byte-exact styling that interactive shells/TUIs
            // rely on. Tests can still flip this on explicitly with
            // `enable_tick_emit_for_test`; production needs a negotiated
            // consumer mode before making synthesized ticks the human path.
            consumer_tick_emits: false,
            pty_rx,
            pty_tx,
            pty,
            output_tx: output_tx.clone(),
            exit_notify: Some(exit_tx),
            token,
            event_sink: None,
            last_title: String::new(),
            in_output_burst: false,
            event_subscribers: RefCell::new(Vec::new()),
            last_known_cwd: RefCell::new(std::env::var("HOME").unwrap_or_default()),
            dirty_event_emitted_this_burst: false,
            subscribe_to_events_rx,
            unsubscribe_from_events_rx,
            wire_terminal_id: 0,
            cols,
            rows,
        };
        let handle = TerminalHandle {
            input: input_tx,
            snapshot: snapshot_tx,
            screen: screen_tx,
            pwd: pwd_tx,
            output: output_tx,
            resize: resize_tx,
            consumer_attach: consumer_attach_tx,
            consumer_detach: consumer_detach_tx,
            consumer_ack: consumer_ack_tx,
            subscribe_to_events: subscribe_to_events_tx,
            unsubscribe_from_events: unsubscribe_from_events_tx,
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
    /// canonical state. The per-consumer reference must reflect that same
    /// reference point — otherwise the first incremental emission would
    /// treat every row as changed and re-paint the screen the snapshot
    /// just installed.
    ///
    /// Idempotent: re-attaching the same `client_id` (e.g. on a runtime
    /// bug) overwrites the prior entry.
    fn register_consumer(
        &mut self,
        client_id: ClientId,
        outbound: mpsc::Sender<Outbound>,
        wire_terminal_id: u32,
        wants_state_sync: bool,
    ) -> Result<(), ConsumerAttachError> {
        // Priming the per-consumer reference + cursor/mode capture costs two
        // full-grid render passes, but a raw broadcast-pump consumer (the
        // human attach path) never reads either: the tick serves only
        // tick-managed consumers and `FRAME_ACK` is dropped for raw ones, and
        // `wants_state_sync` is fixed at registration with no flip path. So
        // do the work only when this consumer is actually tick-managed; a raw
        // consumer attaches with an empty reference and a placeholder capture.
        // (If it were ever tick-served, `needs_initial_emit` forces a full
        // pass that primes both — see the tick emit gate.)
        let tick_managed = wants_state_sync || self.consumer_tick_emits;
        let (last_cursor_mode, reference) = if tick_managed {
            let terminal = self.terminal.borrow();
            // Cursor + DEC mode capture happens against a one-shot
            // `RenderState` so we don't conflict with the shared
            // synthesizer's borrow used to prime the reference below.
            let last_cursor_mode = {
                let mut render_state = RenderState::new()?;
                let snapshot = render_state.update(&terminal)?;
                LastAckedCursorMode::capture(&terminal, &snapshot)
            };
            // Prime the reference against the live terminal so the next
            // `synthesize_against_reference` emits only deltas from *now* —
            // the `TERMINAL_SNAPSHOT` the runtime emits right after this call
            // already brings the consumer's mirror to this same point.
            let mut reference = ConsumerReference::new();
            self.synth
                .borrow_mut()
                .prime_reference(&terminal, &mut reference)?;
            (last_cursor_mode, reference)
        } else {
            (LastAckedCursorMode::unprimed(), ConsumerReference::new())
        };
        self.consumer_states.insert(
            client_id,
            ConsumerSyncState {
                reference,
                outbound,
                wire_terminal_id,
                // First emission gets `seq == 1`. `0` is reserved for
                // the "empty initial frame" sentinel matching
                // `FrameId::ZERO` in [`LastAckedCursorMode`]'s doc.
                next_seq: 1,
                last_acked_seq: 0,
                last_cursor_mode,
                // Force one synthesis pass on the next tick even if the
                // terminal is Clean since the previous tick (phux-4l0).
                needs_initial_emit: true,
                // Fresh consumer is not behind; `needs_initial_emit` already
                // guarantees its first pass runs.
                behind: false,
                // No RTT sample yet — runs at the cold-start default until the
                // first FRAME_ACK round-trip lands (phux-q0e.5).
                rtt: RttEstimator::default(),
                emit_instants: std::collections::BTreeMap::new(),
                wants_state_sync,
            },
        );
        Ok(())
    }

    /// Drop the per-consumer state for `client_id` if present
    /// (phux-q0e.2). Silent no-op if absent — matches the idempotency
    /// of `ServerState::detach`.
    fn unregister_consumer(&mut self, client_id: ClientId) {
        // `HashMap::remove` returns the entry; dropping it frees the
        // per-consumer reference grid.
        let _ = self.consumer_states.remove(&client_id);
    }

    /// Handle an inbound `FRAME_ACK` from `client_id` carrying cumulative
    /// `seq` (phux-q0e.4, ADR-0018 addendum).
    ///
    /// Under the v0.1 emit-once model (phux-ia4) the per-consumer
    /// reference advances on *emit*, not on ack: a given change is shipped
    /// exactly once and the reference is committed before the frame goes
    /// out (see
    /// [`crate::grid::SnapshotSynthesizer::synthesize_against_reference`]).
    /// `FRAME_ACK` therefore no longer drives cache eviction; it tracks
    /// `last_acked_seq` for backpressure accounting (proto.md §8.2) and
    /// refreshes the informational cursor/mode capture. The loss-tolerance
    /// "re-diff against an older reference on a dropped frame" property is
    /// a future lossy-transport concern (ADR-0018), not wired on the
    /// reliable v0.1 transports.
    ///
    /// Per proto.md §8.2 acks are cumulative: an ack for `seq = N` implies
    /// all prior emissions up to `N`. Older / duplicate / out-of-order
    /// acks (`seq <= last_acked_seq`) are silently dropped.
    ///
    /// Silent no-op if `client_id` is not currently registered. This
    /// races cleanly against detach: the runtime may dispatch an
    /// in-flight ack just as the consumer is being torn down, and the
    /// ack should evaporate rather than recreate a dropped entry.
    ///
    /// Returns `true` when this ack folded a fresh RTT sample into the
    /// consumer's [`RttEstimator`] (phux-q0e.5) — the `run` loop uses that as
    /// a cue to recompute the shared adaptive tick cadence. `false` when no
    /// sample was produced (no matching emit instant, older/duplicate ack,
    /// or unregistered consumer).
    fn on_frame_ack(&mut self, client_id: ClientId, seq: u64) -> bool {
        // Captured before the `&mut` borrow below: the global test override
        // that forces every consumer onto the tick.
        let force_all_consumers = self.consumer_tick_emits;
        let Some(consumer) = self.consumer_states.get_mut(&client_id) else {
            // Race against detach (or an ack for an unknown client). No
            // bookkeeping; no warning — this is a steady-state event,
            // not a misuse.
            trace!(
                ?client_id,
                seq, "FRAME_ACK for unregistered consumer; dropping"
            );
            return false;
        };
        // phux-38k6: only a tick-managed consumer's acks belong to this
        // per-consumer seq space. A raw (broadcast-pump) consumer acks the
        // pump's *local* seq, which is unrelated to this state's `next_seq` /
        // `emit_instants`; folding it in would set `last_acked_seq` from a
        // foreign counter and skew the RTT/backpressure accounting once the
        // consumer is (or becomes) state-sync. Drop it — the pump owns no
        // per-consumer state to update (phux-fseo made modes negotiable, so
        // this is now reachable, not just defensive).
        if !force_all_consumers && !consumer.wants_state_sync {
            trace!(
                ?client_id,
                seq, "FRAME_ACK for raw-broadcast consumer; not a tick ack, dropping"
            );
            return false;
        }
        if seq <= consumer.last_acked_seq {
            // Older or duplicate ack — acks are cumulative (proto.md
            // §8.2), so `seq <= last_acked_seq` carries no new information.
            trace!(
                ?client_id,
                seq,
                last_acked_seq = consumer.last_acked_seq,
                "FRAME_ACK older/duplicate; dropping",
            );
            return false;
        }
        consumer.last_acked_seq = seq;

        // RTT sample (phux-q0e.5). Acks are cumulative, so `seq` acknowledges
        // every emission up to and including it. Find the emit instant for
        // the highest emitted seq that is `<= seq` (the most recent frame
        // this ack covers) and time it against now. Then prune every emit
        // instant `<= seq`: those frames are acked and can never produce a
        // future sample, so the map stays bounded by the in-flight window.
        let now = tokio::time::Instant::now();
        let rtt_sample = consumer
            .emit_instants
            .range(..=seq)
            .next_back()
            .map(|(_, &emitted_at)| now.saturating_duration_since(emitted_at));
        // `split_off(&(seq + 1))` keeps only the strictly-greater keys; the
        // returned (acked) half is dropped. `seq + 1` cannot overflow in
        // practice (u64 seq at the clamped cadence) but saturate for safety.
        let still_in_flight = consumer.emit_instants.split_off(&seq.saturating_add(1));
        consumer.emit_instants = still_in_flight;
        let sampled = if let Some(sample) = rtt_sample {
            consumer.rtt.observe(sample);
            trace!(
                ?client_id,
                seq,
                rtt_ms = sample.as_secs_f64() * 1000.0,
                srtt_ms = consumer.rtt.smoothed().map(|d| d.as_secs_f64() * 1000.0),
                "FRAME_ACK: RTT sample folded into EMA",
            );
            true
        } else {
            false
        };

        // Refresh the informational cursor/mode capture. Uses a one-shot
        // `RenderState` so it doesn't disturb the per-consumer reference.
        let terminal = self.terminal.borrow();
        let cursor_mode = match RenderState::new() {
            Ok(mut rs) => match rs.update(&terminal) {
                Ok(snapshot) => Some(LastAckedCursorMode::capture(&terminal, &snapshot)),
                Err(err) => {
                    warn!(
                        ?client_id,
                        seq,
                        error = %err,
                        "FRAME_ACK: cursor/mode capture update failed; keeping prior capture",
                    );
                    None
                }
            },
            Err(err) => {
                warn!(
                    ?client_id,
                    seq,
                    error = %err,
                    "FRAME_ACK: cursor/mode RenderState alloc failed; keeping prior capture",
                );
                None
            }
        };
        if let Some(cm) = cursor_mode {
            consumer.last_cursor_mode = cm;
        }

        trace!(
            ?client_id,
            seq, "FRAME_ACK applied: last_acked_seq advanced"
        );
        sampled
    }

    /// The shared adaptive tick interval for this actor: the minimum over
    /// every attached consumer's desired interval (phux-q0e.5).
    ///
    /// One `tokio::time::Interval` drives the whole pane, but RTT is
    /// per-consumer. Taking the *minimum* means the most-demanding (lowest
    /// half-RTT) consumer sets the cadence: a fast local peer keeps its 50 Hz
    /// feel even when sharing the pane with a slow satellite peer, and the
    /// slow peer simply sees more empty/short diffs per tick (harmless — the
    /// per-consumer reference advances only on a real delta). With no
    /// consumers, or none yet sampled, this is [`DEFAULT_TICK_INTERVAL`].
    fn adaptive_tick_interval(&self) -> std::time::Duration {
        self.consumer_states
            .values()
            .map(|s| s.rtt.desired_tick_interval())
            .min()
            .unwrap_or(DEFAULT_TICK_INTERVAL)
    }

    /// Rebuild the shared state-sync timer to fire at `desired` if it differs
    /// from the currently-armed `current` by more than [`TICK_RESET_DEADBAND`]
    /// (phux-q0e.5).
    ///
    /// The deadband keeps a steady RTT from churning the scheduler on every
    /// sub-millisecond EMA wobble. `tokio::time::Interval::reset_after`
    /// re-anchors only the next deadline; the recurring `period` is fixed at
    /// construction, so changing the cadence means rebuilding the interval.
    /// The first new tick is anchored one full `desired` out so the cadence
    /// change doesn't fire a tick immediately.
    fn rearm_tick(
        tick: &mut tokio::time::Interval,
        current: &mut std::time::Duration,
        desired: std::time::Duration,
    ) {
        if current.abs_diff(desired) < TICK_RESET_DEADBAND {
            return;
        }
        *current = desired;
        let mut next = tokio::time::interval_at(tokio::time::Instant::now() + desired, desired);
        next.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        *tick = next;
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

    /// Wire an agent-event sink (SPEC §7.5, phux-y2t). The actor emits
    /// `bell` / `title_changed` / `dirty` / `idle` / `command_*` events to
    /// `sink`; the runtime drains it and fans each event out to
    /// event-stream subscribers scoped to this pane. Called by the spawn
    /// path before the actor is handed to `spawn_local`.
    pub fn set_event_sink(&mut self, sink: mpsc::Sender<AgentEvent>) {
        self.event_sink = Some(sink);
    }

    /// Best-effort agent-event emission (SPEC §7.5). `try_send` so a full
    /// sink drops the event rather than stalling the actor — the event
    /// stream is an accelerator, never a guarantee. No-op when no sink is
    /// wired (the common test path).
    fn emit_event(&self, event: AgentEvent) {
        if let Some(sink) = self.event_sink.as_ref() {
            let _ = sink.try_send(event);
        }
    }

    /// Source agent events from a freshly-applied PTY chunk (phux-y2t),
    /// called right after `vt_write`. Sources, in order:
    ///
    /// - `bell` — a BEL (`0x07`) anywhere in the chunk. Emitted once per
    ///   chunk even if several BELs arrive together (a burst of bells is
    ///   one alert from the consumer's perspective).
    /// - `title_changed` — the libghostty-tracked OSC 0 / OSC 2 title now
    ///   differs from the last observed value.
    /// - `dirty` — the chunk mutated the grid (a new output burst began).
    ///   Coalesced: at most one `dirty` per burst; the settling `idle`
    ///   fires from the tick arm.
    /// - `OutputReceived` — broadcast to semantic event subscribers.
    /// - `GridChanged` — broadcast to semantic event subscribers.
    ///
    /// `command_started` / `command_finished` are NOT sourced here — see
    /// the wire spec (SPEC §7.5.1) and the bead for the deferral: the
    /// OSC-133 command boundary is not cleanly observable from the actor
    /// without disturbing the per-consumer state-sync synthesizer's
    /// dirty-consumption model. The wire tags are allocated so a future
    /// server can emit them without a wire change.
    fn source_events_from_chunk(&mut self, chunk: &[u8]) {
        if self.event_sink.is_none() {
            return;
        }
        if chunk.contains(&0x07) {
            self.emit_event(AgentEvent::Bell);
        }
        // Title: poll libghostty's tracked title and emit on change. The
        // borrow is released before `emit_event` (which doesn't touch the
        // terminal) to keep the RefCell discipline simple.
        let current_title = self.terminal.borrow().title().unwrap_or("").to_owned();
        if current_title != self.last_title {
            self.last_title.clone_from(&current_title);
            self.emit_event(AgentEvent::TitleChanged {
                title: current_title,
            });
        }
        // Dirty: a chunk arrived, so the grid mutated. Coalesce to one
        // `dirty` per burst; `idle` (from the tick arm) closes the burst.
        if !self.in_output_burst {
            self.in_output_burst = true;
            self.emit_event(AgentEvent::Dirty);
            if !self.dirty_event_emitted_this_burst {
                self.broadcast_agent_event(&AgentEvent::Dirty);
                self.dirty_event_emitted_this_burst = true;
            }
        }
    }

    /// Emit `idle` when an output burst has settled (phux-y2t), called from
    /// the tick arm. A burst is "settled" when a tick fires and the grid
    /// has not been mutated since the previous tick
    /// (`!terminal_dirty_since_tick`). Idempotent: only the first settled
    /// tick after a `dirty` emits `idle`; subsequent idle ticks are silent
    /// until the next burst.
    fn maybe_emit_idle(&mut self) {
        if self.in_output_burst && !self.terminal_dirty_since_tick {
            self.in_output_burst = false;
            self.dirty_event_emitted_this_burst = false;
            self.emit_event(AgentEvent::Idle);
            self.broadcast_agent_event(&AgentEvent::Idle);
        }
    }

    /// Register a new event subscriber to receive semantic terminal events.
    /// Non-blocking: failure to send is silently dropped (accelerator semantics).
    /// Also updates the actor's `wire_terminal_id` for use in Event frames.
    fn subscribe_to_events(&mut self, request: SubscribeToEventsRequest) {
        self.wire_terminal_id = request.wire_terminal_id;
        self.event_subscribers.borrow_mut().push(request.subscriber);
    }

    /// Unsubscribe from semantic terminal events by removing the subscriber
    /// whose outbound mailbox pointer matches the provided reference.
    /// Silent no-op if the subscriber is not found.
    fn unsubscribe_from_events(&self, request: &UnsubscribeFromEventsRequest) {
        let mut subs = self.event_subscribers.borrow_mut();
        subs.retain(|sub| !std::ptr::eq(&raw const sub.outbound, request.outbound_ptr));
    }

    /// Broadcast an `AgentEvent` to all interested subscribers based on the
    /// event type. Uses `try_send`: drops events if a subscriber's mailbox is full.
    fn broadcast_agent_event(&self, event: &AgentEvent) {
        let subs = self.event_subscribers.borrow();
        for subscriber in subs.iter() {
            // Check if this subscriber is interested in this event type.
            // Map AgentEvent variants to TerminalEventType for filtering.
            let event_type = match event {
                AgentEvent::CommandStarted => Some(TerminalEventType::CommandStarted),
                AgentEvent::CommandFinished { .. } => Some(TerminalEventType::CommandEnded),
                AgentEvent::Dirty => Some(TerminalEventType::GridChanged),
                AgentEvent::Idle => Some(TerminalEventType::OutputReceived),
                // Other event types don't map to semantic filters yet
                _ => None,
            };

            if let Some(event_type) = event_type
                && (subscriber.event_types.is_empty()
                    || subscriber.event_types.contains(&event_type))
            {
                let frame = FrameKind::Event {
                    terminal: if self.wire_terminal_id != 0 {
                        Some(phux_protocol::ids::TerminalId::local(self.wire_terminal_id))
                    } else {
                        None
                    },
                    event: event.clone(),
                };
                let _ = subscriber.outbound.try_send(Outbound::Frame(frame));
            }
        }
    }

    /// Test-only: this actor's current shared adaptive tick interval
    /// (phux-q0e.5). Exposes [`Self::adaptive_tick_interval`] so tests can
    /// assert the cadence the `run` loop would arm without driving real time.
    #[cfg(test)]
    pub fn adaptive_tick_interval_for_test(&self) -> std::time::Duration {
        self.adaptive_tick_interval()
    }

    /// Test-only: drive `on_frame_ack` synchronously and report whether it
    /// produced a fresh RTT sample (phux-q0e.5). The `run` loop uses the
    /// return value to decide whether to re-arm the shared tick.
    #[cfg(test)]
    pub fn on_frame_ack_for_test(&mut self, client_id: ClientId, seq: u64) -> bool {
        self.on_frame_ack(client_id, seq)
    }

    /// Test-only: enable the per-consumer tick emission gate
    /// (`consumer_tick_emits`). Production defaults this OFF for human
    /// attach; this setter lets state-sync tests opt into the synthesized
    /// output path explicitly.
    #[cfg(test)]
    pub const fn enable_tick_emit_for_test(&mut self) {
        self.consumer_tick_emits = true;
    }

    /// Test-only: disable the per-consumer tick emission gate so the
    /// `tick_emit`-stays-silent path can be asserted locally, independent
    /// of the production default.
    #[cfg(test)]
    pub const fn disable_tick_emit_for_test(&mut self) {
        self.consumer_tick_emits = false;
    }

    /// Test-only: write `bytes` into the actor's `Terminal` and mark the
    /// per-tick dirty flag, mirroring the production PTY-byte path so the
    /// phux-4l0 idle short-circuit sees the mutation. Tests must use this
    /// rather than poking `terminal.borrow_mut().vt_write` directly, or
    /// the next `tick_emit` would short-circuit and skip the write.
    #[cfg(test)]
    pub fn vt_write_for_test(&mut self, bytes: &[u8]) {
        self.terminal.borrow_mut().vt_write(bytes);
        self.terminal_dirty_since_tick = true;
    }

    /// Test-only: install in-memory PTY channels on a no-PTY actor so a
    /// test can inject a PTY-output burst (the returned
    /// [`mpsc::UnboundedSender<PtyEvent>`]) and observe the encoded input
    /// the actor forwards toward the PTY writer thread (the returned
    /// [`mpsc::UnboundedReceiver<Vec<u8>>`]). Faithful to production
    /// wiring: queued output is consumed by `vt_write`; serviced input
    /// surfaces on the writer receiver. `pty` stays `None` — the run loop
    /// only reads `pty_tx` for input forwarding and `pty` for cwd/EOF,
    /// neither of which this seam exercises.
    #[cfg(test)]
    pub(crate) fn install_test_pty_channels(
        &mut self,
    ) -> (
        mpsc::UnboundedSender<PtyEvent>,
        mpsc::UnboundedReceiver<Vec<u8>>,
    ) {
        let (evt_tx, evt_rx) = mpsc::unbounded_channel::<PtyEvent>();
        let (writer_tx, writer_rx) = mpsc::unbounded_channel::<Vec<u8>>();
        self.pty_rx = Some(evt_rx);
        self.pty_tx = Some(writer_tx);
        (evt_tx, writer_rx)
    }

    /// Synthesize a snapshot of the current `Terminal` state. Exposed
    /// for tests that want to drive the synthesis path synchronously
    /// without going through the actor's `select!` loop.
    fn synthesize(&self) -> Result<SnapshotBytes, crate::grid::SynthesisError> {
        self.synthesize_with_scrollback(None)
    }

    /// Synthesize an ATTACH snapshot, optionally priming the client's
    /// scrollback with retained history rows (`phux-9q5f`). `scrollback`
    /// follows the [`crate::grid::SnapshotSynthesizer::synthesize_with_scrollback`]
    /// convention. Exposed for tests that drive the synthesis path
    /// synchronously without the actor's `select!` loop.
    fn synthesize_with_scrollback(
        &self,
        scrollback: Option<u32>,
    ) -> Result<SnapshotBytes, crate::grid::SynthesisError> {
        let terminal = self.terminal.borrow();
        // phux-uow0: the full snapshot uses a fresh RenderState internally, so
        // it needs only a shared borrow — taking `&mut` here would falsely
        // serialize it against the per-consumer tick path that also reads
        // `self.synth`.
        let synth = self.synth.borrow();
        synth.synthesize_with_scrollback(&terminal, scrollback)
    }

    /// Project the current `Terminal` grid into a structured
    /// [`phux_core::screen::ScreenState`], stamping `pane` as the
    /// wire-local id. Side-effect-free — the read path for `GET_SCREEN`.
    fn screen_state(
        &self,
        pane: u32,
        scrollback: Option<u32>,
        cells: bool,
    ) -> Result<phux_core::screen::ScreenState, crate::grid::SynthesisError> {
        let terminal = self.terminal.borrow();
        let mut synth = self.synth.borrow_mut();
        synth.screen_state_with_scrollback(&terminal, pane, scrollback, cells)
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

    /// Encode one input event and forward it to the PTY writer thread.
    /// Shared by the bounded `input_rx` drain in [`Self::run`]. A failed
    /// encode or a closed writer logs and is dropped — a single bad event
    /// must not kill the actor.
    fn service_input(&self, input: &TerminalInput) {
        match self.encode_input(input) {
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

    /// Apply a resize to both the libghostty `Terminal` and the PTY
    /// kernel-side winsize. Idempotent; logs and continues on errors.
    fn handle_resize(&mut self, cols: u16, rows: u16) {
        // libghostty has no concept of a zero-dimension grid: a 0-col or
        // 0-row resize fails with `InvalidValue` and leaves the grid at its
        // prior size. SPEC §10.5 already treats a zero-dimension viewport as
        // a no-op at the ATTACH path; clamp the live VIEWPORT_RESIZE path to
        // the same 1-cell minimum here so a `0x0` from a client (a host
        // terminal collapsing to nothing) can never reach libghostty.
        let cols = cols.max(1);
        let rows = rows.max(1);

        // `Terminal::resize` takes pixel dims for image-protocol sizing;
        // pass 0 (server does not maintain pixel metrics — clients
        // own pixel rendering per ADR-0013).
        //
        // A both-axes shrink in a single resize() call once overflowed
        // libghostty's `PageList.resizeCols` (phux-y06, the SIGABRT
        // reproduced by the resize-extremes storm); the fix now lives in
        // the vendored ghostty (phall1/ghostty 6d89054f3, "fix: resize
        // overflow"), so a both-shrink is a single safe call.
        let applied = {
            let mut term = self.terminal.borrow_mut();
            let result = term.resize(cols, rows, 0, 0);
            if let Err(err) = result {
                warn!(?err, cols, rows, "terminal resize failed");
            }
            // Cache the dims libghostty actually settled on, never the
            // requested dims: on error (e.g. a clamped 0 that still failed)
            // the grid is unchanged, so caching the request would desync
            // the cache from the real grid size.
            (term.cols().unwrap_or(cols), term.rows().unwrap_or(rows))
        };
        self.cols = applied.0;
        self.rows = applied.1;
        // A resize reflows the grid: every consumer reference is rebuilt
        // on the next diff, so force the next tick to walk (phux-4l0).
        self.terminal_dirty_since_tick = true;
        if let Some(pty) = &self.pty {
            let size = PtySize {
                rows: applied.1,
                cols: applied.0,
                pixel_width: 0,
                pixel_height: 0,
            };
            if let Ok(master) = pty.master.lock()
                && let Err(err) = master.resize(size)
            {
                warn!(
                    ?err,
                    cols = applied.0,
                    rows = applied.1,
                    "pty resize ioctl failed"
                );
            }
        }
    }

    /// phux-8v1: after a resize reflows the canonical `Terminal`,
    /// broadcast a full synthesized snapshot of the post-reflow grid to
    /// every attached client.
    ///
    /// Why this is needed: a resize triggers an *independent* reflow on
    /// both the server's canonical `Terminal` and each client's mirror
    /// `Terminal`. Those reflows can diverge — libghostty's cols-shrink
    /// reflow does not reproduce the client mirror's content identically,
    /// dropping rows — so after a resize the client mirror and the server
    /// grid disagree. The live output path (the PTY-byte broadcast fanned
    /// out by the per-attach pump in `runtime.rs`) only carries *new* PTY
    /// bytes, so the historical grid content is never re-sent and the
    /// divergence is permanent: the user sees lost / duplicated rows
    /// ("repeating/duplicated characters on resize").
    ///
    /// The synthesized bytes from [`SnapshotSynthesizer::synthesize`] open
    /// with a `DECSTR + ED2 + home` reset preamble, so feeding them to the
    /// client mirror via the ordinary `TERMINAL_OUTPUT` → `vt_write` path
    /// resets that mirror and repaints it from authoritative state. We
    /// reuse the existing output broadcast rather than the per-consumer
    /// state-sync path (`consumer_states`) because the runtime drives the
    /// broadcast/pump path; the q0e per-consumer tick is not wired into
    /// the runtime today.
    fn broadcast_resync_after_resize(&self) {
        // No subscribers → nothing to resync. `receiver_count` is the
        // broadcast channel's live-subscriber count; the seed receiver
        // held by the actor was dropped at construction, so this is the
        // attached-pump count.
        if self.output_tx.receiver_count() == 0 {
            return;
        }
        match self.synthesize() {
            Ok(snap) => {
                // A `Lagged`/no-receiver send error is benign here — the
                // next PTY output or a re-attach snapshot re-syncs.
                // phux-3ns5: ship the post-reflow grid as a `Resync` (→
                // `TERMINAL_SNAPSHOT`) carrying the settled dims, so the
                // client mirror resizes to `(cols, rows)` before applying
                // the replay. Delivered as raw output it could not resize
                // the mirror, stranding a resize-grow with blank space.
                let _ = self.output_tx.send(PaneOutput::Resync {
                    cols: self.cols,
                    rows: self.rows,
                    bytes: Bytes::from(snap.bytes),
                });
            }
            Err(err) => {
                warn!(
                    error = %err,
                    "resize resync: snapshot synthesis failed; clients recover on next output",
                );
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
    ///
    /// Returns the exit status in the shape the `TERMINAL_CLOSED` wire
    /// frame wants (phux-4li.11): `Some(code)` for a normal `_exit(n)`,
    /// `None` for signal-killed children or otherwise-unknown exits.
    /// `portable_pty::ExitStatus.signal` is the discriminator — a
    /// non-`None` signal name means the kernel reports the death as
    /// signal-driven, which collapses to `exit_status = None` on the
    /// wire per the SPEC §10.1 compact-subset rule.
    fn reap_child_if_any(&mut self) -> Option<i32> {
        let pty = self.pty.as_mut()?;
        match pty.child.try_wait() {
            Ok(Some(status)) => {
                debug!(?status, "child reaped on PTY EOF");
                exit_status_to_wire(&status)
            }
            Ok(None) => {
                trace!("PTY EOF but child still alive — leaving to shutdown path");
                None
            }
            Err(err) => {
                debug!(?err, "child try_wait failed on PTY EOF");
                None
            }
        }
    }

    /// React to PTY EOF (the child went away): detach the PTY-read branch
    /// and notify the runtime so it can broadcast `TERMINAL_CLOSED`.
    ///
    /// Dropping `pty_rx` parks the pump's `select!` arm forever, but the
    /// actor deliberately stays alive — it must remain reachable for
    /// late-arriving `SnapshotRequest`s (a client attaching just after the
    /// child exited) and for orderly shutdown via the cancellation token.
    /// The child is reaped here so we don't leave a zombie waiting for the
    /// explicit shutdown signal. (phux-it8: firing `exit_notify` is what
    /// lets attached clients learn the shell exited instead of freezing in
    /// alt-screen.)
    ///
    /// TODO(phux-9gw): multi-pane lifecycle — when a session has more than
    /// one pane, a single EOF should switch focus to a sibling rather than
    /// detach the whole session. Today sessions are 1:1 with panes in
    /// practice so the simpler "EOF → detach attached" model is correct.
    fn handle_pty_eof(&mut self) {
        debug!("PTY EOF; firing exit_notify and keeping actor alive for late snapshot/input drain");
        self.pty_rx = None;
        let exit_status = self.reap_child_if_any();
        if let Some(tx) = self.exit_notify.take() {
            let _ = tx.send(exit_status);
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

        // State-sync tick driver (phux-q0e.3 / phux-q0e.5). RTT-adaptive
        // cadence: starts at the `DEFAULT_TICK_INTERVAL` cold-start value and
        // is rebuilt toward each consumer's measured `RTT/2` (clamped to
        // [`MIN_TICK_INTERVAL`, `MAX_TICK_INTERVAL`]) as `FRAME_ACK`
        // round-trips land. The shared timer runs at the minimum desired
        // interval across consumers (see [`Self::adaptive_tick_interval`]).
        // `MissedTickBehavior::Delay` — if the actor falls behind under heavy
        // PTY traffic we want subsequent ticks spaced by the interval from
        // when they ran, not bunched up to "catch up" (which would defeat the
        // rate limit's purpose). `Burst` (the default) would spam emissions
        // when a long PTY chunk delays us past several tick boundaries.
        let mut tick_interval = DEFAULT_TICK_INTERVAL;
        let mut tick = tokio::time::interval(tick_interval);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        // Eat the first immediate tick (Interval fires synchronously on
        // first poll). Without this, the very first iteration would
        // tick before any other branch has a chance to react.
        let _ = tick.tick().await;

        // phux-8v1 drag fix: debounce timer for the post-resize client
        // resync. (Re)armed on each resync-requesting resize; when it
        // fires we broadcast ONE snapshot at the settled size. Init far
        // out — `resync_pending` is false until a resize arms it, and we
        // always `reset()` the deadline when arming, so the initial
        // instant is never observed.
        let resync_debounce = tokio::time::sleep(std::time::Duration::from_secs(3600));
        tokio::pin!(resync_debounce);
        let mut resync_pending = false;

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

                // Input → PTY. Polled before the PTY-output arm (biased
                // order) so a queued keystroke is serviced this turn
                // rather than waiting behind an output burst — the fix for
                // load-correlated input starvation. Bounded by
                // `MAX_INPUT_COALESCE`: the arm fires on the first ready
                // event, then drains up to a capped batch via `try_recv`
                // so a paste the encoder expands cannot inflate one turn
                // without limit. The PTY-output arm's structural bound is
                // `MAX_PTY_COALESCE_BYTES`.
                Some(input) = self.input_rx.recv() => {
                    self.service_input(&input);
                    for _ in 1..MAX_INPUT_COALESCE {
                        match self.input_rx.try_recv() {
                            Ok(next) => self.service_input(&next),
                            // Empty (nothing more ready) or Disconnected —
                            // stop draining.
                            Err(_) => break,
                        }
                    }
                }

                // PTY → Terminal + broadcast. Polled after the input arm:
                // when this arm finishes one bounded payload and returns to
                // the `select!`, the biased re-poll offers `input_rx`
                // first, so a keystroke queued during the burst is serviced
                // before the next `vt_write`. When the byte cap stops the
                // drain mid-burst the arm additionally yields to the
                // scheduler before re-entering, so sibling tasks on the
                // LocalSet (and a freshly-queued keystroke) get a turn
                // between bounded parses rather than after the whole burst.
                evt = recv_or_pending(self.pty_rx.as_mut()) => {
                    match evt {
                        Some(PtyEvent::Bytes(first)) => {
                            // Coalesce any chunks already queued behind this one
                            // into a single Terminal write + broadcast frame
                            // (phux-ahk burst path). A lone chunk takes the
                            // fast path below: its `Vec` moves into `Bytes`
                            // with no copy. Only a genuine burst (several
                            // reads queued) allocates a join buffer. The drain
                            // stops on the chunk-count cap, on EOF, or once the
                            // payload would cross `MAX_PTY_COALESCE_BYTES` — in
                            // the byte-cap case the crossing chunk is left
                            // queued for the next turn (mpsc has no peek, so the
                            // length is checked before `try_recv`).
                            let mut coalesced: Vec<u8> = Vec::new();
                            let mut saw_eof = false;
                            // `true` when the drain stopped because the next
                            // chunk would cross the byte cap (more output is
                            // likely queued) rather than because the queue
                            // emptied. Drives the post-broadcast yield so a
                            // sustained burst hands the scheduler a turn
                            // between bounded parses.
                            let mut hit_byte_cap = false;
                            for _ in 0..MAX_PTY_COALESCE {
                                // Length so far: the lone `first` chunk before
                                // any coalescing, else the join buffer. Stop
                                // before consuming a chunk that would push the
                                // payload past the byte cap so each `vt_write`
                                // is a bounded synchronous parse. The first
                                // chunk always lands; only coalescing is capped.
                                let current_len = if coalesced.is_empty() {
                                    first.len()
                                } else {
                                    coalesced.len()
                                };
                                if current_len >= MAX_PTY_COALESCE_BYTES {
                                    hit_byte_cap = true;
                                    break;
                                }
                                match self.pty_rx.as_mut().map(mpsc::UnboundedReceiver::try_recv) {
                                    Some(Ok(PtyEvent::Bytes(more))) => {
                                        if coalesced.is_empty() {
                                            coalesced.reserve(first.len() + more.len());
                                            coalesced.extend_from_slice(&first);
                                        }
                                        coalesced.extend_from_slice(&more);
                                    }
                                    // A queued EOF: flush the coalesced bytes
                                    // first, then handle EOF below.
                                    Some(Ok(PtyEvent::Eof)) => {
                                        saw_eof = true;
                                        break;
                                    }
                                    // Empty (nothing more ready) or the sender
                                    // dropped — stop draining. A dropped sender
                                    // surfaces as EOF on the next pump wakeup.
                                    _ => break,
                                }
                            }
                            let payload: Bytes = if coalesced.is_empty() {
                                Bytes::from(first)
                            } else {
                                Bytes::from(coalesced)
                            };
                            // Trace level: per-wakeup volume is the raw input
                            // rate, useful for "what was the PTY doing right
                            // before a stall" but far too chatty for the
                            // default filter — off unless `phux=trace`.
                            trace!(bytes = payload.len(), "vt_write: PTY chunk(s) -> Terminal");
                            self.terminal.borrow_mut().vt_write(&payload);
                            // The grid changed: let the next tick walk
                            // the rows (phux-4l0 idle short-circuit).
                            self.terminal_dirty_since_tick = true;
                            // phux-y2t: source agent events (bell, title,
                            // dirty) from the just-applied bytes for any
                            // event-stream subscriber. No-op without a sink.
                            self.source_events_from_chunk(&payload);
                            // Broadcast send fails only when no
                            // subscribers exist; that's a normal
                            // steady-state (no attached clients) and
                            // we silently drop.
                            let _ = self.output_tx.send(PaneOutput::Live(payload));
                            if saw_eof {
                                self.handle_pty_eof();
                            } else if hit_byte_cap {
                                // A capped payload with more output queued:
                                // yield so the runtime re-polls (input arm
                                // first) and sibling LocalSet tasks advance,
                                // bounding the output arm at the thread level.
                                // The next loop turn coalesces the next
                                // bounded payload, so throughput is preserved.
                                tokio::task::yield_now().await;
                            }
                        }
                        Some(PtyEvent::Eof) | None => {
                            self.handle_pty_eof();
                        }
                    }
                }

                Some(req) = self.snapshot_rx.recv() => {
                    let snap = match self.synthesize_with_scrollback(req.scrollback) {
                        Ok(s) => s,
                        Err(err) => {
                            warn!(error = %err, "snapshot synthesis failed; replying with empty");
                            SnapshotBytes {
                                cols: self.cols,
                                rows: self.rows,
                                bytes: Vec::new(),
                                scrollback: Vec::new(),
                            }
                        }
                    };
                    let _ = req.reply.send(snap);
                }

                Some(req) = self.screen_rx.recv() => {
                    let want_cells = req.cells;
                    let screen = self.screen_state(req.pane, req.scrollback, req.cells).unwrap_or_else(|err| {
                        warn!(error = %err, "screen projection failed; replying with empty");
                        phux_core::screen::ScreenState {
                            schema_version: phux_core::screen::SCHEMA_VERSION,
                            pane: req.pane,
                            cols: self.cols,
                            rows: self.rows,
                            cursor: None,
                            lines: Vec::new(),
                            scrollback: Vec::new(),
                            // Honour the request shape even on the error
                            // path: an empty cells vec, not a misleading
                            // `None`, when the caller asked for cells.
                            cells: want_cells.then(Vec::new),
                        }
                    });
                    let _ = req.reply.send(screen);
                }

                Some(req) = self.pwd_rx.recv() => {
                    // Resolve the pane's live working directory by asking
                    // the kernel for the PTY child's CWD (the shell's
                    // directory *now*, after any `cd`). `None` when there
                    // is no PTY (no-PTY actor), the child has no pid, or
                    // the query is unsupported/denied — the caller then
                    // falls back to a non-inherited default.
                    let cwd = self
                        .pty
                        .as_ref()
                        .and_then(|p| p.child.process_id())
                        .and_then(crate::cwd_query::process_cwd)
                        .map(|p| p.to_string_lossy().into_owned());
                    let _ = req.reply.send(cwd);
                }

                Some(req) = self.resize_rx.recv() => {
                    self.handle_resize(req.cols, req.rows);
                    // phux-8v1: re-broadcast a full snapshot for live
                    // resizes so client mirrors reconverge after their
                    // independent reflow. Suppressed for the ATTACH-time
                    // resize (the handshake snapshot covers it). Debounced
                    // (RESIZE_RESYNC_DEBOUNCE) so a drag storm coalesces
                    // into a single snapshot at the settled size rather
                    // than flooding the client with stale-width snapshots.
                    if req.resync_clients {
                        resync_pending = true;
                        resync_debounce
                            .as_mut()
                            .reset(tokio::time::Instant::now() + RESIZE_RESYNC_DEBOUNCE);
                    }
                }

                // phux-8v1: debounced resize resync — fires once the
                // resize storm settles (RESIZE_RESYNC_DEBOUNCE after the
                // last resync-requesting resize). Guarded by
                // `resync_pending` so the idle far-future timer never
                // fires spuriously.
                () = &mut resync_debounce, if resync_pending => {
                    resync_pending = false;
                    self.broadcast_resync_after_resize();
                }

                Some(req) = self.consumer_attach_rx.recv() => {
                    let ConsumerAttachRequest {
                        client_id,
                        outbound,
                        wire_terminal_id,
                        wants_state_sync,
                        reply,
                    } = req;
                    // phux-3uv / phux-fseo: map register success to an outcome
                    // that tells the runtime whether this actor is
                    // tick-managing the consumer. Tick-managed ⇒ the runtime
                    // suppresses its broadcast pump for this pane (single
                    // emitter). A consumer is tick-managed if it negotiated
                    // `OutputMode::StateSync` (`wants_state_sync`), OR the
                    // global test gate forces every consumer onto the tick.
                    let tick_managed = self.consumer_tick_emits || wants_state_sync;
                    let result = self
                        .register_consumer(client_id, outbound, wire_terminal_id, wants_state_sync)
                        .map(|()| ConsumerAttachOutcome { tick_managed });
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
                            tick_managed,
                            "consumer attached: per-consumer synthesizer primed"
                        );
                    }
                    let _ = reply.send(result);
                }

                Some(req) = self.consumer_detach_rx.recv() => {
                    let ConsumerDetachRequest { client_id, reply } = req;
                    self.unregister_consumer(client_id);
                    trace!(?client_id, "consumer detached: per-consumer RenderState freed");
                    // phux-q0e.5: losing a consumer can raise the minimum
                    // desired interval (e.g. the fastest peer left), so
                    // re-evaluate the shared cadence.
                    Self::rearm_tick(&mut tick, &mut tick_interval, self.adaptive_tick_interval());
                    let _ = reply.send(());
                }

                // ADR-0018 / phux-q0e.4: inbound FRAME_ACK. Clears the
                // per-consumer dirty cache so the next tick re-diffs
                // against the just-acked reference. Loss tolerance: a
                // dropped ack just means the next tick re-emits a larger
                // diff against the same older reference — no
                // retransmit machinery here.
                Some(req) = self.consumer_ack_rx.recv() => {
                    let ConsumerAckRequest { client_id, seq } = req;
                    // phux-q0e.5: a fresh RTT sample may shift the adaptive
                    // cadence. Rebuild the shared tick only when the new
                    // minimum-desired interval moves beyond the deadband, so
                    // a steady RTT does not churn the scheduler.
                    if self.on_frame_ack(client_id, seq) {
                        Self::rearm_tick(&mut tick, &mut tick_interval, self.adaptive_tick_interval());
                    }
                }

                // Semantic event subscription request. Register the subscriber
                // and begin broadcasting matching events to their outbound mailbox.
                Some(req) = self.subscribe_to_events_rx.recv() => {
                    self.subscribe_to_events(req);
                }

                // Semantic event unsubscription request. Remove the subscriber
                // from the broadcast list. Silent no-op if already unsubscribed.
                Some(req) = self.unsubscribe_from_events_rx.recv() => {
                    self.unsubscribe_from_events(&req);
                }

                // State-sync tick driver (phux-q0e.3, phux-ia4, ADR-0018).
                // Iterates each attached consumer, diffs the live terminal
                // against that consumer's own reference grid, and pushes a
                // `TerminalOutput` frame onto its outbound mailbox whenever
                // `synthesize_against_reference` returns non-empty bytes.
                _ = tick.tick() => {
                    // phux-y2t: close an output burst with an `idle` event
                    // when the grid has settled since the previous tick.
                    // MUST run before `tick_emit`, which consumes the
                    // `terminal_dirty_since_tick` flag `maybe_emit_idle`
                    // reads. No-op without an event sink.
                    self.maybe_emit_idle();
                    self.tick_emit();
                }

                else => break,
            }
        }
    }

    /// One tick of the state-sync emission driver (phux-q0e.3, phux-ia4).
    ///
    /// Walks every attached consumer in turn. For each:
    ///
    /// 1. Call [`SnapshotSynthesizer::synthesize_against_reference`] using
    ///    the actor's shared synthesizer and the consumer's *own*
    ///    reference grid. The reference is per-consumer and independent of
    ///    the shared `Terminal` dirty bits, so every consumer on a shared
    ///    pane gets its own correct diff this tick — even though
    ///    libghostty's `RenderState::update` consumes the shared dirty
    ///    state on the first read (the phux-ia4 fix). Synthesis errors are
    ///    logged and that consumer is skipped for this tick (no kill: a
    ///    transient FFI error on one consumer must not poison the others).
    /// 2. If the body is empty, skip — the viewport is byte-identical to
    ///    that consumer's reference (steady state between writes).
    /// 3. Stamp the per-consumer monotonic `seq` (starting at `1`,
    ///    incrementing per emission) and ship a `TerminalOutput` frame
    ///    via the per-consumer outbound mailbox.
    ///
    /// Emit-once (phux-ia4): `synthesize_against_reference` advances the
    /// consumer's reference before returning a non-empty body, so a given
    /// change is emitted exactly once and an unchanged terminal produces no
    /// re-emission on the next tick. This is the v0.1 reliable-transport
    /// model (proto.md §8); the loss-tolerance re-diff property is a future
    /// lossy-transport concern (ADR-0018) and is not wired here.
    #[allow(
        clippy::too_many_lines,
        reason = "single cohesive per-tick emission: the length is inline \
                  safety rationale (permit reservation, emit-once, \
                  backpressure) that splitting would scatter and endanger"
    )]
    fn tick_emit(&mut self) {
        // Per-tick observation span (hot path, so debug level: the default
        // `phux=info` filter leaves it disabled and effectively free —
        // `tracing` skips a disabled span without evaluating its fields).
        // The correlation fields a trace reader greps for to localize
        // server-side lag: how many consumers this tick must serve and
        // whether the grid is dirty. `consumer_count` is read before the
        // gate so the span is consistent on the gated-off / idle-skip
        // return paths too; `emitted` + `total_out_bytes` are recorded at
        // the end of a productive tick.
        let tick_span = tracing::debug_span!(
            "tick_emit",
            consumer_count = self.consumer_states.len(),
            dirty = self.terminal_dirty_since_tick,
            // Filled in at the end of a productive tick via `record`; declared
            // `Empty` so they exist on the span for later assignment.
            emitted = tracing::field::Empty,
            total_out_bytes = tracing::field::Empty,
        )
        .entered();

        // Emission gate (phux-0q8 / phux-3uv / phux-ia4 / phux-fseo). The tick
        // emits only for a *tick-managed* consumer — one that negotiated
        // `OutputMode::StateSync` (`state.wants_state_sync`), or any consumer
        // when the global test gate forces it; the runtime suppresses its
        // broadcast pump for exactly those (see `ConsumerAttachOutcome`). A
        // raw consumer is served by the pump, so the tick stays silent for it
        // to avoid double-painting. `force_all_consumers` is captured here so
        // the loop below reads it without re-borrowing `self` while it holds
        // `&mut self.consumer_states`.
        let force_all_consumers = self.consumer_tick_emits;
        if !force_all_consumers && !self.consumer_states.values().any(|s| s.wants_state_sync) {
            // No tick-managed consumer: nothing to emit (dirty flag untouched).
            return;
        }

        // Idle short-circuit (phux-4l0). The per-consumer reference diff
        // walks + renders every viewport row into a throwaway `Vec<u8>`
        // for every consumer, every tick — pure waste when nothing has
        // changed. Take and reset the "mutated since last tick" flag here;
        // if the terminal is unchanged AND no consumer is awaiting its
        // first emission, skip the entire per-consumer loop.
        //
        // Correctness: a `Clean` terminal cannot have diverged from any
        // consumer's last-emitted reference (the reference advanced to the
        // terminal state on the prior emit, and nothing has mutated the
        // terminal since), so skipping is sound. Two carve-outs suppress the
        // short-circuit even on a `Clean` terminal:
        //
        // - `needs_initial_emit` preserves the phux-ia4 multi-consumer
        //   guarantee: a consumer registered *after* the last write sits on a
        //   clean terminal yet has never had a synthesis pass, so it must be
        //   walked once even though the global flag is clear.
        // - `behind` preserves the backpressure retry: a consumer skipped on
        //   a prior tick because its mailbox was full has a reference behind
        //   the live grid. The grid can stay `Clean` indefinitely, so without
        //   this the held-back delta would never be retried once the client
        //   drains (the wave-hunt/server-lifecycle backpressure leak).
        let mutated = self.terminal_dirty_since_tick;
        self.terminal_dirty_since_tick = false;
        if !mutated
            && !self
                .consumer_states
                .values()
                .any(|s| s.needs_initial_emit || s.behind)
        {
            return;
        }

        // Borrow the terminal + shared synthesizer once per tick. The
        // synthesizer's `RenderState`/iterators are reused across
        // consumers; the per-consumer state lives in each `reference`.
        let terminal = self.terminal.borrow();
        let mut synth = self.synth.borrow_mut();
        // Consumers whose outbound mailbox is `Closed` (receiver dropped)
        // are reaped after the loop so a missed detach (phux-ddg) does not
        // leave a dead `ConsumerReference` to be re-rendered forever.
        let mut closed: Vec<ClientId> = Vec::new();
        // Per-tick emission tally recorded onto the tick span on the way out
        // (frames actually shipped + their total byte volume) — the headline
        // "frame N for terminal T was Y bytes" reconstruction signal.
        let mut emitted: u64 = 0;
        let mut total_out_bytes: usize = 0;
        for (client_id, state) in &mut self.consumer_states {
            // phux-fseo: serve only tick-managed consumers. A raw consumer
            // sharing this pane is served by the broadcast pump; emitting here
            // too would double-paint it, so skip it (reference left untouched
            // for a later mode flip).
            if !force_all_consumers && !state.wants_state_sync {
                continue;
            }
            // This consumer is being serviced this tick; it no longer needs
            // a forced first pass.
            state.needs_initial_emit = false;
            // Reserve an outbound permit BEFORE synthesizing
            // (phux-wave-hunt/server-lifecycle). `synthesize_against_reference`
            // commits the per-consumer reference to the just-rendered grid
            // *before* it returns the bytes (emit-once, grid.rs), so once we
            // synthesize the delta is the only copy and the reference has
            // moved past it. If the send then failed `Full` we would drop the
            // delta and never re-emit it (the next tick diffs against the
            // already-advanced reference), silently losing content and
            // diverging the client mirror forever.
            //
            // Reserving first inverts the ordering: a `Full` mailbox means we
            // skip this consumer entirely this tick WITHOUT synthesizing, so
            // the reference (and `next_seq`) stay put and the delta is
            // re-diffed intact on the next tick once the client drains. A
            // `Closed` mailbox reaps the entry (phux-ddg self-heal). Only when
            // we hold a permit — which guarantees the subsequent send cannot
            // fail — do we synthesize, advance the reference, and ship.
            let permit = match state.outbound.try_reserve() {
                Ok(permit) => permit,
                Err(tokio::sync::mpsc::error::TrySendError::Full(())) => {
                    // Backpressure: the consumer mailbox is wedged. Skip
                    // without advancing the reference so no content is lost;
                    // the next tick retries the same delta. Mark `behind` so
                    // the idle short-circuit keeps walking this consumer even
                    // if the grid goes `Clean` before the client drains — the
                    // retry must not depend on a fresh write. At debug so a
                    // stall is visible at the recommended `phux=debug` level.
                    state.behind = true;
                    debug!(
                        ?client_id,
                        wire_terminal_id = state.wire_terminal_id,
                        "state-sync tick: consumer mailbox full; skipping (reference held, retries next tick)",
                    );
                    continue;
                }
                Err(tokio::sync::mpsc::error::TrySendError::Closed(())) => {
                    // The receiver is gone. A `ConsumerDetachRequest` may have
                    // been dropped (best-effort `try_send` on a full detach
                    // mailbox, runtime.rs) so `unregister_consumer` never ran.
                    // Self-heal: reap the entry now so we stop re-rendering a
                    // dead consumer every tick (phux-ddg).
                    debug!(
                        ?client_id,
                        wire_terminal_id = state.wire_terminal_id,
                        "state-sync tick: consumer mailbox closed; reaping entry",
                    );
                    closed.push(*client_id);
                    continue;
                }
            };
            // We hold a permit: the mailbox has room, so this consumer is
            // about to be fully serviced this tick (a delta ships, or the
            // diff is empty and the reference is already at the live grid).
            // Either way it is no longer behind.
            state.behind = false;
            // Per-consumer synthesis span (debug; the per-tick CPU sink —
            // its duration is the key server-side lag signal). Carries the
            // consumer correlation fields; the diff size lands in
            // `synthesize_against_reference`'s own child span.
            let _synth_span = tracing::debug_span!(
                "synthesize",
                ?client_id,
                wire_terminal_id = state.wire_terminal_id,
            )
            .entered();
            let bytes = match synth.synthesize_against_reference(&terminal, &mut state.reference) {
                Ok(snap) => snap.bytes,
                Err(err) => {
                    warn!(
                        ?client_id,
                        wire_terminal_id = state.wire_terminal_id,
                        error = %err,
                        "state-sync tick: synthesize_against_reference failed; skipping consumer",
                    );
                    // The held `permit` drops here, releasing the reserved
                    // slot back to the mailbox — nothing was shipped.
                    continue;
                }
            };
            if bytes.is_empty() {
                // Byte-identical to this consumer's reference; nothing to
                // send this tick. The reserved permit drops unused. A closed
                // mailbox was already reaped by the `try_reserve` arm above,
                // so no extra liveness probe is needed here.
                continue;
            }
            let seq = state.next_seq;
            let out_bytes = bytes.len();
            // Wrapping_add for paranoia; `u64` will not realistically
            // roll over at 33 Hz, but the existing `runtime.rs` pump
            // uses the same idiom and we match it.
            state.next_seq = state.next_seq.wrapping_add(1);
            let frame = FrameKind::TerminalOutput {
                terminal_id: phux_protocol::ids::TerminalId::local(state.wire_terminal_id),
                seq,
                bytes: bytes.into(),
            };
            // Infallible: we hold a reserved permit, so this cannot block,
            // drop, or fail. This preserves the actor's single-poll-budget
            // invariant (the tick arm never yields the loop) while keeping
            // emit-once consistent — a synthesized delta always ships.
            permit.send(Outbound::Frame(frame));
            // Stamp the emit instant for this seq so the matching FRAME_ACK
            // can be turned into an RTT sample (phux-q0e.5). Recorded only
            // for shipped frames — empty/skipped ticks have no round-trip to
            // measure. Pruned on ack, so the map stays as small as the
            // in-flight window.
            state.emit_instants.insert(seq, tokio::time::Instant::now());
            // Defensive bound: ack-pruning keeps this map tiny for a
            // well-behaved consumer, but one that opts into state sync and
            // never sends FRAME_ACK (or a transport that drops acks) would
            // otherwise grow it one entry per emitted tick without bound
            // (~50/s at the 20ms floor cadence). Evict the oldest (lowest-seq)
            // samples past the cap; an unacked sample this stale is already
            // useless for RTT, so dropping it costs nothing and bounds the
            // map to a few KB per consumer. See `MAX_EMIT_INSTANTS`.
            while state.emit_instants.len() > MAX_EMIT_INSTANTS {
                state.emit_instants.pop_first();
            }
            emitted += 1;
            total_out_bytes += out_bytes;
            trace!(
                ?client_id,
                wire_terminal_id = state.wire_terminal_id,
                seq,
                out_bytes,
                "state-sync tick: TERMINAL_OUTPUT emitted",
            );
        }
        drop(synth);
        drop(terminal);
        // Record the per-tick emission tally on the tick span so a reader
        // can reconstruct "tick served N consumers, shipped M frames /
        // B bytes" without re-deriving it from the per-consumer trace lines.
        tick_span.record("emitted", emitted);
        tick_span.record("total_out_bytes", total_out_bytes);
        for client_id in closed {
            self.consumer_states.remove(&client_id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// phux-07y: `shell_command` runs the user's command via
    /// `$SHELL -c <command>` so quoting / args work and the pane closes
    /// when the command exits.
    #[test]
    fn shell_command_wraps_in_shell_dash_c() {
        let cmd = shell_command("btop --utf-force");
        let argv = cmd.get_argv();
        assert_eq!(argv.len(), 3, "expected [shell, -c, command]");
        assert!(
            !argv[0].is_empty(),
            "argv[0] is the resolved shell (SHELL or /bin/sh)"
        );
        assert_eq!(argv[1], "-c");
        assert_eq!(argv[2], "btop --utf-force");
    }

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
                    .send(SnapshotRequest {
                        scrollback: None,
                        reply: reply_tx,
                    })
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

    /// Interactive-latency regression gate: a queued keystroke must
    /// interleave with a large pending PTY-output burst rather than wait
    /// for the entire burst to drain. Pre-queues ~800KB of output (far
    /// exceeding `MAX_PTY_COALESCE_BYTES`) plus one input event, runs the
    /// actor, and asserts the input reaches the PTY writer channel while
    /// the burst is still draining (the cumulative broadcast bytes seen
    /// at that moment are far below the full burst). Fails if input is
    /// serviced only after the entire burst drains (output-first ordering
    /// or an unbounded coalesce that never yields).
    #[tokio::test(flavor = "current_thread")]
    async fn input_interleaves_with_a_large_pty_output_burst() {
        use phux_protocol::input::paste::{PasteEvent, PasteTrust};

        const CHUNK_LEN: usize = 4096;
        const CHUNK_COUNT: usize = 200;
        const BURST_BYTES: usize = CHUNK_LEN * CHUNK_COUNT;

        let local = tokio::task::LocalSet::new();
        local
            .run_until(async {
                let bundle = TerminalActor::new(80, 24).expect("new");
                let handle = bundle.handle.clone();
                let token = bundle.token.clone();
                let mut actor = bundle.actor;
                let (pty_evt_tx, mut writer_rx) = actor.install_test_pty_channels();

                // Subscribe before spawning so no broadcast frame is
                // missed. This is the deterministic ordering gate: the
                // cumulative output bytes observed at the instant input
                // lands must be far below the full burst.
                let mut out_rx = handle.output.subscribe();

                // Pre-queue a burst far larger than MAX_PTY_COALESCE_BYTES
                // so it spans many capped vt_writes.
                let chunk = vec![b'x'; CHUNK_LEN];
                for _ in 0..CHUNK_COUNT {
                    pty_evt_tx
                        .send(PtyEvent::Bytes(chunk.clone()))
                        .expect("queue burst");
                }
                // Queue ONE input event. With bracketed-paste mode 2004
                // off (a fresh Terminal's default) a Trusted paste of
                // b"x" encodes to exactly b"x" on the writer channel.
                handle
                    .input
                    .send(TerminalInput::Paste(PasteEvent {
                        trust: PasteTrust::Trusted,
                        data: b"x".to_vec(),
                    }))
                    .await
                    .expect("queue input");

                tokio::task::spawn_local(actor.run());

                // The keystroke must be serviced within a bounded budget:
                // it interleaves, not waits for the whole burst. The
                // 500ms timeout is a backstop; the byte-ordering check
                // below is the real gate.
                let got =
                    tokio::time::timeout(std::time::Duration::from_millis(500), writer_rx.recv())
                        .await
                        .expect("input must be serviced mid-burst, not after it");
                assert_eq!(
                    got,
                    Some(b"x".to_vec()),
                    "queued keystroke should reach the PTY writer while the burst drains",
                );

                // Count broadcast bytes the actor has emitted so far.
                // Account Lagged-skipped frames toward the total (the
                // broadcast channel is bounded; a fast burst can lag this
                // receiver) so the ordering gate cannot under-report and
                // pass spuriously.
                let mut emitted: usize = 0;
                loop {
                    match out_rx.try_recv() {
                        Ok(PaneOutput::Live(bytes) | PaneOutput::Resync { bytes, .. }) => {
                            emitted += bytes.len();
                        }
                        Err(tokio::sync::broadcast::error::TryRecvError::Lagged(n)) => {
                            // Each lagged frame is one coalesced payload of
                            // at most MAX_PTY_COALESCE_BYTES; bound the
                            // skipped volume by that cap so the assertion
                            // stays conservative (never under-reports).
                            let skipped = usize::try_from(n).unwrap_or(usize::MAX);
                            emitted += skipped.saturating_mul(MAX_PTY_COALESCE_BYTES);
                        }
                        Err(_) => break,
                    }
                }
                token.cancel();
                assert!(
                    emitted < BURST_BYTES,
                    "input must land mid-burst: cumulative output {emitted} should be \
                     below the full burst {BURST_BYTES}",
                );
            })
            .await;
    }

    /// phux-cs6: the actor answers a `PwdRequest` with its PTY child's
    /// live working directory. A shell is spawned that `cd`s into a
    /// freshly-created temp dir and then blocks (`read`), so its CWD is
    /// the temp dir when the kernel query runs. This is the actor-level
    /// proof of the inherit-focused acceptance criterion.
    #[tokio::test(flavor = "current_thread")]
    async fn actor_responds_to_pwd_request_with_pty_child_cwd() {
        let local = tokio::task::LocalSet::new();
        local
            .run_until(async {
                let dir = tempfile::tempdir().expect("tempdir");
                // Canonicalize: macOS hands back the realpath
                // (/private/var/... for /var/...), which is what the
                // kernel query returns too.
                let dir_path = dir.path().canonicalize().expect("canonicalize tempdir");

                let mut cmd = CommandBuilder::new("/bin/sh");
                cmd.arg("-c");
                cmd.arg(format!("cd '{}' && read _", dir_path.display()));
                let bundle = TerminalActor::build_with_token(
                    20,
                    5,
                    Some(cmd),
                    DEFAULT_MAX_SCROLLBACK,
                    CancellationToken::new(),
                )
                .expect("build_with_token");
                let handle = bundle.handle.clone();
                let token = bundle.token;
                let join = tokio::task::spawn_local(bundle.actor.run());

                // Poll the actor until the shell has executed the `cd`.
                // The query races the child's startup, so retry briefly.
                let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
                let mut got: Option<String> = None;
                while tokio::time::Instant::now() < deadline {
                    let (reply_tx, reply_rx) = oneshot::channel();
                    handle
                        .pwd
                        .send(PwdRequest { reply: reply_tx })
                        .await
                        .expect("send pwd request");
                    got = reply_rx.await.expect("pwd reply");
                    if got.as_deref() == Some(dir_path.to_str().expect("utf8 path")) {
                        break;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(25)).await;
                }
                assert_eq!(
                    got.as_deref(),
                    Some(dir_path.to_str().expect("utf8 path")),
                    "actor should report the PTY child's live CWD",
                );

                token.cancel();
                let _ = tokio::time::timeout(std::time::Duration::from_millis(500), join).await;
            })
            .await;
    }

    /// phux-cs6: a no-PTY actor has no child to query, so `pwd` is `None`
    /// and the spawn path falls back to a non-inherited default.
    #[tokio::test(flavor = "current_thread")]
    async fn actor_pwd_request_is_none_without_pty() {
        let local = tokio::task::LocalSet::new();
        local
            .run_until(async {
                let bundle = TerminalActor::new_with_seed(20, 5, b"no pty here").expect("seed");
                let handle = bundle.handle.clone();
                let _token = bundle.token;
                tokio::task::spawn_local(bundle.actor.run());

                let (reply_tx, reply_rx) = oneshot::channel();
                handle
                    .pwd
                    .send(PwdRequest { reply: reply_tx })
                    .await
                    .expect("send pwd request");
                assert_eq!(reply_rx.await.expect("pwd reply"), None);
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
                let _ = handle.snapshot.try_send(SnapshotRequest {
                    scrollback: None,
                    reply: reply_tx,
                });
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
                    TerminalActor::build_with_token(20, 5, None, DEFAULT_MAX_SCROLLBACK, child)
                        .expect("build_with_token");
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
        actor
            .register_consumer(a, tx_a, 1, false)
            .expect("register a");
        assert_eq!(actor.consumer_count(), 1);
        actor
            .register_consumer(b, tx_b, 2, false)
            .expect("register b");
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
        // A tick-managed (state-sync) consumer: priming runs, so the
        // capture must reflect the live terminal.
        actor
            .register_consumer(client, tx, 11, true)
            .expect("register");

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

    /// A raw broadcast-pump consumer (the human attach path) skips the two
    /// full-grid render passes priming would cost: its reference and
    /// cursor/mode capture are never read (the tick serves only tick-managed
    /// consumers). So it registers with the `unprimed` placeholder, not a
    /// live capture (phux-ahk register-prime gating).
    #[test]
    fn register_raw_consumer_skips_priming() {
        let bundle = TerminalActor::new_with_seed(20, 5, b"hello").expect("new_with_seed");
        let mut actor = bundle.actor;
        let client = ClientId(7);
        let (tx, _rx) = dummy_outbound();
        actor
            .register_consumer(client, tx, 11, false)
            .expect("register");

        let state = actor.consumer_state(client).expect("state present");
        // Not primed: the placeholder capture, not the seeded cursor at (5, 0).
        assert_eq!(state.last_cursor_mode.cursor_x, None);
        assert_eq!(state.last_cursor_mode.cursor_y, None);
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
                        wants_state_sync: false,
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

    /// phux-q0e.4: `on_frame_ack` advances `last_acked_seq` monotonically
    /// for in-order acks. Three acks (1, 2, 3) walk the field forward.
    #[test]
    fn on_frame_ack_advances_last_acked_seq_in_order() {
        let bundle = TerminalActor::new_with_seed(20, 5, b"hello").expect("new_with_seed");
        let mut actor = bundle.actor;
        let client = ClientId(1);
        let (tx, _rx) = dummy_outbound();
        // State-sync consumer: its acks belong to the per-consumer tick seq
        // space, so `on_frame_ack` folds them in (phux-38k6).
        actor
            .register_consumer(client, tx, 11, true)
            .expect("register");

        for seq in 1..=3 {
            actor.on_frame_ack(client, seq);
            assert_eq!(
                actor.consumer_state(client).expect("state").last_acked_seq,
                seq,
                "in-order ack must advance last_acked_seq",
            );
        }
    }

    /// phux-q0e.4: older or duplicate acks (`seq <= last_acked_seq`) MUST
    /// be silently dropped — they carry no new state information under
    /// SPEC §12.2's cumulative-ack semantics. After ack=5 then ack=3, the
    /// field must stay at 5.
    #[test]
    fn on_frame_ack_older_or_duplicate_is_dropped() {
        let bundle = TerminalActor::new_with_seed(20, 5, b"hello").expect("new_with_seed");
        let mut actor = bundle.actor;
        let client = ClientId(1);
        let (tx, _rx) = dummy_outbound();
        // State-sync consumer so its acks are processed (phux-38k6).
        actor
            .register_consumer(client, tx, 11, true)
            .expect("register");

        actor.on_frame_ack(client, 5);
        assert_eq!(actor.consumer_state(client).unwrap().last_acked_seq, 5);

        // Older ack.
        actor.on_frame_ack(client, 3);
        assert_eq!(
            actor.consumer_state(client).unwrap().last_acked_seq,
            5,
            "older ack must NOT regress last_acked_seq",
        );

        // Duplicate ack.
        actor.on_frame_ack(client, 5);
        assert_eq!(
            actor.consumer_state(client).unwrap().last_acked_seq,
            5,
            "duplicate ack must NOT touch last_acked_seq",
        );

        // Higher ack still progresses.
        actor.on_frame_ack(client, 6);
        assert_eq!(actor.consumer_state(client).unwrap().last_acked_seq, 6);
    }

    /// phux-38k6: a `FRAME_ACK` from a raw (broadcast-pump) consumer carries a
    /// pump-local seq unrelated to this per-consumer tick state, so
    /// `on_frame_ack` drops it — `last_acked_seq` must NOT move. Otherwise a
    /// foreign counter would skew the RTT/backpressure accounting if the
    /// consumer later went state-sync.
    #[test]
    fn on_frame_ack_for_raw_consumer_is_dropped() {
        let bundle = TerminalActor::new_with_seed(20, 5, b"hello").expect("new_with_seed");
        let mut actor = bundle.actor;
        // Global gate OFF (production human-attach default) and a raw consumer.
        let client = ClientId(1);
        let (tx, _rx) = dummy_outbound();
        actor
            .register_consumer(client, tx, 11, false)
            .expect("register");

        let folded = actor.on_frame_ack(client, 7);
        assert!(!folded, "raw-consumer ack produces no RTT sample");
        assert_eq!(
            actor.consumer_state(client).expect("state").last_acked_seq,
            0,
            "raw-pump ack must not advance the per-consumer last_acked_seq",
        );
    }

    /// phux-q0e.4: `on_frame_ack` for an unregistered client is a silent
    /// no-op — no panic, no entry created. Mirrors the rest of the
    /// consumer lifecycle's idempotency.
    #[test]
    fn on_frame_ack_for_unregistered_consumer_is_noop() {
        let bundle = TerminalActor::new_with_seed(20, 5, b"hello").expect("new_with_seed");
        let mut actor = bundle.actor;

        let stranger = ClientId(999);
        assert_eq!(actor.consumer_count(), 0);
        actor.on_frame_ack(stranger, 42);
        assert_eq!(actor.consumer_count(), 0, "no entry created by stray ack");
        assert!(actor.consumer_state(stranger).is_none());
    }

    /// phux-q0e.4: register, ack, then detach, then re-ack — the re-ack
    /// after detach is a no-op (no panic, no resurrection of the entry).
    #[test]
    fn on_frame_ack_after_detach_is_noop() {
        let bundle = TerminalActor::new_with_seed(20, 5, b"hello").expect("new_with_seed");
        let mut actor = bundle.actor;
        let client = ClientId(7);
        let (tx, _rx) = dummy_outbound();
        // State-sync so the pre-detach ack is folded in (phux-38k6); the
        // point of the test is that a *post*-detach ack does not resurrect.
        actor
            .register_consumer(client, tx, 11, true)
            .expect("register");
        actor.on_frame_ack(client, 2);
        assert_eq!(actor.consumer_state(client).unwrap().last_acked_seq, 2);

        actor.unregister_consumer(client);
        assert!(actor.consumer_state(client).is_none());

        // Late ack after detach: must not resurrect the entry.
        actor.on_frame_ack(client, 9);
        assert!(actor.consumer_state(client).is_none());
        assert_eq!(actor.consumer_count(), 0);
    }

    /// phux-0q8 coexistence gate: with a consumer registered but the
    /// emission gate forced OFF (`consumer_tick_emits == false`),
    /// `tick_emit` MUST NOT push any frame onto the consumer's outbound
    /// mailbox — even with dirty seeded content. This is the invariant
    /// that lets the per-consumer lifecycle run live alongside the
    /// broadcast pump without double-painting the client when the gate is
    /// off. Production defaults the gate OFF for human attach (phux-yeca),
    /// but this test still disables it explicitly so the invariant is local.
    #[test]
    fn tick_emit_is_silent_while_gate_is_off() {
        let bundle = TerminalActor::new(20, 5).expect("new");
        let mut actor = bundle.actor;
        actor.disable_tick_emit_for_test();
        let client = ClientId(1);
        let (tx, mut rx) = dummy_outbound();
        actor
            .register_consumer(client, tx, 11, false)
            .expect("register");
        // Make the grid genuinely dirty AFTER register so a non-gated tick
        // would have something to emit — proving the gate, not an empty diff.
        actor.vt_write_for_test(b"dirty-content");

        // Several ticks: the gate must keep every one silent.
        for _ in 0..3 {
            actor.tick_emit();
        }
        assert!(
            rx.try_recv().is_err(),
            "gate off: tick_emit must not emit while the broadcast pump is the live path",
        );
        // The per-consumer entry is still live (lifecycle is active) —
        // only emission is suppressed.
        assert_eq!(actor.consumer_count(), 1);
        assert_eq!(
            actor.consumer_state(client).expect("state").next_seq,
            1,
            "no emission means the per-consumer seq never advanced",
        );
    }

    /// phux-yeca: production defaults the synthesized tick emitter OFF so
    /// human TUI attach stays on the immediate raw PTY broadcast path.
    #[test]
    fn tick_emit_gate_defaults_off_for_human_attach() {
        let bundle = TerminalActor::new(20, 5).expect("new");
        let mut actor = bundle.actor;
        let client = ClientId(1);
        let (tx, mut rx) = dummy_outbound();
        actor
            .register_consumer(client, tx, 11, false)
            .expect("register");
        actor.vt_write_for_test(b"dirty-content");

        actor.tick_emit();

        assert!(
            rx.try_recv().is_err(),
            "default human attach path must not wait for synthesized tick output",
        );
    }

    /// phux-fseo: a consumer that negotiated `OutputMode::StateSync`
    /// (`wants_state_sync == true`) is served by the tick even with the
    /// global test gate OFF — the per-consumer opt-in is the production
    /// path. Proves the negotiation actually reaches `tick_emit` without
    /// relying on `enable_tick_emit_for_test`.
    #[test]
    fn tick_emit_serves_negotiated_state_sync_consumer_with_gate_off() {
        let bundle = TerminalActor::new(20, 5).expect("new");
        let mut actor = bundle.actor;
        // NB: global gate left at its production default (OFF).
        let client = ClientId(1);
        let (tx, mut rx) = dummy_outbound();
        actor
            .register_consumer(client, tx, 11, true)
            .expect("register");
        actor.vt_write_for_test(b"state-sync-marker");

        actor.tick_emit();

        let frame = rx
            .try_recv()
            .expect("state-sync consumer must be served by the tick even with the gate off");
        let Outbound::Frame(FrameKind::TerminalOutput { seq, .. }) = frame else {
            panic!("expected a TerminalOutput frame for the state-sync consumer");
        };
        assert_eq!(seq, 1, "first tick emission stamps seq=1");
    }

    /// phux-fseo: with the global gate OFF and two consumers sharing one
    /// pane — one `StateSync`, one `Raw` — the tick serves ONLY the
    /// state-sync consumer. The raw consumer is served by the runtime's
    /// broadcast pump; emitting to it here too would double-paint it.
    #[test]
    fn tick_emit_mixed_mode_serves_only_state_sync_consumer() {
        let bundle = TerminalActor::new(20, 5).expect("new");
        let mut actor = bundle.actor;
        let sync_client = ClientId(1);
        let raw_client = ClientId(2);
        let (sync_tx, mut sync_rx) = dummy_outbound();
        let (raw_tx, mut raw_rx) = dummy_outbound();
        actor
            .register_consumer(sync_client, sync_tx, 11, true)
            .expect("register state-sync");
        actor
            .register_consumer(raw_client, raw_tx, 12, false)
            .expect("register raw");
        actor.vt_write_for_test(b"shared-pane-write");

        actor.tick_emit();

        assert!(
            matches!(
                sync_rx.try_recv(),
                Ok(Outbound::Frame(FrameKind::TerminalOutput { .. })),
            ),
            "state-sync consumer must receive the synthesized delta",
        );
        assert!(
            raw_rx.try_recv().is_err(),
            "raw consumer must stay on the broadcast pump — tick must not double-paint it",
        );
    }

    /// phux-0q8 / phux-q0e.3 / phux-3uv / phux-ia4: with the gate ON for
    /// a SINGLE consumer, `tick_emit` diffs the
    /// dirty seeded grid against the consumer's reference and ships exactly
    /// one `TerminalOutput` carrying the content, stamping `seq = 1`.
    ///
    /// Emit-once (phux-ia4): the consumer's reference advances on emit, so
    /// a second tick with no further writes is SILENT — the change is
    /// delivered exactly once, not re-emitted every tick. A subsequent
    /// write produces a fresh single emission (`seq = 2`).
    #[test]
    fn tick_emit_emits_once_when_gate_is_on() {
        let bundle = TerminalActor::new(20, 5).expect("new");
        let mut actor = bundle.actor;
        actor.enable_tick_emit_for_test();
        let client = ClientId(1);
        let (tx, mut rx) = dummy_outbound();
        // Register against the (blank) terminal: the reference is primed
        // so deltas are measured "from now." Writing AFTER register is what
        // makes the next tick produce a diff.
        actor
            .register_consumer(client, tx, 11, false)
            .expect("register");
        actor.vt_write_for_test(b"q0e-marker");

        actor.tick_emit();
        let frame = rx
            .try_recv()
            .expect("gate on: first tick must emit the changed grid");
        let Outbound::Frame(FrameKind::TerminalOutput {
            terminal_id,
            seq,
            bytes,
        }) = frame
        else {
            panic!("expected a TerminalOutput frame from tick_emit");
        };
        assert_eq!(seq, 1, "first tick emission stamps seq=1");
        assert_eq!(
            terminal_id.local_id(),
            Some(11),
            "tick frame carries the registered wire terminal id",
        );
        assert!(
            contains_subslice(&bytes, b"q0e-marker"),
            "tick emission must carry the seeded grid content; got {:?}",
            String::from_utf8_lossy(&bytes),
        );

        // Emit-once: with no further writes, the reference now matches the
        // live grid, so the next tick is silent — NO re-emission of the
        // already-delivered change.
        actor.tick_emit();
        assert!(
            rx.try_recv().is_err(),
            "gate on: emit-once — an already-emitted change is not re-sent on the next tick",
        );

        // A fresh write produces a new single emission with the next seq.
        actor.vt_write_for_test(b" more");
        actor.tick_emit();
        let frame = rx
            .try_recv()
            .expect("gate on: a new write must emit a fresh diff");
        let Outbound::Frame(FrameKind::TerminalOutput { seq, bytes, .. }) = frame else {
            panic!("expected a TerminalOutput frame on the new write");
        };
        assert_eq!(seq, 2, "second distinct change stamps seq=2");
        assert!(
            contains_subslice(&bytes, b"more"),
            "second emission must carry the newly-written content; got {:?}",
            String::from_utf8_lossy(&bytes),
        );

        // FRAME_ACK advances last_acked_seq but does not itself trigger any
        // emission; the grid is unchanged so the next tick stays silent.
        actor.on_frame_ack(client, 2);
        actor.tick_emit();
        assert!(
            rx.try_recv().is_err(),
            "gate on: an unchanged grid stays silent after ack",
        );
        assert_eq!(
            actor.consumer_state(client).expect("state").next_seq,
            3,
            "two emissions advanced next_seq to 3; the post-ack tick was silent",
        );
        assert_eq!(
            actor.consumer_state(client).expect("state").last_acked_seq,
            2,
            "FRAME_ACK advanced last_acked_seq",
        );
    }

    /// phux-ia4 regression: TWO consumers sharing one pane. A single tick
    /// of new output MUST deliver the incremental to BOTH consumers — not
    /// just the first one walked.
    ///
    /// This is the exact starvation the ticket is about. Under the old
    /// per-consumer-`RenderState` dirty model, the first consumer's
    /// `RenderState::update` consumed the shared `Terminal` dirty bits, so
    /// the second consumer that tick observed `Dirty::Clean` and emitted
    /// nothing. The per-consumer reference grid removes that coupling: each
    /// consumer diffs against its own last-synced rows, so both receive the
    /// change in the same tick regardless of walk order.
    #[test]
    fn tick_emit_serves_every_consumer_on_a_shared_pane() {
        let bundle = TerminalActor::new(20, 5).expect("new");
        let mut actor = bundle.actor;
        // Gate ON (production default).
        actor.enable_tick_emit_for_test();

        // Two consumers on the same pane, primed against the same blank
        // terminal.
        let client_a = ClientId(1);
        let client_b = ClientId(2);
        let (tx_a, mut rx_a) = dummy_outbound();
        let (tx_b, mut rx_b) = dummy_outbound();
        actor
            .register_consumer(client_a, tx_a, 11, false)
            .expect("register a");
        actor
            .register_consumer(client_b, tx_b, 11, false)
            .expect("register b");

        // One tick of new output AFTER both are primed.
        actor.vt_write_for_test(b"shared-marker");
        actor.tick_emit();

        // BOTH consumers must receive a TerminalOutput carrying the marker.
        let recv_marker = |rx: &mut mpsc::Receiver<Outbound>, who: &str| {
            let frame = rx
                .try_recv()
                .unwrap_or_else(|_| panic!("consumer {who} starved: no frame this tick"));
            let Outbound::Frame(FrameKind::TerminalOutput { seq, bytes, .. }) = frame else {
                panic!("consumer {who}: expected a TerminalOutput frame");
            };
            assert_eq!(seq, 1, "consumer {who}: first emission stamps seq=1");
            assert!(
                contains_subslice(&bytes, b"shared-marker"),
                "consumer {who}: incremental must carry the shared marker; got {:?}",
                String::from_utf8_lossy(&bytes),
            );
        };
        recv_marker(&mut rx_a, "A");
        recv_marker(&mut rx_b, "B");

        // Emit-once per consumer: with no further writes, neither consumer
        // gets a re-emission on the next tick.
        actor.tick_emit();
        assert!(
            rx_a.try_recv().is_err(),
            "consumer A: emit-once — no re-emission on an unchanged tick",
        );
        assert!(
            rx_b.try_recv().is_err(),
            "consumer B: emit-once — no re-emission on an unchanged tick",
        );

        // Per-consumer independence: a consumer that detaches does not
        // perturb the other. A fresh write reaches the survivor exactly
        // once.
        actor.unregister_consumer(client_a);
        actor.vt_write_for_test(b" again");
        actor.tick_emit();
        let frame = rx_b.try_recv().expect("consumer B: must get the new write");
        let Outbound::Frame(FrameKind::TerminalOutput { seq, bytes, .. }) = frame else {
            panic!("consumer B: expected a TerminalOutput frame");
        };
        assert_eq!(seq, 2, "consumer B: second distinct change stamps seq=2");
        assert!(
            contains_subslice(&bytes, b"again"),
            "consumer B: must carry the second write; got {:?}",
            String::from_utf8_lossy(&bytes),
        );
        assert!(
            rx_a.try_recv().is_err(),
            "consumer A detached: must receive nothing further",
        );
    }

    /// phux-4l0: an idle tick (no write since the last tick, no consumer
    /// awaiting its first emission) short-circuits and emits nothing.
    #[test]
    fn idle_tick_short_circuits_and_emits_nothing() {
        let bundle = TerminalActor::new(20, 5).expect("new");
        let mut actor = bundle.actor;
        actor.enable_tick_emit_for_test();

        let client = ClientId(1);
        let (tx, mut rx) = dummy_outbound();
        actor
            .register_consumer(client, tx, 11, false)
            .expect("register");

        // First tick: the consumer needs its initial pass, so it is walked
        // (returns empty here — primed against a blank terminal) and the
        // dirty flag set at construction is consumed.
        actor.tick_emit();
        // Drain whatever the first tick produced (expected: nothing, since
        // the reference was primed to the same blank state).
        while rx.try_recv().is_ok() {}

        // Now write, tick, drain: the consumer receives the write.
        actor.vt_write_for_test(b"hello");
        actor.tick_emit();
        let got = rx.try_recv().expect("write must reach the consumer");
        let Outbound::Frame(FrameKind::TerminalOutput { bytes, .. }) = got else {
            panic!("expected TerminalOutput");
        };
        assert!(contains_subslice(&bytes, b"hello"));

        // Many idle ticks (no further writes): the short-circuit must keep
        // each one silent and must not perturb the consumer entry.
        for _ in 0..5 {
            actor.tick_emit();
            assert!(
                rx.try_recv().is_err(),
                "idle tick must emit nothing (short-circuit)",
            );
        }
        assert_eq!(actor.consumer_count(), 1, "consumer entry intact");
    }

    /// phux-4l0: a consumer registered AFTER the last write sits on a
    /// terminal that is `Clean` since the previous tick, yet has never had
    /// a synthesis pass. The `needs_initial_emit` carve-out must keep the
    /// short-circuit from starving it: the next tick must still walk it
    /// (here the write predates the attach, so it is already primed and
    /// the body is empty — the point is the entry is serviced, not
    /// skipped, preserving the phux-ia4 multi-consumer guarantee).
    #[test]
    fn new_consumer_served_even_when_terminal_clean() {
        let bundle = TerminalActor::new(20, 5).expect("new");
        let mut actor = bundle.actor;
        actor.enable_tick_emit_for_test();

        // Consumer A attaches, a write lands, a tick delivers it, then a
        // steady-state tick clears the dirty flag.
        let client_a = ClientId(1);
        let (tx_a, mut rx_a) = dummy_outbound();
        actor
            .register_consumer(client_a, tx_a, 11, false)
            .expect("reg a");
        actor.vt_write_for_test(b"first");
        actor.tick_emit();
        while rx_a.try_recv().is_ok() {}
        // Steady-state tick: terminal now Clean since last tick.
        actor.tick_emit();
        assert!(rx_a.try_recv().is_err(), "A steady-state: nothing");

        // Consumer B attaches with NO intervening write. The terminal is
        // Clean, but B has needs_initial_emit set, so the short-circuit
        // must NOT fire — B must be walked. (Primed to current state, so
        // the body is empty, but the entry is serviced and the flag
        // cleared.)
        let client_b = ClientId(2);
        let (tx_b, mut rx_b) = dummy_outbound();
        actor
            .register_consumer(client_b, tx_b, 11, false)
            .expect("reg b");
        assert!(
            actor
                .consumer_state(client_b)
                .expect("b present")
                .needs_initial_emit,
            "B should be awaiting its first emission",
        );
        actor.tick_emit();
        // B primed to current state ⇒ empty body, but the pass ran:
        // needs_initial_emit is now cleared.
        assert!(
            !actor
                .consumer_state(client_b)
                .expect("b present")
                .needs_initial_emit,
            "B's first pass must have run despite the Clean terminal",
        );
        assert!(rx_b.try_recv().is_err(), "B primed ⇒ empty first pass");

        // A fresh write after both are primed reaches BOTH.
        actor.vt_write_for_test(b" again");
        actor.tick_emit();
        let frame_a = rx_a.try_recv().expect("A gets the new write");
        let frame_b = rx_b.try_recv().expect("B gets the new write");
        for (who, frame) in [("A", frame_a), ("B", frame_b)] {
            let Outbound::Frame(FrameKind::TerminalOutput { bytes, .. }) = frame else {
                panic!("{who}: expected TerminalOutput");
            };
            assert!(
                contains_subslice(&bytes, b"again"),
                "{who} must carry the new write",
            );
        }
    }

    /// phux-ddg: a consumer whose outbound receiver has been dropped (a
    /// detach whose `ConsumerDetachRequest` never reached the actor — full
    /// mailbox) must be reaped by `tick_emit` rather than re-rendered every
    /// tick forever. The tick is self-healing: a `Closed` mailbox removes
    /// the entry.
    #[test]
    fn tick_emit_reaps_consumer_with_closed_mailbox() {
        let bundle = TerminalActor::new(20, 5).expect("new");
        let mut actor = bundle.actor;
        actor.enable_tick_emit_for_test();

        let client = ClientId(1);
        let (tx, rx) = dummy_outbound();
        actor
            .register_consumer(client, tx, 11, false)
            .expect("register");
        assert_eq!(actor.consumer_count(), 1);

        // Simulate the dropped-detach leak: the client's receiver goes
        // away (disconnect) but the detach request was lost, so the
        // per-consumer entry is still present.
        drop(rx);

        // A write makes the tick try to emit to the dead consumer; the
        // send fails Closed and the entry is reaped.
        actor.vt_write_for_test(b"content");
        actor.tick_emit();
        assert_eq!(
            actor.consumer_count(),
            0,
            "closed-mailbox consumer must be reaped by the tick",
        );

        // Subsequent ticks are stable no-ops.
        actor.vt_write_for_test(b"more");
        actor.tick_emit();
        assert_eq!(actor.consumer_count(), 0, "stays reaped");
    }

    /// phux-ddg: a consumer with a closed mailbox is reaped even when the
    /// diff body is empty (idle dead consumer). Without this, an idle but
    /// dead consumer would never hit the `try_send` Closed arm and would
    /// linger until pane teardown.
    #[test]
    fn tick_emit_reaps_idle_consumer_with_closed_mailbox() {
        let bundle = TerminalActor::new(20, 5).expect("new");
        let mut actor = bundle.actor;
        actor.enable_tick_emit_for_test();

        let client = ClientId(1);
        let (tx, rx) = dummy_outbound();
        actor
            .register_consumer(client, tx, 11, false)
            .expect("register");

        // Prime past the initial-emit pass with one tick (empty body).
        actor.tick_emit();
        assert_eq!(actor.consumer_count(), 1);

        // Receiver drops (disconnect). A write keeps the per-consumer loop
        // running this tick; whether the diff body is empty or not, the
        // `is_closed()` probe on the empty-body path and the `Closed` arm
        // on the send path both reap the entry.
        drop(rx);
        actor.vt_write_for_test(b"x");
        actor.tick_emit();
        assert_eq!(
            actor.consumer_count(),
            0,
            "dead consumer reaped even though it never acked",
        );
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

                handle
                    .resize
                    .send(ResizeRequest {
                        cols: 120,
                        rows: 40,
                        resync_clients: false,
                    })
                    .await
                    .expect("send resize");
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

    /// phux-8v1 regression: a resize must re-broadcast a full snapshot of
    /// the post-reflow grid so attached clients (whose mirror reflowed
    /// independently and may have dropped rows) reconverge on the
    /// canonical content. We assert the broadcast that follows a resize
    /// carries the snapshot reset preamble (`ESC [ ! p`, DECSTR) AND the
    /// content that was on the grid before the resize — without this fix
    /// the only post-resize bytes are new PTY output, so prior content is
    /// never re-sent and the client shows lost/duplicated rows.
    #[tokio::test]
    async fn resize_rebroadcasts_grid_snapshot_for_phux_8v1() {
        let local = tokio::task::LocalSet::new();
        local
            .run_until(async {
                let bundle = TerminalActor::new_with_seed(80, 24, b"phux8v1-marker").expect("seed");
                let handle = bundle.handle.clone();
                let token = bundle.token;
                // Subscribe BEFORE the actor runs so we don't miss the
                // resize broadcast.
                let mut out = handle.output.subscribe();
                let join = tokio::task::spawn_local(bundle.actor.run());

                handle
                    .resize
                    .send(ResizeRequest {
                        cols: 40,
                        rows: 10,
                        resync_clients: true,
                    })
                    .await
                    .expect("send resize");

                // Collect broadcast bytes for a bounded window and look
                // for the snapshot. `recv` resolves as soon as the resize
                // broadcast lands. phux-3ns5: the resync rides a
                // `PaneOutput::Resync` carrying the post-reflow dims, so
                // also capture them to assert the client mirror is told to
                // resize to 40x10.
                let mut acc: Vec<u8> = Vec::new();
                let mut resync_dims: Option<(u16, u16)> = None;
                for _ in 0..32 {
                    match tokio::time::timeout(std::time::Duration::from_millis(100), out.recv())
                        .await
                    {
                        Ok(Ok(PaneOutput::Resync { cols, rows, bytes })) => {
                            resync_dims = Some((cols, rows));
                            acc.extend_from_slice(&bytes);
                            if contains_subslice(&acc, b"\x1b[!p")
                                && contains_subslice(&acc, b"phux8v1-marker")
                            {
                                break;
                            }
                        }
                        Ok(Ok(PaneOutput::Live(bytes))) => acc.extend_from_slice(&bytes),
                        Ok(Err(_)) => break, // channel closed
                        Err(_) => tokio::task::yield_now().await, // timeout tick
                    }
                }
                assert_eq!(
                    resync_dims,
                    Some((40, 10)),
                    "resize resync must carry the post-reflow grid dims (phux-3ns5)",
                );

                assert!(
                    contains_subslice(&acc, b"\x1b[!p"),
                    "resize broadcast missing DECSTR snapshot preamble; got {:?}",
                    String::from_utf8_lossy(&acc),
                );
                assert!(
                    contains_subslice(&acc, b"phux8v1-marker"),
                    "resize broadcast did not re-send pre-resize grid content; got {:?}",
                    String::from_utf8_lossy(&acc),
                );

                token.cancel();
                tokio::time::timeout(std::time::Duration::from_millis(500), join)
                    .await
                    .expect("actor did not exit within 500ms")
                    .expect("actor task panicked");
            })
            .await;
    }

    /// phux-8v1 drag fix: a STORM of rapid live resizes (a window drag)
    /// must COALESCE into a single resync snapshot, not one per resize.
    /// Without the debounce the client gets flooded with snapshots
    /// synthesized at successive widths, and a stale-width one corrupts
    /// the mirror (the duplicated-characters-while-dragging symptom).
    /// We count broadcasts carrying the snapshot preamble (`ESC [ ! p`);
    /// each resync is exactly one such message, so the count is the
    /// snapshot count regardless of any interleaved PTY output.
    #[tokio::test]
    async fn rapid_resizes_coalesce_into_one_resync_snapshot() {
        let local = tokio::task::LocalSet::new();
        local
            .run_until(async {
                let bundle =
                    TerminalActor::new_with_seed(80, 24, b"drag-marker").expect("seed");
                let handle = bundle.handle.clone();
                let token = bundle.token;
                let mut out = handle.output.subscribe();
                let join = tokio::task::spawn_local(bundle.actor.run());

                // Fire a storm of live resizes back-to-back, well within
                // the RESIZE_RESYNC_DEBOUNCE window.
                for w in [70u16, 60, 50, 60, 70, 80, 90, 100] {
                    handle
                        .resize
                        .send(ResizeRequest { cols: w, rows: 24, resync_clients: true })
                        .await
                        .expect("send resize");
                }

                // Wait comfortably past the debounce so the single
                // coalesced snapshot has fired.
                tokio::time::sleep(RESIZE_RESYNC_DEBOUNCE * 4).await;

                // Count resync broadcasts. Debounced => exactly 1.
                // phux-3ns5: each resync is a `PaneOutput::Resync`, so the
                // variant itself is the count (no preamble sniffing needed).
                let mut snapshots = 0usize;
                loop {
                    match out.try_recv() {
                        Ok(PaneOutput::Resync { bytes, .. }) => {
                            debug_assert!(contains_subslice(&bytes, b"\x1b[!p"));
                            snapshots += 1;
                        }
                        // Live output and a lagged drop are both "not a
                        // resync" — skip and keep draining.
                        Ok(PaneOutput::Live(_))
                        | Err(tokio::sync::broadcast::error::TryRecvError::Lagged(_)) => {}
                        Err(_) => break,
                    }
                }
                assert_eq!(
                    snapshots, 1,
                    "a resize storm must coalesce into exactly one resync snapshot, got {snapshots}",
                );

                token.cancel();
                tokio::time::timeout(std::time::Duration::from_millis(500), join)
                    .await
                    .expect("actor did not exit within 500ms")
                    .expect("actor task panicked");
            })
            .await;
    }

    /// Crash-hunt: a storm of *degenerate* resizes — `0x0`, `1x1`,
    /// `1x200`, `200x1`, a 1000x1000 monster, and repeated both-axes
    /// shrinks crossing the 1-cell clamp — must NOT panic the actor task.
    /// `handle_resize` clamps to a 1-cell minimum so a zero dimension never
    /// reaches libghostty; the both-axes-shrink overflow in the Zig
    /// `PageList.resizeCols` is fixed in the vendored ghostty (phall1/ghostty
    /// 6d89054f3). We assert the actor is still alive (the `join` unwrap
    /// surfaces a panicked task) and a final sane resize still applies.
    #[tokio::test]
    async fn degenerate_resize_storm_does_not_panic_actor() {
        let local = tokio::task::LocalSet::new();
        local
            .run_until(async {
                let bundle = TerminalActor::new_with_seed(80, 24, b"crash-hunt").expect("seed");
                let handle = bundle.handle.clone();
                let token = bundle.token;
                let join = tokio::task::spawn_local(bundle.actor.run());

                let storm: &[(u16, u16)] = &[
                    (0, 0),
                    (1, 1),
                    (1, 200),
                    (200, 1),
                    (0, 0),
                    (1000, 1000),
                    (1, 1),
                    (3, 3),
                    (2, 2),
                    (1, 1),
                    (5, 1),
                    (1, 5),
                    (1, 1),
                ];
                for &(cols, rows) in storm {
                    handle
                        .resize
                        .send(ResizeRequest {
                            cols,
                            rows,
                            resync_clients: false,
                        })
                        .await
                        .expect("send resize");
                }
                // Let the actor drain the whole mailbox.
                for _ in 0..64 {
                    tokio::task::yield_now().await;
                }

                // A final sane resize must still take effect — proof the
                // actor survived and is processing, not wedged.
                handle
                    .resize
                    .send(ResizeRequest {
                        cols: 100,
                        rows: 30,
                        resync_clients: false,
                    })
                    .await
                    .expect("send final resize");
                for _ in 0..16 {
                    tokio::task::yield_now().await;
                }

                token.cancel();
                tokio::time::timeout(std::time::Duration::from_millis(500), join)
                    .await
                    .expect("actor did not exit within 500ms")
                    .expect("actor task panicked under degenerate resize storm");
            })
            .await;
    }

    /// phux-y06 regression (crash-hunt): a degenerate resize storm that
    /// includes both-axes shrinks (e.g. real `80x24 -> 1x1`) issued as
    /// BARE single `resize()` calls must NOT abort libghostty's
    /// `PageList.resizeCols` with an integer overflow.
    ///
    /// libghostty's `PageList.resizeCols` once overflowed (panic in Zig →
    /// SIGABRT) when cols AND rows shrank in one `resize()` call. The fix
    /// lives in the vendored ghostty (phall1/ghostty 6d89054f3, "fix:
    /// resize overflow"); this test proves THAT fix — not any phux-side
    /// axis decomposition — carries the load. It feeds reflowable content,
    /// then drives the storm with a 1-cell clamp only (the same input
    /// hygiene `handle_resize` keeps), issuing each step as one direct
    /// `resize()`. It must survive every step and settle at the final
    /// size. (Run as a plain `GhosttyTerminal` test so a regression aborts THIS
    /// test, not a flaky e2e teardown.)
    #[test]
    fn resize_desync_then_both_shrink_does_not_overflow() {
        let mut term = GhosttyTerminal::new(TerminalOptions {
            cols: 80,
            rows: 24,
            max_scrollback: 100,
        })
        .expect("term");
        // Enough scrollback content that a cols-reflow actually walks rows
        // (the overflow needs real content to reflow).
        for i in 0..300u32 {
            let line = format!("row-{i}-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\r\n");
            term.vt_write(line.as_bytes());
        }

        // The minimal trigger: a 0x0 (fails, no-op) immediately followed by
        // a both-shrink to 1x1, then the wider degenerate storm.
        let storm: &[(u16, u16)] = &[
            (0, 0),
            (1, 1),
            (1, 200),
            (200, 1),
            (0, 0),
            (1000, 1000),
            (1, 1),
            (3, 3),
            (2, 2),
            (1, 1),
            (100, 30),
        ];
        for &(req_cols, req_rows) in storm {
            for i in 0..40u32 {
                let line = format!("interleave-{i}-bbbbbbbbbbbbbbbbbbbbbbbbbbbb\r\n");
                term.vt_write(line.as_bytes());
            }
            // Mirror `handle_resize`: 1-cell clamp (input hygiene) only,
            // then a BARE single resize() per step — no axis decomposition.
            // The vendored ghostty fix is what keeps the both-shrink steps
            // from overflowing.
            let cols = req_cols.max(1);
            let rows = req_rows.max(1);
            let _ = term.resize(cols, rows, 0, 0);
        }

        // Survived without SIGABRT; the grid settled at the final sane size.
        assert_eq!(term.cols().expect("cols"), 100);
        assert_eq!(term.rows().expect("rows"), 30);
    }

    /// Drain every `TerminalOutput` currently queued on `rx`, returning the
    /// concatenated payload bytes and the ordered list of `seq`s.
    fn drain_terminal_output(rx: &mut mpsc::Receiver<Outbound>) -> (Vec<u8>, Vec<u64>) {
        let mut bytes = Vec::new();
        let mut seqs = Vec::new();
        while let Ok(frame) = rx.try_recv() {
            if let Outbound::Frame(FrameKind::TerminalOutput {
                seq, bytes: body, ..
            }) = frame
            {
                seqs.push(seq);
                bytes.extend_from_slice(&body);
            }
        }
        (bytes, seqs)
    }

    /// wave-hunt/server-lifecycle: a consumer whose outbound mailbox fills up
    /// under sustained output MUST NOT lose grid content. Once the client
    /// drains, every written marker must still be reconstructable from the
    /// delivered stream.
    ///
    /// Pre-fix this failed: `tick_emit` synthesized the delta (which commits
    /// the per-consumer reference to the just-rendered grid, emit-once) and
    /// THEN dropped the frame on a `Full` mailbox. The reference had already
    /// advanced past the dropped delta, so the next tick diffed against a
    /// reference that already included the dropped content and never
    /// re-emitted it — silent permanent content loss / mirror divergence.
    /// The fix reserves the outbound permit BEFORE synthesizing, so a full
    /// mailbox skips the consumer without advancing its reference.
    #[test]
    fn backpressured_consumer_loses_no_content_after_draining() {
        // More rounds than the mailbox holds so the tick's send hits `Full`.
        const ROUNDS: usize = 12;
        let bundle = TerminalActor::new(80, 24).expect("new");
        let mut actor = bundle.actor;
        actor.enable_tick_emit_for_test();

        let client = ClientId(1);
        // Tiny mailbox so a few ticks saturate it — same shape as the
        // production `DEFAULT_CLIENT_MAILBOX` pressure, smaller and faster.
        let (tx, mut rx) = mpsc::channel::<Outbound>(2);
        actor
            .register_consumer(client, tx, 11, false)
            .expect("register");

        // Write a distinct marker on its own line and tick to emit it,
        // WITHOUT draining the receiver. Every marker must survive.
        let markers: Vec<String> = (0..ROUNDS).map(|i| format!("MARK{i:03}=")).collect();
        for marker in &markers {
            actor.vt_write_for_test(format!("{marker}\r\n").as_bytes());
            actor.tick_emit();
        }

        // Drain, then keep ticking + draining so any content held back under
        // backpressure flows once there is room.
        let mut delivered = Vec::new();
        let (chunk, _seqs) = drain_terminal_output(&mut rx);
        delivered.extend_from_slice(&chunk);
        for _ in 0..ROUNDS * 2 {
            actor.tick_emit();
            let (chunk, _seqs) = drain_terminal_output(&mut rx);
            delivered.extend_from_slice(&chunk);
        }

        for marker in &markers {
            assert!(
                contains_subslice(&delivered, marker.as_bytes()),
                "marker {marker:?} never reached the consumer: content was lost \
                 under mailbox backpressure (reference advanced past a dropped \
                 frame). delivered={:?}",
                String::from_utf8_lossy(&delivered),
            );
        }
    }

    /// wave-hunt/server-lifecycle: the per-consumer monotonic `seq` must have
    /// no gaps in the delivered stream. A frame that is NOT shipped must NOT
    /// consume a `seq`. Pre-fix, `tick_emit` incremented `next_seq` and then
    /// dropped the frame on `Full`, burning a seq for a frame the consumer
    /// never saw — the client would observe a hole in the otherwise
    /// contiguous reliable-transport stream (SPEC §12.2) and could not
    /// distinguish loss from reorder.
    #[test]
    fn backpressured_consumer_sees_contiguous_seq_stream() {
        const ROUNDS: usize = 10;
        let bundle = TerminalActor::new(80, 24).expect("new");
        let mut actor = bundle.actor;
        actor.enable_tick_emit_for_test();

        let client = ClientId(1);
        let (tx, mut rx) = mpsc::channel::<Outbound>(2);
        actor
            .register_consumer(client, tx, 11, false)
            .expect("register");

        for i in 0..ROUNDS {
            actor.vt_write_for_test(format!("seqmark{i:03}\r\n").as_bytes());
            actor.tick_emit();
        }

        let mut all_seqs = Vec::new();
        let (_b, seqs) = drain_terminal_output(&mut rx);
        all_seqs.extend(seqs);
        for _ in 0..ROUNDS * 2 {
            actor.tick_emit();
            let (_b, seqs) = drain_terminal_output(&mut rx);
            all_seqs.extend(seqs);
        }

        assert!(
            !all_seqs.is_empty(),
            "expected at least one delivered frame"
        );
        for (idx, seq) in all_seqs.iter().enumerate() {
            let expected = u64::try_from(idx).expect("fits") + 1;
            assert_eq!(
                *seq, expected,
                "delivered seq stream must be contiguous from 1 with no gaps; \
                 a dropped-but-seq-burned frame leaves a hole. got={all_seqs:?}",
            );
        }
    }

    /// Naive subsequence search for test assertions on VT byte streams.
    fn contains_subslice(haystack: &[u8], needle: &[u8]) -> bool {
        haystack.windows(needle.len()).any(|w| w == needle)
    }

    // ---- phux-q0e.5: RTT-adaptive tick interval ----

    /// The EMA seeds on the first sample and converges toward a steady RTT
    /// across a handful of samples (TCP-RTO-style `α = 0.125`).
    #[test]
    fn rtt_estimator_seeds_then_converges() {
        let mut est = RttEstimator::default();
        assert_eq!(est.smoothed(), None, "no sample yet");

        // First sample seeds srtt directly.
        est.observe(std::time::Duration::from_millis(100));
        assert_eq!(
            est.smoothed(),
            Some(std::time::Duration::from_millis(100)),
            "first sample seeds srtt exactly",
        );

        // Feed a steady 100ms stream; srtt must stay put (no drift).
        for _ in 0..20 {
            est.observe(std::time::Duration::from_millis(100));
        }
        let srtt = est.smoothed().expect("has srtt");
        assert!(
            srtt.abs_diff(std::time::Duration::from_millis(100))
                < std::time::Duration::from_millis(1),
            "steady 100ms stream keeps srtt ~100ms, got {srtt:?}",
        );

        // Step the RTT up to 300ms; the EMA moves slowly (one step folds in
        // only α of the gap), then converges over many samples.
        let before = est.smoothed().expect("has srtt");
        est.observe(std::time::Duration::from_millis(300));
        let after_one = est.smoothed().expect("has srtt");
        assert!(
            after_one > before && after_one < std::time::Duration::from_millis(150),
            "one 300ms sample nudges srtt but does not jump to it: {before:?} -> {after_one:?}",
        );
        for _ in 0..100 {
            est.observe(std::time::Duration::from_millis(300));
        }
        let converged = est.smoothed().expect("has srtt");
        assert!(
            converged.abs_diff(std::time::Duration::from_millis(300))
                < std::time::Duration::from_millis(2),
            "srtt converges toward the new 300ms RTT, got {converged:?}",
        );
    }

    /// The adaptive interval is `RTT/2` clamped to [20ms, 200ms]: a near-zero
    /// RTT clamps to the 20ms floor (snappier than the 30ms default), and a
    /// huge RTT clamps to the 200ms ceiling.
    #[test]
    fn adaptive_interval_clamps_both_ends() {
        // Near-zero local RTT -> floor (50 Hz), strictly faster than the
        // fixed 33 Hz default this replaces.
        assert_eq!(
            adaptive_tick_interval(std::time::Duration::from_micros(10)),
            MIN_TICK_INTERVAL,
            "near-zero RTT clamps to the 20ms floor",
        );
        assert!(
            MIN_TICK_INTERVAL < DEFAULT_TICK_INTERVAL,
            "the floor must be snappier than the old fixed cadence",
        );

        // Mid-band: 80ms RTT -> 40ms tick (unclamped RTT/2).
        assert_eq!(
            adaptive_tick_interval(std::time::Duration::from_millis(80)),
            std::time::Duration::from_millis(40),
            "mid-band RTT maps to exactly RTT/2",
        );

        // Satellite-class RTT -> ceiling (5 Hz).
        assert_eq!(
            adaptive_tick_interval(std::time::Duration::from_secs(2)),
            MAX_TICK_INTERVAL,
            "huge RTT clamps to the 200ms ceiling",
        );
    }

    /// An estimator with no sample reports the cold-start default; once a
    /// sample lands, `desired_tick_interval` tracks the clamped RTT/2.
    #[test]
    fn desired_interval_defaults_then_adapts() {
        let mut est = RttEstimator::default();
        assert_eq!(
            est.desired_tick_interval(),
            DEFAULT_TICK_INTERVAL,
            "no sample -> cold-start default",
        );
        est.observe(std::time::Duration::from_millis(120));
        assert_eq!(
            est.desired_tick_interval(),
            std::time::Duration::from_millis(60),
            "after a 120ms sample -> 60ms tick",
        );
    }

    /// End-to-end through the actor: a `FRAME_ACK` measured against a large
    /// simulated transit time backs the shared cadence off toward the 200ms
    /// ceiling; a near-zero transit time pins it to the 20ms floor. Uses
    /// paused tokio time so the emit->ack gap is exact and deterministic.
    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn actor_cadence_backs_off_on_high_rtt_and_floors_on_low() {
        let bundle = TerminalActor::new(80, 24).expect("new");
        let mut actor = bundle.actor;
        actor.enable_tick_emit_for_test();

        // Slow peer: a write + tick emits seq=1 and stamps its emit instant.
        let slow = ClientId(1);
        let (tx_slow, _rx_slow) = mpsc::channel::<Outbound>(16);
        actor
            .register_consumer(slow, tx_slow, 11, false)
            .expect("register slow");
        assert_eq!(
            actor.adaptive_tick_interval_for_test(),
            DEFAULT_TICK_INTERVAL,
            "no sample yet -> cold-start cadence",
        );

        actor.vt_write_for_test(b"hello\r\n");
        actor.tick_emit();
        // Simulate a 400ms round trip before the client acks seq=1.
        tokio::time::advance(std::time::Duration::from_millis(400)).await;
        assert!(
            actor.on_frame_ack_for_test(slow, 1),
            "ack of an emitted seq produces an RTT sample",
        );
        // 400ms RTT -> 200ms RTT/2 -> clamps to the 200ms ceiling.
        assert_eq!(
            actor.adaptive_tick_interval_for_test(),
            MAX_TICK_INTERVAL,
            "high-RTT consumer backs the cadence off to the ceiling",
        );

        // Fast peer joins; the shared cadence is the MINIMUM desired, so the
        // near-zero-RTT peer pulls it back down to the floor regardless of
        // the slow peer.
        let fast = ClientId(2);
        let (tx_fast, _rx_fast) = mpsc::channel::<Outbound>(16);
        actor
            .register_consumer(fast, tx_fast, 12, false)
            .expect("register fast");
        actor.vt_write_for_test(b"world\r\n");
        actor.tick_emit();
        // Near-instant ack: advance time by a sub-millisecond sliver.
        tokio::time::advance(std::time::Duration::from_micros(50)).await;
        // Both consumers were emitted to on the tick above; the fast peer's
        // first emitted seq is 1.
        assert!(
            actor.on_frame_ack_for_test(fast, 1),
            "fast peer ack produces a sample",
        );
        assert_eq!(
            actor.adaptive_tick_interval_for_test(),
            MIN_TICK_INTERVAL,
            "the fastest consumer pins the shared cadence to the floor",
        );

        // The slow peer leaving must not regress the floor (fast peer still
        // present), and dropping the fast peer reverts to the cold-start
        // default (no samples left to consult).
        actor.unregister_consumer(slow);
        assert_eq!(
            actor.adaptive_tick_interval_for_test(),
            MIN_TICK_INTERVAL,
            "fast peer still present -> still at the floor",
        );
        actor.unregister_consumer(fast);
        assert_eq!(
            actor.adaptive_tick_interval_for_test(),
            DEFAULT_TICK_INTERVAL,
            "no consumers left -> cold-start default",
        );
    }

    /// An ack that matches no recorded emit instant (e.g. the consumer never
    /// had a frame shipped) yields no RTT sample and leaves the cadence at
    /// the default — the round-trip machinery is inert without an emission.
    #[test]
    fn ack_without_emit_instant_produces_no_sample() {
        let bundle = TerminalActor::new(80, 24).expect("new");
        let mut actor = bundle.actor;
        actor.enable_tick_emit_for_test();

        let client = ClientId(1);
        let (tx, _rx) = dummy_outbound();
        actor
            .register_consumer(client, tx, 11, false)
            .expect("register");

        // No tick_emit ran, so no emit instant was stamped. An ack here
        // advances last_acked_seq but cannot time a round trip.
        assert!(
            !actor.on_frame_ack_for_test(client, 5),
            "ack with no matching emit instant produces no RTT sample",
        );
        assert_eq!(
            actor.adaptive_tick_interval_for_test(),
            DEFAULT_TICK_INTERVAL,
            "cadence stays at the default without a sample",
        );
    }

    /// phux-ahk: a state-sync consumer that never sends `FRAME_ACK` must not
    /// grow `emit_instants` without bound. Ack-pruning never runs for it, so
    /// the per-tick insert is bounded only by the defensive
    /// [`MAX_EMIT_INSTANTS`] cap (oldest-evicted). Drive many more emitting
    /// ticks than the cap, never acking, and assert the map stays capped and
    /// retains the newest (highest-`seq`) samples rather than the stale ones.
    #[test]
    fn emit_instants_is_capped_for_never_acking_consumer() {
        let bundle = TerminalActor::new(20, 5).expect("new");
        let mut actor = bundle.actor;
        let client = ClientId(1);
        let (tx, mut rx) = dummy_outbound();
        actor
            .register_consumer(client, tx, 11, true)
            .expect("register");

        // Far more emitting ticks than the cap. Distinct content each tick
        // keeps the grid dirty so the diff is non-empty and the tick actually
        // emits (and inserts). Drain the mailbox each tick so the send keeps
        // succeeding — a full mailbox would backpressure and skip the insert,
        // hiding the growth this test pins.
        let ticks = MAX_EMIT_INSTANTS + 64;
        for i in 0..ticks {
            actor.vt_write_for_test(&[b'a' + u8::try_from(i % 26).expect("0..26 fits u8")]);
            actor.tick_emit();
            while rx.try_recv().is_ok() {}
        }

        let state = actor.consumer_state(client).expect("state present");
        assert!(
            state.emit_instants.len() <= MAX_EMIT_INSTANTS,
            "emit_instants must stay capped at {} for a never-acking consumer; got {}",
            MAX_EMIT_INSTANTS,
            state.emit_instants.len(),
        );
        // Eviction drops the oldest seqs, so emission must actually have run
        // past the cap (otherwise this test proves nothing) and the lowest
        // retained key is well above the first seq.
        let lowest = *state.emit_instants.keys().next().expect("non-empty map");
        assert!(
            lowest > 1,
            "oldest emit instants should have been evicted; lowest retained seq = {lowest}",
        );
    }
}
