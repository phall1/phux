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

use crate::grid::{ConsumerReference, SnapshotBytes, SnapshotSynthesizer};
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
    /// `ALT_SCREEN` (DEC private mode 1047).
    pub alt_screen: bool,
    /// `ALT_SCREEN_SAVE` (DEC private mode 1049) — the mode vim/less/man/
    /// htop/tmux actually use. Tracked alongside 47 so a 47<->1049
    /// transition still trips the diff trigger; 47 and 1049 are
    /// independent bits in libghostty, so tracking only 47 would miss it.
    pub alt_screen_save: bool,
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
            alt_screen: terminal.mode(Mode::ALT_SCREEN).unwrap_or(false),
            alt_screen_save: terminal.mode(Mode::ALT_SCREEN_SAVE).unwrap_or(false),
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
    /// Per-consumer reference grid for the lazy state-sync diff
    /// (phux-ia4). Holds the last-synced rendered body of every viewport
    /// row plus the last-synced cursor/mode state. The tick driver diffs
    /// the live `Terminal` against this (via the actor's shared
    /// [`SnapshotSynthesizer`]) and advances it on emit.
    ///
    /// This replaces the earlier per-consumer `RenderState` dirty cache.
    /// `RenderState::update` *consumes* the shared `Terminal` dirty bits
    /// on the first read each tick (libghostty `render.zig`), so a
    /// per-consumer `RenderState` could not isolate dirty across N
    /// consumers on one pane: the first consumer's `update` starved the
    /// rest. The reference grid is fully independent per consumer and
    /// never reads the shared dirty bits, so every consumer gets its own
    /// correct diff each tick regardless of attach/ack divergence. See
    /// [`SnapshotSynthesizer::synthesize_against_reference`].
    pub reference: ConsumerReference,
    /// Per-consumer outbound mailbox the tick driver pushes
    /// `TERMINAL_OUTPUT` frames into. Cloned from the
    /// [`crate::state::AttachedClient`]'s `tx` at ATTACH time.
    pub outbound: mpsc::Sender<Outbound>,
    /// Wire-level terminal id for the `TerminalOutput` frame
    /// (`docs/spec/L1.md` §2.1). Carried per-consumer because the runtime owns
    /// the mapping `(TerminalActor, WireTerminalId)` and may differ
    /// across consumers in future tier topologies.
    pub wire_terminal_id: u32,
    /// Per-consumer monotonic sequence id for `TERMINAL_OUTPUT`
    /// (`docs/spec/L1.md` §2.1, §12). Starts at `1` and increments on each
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
    /// Set `true` at registration, cleared after this consumer's first
    /// pass through the actor's per-tick synthesis (phux-4l0).
    ///
    /// The idle short-circuit skips the per-consumer row walk when the
    /// shared terminal is `Clean` since the previous tick. A consumer
    /// registered *after* the last write sits on a `Clean` terminal yet
    /// has never been diffed; its reference is primed (so the diff is
    /// empty and emit-once still holds), but we must still run its first
    /// synthesis pass rather than silently skip it — this is the
    /// "needs prime / has diverged" case the phux-ia4 fix must preserve.
    /// While any consumer has this set, the short-circuit is suppressed.
    pub needs_initial_emit: bool,
    /// Set when a tick skipped this consumer because its outbound mailbox
    /// was full (backpressure), so its reference is *behind* the live grid
    /// even though the grid has not mutated since. The idle short-circuit
    /// must not skip the per-consumer loop while any consumer is behind, or
    /// the held-back delta would never be retried once the client drains
    /// (the terminal can stay `Clean` indefinitely). Cleared the moment this
    /// consumer is successfully served — either a delta ships or the diff is
    /// empty (reference caught up to the grid).
    pub behind: bool,
    /// Smoothed RTT estimate for the RTT-adaptive tick cadence (phux-q0e.5).
    /// Fed one sample per `FRAME_ACK`; drives this consumer's desired tick
    /// interval. See [`RttEstimator`].
    pub rtt: RttEstimator,
    /// Emit timestamps for in-flight (emitted, not-yet-acked) `seq`s, used to
    /// measure RTT server-side when the matching `FRAME_ACK` arrives
    /// (phux-q0e.5). Keyed by the `seq` stamped on each `TERMINAL_OUTPUT`;
    /// the value is the `tokio::time::Instant` the frame was handed to the
    /// outbound mailbox. Pruned up to the acked `seq` on every ack so it
    /// stays bounded by the number of frames in flight within one RTT (a
    /// handful at the clamped cadence). No wire change: the RTT round-trip
    /// rides the `seq` that `FRAME_ACK` already echoes.
    pub emit_instants: std::collections::BTreeMap<u64, tokio::time::Instant>,
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
    /// `Ok(outcome)` on success (the outcome reports whether this actor
    /// is tick-managing the consumer); `Err(...)` if the per-consumer
    /// `SnapshotSynthesizer` or its priming pass could not be allocated.
    /// Dropping the receiver on the caller side is benign — the actor
    /// uses `send().ok()`.
    pub reply: oneshot::Sender<Result<ConsumerAttachOutcome, ConsumerAttachError>>,
}

/// Successful outcome of a [`ConsumerAttachRequest`].
///
/// phux-3uv: the runtime needs to know whether this actor will *emit*
/// `TERMINAL_OUTPUT` for the consumer via the state-sync tick
/// (`consumer_tick_emits == true`). If so, the runtime must SUPPRESS its
/// own broadcast pump for this pane so exactly one emitter serves the
/// consumer (SPEC §12.2 monotonic-per-consumer; two independent `seq`
/// streams on one mailbox would double-paint). If the actor is not
/// tick-managing (gate off, or a non-emitting variant), the runtime keeps
/// the broadcast pump as the sole emitter.
#[derive(Debug, Clone, Copy)]
pub struct ConsumerAttachOutcome {
    /// `true` ⇒ this actor's `tick_emit` will push `TERMINAL_OUTPUT`
    /// frames for the consumer; the runtime must NOT also spawn a
    /// broadcast pump for this pane.
    pub tick_managed: bool,
}

/// Errors surfaced by the private `TerminalActor::register_consumer`
/// path in response to a [`ConsumerAttachRequest`].
#[derive(Debug, thiserror::Error)]
pub enum ConsumerAttachError {
    /// libghostty refused to allocate the one-shot `RenderState` used to
    /// capture the consumer's initial cursor/mode state.
    #[error("libghostty allocation failed: {0}")]
    Ghostty(#[from] libghostty_vt::Error),
    /// Priming the per-consumer reference grid
    /// (`SnapshotSynthesizer::prime_reference`) failed.
    #[error("reference priming failed: {0}")]
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

/// Inbound `FRAME_ACK` request for the per-consumer state-sync loop
/// (phux-q0e.4 / ADR-0018 addendum).
///
/// Routed by `runtime.rs::handle_frame_ack` after it has resolved the
/// `wire_terminal_id` to a `TerminalActor`. The runtime is the only
/// thing that knows the `(client_id, wire_terminal_id) -> actor` mapping,
/// so it strips `terminal_id` before forwarding — the actor already
/// knows which terminal it is.
///
/// Silent no-op if `client_id` is not currently registered (matches the
/// idempotency of the rest of the consumer lifecycle). No reply: the
/// actor does the work in-process and the runtime fires-and-forgets.
#[derive(Debug)]
pub struct ConsumerAckRequest {
    /// Identifier whose [`ConsumerSyncState`]'s dirty cache to evict.
    pub client_id: ClientId,
    /// Cumulative ack sequence (per SPEC §12.2): the highest `seq` from
    /// `TERMINAL_OUTPUT` this consumer has applied. Strictly-monotonic
    /// against the per-consumer `last_acked_seq` — older/duplicate acks
    /// are silently dropped.
    pub seq: u64,
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

/// Per-Terminal scrollback cap used by the no-config convenience
/// constructors ([`TerminalActor::new`] / [`TerminalActor::new_with_command`]).
/// A tmux-style mid-range value; the runtime path overrides it with
/// `defaults.history-limit` via [`TerminalActor::build_with_token`].
const DEFAULT_MAX_SCROLLBACK: u32 = 10_000;

/// Default tick interval for the state-sync emission driver, used until a
/// consumer's RTT has been measured (phux-q0e.3, phux-q0e.5).
///
/// 30 ms ≈ 33 Hz; per ADR-0018 / `research/archive/2026-05-26-state-sync-algorithm.md`
/// §"tick scheduler" first-cut. Once a consumer's RTT is known the cadence
/// becomes RTT-adaptive (see [`adaptive_tick_interval`] and
/// [`RttEstimator`]); this value is the cold-start cadence before the first
/// `FRAME_ACK` round-trip lands, and the steady-state cadence on transports
/// (no-PTY test actors, never-acking peers) that produce no RTT samples.
pub const DEFAULT_TICK_INTERVAL: std::time::Duration = std::time::Duration::from_millis(30);

/// Lower clamp on the RTT-adaptive tick interval (phux-q0e.5).
///
/// 20 ms ≈ 50 Hz. Mosh (`research/archive/2026-05-26-state-sync-algorithm.md`
/// §"tick scheduler") clamps the `RTT/2` cadence to `[20 ms, 200 ms]`; we
/// adopt the same band. The floor is deliberately *below* the 30 ms cold-start
/// default so a near-zero local-UDS RTT clamps here (50 Hz) — snappier than,
/// and never slower than, today's fixed 33 Hz. High-RTT transports back off
/// toward the ceiling.
pub const MIN_TICK_INTERVAL: std::time::Duration = std::time::Duration::from_millis(20);

/// Upper clamp on the RTT-adaptive tick interval (phux-q0e.5).
///
/// 200 ms = 5 Hz. The Mosh ceiling: past this a high-RTT/satellite link is
/// shipping state nobody can ack in time, so we stop spending CPU + bandwidth
/// synthesizing diffs faster than the link can drain them.
pub const MAX_TICK_INTERVAL: std::time::Duration = std::time::Duration::from_millis(200);

/// EMA smoothing factor for the per-consumer smoothed RTT (phux-q0e.5).
///
/// `srtt = (1 - α)·srtt + α·sample`. `α = 1/8 = 0.125` is TCP's RTO
/// estimator constant (RFC 6298 §2); it weights ~8 recent samples, so a
/// single spurious RTT spike nudges the cadence rather than yanking it.
/// "Adjust slowly" (Mosh §3) is exactly this slow convergence. The factor is
/// a documented default; real-traffic tuning (the ticket's deferred data) can
/// revisit it without touching the surrounding machinery.
pub const RTT_EMA_ALPHA: f64 = 0.125;

/// Smallest tick-interval change (in either direction) that triggers a
/// rebuild of the shared `tokio::time::Interval` (phux-q0e.5).
///
/// Rebuilding the shared timer on every sub-millisecond EMA wobble would churn
/// the scheduler for no observable benefit. A 5 ms deadband means the cadence
/// only re-arms on a meaningful RTT shift, and the steady state is stable.
const TICK_RESET_DEADBAND: std::time::Duration = std::time::Duration::from_millis(5);

/// Per-consumer smoothed round-trip-time estimator (phux-q0e.5, Mosh §3).
///
/// Feeds one RTT sample per `FRAME_ACK` (measured server-side as
/// `now − emit_instant` for the acked `seq`; no wire change — `seq` already
/// round-trips on `FRAME_ACK`) into a TCP-RTO-style EMA. The smoothed value
/// drives the adaptive tick cadence via [`adaptive_tick_interval`].
///
/// `None` smoothed value means "no sample yet": the consumer runs at the
/// [`DEFAULT_TICK_INTERVAL`] cold-start cadence and contributes that to the
/// shared-tick minimum (the actor takes the per-consumer minimum to drive
/// one shared timer).
#[derive(Debug, Clone, Copy, Default)]
pub struct RttEstimator {
    /// Smoothed RTT (`srtt`). `None` until the first sample lands.
    srtt: Option<std::time::Duration>,
}

impl RttEstimator {
    /// Fold one RTT `sample` into the smoothed estimate.
    ///
    /// First sample seeds `srtt` directly (RFC 6298 §2.2 initial assignment);
    /// later samples blend via `srtt = (1 − α)·srtt + α·sample` with
    /// `α` is [`RTT_EMA_ALPHA`]. Saturating, f64-internal math: a wild sample
    /// can only move `srtt` toward it, never panic or overflow.
    pub fn observe(&mut self, sample: std::time::Duration) {
        let sample_s = sample.as_secs_f64();
        let next = self.srtt.map_or(sample_s, |prev| {
            let prev_s = prev.as_secs_f64();
            RTT_EMA_ALPHA.mul_add(sample_s, (1.0 - RTT_EMA_ALPHA) * prev_s)
        });
        // `from_secs_f64` panics on negative/NaN/overflow; clamp the input to
        // a sane non-negative range first so a degenerate sample is inert.
        self.srtt = Some(std::time::Duration::from_secs_f64(next.clamp(0.0, 3600.0)));
    }

    /// The current smoothed RTT, or `None` if no sample has landed yet.
    #[must_use]
    pub const fn smoothed(&self) -> Option<std::time::Duration> {
        self.srtt
    }

    /// This consumer's desired tick interval: `clamp(srtt/2, MIN, MAX)`, or
    /// [`DEFAULT_TICK_INTERVAL`] while no sample exists. See
    /// [`adaptive_tick_interval`].
    #[must_use]
    pub fn desired_tick_interval(&self) -> std::time::Duration {
        self.srtt
            .map_or(DEFAULT_TICK_INTERVAL, adaptive_tick_interval)
    }
}

/// Map a smoothed RTT to a tick interval: `RTT/2` clamped to the
/// [`MIN_TICK_INTERVAL`]..=[`MAX_TICK_INTERVAL`] band (phux-q0e.5, Mosh §3).
///
/// Half-RTT is the Mosh target: a tick every half round-trip keeps the
/// emission cadence matched to how fast the consumer can actually ack. A
/// near-zero local RTT clamps to the 20 ms floor (50 Hz); a 400 ms satellite
/// RTT clamps to the 200 ms ceiling (5 Hz).
#[must_use]
pub fn adaptive_tick_interval(srtt: std::time::Duration) -> std::time::Duration {
    (srtt / 2).clamp(MIN_TICK_INTERVAL, MAX_TICK_INTERVAL)
}

/// Debounce window for the post-resize client resync (phux-8v1).
///
/// Dragging a terminal window fires a SIGWINCH storm — one
/// `VIEWPORT_RESIZE` (hence one resize) per step, many per second.
/// Broadcasting a full snapshot on each would flood the client with
/// snapshots synthesized at successive widths; one synthesized at width
/// N that lands on a mirror already resized to width M wraps/duplicates
/// rows. Instead we coalesce: arm a timer on each resync-requesting
/// resize and emit a single snapshot this long after the *last* one,
/// synthesized at the final settled size. 50 ms sits above per-SIGWINCH
/// cadence and below human settle perception.
pub const RESIZE_RESYNC_DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(50);

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

/// Request for the pane's current screen as structured data
/// (`phux-oki`, ADR-0022).
///
/// Sent by the `GET_SCREEN` command handler. The actor walks its own
/// `Terminal` into a [`phux_core::screen::ScreenState`] and replies on the
/// oneshot — side-effect-free: no resize, no client disturbance, unlike
/// the attach path.
#[derive(Debug)]
pub struct ScreenRequest {
    /// Wire-local pane id to stamp into the projected `ScreenState`.
    pub pane: u32,
    /// Requested scrollback history (`phux-o1v`): `None` for viewport only,
    /// `Some(0)` for all retained history, `Some(n)` for the most-recent
    /// `n` history rows. Carried from `GET_SCREEN.request_scrollback`.
    pub scrollback: Option<u32>,
    /// When `true`, populate [`phux_core::screen::ScreenState::cells`] with
    /// per-cell semantic marks + styles. Carried from `GET_SCREEN.cells`
    /// (`phux-8yl`).
    pub cells: bool,
    /// Channel the actor uses to ship the projection back. Dropping the
    /// receiver is benign — the actor discards the reply.
    pub reply: oneshot::Sender<phux_core::screen::ScreenState>,
}

/// Request for the pane's live current working directory (`phux-cs6`).
///
/// Sent by the `SPAWN_TERMINAL` handler when `defaults.cwd-inheritance`
/// is [`phux_config::CwdInheritance::InheritFocused`] and the wire frame
/// left `cwd` unset: the new pane should open in the focused pane's live
/// CWD. The actor asks the kernel for its PTY child's working directory
/// via [`crate::cwd_query::process_cwd`] (the shell's directory *now*,
/// after any `cd`) and replies on the oneshot. Side-effect-free: no
/// resize, no client disturbance.
///
/// This deliberately does not use libghostty's `Terminal::pwd`: the
/// bundled libghostty surfaces OSC 7 only as an opaque `ReportPwd`
/// command without exposing the announced path, so that getter never
/// populates from the byte stream. The kernel query also needs no shell
/// OSC 7 configuration.
///
/// The reply is `None` when there is no PTY (no-PTY actor), the child has
/// no pid (already exited), or the platform query is unsupported/denied —
/// the caller then falls back to a non-inherited default.
#[derive(Debug)]
pub struct PwdRequest {
    /// Channel the actor uses to ship the working directory back.
    /// `None` ⇒ no resolvable CWD. Dropping the receiver is benign — the
    /// actor discards the reply.
    pub reply: oneshot::Sender<Option<String>>,
}

/// A resize request delivered to a [`TerminalActor`] over its `resize`
/// mailbox.
///
/// `resync_clients` controls the phux-8v1 post-resize behavior: when
/// `true`, the actor re-broadcasts a full grid snapshot after the reflow
/// so attached clients (whose mirror reflowed independently and may have
/// dropped rows) reconverge. It is `true` for *live* resizes from an
/// already-attached client (SIGWINCH → `VIEWPORT_RESIZE`/`TERMINAL_RESIZE`)
/// and `false` for the ATTACH-time resize — the attach handshake already
/// sends an authoritative `TERMINAL_SNAPSHOT`, and a resync broadcast
/// there would race ahead of it and reorder the handshake.
#[derive(Debug, Clone, Copy)]
pub struct ResizeRequest {
    /// New grid width in cells.
    pub cols: u16,
    /// New grid height in cells.
    pub rows: u16,
    /// Re-broadcast a full snapshot after reflow (live resize) vs stay
    /// quiet (attach-time resize). See the type-level doc.
    pub resync_clients: bool,
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
    /// Sender for structured screen reads. The `GET_SCREEN` command
    /// handler uses this to project the pane's grid to JSON without
    /// attaching (`phux-oki`, ADR-0022 §5).
    pub screen: mpsc::Sender<ScreenRequest>,
    /// Sender for working-directory reads (`phux-cs6`). The
    /// `SPAWN_TERMINAL` handler uses this to resolve
    /// `defaults.cwd-inheritance = inherit-focused`: it asks the focused
    /// pane's actor for its live CWD (a kernel query against the PTY
    /// child, see [`PwdRequest`]) and seeds the new pane's
    /// `CommandBuilder.cwd` with it.
    pub pwd: mpsc::Sender<PwdRequest>,
    /// Output broadcast channel; subscribers receive every PTY byte
    /// chunk forwarded by the actor.
    pub output: broadcast::Sender<Bytes>,
    /// Resize control channel. The actor honours each request by
    /// resizing libghostty's `Terminal` and the PTY winsize ioctl, and
    /// (when [`ResizeRequest::resync_clients`] is set) re-broadcasting a
    /// full grid snapshot so client mirrors reconverge after reflow
    /// (phux-8v1).
    pub resize: mpsc::Sender<ResizeRequest>,
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
    /// ADR-0018 inbound `FRAME_ACK` channel (phux-q0e.4). The runtime
    /// sends one [`ConsumerAckRequest`] per decoded `FRAME_ACK` whose
    /// `terminal_id` resolved to this actor; the actor evicts the
    /// per-consumer dirty cache so the next tick re-diffs against the
    /// freshly-acked reference. Silent no-op if the consumer is not
    /// currently registered.
    pub consumer_ack: mpsc::Sender<ConsumerAckRequest>,
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
    /// Cheap idle short-circuit for [`Self::tick_emit`] (phux-4l0).
    ///
    /// `true` whenever the canonical [`Terminal`] has been mutated
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
    /// `true` in production (phux-ia4). When `true` the tick is the live
    /// server->client emission path: per attached consumer it diffs the
    /// live `Terminal` against that consumer's own
    /// [`crate::grid::ConsumerReference`] (via the actor's shared
    /// [`SnapshotSynthesizer`]) and pushes only the delta with a
    /// per-consumer monotonic `seq`. The reference advances on emit
    /// (emit-once); the runtime suppresses its broadcast pump for any
    /// tick-managed consumer so exactly one emitter serves each consumer.
    ///
    /// Three prerequisites had to land before flipping this on; all are
    /// now met:
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
    /// Tests may set it `false` (e.g. to assert the gated-off path stays
    /// silent) via the test-only setters; production leaves it `true`.
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

/// Map a `portable_pty::ExitStatus` into the `TERMINAL_CLOSED.exit_status`
/// wire shape (phux-4li.11).
///
/// `Some(code)` for `_exit(n)`, `None` for signal-killed or
/// unknown-cause exits. `portable_pty::ExitStatus` keeps its
/// `signal: Option<String>` field private; the only way through the
/// public surface to distinguish a signal-driven death from `_exit(1)`
/// is the `Display` impl, which formats signal kills as
/// `"Terminated by <name>"` and exits as `"Exited with code N"` /
/// `"Success"`. Parsing the prefix is the stable contract; if upstream
/// ever exposes `signal()` we can swap this for a structured probe
/// without touching call sites.
fn exit_status_to_wire(status: &portable_pty::ExitStatus) -> Option<i32> {
    let rendered = status.to_string();
    if rendered.starts_with("Terminated by") {
        return None;
    }
    // Both "Success" (success() == true) and "Exited with code N" hit
    // this branch. `exit_code()` returns u32 — coerce into i32 saturating
    // at i32::MAX, since `TERMINAL_CLOSED.exit_status` is `Option<i32>`
    // on the wire and the practical exit-code range is 0..=255.
    Some(i32::try_from(status.exit_code()).unwrap_or(i32::MAX))
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
    cmd.env("TERM", DEFAULT_TERM);
    cmd
}

/// The baseline `TERM` baked into [`default_shell_command`] and
/// [`shell_command`].
///
/// Matches `phux_config`'s `defaults.term` schema default. The runtime
/// overrides this per-server with the configured `defaults.term` via
/// [`apply_term`]; this constant is the value used when a `CommandBuilder`
/// is built without server config in scope (tests,
/// [`TerminalActor::new_with_default_shell`]).
///
/// `xterm-256color` is the universally-recognised safe baseline (phux-7vx
/// / phux-ign): 256 colours and the standard xterm key vocabulary, no
/// kitty-keyboard advertisement — so ncurses TUIs like htop keep working.
pub const DEFAULT_TERM: &str = "xterm-256color";

/// Override the `TERM` env on `cmd` with `term`, the server's configured
/// `defaults.term`.
///
/// `CommandBuilder::env` overwrites, so this cleanly replaces the baseline
/// set by [`default_shell_command`] / [`shell_command`]. Callers in the
/// runtime apply this after building the command from the wire/config so a
/// single server-wide `TERM` default flows to the seed session,
/// attach-time creation, and `SPAWN_TERMINAL`.
pub fn apply_term(cmd: &mut CommandBuilder, term: &str) {
    cmd.env("TERM", term);
}

/// Build a [`CommandBuilder`] that runs a user-supplied command line as a
/// seed pane's initial program (e.g. `defaults.spawn-on-attach`,
/// phux-07y).
///
/// The command runs via `$SHELL -c <command>` (falling back to
/// `/bin/sh`), so shell quoting and arguments inside `command` behave the
/// same as they would at an interactive prompt, and the pane closes when
/// the command exits. `TERM` is set to match [`default_shell_command`].
#[must_use]
pub fn shell_command(command: &str) -> CommandBuilder {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_owned());
    let mut cmd = CommandBuilder::new(shell);
    cmd.arg("-c");
    cmd.arg(command);
    cmd.env("TERM", DEFAULT_TERM);
    cmd
}

impl TerminalActor {
    /// Build a fresh actor of the given dimensions **without** a backing
    /// PTY. Used by tests that exercise snapshot / shutdown semantics
    /// without driving a real process.
    ///
    /// The `Terminal` is allocated via libghostty's default allocator
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
        let terminal = Terminal::new(TerminalOptions {
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
            // phux-ia4: flipped ON. The state-sync tick is now the live
            // server->consumer emitter. All three prerequisites are met:
            // (1) broadcast suppression per tick-managed consumer landed in
            // phux-3uv; (2) the client FRAME_ACK loop landed in phux-3uv;
            // (3) per-consumer dirty isolation is solved here by the
            // per-consumer reference grid (`ConsumerReference` +
            // `SnapshotSynthesizer::synthesize_against_reference`), which
            // diffs each consumer against its own last-synced rows rather
            // than the shared `Terminal` dirty bits that
            // `RenderState::update` consumes on first read. See the field
            // doc.
            consumer_tick_emits: true,
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
            screen: screen_tx,
            pwd: pwd_tx,
            output: output_tx,
            resize: resize_tx,
            consumer_attach: consumer_attach_tx,
            consumer_detach: consumer_detach_tx,
            consumer_ack: consumer_ack_tx,
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
    ) -> Result<(), ConsumerAttachError> {
        let terminal = self.terminal.borrow();
        // Cursor + DEC mode capture happens against a one-shot
        // `RenderState` so we don't conflict with the shared
        // synthesizer's borrow used to prime the reference below.
        // Allocation cost is one libghostty handle; freed at end of scope.
        let last_cursor_mode = {
            let mut render_state = RenderState::new()?;
            let snapshot = render_state.update(&terminal)?;
            LastAckedCursorMode::capture(&terminal, &snapshot)
        };
        // Prime the per-consumer reference grid against the live terminal
        // so the next `synthesize_against_reference` emits only deltas
        // from *now* — the `TERMINAL_SNAPSHOT` the runtime emits right
        // after this call already brings the consumer's mirror to this
        // same point. Uses the actor's shared synthesizer (its
        // `RenderState`/iterators); the reference itself is per-consumer.
        let mut reference = ConsumerReference::new();
        self.synth
            .borrow_mut()
            .prime_reference(&terminal, &mut reference)?;
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
    /// (`consumer_tick_emits`). Production now defaults this ON (phux-ia4);
    /// this setter is retained for tests that toggle it back on after
    /// disabling it.
    #[cfg(test)]
    pub const fn enable_tick_emit_for_test(&mut self) {
        self.consumer_tick_emits = true;
    }

    /// Test-only: disable the per-consumer tick emission gate so the
    /// `tick_emit`-stays-silent path can be asserted. Production defaults
    /// it ON (phux-ia4).
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

    /// Synthesize a snapshot of the current `Terminal` state. Exposed
    /// for tests that want to drive the synthesis path synchronously
    /// without going through the actor's `select!` loop.
    fn synthesize(&self) -> Result<SnapshotBytes, crate::grid::SynthesisError> {
        let terminal = self.terminal.borrow();
        let mut synth = self.synth.borrow_mut();
        synth.synthesize(&terminal)
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
                let _ = self.output_tx.send(Bytes::from(snap.bytes));
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

                // PTY → Terminal + broadcast.
                evt = recv_or_pending(self.pty_rx.as_mut()) => {
                    match evt {
                        Some(PtyEvent::Bytes(chunk)) => {
                            // One event per PTY chunk drained into the canonical
                            // Terminal. Trace level: per-chunk volume is the raw
                            // input rate, useful for "what was the PTY doing
                            // right before a stall" but far too chatty for the
                            // default filter — off unless `phux=trace`.
                            trace!(bytes = chunk.len(), "vt_write: PTY chunk -> Terminal");
                            self.terminal.borrow_mut().vt_write(&chunk);
                            // The grid changed: let the next tick walk
                            // the rows (phux-4l0 idle short-circuit).
                            self.terminal_dirty_since_tick = true;
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
                            let exit_status = self.reap_child_if_any();
                            if let Some(tx) = self.exit_notify.take() {
                                let _ = tx.send(exit_status);
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
                        reply,
                    } = req;
                    // phux-3uv: map register success to an outcome that
                    // tells the runtime whether this actor is tick-managing
                    // the consumer. Tick-managed ⇒ the runtime suppresses
                    // its broadcast pump for this pane (single emitter).
                    let tick_managed = self.consumer_tick_emits;
                    let result = self
                        .register_consumer(client_id, outbound, wire_terminal_id)
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

                // State-sync tick driver (phux-q0e.3, phux-ia4, ADR-0018).
                // Iterates each attached consumer, diffs the live terminal
                // against that consumer's own reference grid, and pushes a
                // `TerminalOutput` frame onto its outbound mailbox whenever
                // `synthesize_against_reference` returns non-empty bytes.
                _ = tick.tick() => {
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

        // Emission gate (phux-0q8 / phux-3uv / phux-ia4). When ON, the
        // tick is the single server->consumer emitter and the runtime
        // suppresses its broadcast pump per tick-managed consumer (see
        // `ConsumerAttachOutcome`). When OFF, the broadcast pump in
        // `runtime.rs` is the live emitter and the tick stays silent so the
        // consumer does not double-paint. Either way the per-consumer
        // reference is maintained by `register_consumer` (prime) and the
        // tick itself (advance-on-emit); the gate only controls *emission*.
        if !self.consumer_tick_emits {
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
                bytes,
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

    /// phux-q0e.4: `on_frame_ack` advances `last_acked_seq` monotonically
    /// for in-order acks. Three acks (1, 2, 3) walk the field forward.
    #[test]
    fn on_frame_ack_advances_last_acked_seq_in_order() {
        let bundle = TerminalActor::new_with_seed(20, 5, b"hello").expect("new_with_seed");
        let mut actor = bundle.actor;
        let client = ClientId(1);
        let (tx, _rx) = dummy_outbound();
        actor.register_consumer(client, tx, 11).expect("register");

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
        actor.register_consumer(client, tx, 11).expect("register");

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
        actor.register_consumer(client, tx, 11).expect("register");
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
    /// off. (Production defaults the gate ON since phux-ia4; this test
    /// disables it explicitly.)
    #[test]
    fn tick_emit_is_silent_while_gate_is_off() {
        let bundle = TerminalActor::new(20, 5).expect("new");
        let mut actor = bundle.actor;
        actor.disable_tick_emit_for_test();
        let client = ClientId(1);
        let (tx, mut rx) = dummy_outbound();
        actor.register_consumer(client, tx, 11).expect("register");
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

    /// phux-0q8 / phux-q0e.3 / phux-3uv / phux-ia4: with the gate ON (its
    /// production default) for a SINGLE consumer, `tick_emit` diffs the
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
        // Gate is ON by default (phux-ia4); be explicit for clarity.
        actor.enable_tick_emit_for_test();
        let client = ClientId(1);
        let (tx, mut rx) = dummy_outbound();
        // Register against the (blank) terminal: the reference is primed
        // so deltas are measured "from now." Writing AFTER register is what
        // makes the next tick produce a diff.
        actor.register_consumer(client, tx, 11).expect("register");
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
            .register_consumer(client_a, tx_a, 11)
            .expect("register a");
        actor
            .register_consumer(client_b, tx_b, 11)
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
        actor.register_consumer(client, tx, 11).expect("register");

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
        actor.register_consumer(client_a, tx_a, 11).expect("reg a");
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
        actor.register_consumer(client_b, tx_b, 11).expect("reg b");
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
        actor.register_consumer(client, tx, 11).expect("register");
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
        actor.register_consumer(client, tx, 11).expect("register");

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
                // broadcast lands.
                let mut acc: Vec<u8> = Vec::new();
                for _ in 0..32 {
                    match tokio::time::timeout(std::time::Duration::from_millis(100), out.recv())
                        .await
                    {
                        Ok(Ok(bytes)) => {
                            acc.extend_from_slice(&bytes);
                            if contains_subslice(&acc, b"\x1b[!p")
                                && contains_subslice(&acc, b"phux8v1-marker")
                            {
                                break;
                            }
                        }
                        Ok(Err(_)) => break, // channel closed
                        Err(_) => tokio::task::yield_now().await, // timeout tick
                    }
                }

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

                // Count snapshot-bearing broadcasts. Debounced => exactly 1.
                let mut snapshots = 0usize;
                loop {
                    match out.try_recv() {
                        Ok(bytes) => {
                            if contains_subslice(&bytes, b"\x1b[!p") {
                                snapshots += 1;
                            }
                        }
                        // Lagged: skip and keep draining (no-op falls
                        // through to the loop's next iteration).
                        Err(tokio::sync::broadcast::error::TryRecvError::Lagged(_)) => {}
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
    /// size. (Run as a plain `Terminal` test so a regression aborts THIS
    /// test, not a flaky e2e teardown.)
    #[test]
    fn resize_desync_then_both_shrink_does_not_overflow() {
        let mut term = Terminal::new(TerminalOptions {
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
        actor.register_consumer(client, tx, 11).expect("register");

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
        actor.register_consumer(client, tx, 11).expect("register");

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
            adaptive_tick_interval(std::time::Duration::from_millis(2000)),
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
            .register_consumer(slow, tx_slow, 11)
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
            .register_consumer(fast, tx_fast, 12)
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
        actor.register_consumer(client, tx, 11).expect("register");

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
}
