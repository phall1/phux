//! Submodule for terminal actor internals.

use crate::grid::SnapshotBytes;
use crate::state::{Outbound, TerminalInput};
use bytes::Bytes;
use phux_protocol::ClientId;
use phux_protocol::wire::frame::TerminalEventType;
use tokio::sync::{broadcast, mpsc, oneshot};

/// Request to register a new consumer with the actor.
///
/// Drives the ADR-0018 per-consumer state lifecycle. The caller is the
/// runtime's ATTACH path, which has just installed the client in
/// `ServerState`.
///
/// The actor allocates a fresh `RenderState`, primes it against the
/// live `Terminal` (so the next incremental synthesis emits only
/// deltas *from now*), captures the cursor + mode state, and stores
/// the resulting [`super::ConsumerSyncState`] keyed by `client_id`. The reply
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
    /// per-consumer [`super::ConsumerSyncState`] and uses it on every tick
    /// (phux-q0e.3) to push a `TerminalOutput` frame carrying the
    /// incremental synthesis bytes.
    pub outbound: mpsc::Sender<Outbound>,
    /// Wire-level terminal id (`u32`). The actor stamps it on every
    /// emitted `TerminalOutput` frame. The runtime owns the mapping
    /// from the actor's [`phux_core::ids::TerminalId`] to this wire id and
    /// passes the resolved value here at ATTACH time.
    pub wire_terminal_id: u32,
    /// Whether this consumer negotiated the synthesized state-sync tick
    /// emitter (`OutputMode::StateSync`) at HELLO time (phux-fseo). When
    /// `true` the actor's `tick_emit` serves this consumer and the runtime
    /// suppresses its broadcast pump for it; when `false` the consumer
    /// stays on the raw PTY broadcast (the human-TUI default).
    pub wants_state_sync: bool,
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
    /// Identifier whose [`super::ConsumerSyncState`] entry to remove.
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
    /// Identifier whose [`super::ConsumerSyncState`]'s dirty cache to evict.
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

/// Request for the pane's current `vt_replay_bytes` snapshot.
///
/// Sent by the ATTACH handler on the per-client task; the actor walks
/// its `Terminal` via [`crate::grid::SnapshotSynthesizer`] and replies on the
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

/// A resize request delivered to a [`super::TerminalActor`] over its `resize`
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

/// Cross-task handle to a [`super::TerminalActor`].
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
    /// Subscribe to semantic terminal events for this pane. The runtime sends
    /// a [`SubscribeToEventsRequest`] when a client subscribes; the actor
    /// registers the subscriber and begins broadcasting matching events.
    pub subscribe_to_events: mpsc::Sender<SubscribeToEventsRequest>,
    /// Unsubscribe from semantic terminal events. The runtime sends an
    /// [`UnsubscribeFromEventsRequest`] when a client detaches; the actor
    /// removes the subscriber from its list (idempotent).
    pub unsubscribe_from_events: mpsc::Sender<UnsubscribeFromEventsRequest>,
    /// Pane viewport width in cells at construction time.
    pub cols: u16,
    /// Pane viewport height in cells at construction time.
    pub rows: u16,
}

/// A client subscribed to semantic events for a single Terminal (pane).
/// Holds the client's outbound mailbox and event type filter.
#[derive(Clone, Debug)]
pub struct TerminalEventSubscriber {
    /// Client's outbound frame channel (where Event frames are sent).
    pub outbound: tokio::sync::mpsc::Sender<Outbound>,
    /// Event type filter (empty = all types). Only events matching a type
    /// in this list are forwarded; if empty, all events are sent.
    pub event_types: Vec<TerminalEventType>,
}

/// Request to subscribe to semantic terminal events.
#[derive(Debug)]
pub struct SubscribeToEventsRequest {
    /// The new subscriber to register.
    pub subscriber: TerminalEventSubscriber,
    /// Wire-level terminal id for Event frames (SPEC §7.1).
    /// The runtime passes this when registering.
    pub wire_terminal_id: u32,
}

/// Request to unsubscribe from semantic terminal events.
#[derive(Debug)]
pub struct UnsubscribeFromEventsRequest {
    /// Pointer to the subscriber's outbound mailbox (used for identification).
    pub outbound_ptr: *const tokio::sync::mpsc::Sender<Outbound>,
}
