//! Frame header and `FrameKind` enum.
//!
//! See `docs/spec/proto.md` §5 (framing) and §7 (message catalog).
//!
//! Wire layout (per `docs/spec/proto.md` §5):
//!
//! ```text
//! +-------------------------+
//! | length: u32 big-endian  |   number of bytes that follow
//! +-------------------------+
//! | type:   u8              |   message discriminant from §7
//! +-------------------------+
//! | payload: length-1 bytes |
//! +-------------------------+
//! ```
//!
//! `length` is at least `1` (the type byte) and at most `MAX_FRAME_LEN`.
//!
//! Under [ADR-0013] terminal content rides as raw VT bytes (`TERMINAL_OUTPUT`).
//! There is no structured per-cell diff variant on this enum — earlier
//! drafts carried `PaneDiff` at type byte `0x40`; that slot is retired
//! and `TERMINAL_OUTPUT` (type `0x90` per SPEC §7.2) takes its place.
//!
//! [ADR-0013]: https://github.com/phall1/phux/blob/main/ADR/0013-libghostty-bytes-on-wire.md

use bytes::BytesMut;

use crate::caps::{ClientCapabilities, ServerCapabilities};
use crate::ids::{
    ClientId, CollectionId, SatelliteHost, SessionId, TERMINAL_ID_TAG_LOCAL,
    TERMINAL_ID_TAG_SATELLITE, TerminalId,
};
use crate::input::InputEvent;
use crate::input::focus::FocusEvent;
use crate::input::key::KeyEvent;
use crate::input::mouse::MouseEvent;
use crate::input::paste::PasteEvent;
use crate::input::selection::{SelectionEvent, SelectionMode};

use super::decode::Decoder;
use super::encode::Encoder;
use super::error::DecodeError;
use super::info::{
    SessionSnapshot, decode_session_snapshot, encode_client_id, encode_session_snapshot,
};

/// Maximum permitted value of the wire-frame `length` field, per `docs/spec/proto.md` §5
/// ("at most `16_777_216` (16 MiB)").
pub const MAX_FRAME_LEN: u32 = 16 * 1024 * 1024;

// -----------------------------------------------------------------------------
// Message discriminants from SPEC §7. Only the variants implemented in this
// scaffold are exposed via `FrameKind`; the remaining IDs are recorded here so
// sibling tasks can wire them up without re-deriving the catalog.
// -----------------------------------------------------------------------------

/// Discriminant for `HELLO` (client to server, `docs/spec/proto.md` §6.1).
pub const TYPE_HELLO: u8 = 0x01;
/// Discriminant for `ATTACH` (client to server, `docs/spec/proto.md` §7.1 / §13).
pub const TYPE_ATTACH: u8 = 0x02;
/// Discriminant for `DETACH` (client to server, `docs/spec/proto.md` §7.1 / §7.3).
pub const TYPE_DETACH: u8 = 0x03;
/// Discriminant for `INPUT_KEY` (client to server, `docs/spec/input.md` §2).
pub const TYPE_INPUT_KEY: u8 = 0x10;
/// Discriminant for `INPUT_PASTE` (client to server, `docs/spec/input.md` §5).
pub const TYPE_INPUT_PASTE: u8 = 0x11;
/// Discriminant for `INPUT_MOUSE` (client to server, `docs/spec/input.md` §3).
pub const TYPE_INPUT_MOUSE: u8 = 0x12;
/// Discriminant for `INPUT_FOCUS` (client to server, `docs/spec/input.md` §4).
pub const TYPE_INPUT_FOCUS: u8 = 0x14;
/// Discriminant for `INPUT_SELECTION` (client to server, `docs/spec/input.md` §6).
///
/// Carries selection mode and rectangular-mode flag. Selection is client-owned
/// state (server does not interpret or act on selections). Per ADR-0025, the
/// server receives `SelectionEvent` frames, updates per-Terminal selection state
/// (start, end, mode), and emits no output to the PTY — selection is a pure
/// client UI concern. Extraction (plaintext copy via libghostty `format_selection_alloc`)
/// is requested via `COMMAND` with `RouteInput`(`InputEvent::Selection`) payload.
pub const TYPE_INPUT_SELECTION: u8 = 0x15;
/// Discriminant for `FRAME_ACK` (client to server, `docs/spec/proto.md` §7.2 / §8.2).
///
/// Per-Terminal cumulative acknowledgement of `TERMINAL_OUTPUT` (§8.1) frames
/// the client has applied to its local `libghostty_vt::Terminal`. The server
/// uses these acks to evict per-consumer cached reference state under ADR-0018
/// lazy state synchronization — calling `mark_synced` on the per-consumer
/// `SnapshotSynthesizer` so the next tick re-diffs against the just-acked
/// reference rather than the prior one.
///
/// Loss tolerance falls out: a dropped ack leaves the dirty bits set on the
/// per-consumer `RenderState`, so the next tick re-emits a larger diff against
/// the same older reference. No retransmit machinery; the ack is a hint, not a
/// guarantee.
pub const TYPE_FRAME_ACK: u8 = 0x21;
/// Discriminant for `VIEWPORT_RESIZE` (client to server, `docs/spec/proto.md` §7.1 / §10.5).
///
/// The client emits this when its outer terminal changes size (SIGWINCH on
/// Unix, the GUI resize event on graphical hosts). Payload reuses the
/// [`ViewportInfo`] shape carried by `ATTACH` (§13) — phux-4hp keeps the wire
/// shape minimal and lets future tickets grow the per-cell pixel + padding
/// metrics from SPEC §10.5 when the mouse-encoder needs them.
pub const TYPE_VIEWPORT_RESIZE: u8 = 0x20;
/// Discriminant for `PING` (client to server, `docs/spec/proto.md` §7.4).
pub const TYPE_PING: u8 = 0x7F;
/// Discriminant for `HELLO_OK` (server to client, `docs/spec/proto.md` §6.1).
pub const TYPE_HELLO_OK: u8 = 0x80;
/// Discriminant for `ATTACHED` (server to client, `docs/spec/L1.md` §1 / §13).
pub const TYPE_ATTACHED: u8 = 0x81;
/// Discriminant for `DETACHED` (server to client, `docs/spec/L1.md` §1 / §7.3).
pub const TYPE_DETACHED: u8 = 0x82;
/// Discriminant for `BELL` (server to client, `docs/spec/L1.md` §1.2).
pub const TYPE_BELL: u8 = 0xB0;
/// Discriminant for `ERROR` (server to client, `docs/spec/proto.md` §9).
///
/// Carries a structured [`ErrorCode`] plus a human-readable UTF-8 message
/// and an optional `request_id` correlating the error with a prior
/// `COMMAND` (per SPEC §14). Fatal errors MUST be followed by `DETACHED
/// { reason: PROTOCOL_ERROR }` and transport close.
pub const TYPE_ERROR: u8 = 0xC1;
/// Discriminant for `PONG` (server to client, `docs/spec/proto.md` §7.4).
pub const TYPE_PONG: u8 = 0xFF;
/// Discriminant for `TERMINAL_OUTPUT` (server to client, `docs/spec/L1.md` §1 / §8.1).
///
/// Hot-path terminal content under [ADR-0013]: the server forwards PTY bytes
/// (possibly downsampled per the client's [`crate::caps::ColorSupport`])
/// in `TERMINAL_OUTPUT` frames. Supersedes the earlier `PANE_DIFF` slot;
/// `PANE_DIFF` is retired and its old discriminant (`0x40`) is no longer
/// recognised.
pub const TYPE_TERMINAL_OUTPUT: u8 = 0x90;
/// Discriminant for `TERMINAL_SNAPSHOT` (server to client, `docs/spec/L1.md` §1 / §8.4).
///
/// Required per SPEC §16 conformance. Under [ADR-0013] the payload is a
/// synthesised VT byte sequence (`vt_replay_bytes`) plus optional
/// `scrollback_bytes`; the client `vt_write`s them into a fresh Terminal
/// of the declared `cols × rows`.
pub const TYPE_TERMINAL_SNAPSHOT: u8 = 0x91;

// -----------------------------------------------------------------------------
// L3 metadata frame discriminants — SPEC §7.4 (phux-4li.2).
//
// Contiguous block 0x50..=0x54 for C→S commands; 0xD0 for the single S→C
// notification. Sits between the L1 hot-path C→S range (0x10..=0x21) and
// the proto SUBSCRIBE slot (0x40), leaving 0x55..=0x5F open for the L2
// command allocation that follows. The S→C side uses 0xD0..=0xDF as a
// matching unallocated block, with `BELL` (0xB0) / `ALERT` (0xB2) and
// `ERROR` (0xC1) already on lower discriminants.
// -----------------------------------------------------------------------------

/// Discriminant for `GET_METADATA` (client to server, `docs/spec/L3.md` §1 / §11.L3).
pub const TYPE_GET_METADATA: u8 = 0x50;
/// Discriminant for `SET_METADATA` (client to server, `docs/spec/L3.md` §1 / §11.L3).
pub const TYPE_SET_METADATA: u8 = 0x51;
/// Discriminant for `DELETE_METADATA` (client to server, `docs/spec/L3.md` §1 / §11.L3).
pub const TYPE_DELETE_METADATA: u8 = 0x52;
/// Discriminant for `LIST_METADATA` (client to server, `docs/spec/L3.md` §1 / §11.L3).
pub const TYPE_LIST_METADATA: u8 = 0x53;
/// Discriminant for `SUBSCRIBE_METADATA` (client to server, `docs/spec/L3.md` §1).
pub const TYPE_SUBSCRIBE_METADATA: u8 = 0x54;

/// Discriminant for `METADATA_CHANGED` (server to client, `docs/spec/L3.md` §1).
pub const TYPE_METADATA_CHANGED: u8 = 0xD0;

/// Discriminant for `METADATA_VALUE` (server to client, `docs/spec/L3.md` §1).
///
/// Reply frame for `GET_METADATA`; correlated by `request_id`. Carries
/// `Option<bytes>` — `Some(bytes)` when the key holds a value,
/// `None` when the key is absent. Allocated by phux-4li.8.
pub const TYPE_METADATA_VALUE: u8 = 0xD1;

/// Discriminant for `METADATA_KEYS` (server to client, `docs/spec/L3.md` §1).
///
/// Reply frame for `LIST_METADATA`; correlated by `request_id`. Carries
/// the lexicographically sorted list of key names present in the
/// requested scope (values are not included; LIST is by-key-name only).
/// Allocated by phux-4li.8.
pub const TYPE_METADATA_KEYS: u8 = 0xD2;

// -----------------------------------------------------------------------------
// L1 Terminal lifecycle frame discriminants — SPEC §7.2 / §10.1 (phux-4li.10).
//
// Allocates the SPAWN / CLOSED / RESIZE wire-frames needed to lift split-pane
// and kill-pane out of the `phux-4li.5` warn+bell stubs and to drive per-pane
// `ioctl(TIOCSWINSZ)` from the post-SIGWINCH ReflowDiff (phux-4li.9). The
// server-side handler + client-side emission land in follow-up tickets.
//
// C→S allocations slot into `0x22..=0x23`, the first free pair after
// VIEWPORT_RESIZE (`0x20`) / FRAME_ACK (`0x21`). The 0x14..=0x1F
// hot-path reservation in Appendix B is preserved by skipping past it.
// S→C allocations honour the spec-only reservations carried in SPEC §7.2
// (`0xA1 TERMINAL_CLOSED`) and extend by one (`0xA2 TERMINAL_SPAWNED`)
// for the dedicated SPAWN reply — see SPEC Appendix C for the
// 0.2.0-draft.2 entry.
// -----------------------------------------------------------------------------

/// Discriminant for `SPAWN_TERMINAL` (client to server, `docs/spec/L1.md` §1 / §10.1).
///
/// Carries `{ request_id, collection, command: Option<list<str>>,
/// cwd: Option<str>, env: Option<list<(str, str)>> }`. The reply rides on
/// [`TYPE_TERMINAL_SPAWNED`] correlated by `request_id`.
pub const TYPE_SPAWN_TERMINAL: u8 = 0x22;
/// Discriminant for `TERMINAL_RESIZE` (client to server, `docs/spec/L1.md` §1 / §10.2).
///
/// Per-Terminal PTY resize. Drives `ioctl(TIOCSWINSZ)` server-side; the
/// outer-viewport `VIEWPORT_RESIZE` (`0x20`) remains the
/// minimum-bounding-box signal. Both flow from a single SIGWINCH on the
/// client (phux-4li.9).
pub const TYPE_TERMINAL_RESIZE: u8 = 0x23;

/// Discriminant for `TERMINAL_CLOSED` (server to client, `docs/spec/L1.md` §1 / §10.1).
///
/// Push notification when a Terminal's PTY exits, naturally or via
/// `KILL_TERMINAL`. Honours the spec-only reservation at `0xA1`.
pub const TYPE_TERMINAL_CLOSED: u8 = 0xA1;
/// Discriminant for `TERMINAL_SPAWNED` (server to client, `docs/spec/L1.md` §1 / §10.1).
///
/// Reply frame for `SPAWN_TERMINAL`; correlated by `request_id`. Carries a
/// `Result<TerminalId, SpawnError>` tagged union — see [`SpawnResult`].
pub const TYPE_TERMINAL_SPAWNED: u8 = 0xA2;

// Wire tags for the `SpawnResult` tagged union (SPEC §7.2 / §10.1).
//
// Convention: `Ok = 0x00`, `Err = 0x01` — established here by phux-4li.10
// and reusable by future `Result<T, E>`-shaped reply frames (e.g. when
// `COMMAND_RESULT` lands per SPEC §11). The convention deliberately
// matches the `Option` tag convention (`None = 0x00`, `Some = 0x01`) so
// hex-dump readers do not have to remember a second per-shape table.
/// Wire tag for [`SpawnResult::Ok`].
pub(crate) const SPAWN_RESULT_OK: u8 = 0;
/// Wire tag for [`SpawnResult::Err`].
pub(crate) const SPAWN_RESULT_ERR: u8 = 1;

// Wire tags for the `SpawnError` tagged union (SPEC §7.2 / §10.1).
/// Wire tag for [`SpawnError::CollectionNotFound`].
pub(crate) const SPAWN_ERROR_TAG_COLLECTION_NOT_FOUND: u8 = 0;
/// Wire tag for [`SpawnError::SpawnFailed`].
pub(crate) const SPAWN_ERROR_TAG_SPAWN_FAILED: u8 = 1;

// Wire tags for the `Scope` tagged union (SPEC §7.4 / §11.L3).
/// Wire tag for [`Scope::Terminal`].
pub(crate) const SCOPE_TAG_TERMINAL: u8 = 0;
/// Wire tag for [`Scope::Collection`].
pub(crate) const SCOPE_TAG_COLLECTION: u8 = 1;
/// Wire tag for [`Scope::Global`].
pub(crate) const SCOPE_TAG_GLOBAL: u8 = 2;

// -----------------------------------------------------------------------------
// Control-plane frame discriminants — SPEC §5 (phux-k61 / ADR-0021).
//
// The generic command envelope. `COMMAND` (C→S) carries a typed `Command`
// correlated by `request_id`; `COMMAND_RESULT` (S→C) carries the matching
// `CommandResult`. Allocated from the control-plane ranges reserved in
// Appendix B (`0x31..=0x3F` C→S, `0xC2..=0xCF` S→C; `0xC1` is ERROR).
// ADR-0021 routes the CLI control verbs (`ls`, `kill`) through this rather
// than minting per-verb frames.
// -----------------------------------------------------------------------------

/// Discriminant for `COMMAND` (client to server, `docs/spec/L1.md` §5).
pub const TYPE_COMMAND: u8 = 0x31;
/// Discriminant for `COMMAND_RESULT` (server to client, `docs/spec/L1.md` §5).
pub const TYPE_COMMAND_RESULT: u8 = 0xC2;

// -----------------------------------------------------------------------------
// Agent-event frame discriminants — SPEC §7.5 (phux-y2t / ADR-0022 'events').
//
// The push half of the agent surface: a client SUBSCRIBES to a stream of
// extensible tagged lifecycle/activity events, and the server PUSHES `EVENT`
// frames as those events occur. This is an *additive accelerator* of the
// CLI-side poll-floor `wait` (which already shipped over `GET_SCREEN`):
// conditions stay matched client-side; events just cut polling latency.
//
// Allocated from the events reserved ranges in Appendix B: `0x41..=0x4F`
// (C→S) and `0xB3..=0xBF` (S→C). `SUBSCRIBE_EVENTS` takes the first C→S
// slot; `EVENT` takes the first S→C slot.
// -----------------------------------------------------------------------------

/// Discriminant for `SUBSCRIBE_EVENTS` (client to server, `docs/spec/L1.md` §7.5).
pub const TYPE_SUBSCRIBE_EVENTS: u8 = 0x41;
/// Discriminant for `EVENT` (server to client, `docs/spec/L1.md` §7.5).
pub const TYPE_EVENT: u8 = 0xB3;

// Wire tags for the `AgentEvent` tagged union (SPEC §7.5 / §10.3).
//
// Each event rides inside the `EVENT` frame as a `tag: u8` followed by a
// length-prefixed `body: bytes`. The length prefix is what makes the
// taxonomy forward-compatible: a decoder that does not recognise `tag`
// reads (and skips) the declared body length and yields
// [`AgentEvent::Unknown`], so a v0.2.x server may add event kinds without
// breaking an older client's frame parse. Tags are allocated sequentially.
/// Wire tag for [`AgentEvent::CommandStarted`].
pub(crate) const EVENT_TAG_COMMAND_STARTED: u8 = 0x00;
/// Wire tag for [`AgentEvent::CommandFinished`].
pub(crate) const EVENT_TAG_COMMAND_FINISHED: u8 = 0x01;
/// Wire tag for [`AgentEvent::TitleChanged`].
pub(crate) const EVENT_TAG_TITLE_CHANGED: u8 = 0x02;
/// Wire tag for [`AgentEvent::Bell`].
pub(crate) const EVENT_TAG_BELL: u8 = 0x03;
/// Wire tag for [`AgentEvent::PaneSpawned`].
pub(crate) const EVENT_TAG_PANE_SPAWNED: u8 = 0x04;
/// Wire tag for [`AgentEvent::PaneClosed`].
pub(crate) const EVENT_TAG_PANE_CLOSED: u8 = 0x05;
/// Wire tag for [`AgentEvent::Dirty`].
pub(crate) const EVENT_TAG_DIRTY: u8 = 0x06;
/// Wire tag for [`AgentEvent::Idle`].
pub(crate) const EVENT_TAG_IDLE: u8 = 0x07;

// Wire tags for the `Command` tagged union (SPEC §5.1). Tags follow the
// spec catalog order so the allocation is stable as later verbs land:
// SPAWN=0x00, ATTACH_TERMINAL=0x01, DETACH_TERMINAL=0x02, KILL_TERMINAL=0x03,
// RESIZE_TERMINAL=0x04, GET_STATE=0x05, RUN_HOOK=0x06. v0.1 implements only
// KILL_TERMINAL and GET_STATE (ADR-0021 §3); the rest are reserved and
// decode as `UnknownEnumValue` until wired.
/// Wire tag for [`Command::KillTerminal`].
pub(crate) const COMMAND_TAG_KILL_TERMINAL: u8 = 0x03;
/// Wire tag for [`Command::GetState`].
pub(crate) const COMMAND_TAG_GET_STATE: u8 = 0x05;
/// Wire tag for [`Command::GetScreen`]. Appended after `RUN_HOOK`'s
/// reserved `0x06` (SPEC §5.1 catalog order); `GET_SCREEN` is an additive
/// agent-surface command (ADR-0022 §5), not part of the original catalog.
pub(crate) const COMMAND_TAG_GET_SCREEN: u8 = 0x07;
/// Wire tag for [`Command::RouteInput`]. Appended after `GET_SCREEN`'s
/// `0x07`; `ROUTE_INPUT` is an additive agent-surface command (ADR-0022)
/// that delivers an already-built input event to a Terminal without an
/// attach, subscription, or resize — the write counterpart to the
/// side-effect-free `GET_SCREEN` read.
pub(crate) const COMMAND_TAG_ROUTE_INPUT: u8 = 0x08;
/// Wire tag for [`Command::CreateSession`]. Appended after `ROUTE_INPUT`'s
/// `0x08`; `CREATE_SESSION` is an additive control command (ADR-0021 §3,
/// `phux-fdh`) that creates a named session under a Collection *without*
/// attaching, subscribing, or resizing. The reply carries the seed pane's
/// [`TerminalId`] via `COMMAND_RESULT { Ok_With(TerminalId) }`,
/// asynchronously correlated by `request_id` — the same shape
/// `SPAWN_TERMINAL` uses, but session-level. Backs `phux new --json`.
pub(crate) const COMMAND_TAG_CREATE_SESSION: u8 = 0x09;
/// Wire tag for [`Command::KillCollection`]. Appended after
/// `CREATE_SESSION`'s `0x09`; `KILL_COLLECTION` is the additive teardown
/// counterpart to `CREATE_SESSION` (ADR-0021 §3, `phux-h9s`). It destroys
/// the named session under a Collection in *one* round-trip — replacing the
/// N `KILL_TERMINAL` round-trips `phux kill SESSION` issued before. The
/// reply rides `COMMAND_RESULT { Ok }`, the same ack shape `KILL_TERMINAL`
/// uses (the async `TERMINAL_CLOSED` frames confirm teardown), but
/// session-level. Backs `phux kill SESSION`.
pub(crate) const COMMAND_TAG_KILL_COLLECTION: u8 = 0x0a;
/// Wire tag for [`Command::RenameSession`]. Appended after
/// `KILL_COLLECTION`'s `0x0a`; `RENAME_SESSION` is the additive rename
/// counterpart to `CREATE_SESSION` (ADR-0021 §3). It resolves the named
/// session under a Collection and reassigns its human-readable name in
/// one round-trip. The reply rides `COMMAND_RESULT { Ok }`, the same ack
/// shape `KILL_COLLECTION` uses; the server is authoritative and the next
/// `ATTACHED` snapshot reconciles the new name to attached clients. An
/// unknown `name` is rejected with `SESSION_NOT_FOUND`; a `new_name`
/// already in use is rejected with `INVALID_COMMAND` (the same code
/// `CREATE_SESSION` uses for a taken name). Backs `phux rename SESSION
/// NEW-NAME` and the TUI `rename-session` action.
pub(crate) const COMMAND_TAG_RENAME_SESSION: u8 = 0x0b;
/// Wire tag for [`Command::GetTerminalState`]. Appended after
/// `RENAME_SESSION`'s `0x0b`; `GET_TERMINAL_STATE` is an additive
/// L2 Collection-aware query (ADR-0015 L2) that returns a comprehensive
/// snapshot of a Terminal's full state: grid, scrollback, cursor, shell
/// metadata, sequence number, and timestamp as a structured JSON
/// `phux_client::l2::state::TerminalState`. Unlike `GET_SCREEN` (L1 raw
/// grid), this returns structured state suitable for agent polling and
/// change detection. The reply rides `COMMAND_RESULT { Ok_With(Json(..)) }`.
pub(crate) const COMMAND_TAG_GET_TERMINAL_STATE: u8 = 0x0c;
pub(crate) const COMMAND_TAG_SUBSCRIBE_TERMINAL_EVENTS: u8 = 0x0d;

// Wire tags for the `InputEvent` tagged union (ROUTE_INPUT arg). These
// mirror the four `INPUT_*` frame atoms (`docs/spec/input.md`).
/// Wire tag for [`InputEvent::Key`].
pub(crate) const INPUT_EVENT_TAG_KEY: u8 = 0x00;
/// Wire tag for [`InputEvent::Mouse`].
pub(crate) const INPUT_EVENT_TAG_MOUSE: u8 = 0x01;
/// Wire tag for [`InputEvent::Focus`].
pub(crate) const INPUT_EVENT_TAG_FOCUS: u8 = 0x02;
/// Wire tag for [`InputEvent::Paste`].
pub(crate) const INPUT_EVENT_TAG_PASTE: u8 = 0x03;
/// Wire tag for [`InputEvent::Selection`].
pub(crate) const INPUT_EVENT_TAG_SELECTION: u8 = 0x04;

// Wire tags for the `StateScope` tagged union (SPEC §5.1, GET_STATE arg).
/// Wire tag for [`StateScope::Server`].
pub(crate) const STATE_SCOPE_TAG_SERVER: u8 = 0x00;

// Wire tags for the `CommandResult` tagged union (SPEC §5).
/// Wire tag for [`CommandResult::Ok`].
pub(crate) const COMMAND_RESULT_TAG_OK: u8 = 0x00;
/// Wire tag for [`CommandResult::OkWith`].
pub(crate) const COMMAND_RESULT_TAG_OK_WITH: u8 = 0x01;
/// Wire tag for [`CommandResult::Error`].
pub(crate) const COMMAND_RESULT_TAG_ERROR: u8 = 0x02;

// Wire tags for the `CommandValue` tagged union (SPEC §5).
/// Wire tag for [`CommandValue::TerminalId`].
pub(crate) const COMMAND_VALUE_TAG_TERMINAL_ID: u8 = 0x00;
/// Wire tag for [`CommandValue::CollectionId`].
pub(crate) const COMMAND_VALUE_TAG_COLLECTION_ID: u8 = 0x01;
/// Wire tag for [`CommandValue::State`].
pub(crate) const COMMAND_VALUE_TAG_STATE: u8 = 0x02;
/// Wire tag for [`CommandValue::Json`].
pub(crate) const COMMAND_VALUE_TAG_JSON: u8 = 0x03;
/// Wire tag for [`CommandValue::Bytes`].
pub(crate) const COMMAND_VALUE_TAG_BYTES: u8 = 0x04;

// -----------------------------------------------------------------------------
// ErrorCode enum — SPEC §14.
// -----------------------------------------------------------------------------

/// Structured error code carried by [`FrameKind::Error`], per SPEC §14.
///
/// Marked `#[non_exhaustive]` so future minor protocol versions can add
/// codes without breaking downstream matches (per the protocol/core
/// independence principle in ADR-0011). Unknown wire values surface as
/// [`DecodeError::UnknownEnumValue`] rather than being silently mapped to
/// a placeholder variant — misinterpreting an error code can mask the
/// underlying failure.
///
/// The numeric values are the wire encoding: `u16` big-endian. The space
/// is intentionally sparse (handshake errors clustered at `1..=9`,
/// attach/session at `100..=199`, command errors at `200..=299`, internal
/// at `u16::MAX`) so future codes can slot in without renumbering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
#[repr(u16)]
pub enum ErrorCode {
    /// SPEC §6.1: HELLO version negotiation found no compatible version.
    VersionIncompatible = 1,
    /// SPEC §6: the peer sent a type byte the receiver does not recognise.
    UnknownMessageType = 2,
    /// SPEC §5 / Appendix A: a message could not be decoded (truncated,
    /// bad enum, invalid UTF-8, ...).
    MalformedMessage = 3,
    /// SPEC §5: a frame's declared length exceeded the protocol cap.
    FrameTooLarge = 4,

    /// SPEC §13: the client issued an operation that requires an attach
    /// while not attached.
    NotAttached = 100,
    /// SPEC §13: the client requested attach while already attached.
    AlreadyAttached = 101,
    /// SPEC §13: the requested session does not exist.
    SessionNotFound = 102,
    /// The requested window does not exist.
    WindowNotFound = 103,
    /// The requested terminal does not exist.
    TerminalNotFound = 104,
    /// The requested client id does not exist.
    ClientNotFound = 105,
    /// SPEC §10.1 / ADR-0016: the frame carried a `TerminalId::Satellite`
    /// but this server is not configured as a federation hub. v0.1 servers
    /// always respond with this code when handed a `Satellite` id.
    UnsupportedSatelliteRoute = 106,

    /// SPEC §11: the requested COMMAND payload was structurally invalid.
    InvalidCommand = 200,
    /// SPEC §15: the requested operation is forbidden for this peer.
    PermissionDenied = 201,
    /// The server has run out of a resource needed to satisfy the request
    /// (file descriptors, memory, PTYs, ...).
    ResourceExhausted = 202,

    /// Catch-all for unexpected server-side failures. Carries
    /// `u16::MAX = 65535` on the wire.
    InternalError = 65535,
}

impl ErrorCode {
    /// Wire encoding of this code: the `#[repr(u16)]` discriminant.
    #[must_use]
    pub const fn as_wire(self) -> u16 {
        self as u16
    }

    /// Inverse of [`Self::as_wire`]; returns `None` for values that do not
    /// correspond to any code in this protocol version.
    #[must_use]
    pub const fn from_wire(value: u16) -> Option<Self> {
        Some(match value {
            1 => Self::VersionIncompatible,
            2 => Self::UnknownMessageType,
            3 => Self::MalformedMessage,
            4 => Self::FrameTooLarge,
            100 => Self::NotAttached,
            101 => Self::AlreadyAttached,
            102 => Self::SessionNotFound,
            103 => Self::WindowNotFound,
            104 => Self::TerminalNotFound,
            105 => Self::ClientNotFound,
            106 => Self::UnsupportedSatelliteRoute,
            200 => Self::InvalidCommand,
            201 => Self::PermissionDenied,
            202 => Self::ResourceExhausted,
            65535 => Self::InternalError,
            _ => return None,
        })
    }
}

// -----------------------------------------------------------------------------
// AttachTarget tagged union — SPEC §13.
// -----------------------------------------------------------------------------

/// Wire tag for [`AttachTarget::Last`].
pub(crate) const ATTACH_TARGET_LAST: u8 = 0;
/// Wire tag for [`AttachTarget::ByName`].
pub(crate) const ATTACH_TARGET_BY_NAME: u8 = 1;
/// Wire tag for [`AttachTarget::ById`].
pub(crate) const ATTACH_TARGET_BY_ID: u8 = 2;
/// Wire tag for [`AttachTarget::CreateIfMissing`].
pub(crate) const ATTACH_TARGET_CREATE_IF_MISSING: u8 = 3;

/// Session the client wishes to attach to, per SPEC §13.
///
/// Tagged union; each variant maps to one of SPEC's four selection modes.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum AttachTarget {
    /// Most-recently-attached session known to the server. Implementations
    /// without prior-attach memory MAY return `SESSION_NOT_FOUND`.
    Last,
    /// Look up a session by its human-readable name.
    ByName(String),
    /// Look up a session by its server-assigned [`SessionId`].
    ById(SessionId),
    /// Look up a session by name; create one if no such session exists.
    CreateIfMissing {
        /// Name for the new session (also used to match an existing one).
        name: String,
        /// Initial command to run in the seed pane, if creation occurs.
        command: Option<Vec<String>>,
        /// Working directory for the seed pane, if creation occurs.
        cwd: Option<String>,
    },
}

/// Viewport metrics the client advertises at attach time.
///
/// SPEC §13: `{ cols, rows, pixel_w: optional<u16>, pixel_h: optional<u16> }`.
/// Pixel dimensions support sub-cell rendering and image protocols; cells are
/// the load-bearing axis.
///
/// `#[non_exhaustive]`; construct via [`Self::new`] plus `with_pixels`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct ViewportInfo {
    /// Viewport width in cells.
    pub cols: u16,
    /// Viewport height in cells.
    pub rows: u16,
    /// Optional viewport width in pixels.
    pub pixel_w: Option<u16>,
    /// Optional viewport height in pixels.
    pub pixel_h: Option<u16>,
}

impl ViewportInfo {
    /// Construct a `ViewportInfo` from cell dimensions, the load-bearing
    /// axis per SPEC §13. Pixel dimensions default to `None`; supply them
    /// via [`Self::with_pixels`] when the host kernel reports them.
    #[must_use]
    pub const fn new(cols: u16, rows: u16) -> Self {
        Self {
            cols,
            rows,
            pixel_w: None,
            pixel_h: None,
        }
    }

    /// Builder setter for the optional pixel dimensions (`pixel_w`,
    /// `pixel_h`). Pass `None` for either axis the kernel did not report.
    #[must_use]
    pub const fn with_pixels(mut self, pixel_w: Option<u16>, pixel_h: Option<u16>) -> Self {
        self.pixel_w = pixel_w;
        self.pixel_h = pixel_h;
        self
    }
}

// -----------------------------------------------------------------------------
// Scope — SPEC §7.4 / §11.L3 (phux-4li.2). The "where does this key live?"
// tagged union shared by every L3 metadata frame.
// -----------------------------------------------------------------------------

/// Scope of an L3 metadata key (SPEC §7.4 / §11.L3).
///
/// Tagged union:
/// - `Terminal { terminal_id }` — keys scoped to a single Terminal. Killed
///   with the Terminal.
/// - `Collection { collection_id }` — keys scoped to an L2 Collection.
///   L2 is not yet wire-allocated; until it ships, v0.1 servers expose a
///   single default Collection that satisfies the reference TUI's
///   `phux.tui.layout/v1` use case (see ADR-0019).
/// - `Global` — keys scoped to the server (e.g. cross-Collection prefs).
///
/// Wire encoding: 1-byte tag + per-variant body.
/// - tag `0x00` → `Terminal`, body = tagged `TerminalId`.
/// - tag `0x01` → `Collection`, body = `u32` (the inner `CollectionId`).
/// - tag `0x02` → `Global`, body = empty.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Scope {
    /// Keys scoped to a single Terminal. Cleared when the Terminal closes.
    Terminal(TerminalId),
    /// Keys scoped to an L2 Collection.
    Collection(CollectionId),
    /// Server-wide keys.
    Global,
}

// -----------------------------------------------------------------------------
// SpawnError / SpawnResult — SPEC §7.2 / §10.1 (phux-4li.10).
//
// `SpawnResult` is the `Result<TerminalId, SpawnError>` carried inside
// `TERMINAL_SPAWNED`. Modelled as a dedicated tagged union (rather than
// reusing the Rust `Result` type directly on the wire) so the codec
// stays in lockstep with the SPEC text and so future error variants can
// land without touching call sites that match on the type.
//
// Both `SpawnResult` and `SpawnError` are `#[non_exhaustive]`: forward-
// compatible additions are protocol-minor changes, mirroring the
// existing [`ErrorCode`] / [`AttachTarget`] / [`Scope`] precedent.
//
// Wire encoding:
//   SpawnResult tag 0x00 Ok  → tagged TerminalId
//   SpawnResult tag 0x01 Err → SpawnError
//   SpawnError  tag 0x00 CollectionNotFound → no body
//   SpawnError  tag 0x01 SpawnFailed        → length-prefixed UTF-8 str
// -----------------------------------------------------------------------------

/// Error variants for [`FrameKind::TerminalSpawned`], SPEC §7.2 / §10.1.
///
/// `#[non_exhaustive]` so a v0.2.x server may add codes (e.g.
/// `PermissionDenied`, `ResourceExhausted`) without breaking downstream
/// matches. Unknown wire tags surface as
/// [`DecodeError::UnknownEnumValue`] rather than coercing to a
/// placeholder.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum SpawnError {
    /// The `collection` named in [`FrameKind::SpawnTerminal`] does not
    /// exist on this server. v0.1 servers expose a single default
    /// Collection at `CollectionId(1)` (SPEC §7.4 L2-dependency note);
    /// any other id MAY surface this error.
    CollectionNotFound,
    /// Spawning the underlying PTY failed for an implementation-specific
    /// reason. The carried string is a human-readable diagnostic — short
    /// enough to log inline; the SPEC does not constrain its contents
    /// beyond UTF-8.
    SpawnFailed(String),
}

/// Tagged union carried by [`FrameKind::TerminalSpawned`], SPEC §7.2 / §10.1.
///
/// Either the server-allocated [`TerminalId`] of the freshly spawned
/// Terminal, or a structured [`SpawnError`]. Modelled as a dedicated
/// enum rather than the Rust `core::result::Result` directly so the
/// codec mirrors the SPEC's tagged-union vocabulary and so the
/// `#[non_exhaustive]` contract carries through unchanged.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum SpawnResult {
    /// The freshly spawned Terminal's identifier.
    Ok(TerminalId),
    /// Structured failure; see [`SpawnError`].
    Err(SpawnError),
}

// -----------------------------------------------------------------------------
// Control-plane command types — SPEC §5 (phux-k61 / ADR-0021).
// -----------------------------------------------------------------------------

/// Semantic event type discriminant for filtering in `SubscribeTerminalEvents`.
/// Enables clients to subscribe only to event classes they care about
/// (e.g., command lifecycle without grid chatter).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TerminalEventType {
    /// Shell state transition (awaiting input → running → idle).
    ShellStateChanged = 0,
    /// Command started (OSC-133 B marker or equivalent).
    CommandStarted = 1,
    /// Command exited with exit code (OSC-133 D marker).
    CommandEnded = 2,
    /// Output arrived on terminal (PTY bytes detected).
    OutputReceived = 3,
    /// Shell prompt ready for input (no output + OSC-133 C or heuristic).
    PromptReady = 4,
    /// Grid mutated (scroll, output, cursor, clear).
    GridChanged = 5,
    /// Working directory changed.
    CwdChanged = 6,
}

impl TerminalEventType {
    /// Convert to wire byte representation.
    #[must_use]
    pub const fn to_u8(self) -> u8 {
        self as u8
    }

    /// Convert from wire byte representation.
    #[must_use]
    pub const fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::ShellStateChanged),
            1 => Some(Self::CommandStarted),
            2 => Some(Self::CommandEnded),
            3 => Some(Self::OutputReceived),
            4 => Some(Self::PromptReady),
            5 => Some(Self::GridChanged),
            6 => Some(Self::CwdChanged),
            _ => None,
        }
    }
}

/// Scope argument for [`Command::GetState`] (SPEC §5.1).
///
/// `#[non_exhaustive]`: v0.1 exposes only `Server` (the whole-server
/// snapshot, which is what `phux ls` and client-side selector resolution
/// need). Narrower scopes (a single Collection, a single Terminal) are
/// additive minor changes when L2 lands — see [ADR-0021](../../../ADR/0021-control-plane-commands.md).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum StateScope {
    /// Snapshot the entire server (every Terminal the caller may see).
    Server,
}

/// A typed control-plane command carried by [`FrameKind::Command`] (SPEC §5.1).
///
/// `#[non_exhaustive]`: the spec catalog has seven L1 commands; v0.1 wires
/// the ones the CLI needs — `KILL_TERMINAL`, `GET_STATE`, the
/// side-effect-free `GET_SCREEN` (ADR-0021 §3, ADR-0022 §5), the appended
/// `ROUTE_INPUT` write counterpart, the appended `CREATE_SESSION`
/// create-without-attach command (`phux-fdh`), its teardown counterpart
/// `KILL_COLLECTION` (`phux-h9s`), and the rename counterpart
/// `RENAME_SESSION`. Unknown wire tags surface as
/// [`DecodeError::UnknownEnumValue`] rather than coercing to a placeholder.
///
/// Only `PartialEq` (not `Eq`): `RouteInput` carries a [`MouseEvent`] whose
/// coordinates are not `Eq`.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum Command {
    /// Terminate the underlying PTY of `terminal_id`. Asynchronously emits
    /// `TERMINAL_CLOSED`. Backs `phux kill` (one command per resolved
    /// Terminal — see ADR-0021).
    KillTerminal {
        /// The Terminal to terminate.
        terminal_id: TerminalId,
    },
    /// Request a snapshot of server state in `scope`. The reply rides on
    /// `COMMAND_RESULT { Ok_With(State(..)) }`. Backs `phux ls` and the
    /// CLI's client-side selector resolution.
    GetState {
        /// What to snapshot.
        scope: StateScope,
    },
    /// Read `terminal_id`'s current screen as structured data, with no
    /// side effects — the server walks its own `Terminal` grid, so unlike
    /// `ATTACH` this neither resizes the pane nor disturbs the live
    /// session (ADR-0022 §5, `phux-oki`). The reply rides on
    /// `COMMAND_RESULT { Ok_With(Json(..)) }` carrying a serialized
    /// `phux_core::ScreenState`. Backs `phux snapshot` and the poll floor
    /// under `phux wait`/`run`.
    GetScreen {
        /// The Terminal whose screen to project.
        terminal_id: TerminalId,
        /// Requested scrollback history (`phux-o1v`):
        /// - `None` — viewport only (the original v0.2.0-draft.6 shape).
        /// - `Some(0)` — all retained history rows (bare `--scrollback`).
        /// - `Some(n)` — the most-recent `n` history rows.
        ///
        /// Encoded as a trailing presence-byte + `u32` so a decoder reading
        /// the original `GET_SCREEN` body (which ended after `terminal_id`)
        /// would see the `0` presence byte: the field is wire-additive.
        request_scrollback: Option<u32>,
        /// When `true`, the reply's `ScreenState` carries the additive
        /// `cells[]` field: per-cell OSC-133 semantic marks + styles
        /// (`phux-8yl`). Encoded as a trailing `bool` byte *after*
        /// `request_scrollback`; a decoder reading a pre-`phux-8yl` body
        /// (which ended after `request_scrollback`) finds no byte and
        /// defaults it to `false`, so the field is wire-additive.
        cells: bool,
    },
    /// Deliver an already-built input `event` to `terminal_id` without an
    /// attach, subscription, or resize. The write counterpart to the
    /// side-effect-free `GetScreen` read: the server feeds the event
    /// straight into the pane's input pipeline, so unlike `ATTACH` this
    /// never disturbs the live session's dimensions (ADR-0022, `phux-3j3`).
    /// The reply rides `COMMAND_RESULT { Ok }` (or an `Error` if the
    /// Terminal is unknown). Backs `phux send-keys`/`run`.
    RouteInput {
        /// The Terminal to deliver the input to.
        terminal_id: TerminalId,
        /// The structured input event (key/mouse/focus/paste).
        event: InputEvent,
    },
    /// Create a named session under `collection` *without* attaching,
    /// subscribing, or resizing — the create-only counterpart to
    /// `ATTACH { CreateIfMissing }`, which always attaches. The server
    /// allocates the session and its seed pane atomically, so two racing
    /// `CREATE_SESSION` callers cannot collide (closing the `GET_STATE`→`ATTACH`
    /// TOCTOU window v0.1's client-side always-new logic carried). The
    /// reply rides `COMMAND_RESULT { Ok_With(TerminalId) }` carrying the
    /// seed pane's id (or an `Error` if `name` is already taken or
    /// `collection` is unknown). Backs `phux new --json` (ADR-0021 §3,
    /// `phux-fdh`).
    CreateSession {
        /// Collection to host the new session under. v0.1 servers accept
        /// only the default `CollectionId(1)`.
        collection: CollectionId,
        /// Name for the new session. A name already in use is rejected
        /// (`Error`) rather than silently reused — `CREATE_SESSION` is
        /// create-only, never create-or-attach.
        name: String,
        /// Initial command to run in the seed pane. `None` falls back to
        /// the server's default shell, mirroring `CreateIfMissing`.
        command: Option<Vec<String>>,
        /// Working directory for the seed pane, if any.
        cwd: Option<String>,
    },
    /// Destroy the session named `name` under `collection`, tearing down
    /// every Terminal it owns in one round-trip — the teardown counterpart
    /// to `CreateSession`. The server resolves `name` to its session,
    /// cancels each pane's actor (the same path a `KILL_TERMINAL` per pane
    /// would take), and replies `COMMAND_RESULT { Ok }` immediately; the
    /// per-pane `TERMINAL_CLOSED` frames follow asynchronously as the panes
    /// reap. An unknown `collection` or an unknown `name` is rejected with
    /// `Error`. Backs `phux kill SESSION`, collapsing its prior N
    /// `KILL_TERMINAL` round-trips into one (ADR-0021 §3, `phux-h9s`).
    KillCollection {
        /// Collection hosting the session. v0.1 servers accept only the
        /// default `CollectionId(1)`.
        collection: CollectionId,
        /// Name of the session to destroy. An unknown name is rejected
        /// (`Error`) rather than silently treated as success.
        name: String,
    },
    /// Rename the session named `name` under `collection` to `new_name`,
    /// reassigning its human-readable name in one round-trip — the rename
    /// counterpart to `CreateSession`. The server resolves `name` to its
    /// session (the same resolution `KillCollection` uses), rejects an
    /// unknown `name` with `SESSION_NOT_FOUND` and a `new_name` already in
    /// use with `INVALID_COMMAND` (the same rule `CreateSession` applies to
    /// a taken name), then reassigns the name and replies
    /// `COMMAND_RESULT { Ok }`. The server is authoritative: each attached
    /// client reconciles the new name on its next `ATTACHED` snapshot. A
    /// live `SESSION_RENAMED` push to other clients is out of scope for this
    /// pass. Backs `phux rename SESSION NEW-NAME` and the TUI
    /// `rename-session` action (ADR-0021 §3).
    RenameSession {
        /// Collection hosting the session. v0.1 servers accept only the
        /// default `CollectionId(1)`.
        collection: CollectionId,
        /// Name of the session to rename. An unknown name is rejected
        /// (`SESSION_NOT_FOUND`).
        name: String,
        /// New name for the session. A name already in use is rejected
        /// (`INVALID_COMMAND`) rather than silently merging sessions.
        new_name: String,
    },
    /// Request a comprehensive snapshot of a terminal's full state: grid,
    /// scrollback, shell metadata, cursor, and sequence number (L2 Collection-aware
    /// agent interface). The reply rides `COMMAND_RESULT { Ok_With(Json(..)) }`
    /// carrying a serialized `phux_client::l2::state::TerminalState`. Backs
    /// agent polling and state inspection (ADR-0015 L2, `phux-y2t`).
    GetTerminalState {
        /// The Terminal whose state to snapshot.
        terminal_id: TerminalId,
        /// Whether to include scrollback lines above the viewport.
        /// When `false`, only the viewport is returned.
        include_scrollback: bool,
        /// Maximum number of scrollback lines to return. Ignored if
        /// `include_scrollback` is `false`.
        max_scrollback_lines: u16,
    },
    /// Subscribe to semantic terminal events for a specific pane without
    /// attaching or resizing. The server pushes typed events (`CommandStarted`,
    /// `CommandEnded`, `GridChanged`, `CwdChanged`, `PromptReady`, `OutputReceived`)
    /// as the pane's state changes. Scoped to the Terminal: only events for
    /// that pane flow to the subscriber. Idempotent: re-subscribing updates
    /// the `event_types` filter (empty = all types). Unsubscription is implicit
    /// on detach. Reply: `COMMAND_RESULT { Ok }`; events flow asynchronously as
    /// `Event` frames (SPEC §7.1). Backs agent-protocol `SubscribeTerminalEvents`.
    SubscribeTerminalEvents {
        /// The Terminal (pane) whose events the client subscribes to.
        terminal_id: TerminalId,
        /// Event type filter: which semantic events to forward.
        /// Empty vector = all event types.
        event_types: Vec<TerminalEventType>,
    },
}

/// A successful command's payload (SPEC §5, `CommandValue`).
///
/// `#[non_exhaustive]` for forward-compatible additions.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum CommandValue {
    /// A Terminal identifier (e.g. the result of a spawn).
    TerminalId(TerminalId),
    /// A Collection identifier (L2).
    CollectionId(CollectionId),
    /// A server-state snapshot (reply to `GET_STATE`). Reuses the
    /// `ATTACHED` snapshot shape — see the wire-bytes note in SPEC §7.
    State(SessionSnapshot),
    /// A structured JSON return, for commands whose result is open-shaped.
    Json(String),
    /// Opaque bytes (e.g. an L3 metadata value).
    Bytes(Vec<u8>),
}

/// The outcome of a [`Command`], carried by [`FrameKind::CommandResult`]
/// (SPEC §5).
///
/// `#[non_exhaustive]` for forward-compatible additions.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum CommandResult {
    /// The command succeeded and returned no value.
    Ok,
    /// The command succeeded and returned a [`CommandValue`].
    OkWith(CommandValue),
    /// The command failed; carries a structured [`ErrorCode`] and a
    /// human-readable UTF-8 diagnostic.
    Error {
        /// Structured failure code.
        code: ErrorCode,
        /// Human-readable diagnostic (UTF-8; unconstrained otherwise).
        message: String,
    },
}

/// A server-pushed agent event carried by [`FrameKind::Event`] (SPEC §7.5 /
/// §10.3, phux-y2t).
///
/// The push half of the agent surface: an extensible taxonomy of terminal
/// lifecycle / activity events the server emits to clients that opted in via
/// [`FrameKind::SubscribeEvents`]. This is an *additive accelerator* of the
/// CLI-side poll-floor `wait` (which already shipped over `GET_SCREEN`) —
/// conditions stay matched client-side, events just cut polling latency.
///
/// # Forward compatibility
///
/// `#[non_exhaustive]`, and the wire encoding is TLV: each event is a `tag:
/// u8` followed by a length-prefixed `body: bytes`. A decoder that does not
/// recognise `tag` reads the declared body length and yields
/// [`AgentEvent::Unknown`] rather than failing the whole frame parse — so a
/// v0.2.x server may add event kinds and an older client skips them
/// cleanly. [`AgentEvent::Unknown`] is *only ever produced by the decoder*;
/// encoders never emit it (encoding it is a no-op-shaped contradiction and
/// is rejected at the match arm).
///
/// Only `PartialEq` / `Eq`: every variant body is a primitive or a `String`.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum AgentEvent {
    /// A shell command began executing in the scoped Terminal. Sourced
    /// from OSC-133 `B`/`C` prompt marks (the shell-integration
    /// command-start boundary). Carries no payload — the command text is
    /// not extracted server-side.
    CommandStarted,
    /// A shell command finished in the scoped Terminal. Sourced from the
    /// OSC-133 `D` prompt mark. `exit_code` is `Some(n)` when the shell's
    /// integration reported one (`OSC 133 ; D ; n ST`) and `None`
    /// otherwise — see the wire-spec note on the exit-code gap.
    CommandFinished {
        /// Process exit code reported by the shell's OSC-133 `D` mark, or
        /// `None` when the shell did not include one.
        exit_code: Option<i32>,
    },
    /// The scoped Terminal's title changed (OSC 0 / OSC 2). Carries the
    /// new title as libghostty tracks it.
    TitleChanged {
        /// The new terminal title.
        title: String,
    },
    /// The scoped Terminal received a BEL (`0x07`). The control-plane
    /// counterpart to the `BELL` frame (`0xB0`), delivered on the event
    /// stream so a subscriber need not also attach.
    Bell,
    /// A new Terminal (pane) was spawned. The carried `terminal_id` is on
    /// the [`FrameKind::Event`] envelope's `terminal_id` field; this
    /// variant body is empty (the id is the scope).
    PaneSpawned,
    /// A Terminal (pane) closed. Mirrors the L1 `TERMINAL_CLOSED` frame
    /// (`0xA1`); the closed Terminal is the envelope's `terminal_id` and
    /// `exit_status` carries the process exit code (or `None` for signal /
    /// unknown), matching `TERMINAL_CLOSED.exit_status`.
    PaneClosed {
        /// Process exit code (`_exit(n)`), or `None` for signals / unknown.
        exit_status: Option<i32>,
    },
    /// The scoped Terminal's grid mutated since the last `Idle` (output
    /// arrived). Sourced from the per-pane tick's dirty flag; coalesced —
    /// the server emits at most one `Dirty` per active burst, then one
    /// [`AgentEvent::Idle`] when the burst settles.
    Dirty,
    /// The scoped Terminal went quiet: no grid mutation across an idle
    /// window after a `Dirty`. The "output has settled" signal a `wait`
    /// consumer keys on.
    Idle,
    /// An event whose `tag` this protocol version does not recognise.
    ///
    /// Produced **only by the decoder** when it reads an `EVENT` frame
    /// whose event tag is outside the known set; the length-prefixed body
    /// is preserved verbatim so a curious consumer can inspect it, but the
    /// common path simply ignores unknown events. Never constructed by an
    /// encoder.
    Unknown {
        /// The unrecognised event tag.
        tag: u8,
        /// The event's opaque body bytes, preserved verbatim.
        body: Vec<u8>,
    },
}

/// Decoded wire frame.
///
/// The phux-6yl.4 scaffold populated `Hello`, `Ping`, and `PaneDiff`. The
/// phux-4az pass added the message-catalog variants needed for the attach
/// lifecycle. The phux-i58 SPEC §13 conformance pass conforms ATTACH/ATTACHED
/// to spec and splits out `TERMINAL_SNAPSHOT` per SPEC §16. Under [ADR-0013] the
/// structured `PaneDiff` variant is replaced by `TerminalOutput` (raw VT bytes)
/// and `TerminalSnapshot` carries `vt_replay_bytes` instead of a `DiffOp` list.
/// The remaining SPEC §7 catalog (`TerminalEvent`, `Alert`,
/// resize/ack/command/etc.) lands in sibling tasks.
///
/// [ADR-0013]: https://github.com/phall1/phux/blob/main/ADR/0013-libghostty-bytes-on-wire.md
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum FrameKind {
    /// `HELLO` — client to server handshake (`docs/spec/proto.md` §6.1).
    ///
    /// Carries the client's free-form identifier, the highest protocol
    /// version triple it supports, and a [`ClientCapabilities`] envelope
    /// (SPEC §6.2). The `client_caps` field is appended to the v0.1 body;
    /// per Appendix A field-tag extensibility, a HELLO without it MUST
    /// still decode — older encoders that emit only the version triple
    /// stay forward-compatible. Decoders that see no trailing bytes
    /// substitute [`ClientCapabilities::default`] (most-permissive
    /// [`crate::caps::ColorSupport::TrueColor`]).
    ///
    /// Sibling tickets grow `ClientCapabilities` with the rest of SPEC §6.2
    /// (mouse protocols, unicode version, deprecated rendering mode).
    /// Additional capability fields append using the same trailing-byte
    /// forward-compat trick.
    Hello {
        /// Free-form client identifier (e.g. `"phux-client 0.1.0"`).
        client_name: String,
        /// Highest protocol major version the client supports.
        protocol_major: u16,
        /// Highest protocol minor version the client supports.
        protocol_minor: u16,
        /// Highest protocol patch version the client supports.
        protocol_patch: u16,
        /// Client capability advertisement (SPEC §6.2). Drives server-side
        /// VT byte-stream downsampling via [`crate::caps::ColorSupport`].
        client_caps: ClientCapabilities,
    },
    /// `HELLO_OK` — server handshake acknowledgement (`docs/spec/proto.md` §6.1).
    ///
    /// Carries the version the server selected, the [`ServerCapabilities`]
    /// it implements, and opaque `server_id` identity bytes. The version
    /// triple mirrors [`FrameKind::Hello`]'s positional `major/minor/patch`
    /// (the wire carries a single concrete version, not the spec's abstract
    /// `VersionRange` list — the server echoes the one it chose). Capability
    /// and identity fields are trailing and length-skippable, so a decoder
    /// reading a shorter (older) body falls back to defaults, and future
    /// server-owned fields append without a wire break (SPEC §6 "skip them
    /// by length").
    ///
    /// Version *negotiation* (rejecting an incompatible client with
    /// `ERROR { VERSION_INCOMPATIBLE }`) is not yet enforced by the
    /// reference server; the single-version protocol echoes its own
    /// `PROTOCOL_VERSION` as the selected version.
    HelloOk {
        /// Selected major version (wire-breaking axis pre-1.0).
        protocol_major: u16,
        /// Selected minor version.
        protocol_minor: u16,
        /// Selected patch version.
        protocol_patch: u16,
        /// The conformance tiers the server mounts; intersect with the
        /// client's `layers` for the negotiated tier set.
        server_caps: ServerCapabilities,
        /// Opaque server identity bytes (SPEC §6.1). Not interpreted by
        /// the client today; reserved for reconnect / multi-server routing.
        server_id: Vec<u8>,
    },

    /// `PING` — liveness probe (`docs/spec/proto.md` §7.4). The peer MUST echo `nonce`
    /// back in a `PONG` frame.
    Ping {
        /// Opaque nonce echoed by the peer in `PONG`.
        nonce: u64,
    },
    /// `PONG` — liveness response (`docs/spec/proto.md` §7.4). Echoes the
    /// nonce from a prior [`FrameKind::Ping`].
    Pong {
        /// Nonce echoed from the corresponding `PING`.
        nonce: u64,
    },

    /// `TERMINAL_OUTPUT` — server-to-client terminal content (`docs/spec/L1.md` §2.1).
    ///
    /// The hot path under [ADR-0013]: the server forwards bytes from the
    /// terminal's PTY (after parsing into its canonical
    /// `libghostty_vt::Terminal` and after any per-client capability
    /// rewriting). The client feeds `bytes` into its local Terminal via
    /// `vt_write`; `RenderState` provides per-row dirty tracking for
    /// efficient local redraw.
    ///
    /// `seq` is a monotonic per-terminal sequence id used by `FRAME_ACK` /
    /// predictive-echo correlation; it carries no structural meaning.
    ///
    /// [ADR-0013]: https://github.com/phall1/phux/blob/main/ADR/0013-libghostty-bytes-on-wire.md
    TerminalOutput {
        /// Target terminal.
        terminal_id: TerminalId,
        /// Monotonic per-terminal sequence id (`docs/spec/proto.md` §8).
        seq: u64,
        /// VT bytes from the PTY (possibly downsampled per
        /// [`crate::caps::ColorSupport`]).
        bytes: Vec<u8>,
    },

    /// `ATTACH` — client requests to attach to a session (`docs/spec/L1.md` §7).
    ///
    /// Conforms to SPEC §13 as of phux-i58: `target` tagged union plus
    /// viewport metrics plus scrollback negotiation.
    Attach {
        /// Which session to attach to. Tagged union with four variants.
        target: AttachTarget,
        /// Client viewport dimensions at attach time.
        viewport: ViewportInfo,
        /// Whether the client wants the server to send scrollback as part of
        /// the attach sequence.
        request_scrollback: bool,
        /// Upper bound on scrollback lines the client will accept.
        ///
        /// The server caps its own retention at `min(server_cap, this)`.
        scrollback_limit_lines: u32,
    },

    /// `DETACH` — client signals clean departure (`docs/spec/proto.md` §7.2).
    ///
    /// Carries no fields in the phux-4az scaffold; SPEC §7.3 also keeps it
    /// empty (the `DetachReason` is sent in `DETACHED` from the server).
    Detach,

    /// `INPUT_KEY` — client forwards a structured key event (`docs/spec/input.md` §2).
    ///
    /// Wire shape: tagged [`TerminalId`] followed by the encoded [`KeyEvent`].
    InputKey {
        /// Target terminal.
        terminal_id: TerminalId,
        /// Structured key event; libghostty atoms inside.
        event: KeyEvent,
    },

    /// `INPUT_MOUSE` — client forwards a mouse event (`docs/spec/input.md` §3).
    InputMouse {
        /// Target terminal.
        terminal_id: TerminalId,
        /// Structured mouse event; coordinates are terminal-local pixels.
        event: MouseEvent,
    },

    /// `INPUT_FOCUS` — client reports focus change on its host window
    /// (`docs/spec/input.md` §4).
    InputFocus {
        /// Target terminal.
        terminal_id: TerminalId,
        /// Whether the client window gained or lost focus.
        event: FocusEvent,
    },

    /// `INPUT_PASTE` — client forwards a paste payload (`docs/spec/input.md` §5).
    InputPaste {
        /// Target terminal.
        terminal_id: TerminalId,
        /// Paste payload plus trust classification.
        event: PasteEvent,
    },

    /// `INPUT_SELECTION` — client reports a selection mode change (`docs/spec/input.md` §6).
    ///
    /// Selection is client-owned state: the server receives this frame,
    /// updates per-terminal selection state, and emits no output to the PTY.
    /// Extraction (plaintext copy) is requested separately via COMMAND.
    /// See ADR-0025 (rectangular selection rationale).
    InputSelection {
        /// Target terminal.
        terminal_id: TerminalId,
        /// Selection mode (off, char, line, rect) and rectangular-mode flag.
        event: SelectionEvent,
    },

    /// `FRAME_ACK` — client acknowledges a `TERMINAL_OUTPUT` it has applied
    /// (`docs/spec/proto.md` §7.2 / §8.2).
    ///
    /// Cumulative ack: acknowledging `seq = N` implies all prior emissions
    /// for `terminal_id` up to and including `N` have been applied to the
    /// client's local `libghostty_vt::Terminal`.
    ///
    /// Under ADR-0018 the server uses this to drive per-consumer cached
    /// reference state eviction — the per-consumer `SnapshotSynthesizer`'s
    /// `mark_synced` clears the dirty bits that produced the acked frame.
    /// Loss tolerance: a dropped ack just means the next tick re-emits a
    /// larger diff against the same older reference; no retransmit.
    FrameAck {
        /// Acked terminal (wire id, per SPEC §13).
        terminal_id: TerminalId,
        /// Cumulative ack sequence — the highest `seq` from
        /// `TERMINAL_OUTPUT` for this `terminal_id` the client has applied.
        seq: u64,
    },

    /// `VIEWPORT_RESIZE` — the attached client's outer terminal changed
    /// size (`docs/spec/proto.md` §7.1 / §10.5).
    ///
    /// The connection itself identifies which client this resize belongs
    /// to — there is no `client_id` field on the wire (consistent with
    /// `ATTACH` / `INPUT_*` / etc., which also rely on the connection's
    /// implicit identity). The server uses this to update the resolved
    /// terminal dimensions for the client's currently-attached terminal.
    ///
    /// `viewport` reuses the [`ViewportInfo`] shape from `ATTACH`. SPEC
    /// §10.5 additionally defines `cell_w`/`cell_h`/`padding_*` for
    /// pixel-precise mouse encoding; those grow alongside the mouse
    /// encoder rework and don't gate the byc.4hp wiring.
    ViewportResize {
        /// New outer-terminal metrics.
        viewport: ViewportInfo,
    },

    /// `ATTACHED` — server acknowledges attach with initial state
    /// (`docs/spec/L1.md` §7).
    ///
    /// Conforms to SPEC §13 as of phux-i58: full `SessionSnapshot` plus the
    /// server-allocated `ClientId` identifying this attachment. The per-
    /// terminal initial state arrives separately via `TERMINAL_SNAPSHOT`
    /// frames per the SPEC §13 attach sequence.
    Attached {
        /// Full graph of sessions/windows/panes plus the attaching client's
        /// initial focus triple.
        snapshot: SessionSnapshot,
        /// Server-allocated client identifier for this attachment.
        initial_client_id: ClientId,
    },

    /// `DETACHED` — server confirms detach and closes the transport
    /// (`docs/spec/proto.md` §7.2).
    ///
    /// Phux-4az scaffold carries no fields. SPEC §7.3 defines
    /// `{ reason: DetachReason, message: str }`; those land in a follow-up
    /// once the server actually distinguishes shutdown causes.
    Detached,

    /// `TERMINAL_SNAPSHOT` — initial state of a single terminal (`docs/spec/L1.md` §2.4).
    ///
    /// REQUIRED per SPEC §16 conformance. Sent after `ATTACHED` for each
    /// terminal the client needs initialised; subsequent updates flow as
    /// `TERMINAL_OUTPUT`. The server MAY also emit `TERMINAL_SNAPSHOT`
    /// mid-stream as a flow-control catch-up (SPEC §12.2) or after a resize
    /// that requires full retransmission.
    ///
    /// Under [ADR-0013] the payload is a synthesised VT byte sequence:
    /// when written to a fresh `libghostty_vt::Terminal` of the declared
    /// `cols × rows`, `vt_replay_bytes` reproduces the server's grid state
    /// at the moment of snapshot emission. `scrollback_bytes` is present
    /// iff the attaching client requested scrollback in `ATTACH`.
    ///
    /// [ADR-0013]: https://github.com/phall1/phux/blob/main/ADR/0013-libghostty-bytes-on-wire.md
    TerminalSnapshot {
        /// Target terminal.
        terminal_id: TerminalId,
        /// Grid width in cells at snapshot time.
        cols: u16,
        /// Grid height in cells at snapshot time.
        rows: u16,
        /// Synthesised VT byte sequence that reproduces the grid when fed
        /// to a fresh `libghostty_vt::Terminal` of `cols × rows`. Opaque
        /// to the client beyond `vt_write`.
        vt_replay_bytes: Vec<u8>,
        /// Optional scrollback replay bytes. Present iff the client
        /// requested scrollback in `ATTACH` and the server can supply it.
        /// Applied before `vt_replay_bytes` (or under whatever construction
        /// the server chooses, per SPEC §8.4).
        scrollback_bytes: Option<Vec<u8>>,
    },

    /// `BELL` — terminal received a bell character (`docs/spec/L1.md` §1.2).
    Bell {
        /// Terminal that bell'd.
        terminal_id: TerminalId,
    },

    /// `ERROR` — server-to-client structured error (`docs/spec/proto.md` §9).
    ///
    /// Carries a numeric [`ErrorCode`] plus a human-readable UTF-8
    /// `message`. `request_id` is `Some(_)` when the error correlates with
    /// a prior `COMMAND` per SPEC §14, and `None` for spontaneous server
    /// errors (e.g. malformed `ATTACH`, fatal protocol violations).
    ///
    /// A fatal error MUST be followed by `DETACHED { reason:
    /// PROTOCOL_ERROR }` and transport close.
    Error {
        /// Correlates this error with a prior `COMMAND`'s `request_id`,
        /// if applicable. `None` for non-command-correlated errors.
        request_id: Option<u32>,
        /// Structured error code; see [`ErrorCode`].
        code: ErrorCode,
        /// Human-readable, UTF-8, free-form message. Implementations
        /// SHOULD keep this short enough to log inline.
        message: String,
    },

    // -------------------------------------------------------------------------
    // L3 metadata frames — SPEC §7.4 / §11.L3 (phux-4li.2). Reserved for
    // consumers that declare `Layer::L3` in `HELLO.client_caps.layers`; the
    // server MUST NOT emit `MetadataChanged` to a non-L3 consumer (SPEC
    // §16.4). The server's K/V store treats values as opaque bytes.
    //
    // Reply paths (GET → value, LIST → keys) are intentionally NOT yet
    // wire-encoded as dedicated frames in v0.1 of L3: SPEC §11 already
    // defines the generic `COMMAND` / `COMMAND_RESULT` envelope for that
    // pattern, and lighting up `COMMAND` is a sibling ticket. v0.1 servers
    // expose the GET / LIST functions as server-side Rust APIs (see
    // `phux_server::state::ServerState`); the wire reply path lands when
    // `COMMAND` does. `MetadataChanged` is independently load-bearing for
    // the ADR-0019 layout-coordination story and ships here.
    // -------------------------------------------------------------------------
    /// `GET_METADATA` — client requests the value at `(scope, key)`
    /// (`docs/spec/L3.md` §1 / §11.L3).
    ///
    /// The reply is currently a server-side function return; the wire
    /// reply path will ride the generic `COMMAND_RESULT` envelope when
    /// it lands. `request_id` is carried so the future reply correlates.
    GetMetadata {
        /// Correlates this request with the eventual `COMMAND_RESULT`.
        request_id: u32,
        /// Where to look the key up.
        scope: Scope,
        /// UTF-8 key name. Convention: `phux.<consumer>.<name>/<version>`
        /// per SPEC §17 (non-normative).
        key: String,
    },

    /// `SET_METADATA` — client writes `value` at `(scope, key)`
    /// (`docs/spec/L3.md` §1 / §11.L3).
    ///
    /// Atomic write: the server stores `value` and broadcasts
    /// `MetadataChanged { scope, key, value: Some(value) }` to every
    /// subscriber matching `(scope, key)`. Implementations MAY enforce a
    /// per-key size limit (recommended: 256 KiB) and reply with
    /// [`ErrorCode::ResourceExhausted`] if exceeded.
    SetMetadata {
        /// Correlates this request with the eventual `COMMAND_RESULT`.
        request_id: u32,
        /// Where to write the key.
        scope: Scope,
        /// UTF-8 key name.
        key: String,
        /// Opaque value bytes. The server MUST NOT interpret them.
        value: Vec<u8>,
    },

    /// `DELETE_METADATA` — client removes `key` from `scope`
    /// (`docs/spec/L3.md` §1 / §11.L3).
    ///
    /// Idempotent: deleting a missing key is not an error. The server
    /// broadcasts `MetadataChanged { scope, key, value: None }` (a
    /// tombstone) to subscribers iff the key existed before the call.
    DeleteMetadata {
        /// Correlates this request with the eventual `COMMAND_RESULT`.
        request_id: u32,
        /// Where to delete the key.
        scope: Scope,
        /// UTF-8 key name.
        key: String,
    },

    /// `LIST_METADATA` — client requests the set of key names in `scope`
    /// (`docs/spec/L3.md` §1 / §11.L3).
    ///
    /// Returns key names only — values are not part of the listing. As
    /// with `GET_METADATA`, the wire reply path is deferred to the
    /// `COMMAND_RESULT` envelope; v0.1 servers expose LIST as a Rust
    /// function return.
    ListMetadata {
        /// Correlates this request with the eventual `COMMAND_RESULT`.
        request_id: u32,
        /// Where to list keys from.
        scope: Scope,
    },

    /// `SUBSCRIBE_METADATA` — client opts into `MetadataChanged` events
    /// matching `(scope, key)` (`docs/spec/L3.md` §1).
    ///
    /// A single subscribe per `(scope, key)` is enough; the server keys
    /// subscribers by `(client, scope, key)` so re-subscribes are
    /// idempotent. Unsubscription is implicit on detach (see
    /// `phux_server::state::ServerState::detach`); a future
    /// `UNSUBSCRIBE_METADATA` ticket may add explicit teardown.
    SubscribeMetadata {
        /// Scope to watch.
        scope: Scope,
        /// Specific key to watch. The subscriber receives
        /// `MetadataChanged` iff the event's `(scope, key)` matches.
        key: String,
    },

    /// `METADATA_CHANGED` — server notifies a subscriber that
    /// `(scope, key)` was written or deleted (`docs/spec/L3.md` §1).
    ///
    /// `value` is `Some(new_bytes)` on a SET and `None` on a DELETE
    /// (the tombstone case). Subscribers MAY re-issue `GET_METADATA`
    /// after receiving the notification; the value is also carried
    /// inline for the common-case path where the subscriber just
    /// wants the new bytes (SPEC §7.4 leaves this latitude — "the
    /// value itself is not carried" was the v0.1 sketch; phux-4li.2
    /// lifts it because the layout coordination use case
    /// (ADR-0019) is a read-on-every-change pattern and the round
    /// trip is wasteful).
    MetadataChanged {
        /// Scope the change happened in.
        scope: Scope,
        /// Key that changed.
        key: String,
        /// New value, or `None` for a deletion (tombstone).
        value: Option<Vec<u8>>,
    },

    /// `METADATA_VALUE` — server reply to a prior `GET_METADATA`
    /// (`docs/spec/L3.md` §1 / §11.L3). Allocated by phux-4li.8.
    ///
    /// Correlated to the originating request by `request_id`. `value` is
    /// `Some(bytes)` when the key was present at the time of the lookup
    /// and `None` when the key was absent (no tombstone distinction —
    /// "absent" subsumes "never written" and "explicitly deleted").
    ///
    /// Design choice (phux-4li.8): a dedicated reply frame rather than
    /// the generic `COMMAND_RESULT` envelope sketched in SPEC §11. The
    /// envelope would have forced design closure on every L1/L2 COMMAND
    /// payload before any L3 consumer needs the reply path; for v0.1 the
    /// metadata family is already opinionated (`METADATA_CHANGED` carries
    /// value inline, departing from the §7.4 sketch) so an ad-hoc
    /// dedicated reply is consistent. The generic envelope ships when
    /// `COMMAND` does, and does not need to subsume `METADATA_VALUE`.
    MetadataValue {
        /// Correlates this reply with a prior `GET_METADATA.request_id`.
        request_id: u32,
        /// `Some(bytes)` when the key was present, `None` when absent.
        value: Option<Vec<u8>>,
    },

    /// `METADATA_KEYS` — server reply to a prior `LIST_METADATA`
    /// (`docs/spec/L3.md` §1 / §11.L3). Allocated by phux-4li.8.
    ///
    /// Correlated to the originating request by `request_id`. Carries
    /// the set of key names present in the requested scope. Server
    /// implementations SHOULD return keys in lexicographic order so
    /// snapshots and tests round-trip stably; clients MUST NOT rely on
    /// any particular ordering for correctness.
    MetadataKeys {
        /// Correlates this reply with a prior `LIST_METADATA.request_id`.
        request_id: u32,
        /// Keys present in the requested scope; values are NOT included
        /// (clients fetch them separately via `GET_METADATA`).
        keys: Vec<String>,
    },

    // -------------------------------------------------------------------------
    // L1 Terminal lifecycle frames — SPEC §7.2 / §10.1 (phux-4li.10).
    //
    // Unblocks split-pane / kill-pane (was warn+bell in phux-4li.5) and the
    // per-pane `ioctl(TIOCSWINSZ)` half of phux-4li.9's SIGWINCH wire-up.
    // The server-side handler + client-side emission land in follow-up
    // tickets; this enum allocation is the wire substrate they build on.
    // -------------------------------------------------------------------------
    /// `SPAWN_TERMINAL` — client requests a new Terminal under `collection`
    /// (`docs/spec/L1.md` §1 / §10.1).
    ///
    /// Async: the server replies with [`FrameKind::TerminalSpawned`]
    /// correlated by `request_id`. `command = None` means "use the server's
    /// default shell" (the same convention as
    /// `AttachTarget::CreateIfMissing.command = None`). `cwd = None` means
    /// "use the server's default working directory" — typically the user's
    /// `$HOME`; the exact policy is implementation-defined. `env = None`
    /// inherits the server's environment as-is; `env = Some([])` is
    /// distinct (start with an empty environment).
    ///
    /// v0.1 servers expose a single default Collection at
    /// `CollectionId(1)` (SPEC §7.4 L2-dependency note). Other collection
    /// ids MAY surface as [`SpawnError::CollectionNotFound`] inside the
    /// reply frame's [`SpawnResult::Err`] arm.
    SpawnTerminal {
        /// Correlates this request with the eventual `TerminalSpawned`.
        request_id: u32,
        /// Collection under which to spawn the new Terminal.
        collection: CollectionId,
        /// Command + argv, or `None` to invoke the server's default shell.
        command: Option<Vec<String>>,
        /// Working directory for the new Terminal, or `None` for the
        /// server's default.
        cwd: Option<String>,
        /// Environment variables for the new Terminal, or `None` to
        /// inherit the server's environment. `Some(vec![])` is distinct
        /// from `None`: it starts with an empty environment.
        env: Option<Vec<(String, String)>>,
    },

    /// `TERMINAL_SPAWNED` — server reply to a prior `SpawnTerminal`
    /// (`docs/spec/L1.md` §1 / §10.1).
    ///
    /// Correlated to the originating request by `request_id`. `result`
    /// carries either the freshly allocated [`TerminalId`] or a structured
    /// [`SpawnError`]. The structured error is deliberately separate from
    /// the generic [`FrameKind::Error`] catch-all so command-correlated
    /// failures stay typed end-to-end (matching the
    /// `METADATA_VALUE` precedent from phux-4li.8).
    TerminalSpawned {
        /// Correlates this reply with a prior `SpawnTerminal.request_id`.
        request_id: u32,
        /// Either the freshly allocated Terminal, or a structured error.
        result: SpawnResult,
    },

    /// `TERMINAL_CLOSED` — server notifies clients that a Terminal exited
    /// (`docs/spec/L1.md` §1 / §10.1).
    ///
    /// Emitted when the underlying PTY exits, whether by `_exit(n)`, by
    /// signal, or via a `KILL_TERMINAL` command. `exit_status = Some(n)`
    /// reports the process's exit code; `None` covers signal kills and
    /// unknown-cause exits (a deliberately compact subset of SPEC §10.1's
    /// `ExitStatus` tagged union — the wider tagged union grows in a
    /// follow-up wire bump if the additional structure proves
    /// load-bearing).
    TerminalClosed {
        /// The Terminal that exited.
        terminal_id: TerminalId,
        /// Process exit code (`_exit(n)`), or `None` for signals / unknown.
        exit_status: Option<i32>,
    },

    /// `TERMINAL_RESIZE` — client signals a per-Terminal PTY resize
    /// (`docs/spec/L1.md` §1 / §10.2).
    ///
    /// Sent in addition to (not in place of) `VIEWPORT_RESIZE`: the
    /// outer-viewport frame conveys the client's smallest-common-bounding-
    /// box; this frame conveys the resolved per-pane dimensions after the
    /// client's layout walk. The server's PTY layer drives
    /// `ioctl(TIOCSWINSZ)` from this. Implementations SHOULD treat `cols`
    /// or `rows` of zero as a no-op rather than a kernel error (the
    /// codec round-trips zero faithfully).
    TerminalResize {
        /// Target Terminal.
        terminal_id: TerminalId,
        /// New width in cells.
        cols: u16,
        /// New height in cells.
        rows: u16,
    },

    /// `COMMAND` — the generic control-plane request envelope
    /// (`docs/spec/L1.md` §5, ADR-0021).
    ///
    /// Carries a typed [`Command`] correlated to its eventual
    /// [`FrameKind::CommandResult`] by `request_id`. Asynchronous: the
    /// server MAY interleave other frames before the result (SPEC §5).
    Command {
        /// Correlates this request with the eventual `CommandResult`.
        request_id: u32,
        /// The command to execute.
        command: Command,
    },

    /// `COMMAND_RESULT` — reply to a prior [`FrameKind::Command`]
    /// (`docs/spec/L1.md` §5, ADR-0021).
    ///
    /// Correlated to the originating request by `request_id`.
    CommandResult {
        /// Correlates this reply with a prior `Command.request_id`.
        request_id: u32,
        /// The command's outcome.
        result: CommandResult,
    },

    // -------------------------------------------------------------------------
    // Agent-event frames — SPEC §7.5 / §10.3 (phux-y2t / ADR-0022 'events').
    // The push half of the agent surface; an additive accelerator of the
    // CLI poll-floor `wait`. `SUBSCRIBE_EVENTS` (C→S `0x41`) opts a client
    // into the stream; `EVENT` (S→C `0xB3`) carries each extensible tagged
    // event. The taxonomy is forward-compat (TLV body) — see [`AgentEvent`].
    // -------------------------------------------------------------------------
    /// `SUBSCRIBE_EVENTS` — client opts into the server-pushed
    /// [`AgentEvent`] stream (`docs/spec/L1.md` §7.5).
    ///
    /// `terminal` scopes the subscription:
    /// - `Some(id)` — only events for that Terminal (per-pane).
    /// - `None` — every event the server emits for any Terminal the
    ///   client may observe (server-scoped), e.g. `pane_spawned` /
    ///   `pane_closed` across the session.
    ///
    /// Idempotent: re-subscribing the same scope is a no-op. Unsubscription
    /// is implicit on detach (matching `SUBSCRIBE_METADATA`); a future
    /// `UNSUBSCRIBE_EVENTS` ticket may add explicit teardown. The
    /// subscription does NOT itself attach, resize, or send a snapshot —
    /// it is purely a push registration, so an agent can `watch` a Terminal
    /// without disturbing the live session.
    SubscribeEvents {
        /// Per-Terminal scope, or `None` for every Terminal the client may
        /// observe.
        terminal: Option<TerminalId>,
    },

    /// `EVENT` — server pushes one [`AgentEvent`] to a subscribed client
    /// (`docs/spec/L1.md` §7.5).
    ///
    /// `terminal` identifies the Terminal the event concerns, or `None`
    /// for a server-scoped event with no single owning Terminal. The
    /// `event` body is TLV-encoded (`tag: u8` + length-prefixed bytes) so
    /// an older client skips unrecognised event kinds via
    /// [`AgentEvent::Unknown`] rather than failing the parse.
    Event {
        /// The Terminal this event concerns, or `None` if server-scoped.
        terminal: Option<TerminalId>,
        /// The event payload.
        event: AgentEvent,
    },
}

impl FrameKind {
    /// Type discriminant from `docs/spec/proto.md` §7.
    #[must_use]
    pub const fn type_byte(&self) -> u8 {
        match self {
            Self::Hello { .. } => TYPE_HELLO,
            Self::HelloOk { .. } => TYPE_HELLO_OK,
            Self::Ping { .. } => TYPE_PING,
            Self::Pong { .. } => TYPE_PONG,
            Self::TerminalOutput { .. } => TYPE_TERMINAL_OUTPUT,
            Self::Attach { .. } => TYPE_ATTACH,
            Self::Detach => TYPE_DETACH,
            Self::InputKey { .. } => TYPE_INPUT_KEY,
            Self::InputMouse { .. } => TYPE_INPUT_MOUSE,
            Self::InputFocus { .. } => TYPE_INPUT_FOCUS,
            Self::InputPaste { .. } => TYPE_INPUT_PASTE,
            Self::InputSelection { .. } => TYPE_INPUT_SELECTION,
            Self::FrameAck { .. } => TYPE_FRAME_ACK,
            Self::ViewportResize { .. } => TYPE_VIEWPORT_RESIZE,
            Self::Attached { .. } => TYPE_ATTACHED,
            Self::Detached => TYPE_DETACHED,
            Self::TerminalSnapshot { .. } => TYPE_TERMINAL_SNAPSHOT,
            Self::Bell { .. } => TYPE_BELL,
            Self::Error { .. } => TYPE_ERROR,
            Self::GetMetadata { .. } => TYPE_GET_METADATA,
            Self::SetMetadata { .. } => TYPE_SET_METADATA,
            Self::DeleteMetadata { .. } => TYPE_DELETE_METADATA,
            Self::ListMetadata { .. } => TYPE_LIST_METADATA,
            Self::SubscribeMetadata { .. } => TYPE_SUBSCRIBE_METADATA,
            Self::MetadataChanged { .. } => TYPE_METADATA_CHANGED,
            Self::MetadataValue { .. } => TYPE_METADATA_VALUE,
            Self::MetadataKeys { .. } => TYPE_METADATA_KEYS,
            Self::SpawnTerminal { .. } => TYPE_SPAWN_TERMINAL,
            Self::TerminalSpawned { .. } => TYPE_TERMINAL_SPAWNED,
            Self::TerminalClosed { .. } => TYPE_TERMINAL_CLOSED,
            Self::TerminalResize { .. } => TYPE_TERMINAL_RESIZE,
            Self::Command { .. } => TYPE_COMMAND,
            Self::CommandResult { .. } => TYPE_COMMAND_RESULT,
            Self::SubscribeEvents { .. } => TYPE_SUBSCRIBE_EVENTS,
            Self::Event { .. } => TYPE_EVENT,
        }
    }

    /// Encode `self` as a complete length-prefixed frame.
    ///
    /// Writes the four-byte big-endian length header, the type byte, and the
    /// payload. The caller owns the `BytesMut` lifecycle.
    #[allow(
        clippy::too_many_lines,
        reason = "single match over the SPEC §7 catalog; splitting would scatter the encoder/decoder symmetry"
    )]
    pub fn encode(&self, out: &mut BytesMut) {
        // Reserve four bytes for the length header; backfill once we know how
        // many bytes the type + payload consumed.
        let header_pos = out.len();
        out.extend_from_slice(&[0u8; 4]);

        let body_start = out.len();
        let mut enc = Encoder::new(out);
        enc.write_u8(self.type_byte());

        match self {
            Self::Hello {
                client_name,
                protocol_major,
                protocol_minor,
                protocol_patch,
                client_caps,
            } => {
                enc.write_str(client_name);
                enc.write_u16_be(*protocol_major);
                enc.write_u16_be(*protocol_minor);
                enc.write_u16_be(*protocol_patch);
                // Trailing fields — older decoders skip them via the length
                // header per SPEC §6 ("skip them by length"). The encoder
                // ALWAYS emits all bytes; the wire shape grows monotonically.
                // Order: color_support (phux-7lf), layers (phux-4li.2),
                // image_protocols/kbd_protocols/hyperlinks (phux-4rj).
                enc.write_u8(client_caps.color_support.as_wire());
                enc.write_u8(client_caps.layers.as_wire());
                enc.write_u8(client_caps.image_protocols.as_wire());
                enc.write_u8(client_caps.kbd_protocols.as_wire());
                enc.write_u8(u8::from(client_caps.hyperlinks));
                // phux-fseo: consumer output-mode preference (raw vs
                // synthesized state-sync tick). Trailing/skippable like the
                // caps bytes above.
                enc.write_u8(client_caps.output_mode.as_wire());
            }
            Self::HelloOk {
                protocol_major,
                protocol_minor,
                protocol_patch,
                server_caps,
                server_id,
            } => {
                enc.write_u16_be(*protocol_major);
                enc.write_u16_be(*protocol_minor);
                enc.write_u16_be(*protocol_patch);
                // Trailing fields — older decoders skip them via the length
                // header per SPEC §6 ("skip them by length"). The encoder
                // ALWAYS emits all bytes; the wire shape grows monotonically.
                // Order: server_caps.layers, then length-prefixed server_id.
                enc.write_u8(server_caps.layers.as_wire());
                enc.write_bytes(server_id);
            }
            // `Ping` and `Pong` share a single-`u64` nonce body; merged to
            // satisfy `clippy::match_same_arms`.
            Self::Ping { nonce } | Self::Pong { nonce } => {
                enc.write_u64_be(*nonce);
            }
            Self::TerminalOutput {
                terminal_id,
                seq,
                bytes,
            } => {
                encode_terminal_id(terminal_id, &mut enc);
                enc.write_u64_be(*seq);
                enc.write_bytes(bytes);
            }
            Self::Attach {
                target,
                viewport,
                request_scrollback,
                scrollback_limit_lines,
            } => {
                encode_attach_target(target, &mut enc);
                encode_viewport_info(viewport, &mut enc);
                enc.write_u8(u8::from(*request_scrollback));
                enc.write_u32_be(*scrollback_limit_lines);
            }
            // `Detach` and `Detached` are unit variants: just the type byte,
            // no payload. Merged to satisfy `clippy::match_same_arms`.
            Self::Detach | Self::Detached => {}
            Self::InputKey { terminal_id, event } => {
                encode_terminal_id(terminal_id, &mut enc);
                encode_key_event(event, &mut enc);
            }
            Self::InputMouse { terminal_id, event } => {
                encode_terminal_id(terminal_id, &mut enc);
                encode_mouse_event(event, &mut enc);
            }
            Self::InputFocus { terminal_id, event } => {
                encode_terminal_id(terminal_id, &mut enc);
                enc.write_u8(encode_focus_event(*event));
            }
            Self::InputPaste { terminal_id, event } => {
                encode_terminal_id(terminal_id, &mut enc);
                encode_paste_event(event, &mut enc);
            }
            Self::InputSelection { terminal_id, event } => {
                encode_terminal_id(terminal_id, &mut enc);
                enc.write_u8(event.mode.as_u8());
                enc.write_u8(u8::from(event.rectangle));
            }
            Self::FrameAck { terminal_id, seq } => {
                encode_terminal_id(terminal_id, &mut enc);
                enc.write_u64_be(*seq);
            }
            Self::ViewportResize { viewport } => {
                encode_viewport_info(viewport, &mut enc);
            }
            Self::Attached {
                snapshot,
                initial_client_id,
            } => {
                encode_session_snapshot(snapshot, &mut enc);
                encode_client_id(*initial_client_id, &mut enc);
            }
            Self::TerminalSnapshot {
                terminal_id,
                cols,
                rows,
                vt_replay_bytes,
                scrollback_bytes,
            } => {
                encode_terminal_id(terminal_id, &mut enc);
                enc.write_u16_be(*cols);
                enc.write_u16_be(*rows);
                enc.write_bytes(vt_replay_bytes);
                encode_optional_bytes(scrollback_bytes.as_deref(), &mut enc);
            }
            Self::Bell { terminal_id } => {
                encode_terminal_id(terminal_id, &mut enc);
            }
            Self::Error {
                request_id,
                code,
                message,
            } => {
                encode_optional_u32(*request_id, &mut enc);
                enc.write_u16_be(code.as_wire());
                enc.write_str(message);
            }
            // GET / DELETE share `{request_id, scope, key}`; merged to
            // satisfy `clippy::match_same_arms`. The wire bodies are
            // intentionally identical — the discriminating type byte is
            // emitted before this match arm runs.
            Self::GetMetadata {
                request_id,
                scope,
                key,
            }
            | Self::DeleteMetadata {
                request_id,
                scope,
                key,
            } => {
                enc.write_u32_be(*request_id);
                encode_scope(scope, &mut enc);
                enc.write_str(key);
            }
            Self::SetMetadata {
                request_id,
                scope,
                key,
                value,
            } => {
                enc.write_u32_be(*request_id);
                encode_scope(scope, &mut enc);
                enc.write_str(key);
                enc.write_bytes(value);
            }
            Self::ListMetadata { request_id, scope } => {
                enc.write_u32_be(*request_id);
                encode_scope(scope, &mut enc);
            }
            Self::SubscribeMetadata { scope, key } => {
                encode_scope(scope, &mut enc);
                enc.write_str(key);
            }
            Self::MetadataChanged { scope, key, value } => {
                encode_scope(scope, &mut enc);
                enc.write_str(key);
                encode_optional_bytes(value.as_deref(), &mut enc);
            }
            Self::MetadataValue { request_id, value } => {
                enc.write_u32_be(*request_id);
                encode_optional_bytes(value.as_deref(), &mut enc);
            }
            Self::MetadataKeys { request_id, keys } => {
                enc.write_u32_be(*request_id);
                // Length-prefixed list of UTF-8 strings: u32 count + N strs.
                // Mirrors the encoder shape used by `encode_optional_string_list`
                // for the `Some` arm, minus the optional tag (the keys list
                // is always present even when empty).
                debug_assert!(
                    u32::try_from(keys.len()).is_ok(),
                    "metadata keys list length exceeds u32",
                );
                let len = u32::try_from(keys.len()).unwrap_or(u32::MAX);
                enc.write_u32_be(len);
                for k in keys {
                    enc.write_str(k);
                }
            }
            Self::SpawnTerminal {
                request_id,
                collection,
                command,
                cwd,
                env,
            } => {
                enc.write_u32_be(*request_id);
                enc.write_u32_be(collection.get());
                encode_optional_string_list(command.as_deref(), &mut enc);
                encode_optional_str(cwd.as_deref(), &mut enc);
                encode_optional_env(env.as_deref(), &mut enc);
            }
            Self::TerminalSpawned { request_id, result } => {
                enc.write_u32_be(*request_id);
                encode_spawn_result(result, &mut enc);
            }
            Self::TerminalClosed {
                terminal_id,
                exit_status,
            } => {
                encode_terminal_id(terminal_id, &mut enc);
                encode_optional_i32(*exit_status, &mut enc);
            }
            Self::TerminalResize {
                terminal_id,
                cols,
                rows,
            } => {
                encode_terminal_id(terminal_id, &mut enc);
                enc.write_u16_be(*cols);
                enc.write_u16_be(*rows);
            }
            Self::Command {
                request_id,
                command,
            } => {
                enc.write_u32_be(*request_id);
                encode_command(command, &mut enc);
            }
            Self::CommandResult { request_id, result } => {
                enc.write_u32_be(*request_id);
                encode_command_result(result, &mut enc);
            }
            Self::SubscribeEvents { terminal } => {
                encode_optional_terminal_id(terminal.as_ref(), &mut enc);
            }
            Self::Event { terminal, event } => {
                encode_optional_terminal_id(terminal.as_ref(), &mut enc);
                encode_agent_event(event, &mut enc);
            }
        }

        // Backfill the length header. The length value excludes the four
        // header bytes themselves but includes the type byte and payload, per
        // SPEC §5.
        let body_len = out.len() - body_start;
        debug_assert!(
            u32::try_from(body_len).is_ok_and(|n| n <= MAX_FRAME_LEN),
            "encoded frame exceeds protocol cap",
        );
        let len_u32 = u32::try_from(body_len).unwrap_or(u32::MAX);
        out[header_pos..header_pos + 4].copy_from_slice(&len_u32.to_be_bytes());
    }

    /// Decode a single frame from `input`. Returns the decoded frame and the
    /// unconsumed tail of `input`.
    pub fn decode(input: &[u8]) -> Result<(Self, &[u8]), DecodeError> {
        Decoder::new(input).read_frame()
    }
}

// -----------------------------------------------------------------------------
// Helpers for the message-catalog variants. Kept in this file so encoder and
// decoder share one source of truth for sub-record layout.
// -----------------------------------------------------------------------------

pub(super) fn encode_attach_target(target: &AttachTarget, enc: &mut Encoder<'_>) {
    match target {
        AttachTarget::Last => {
            enc.write_u8(ATTACH_TARGET_LAST);
        }
        AttachTarget::ByName(name) => {
            enc.write_u8(ATTACH_TARGET_BY_NAME);
            enc.write_str(name);
        }
        AttachTarget::ById(id) => {
            enc.write_u8(ATTACH_TARGET_BY_ID);
            enc.write_u32_be(id.get());
        }
        AttachTarget::CreateIfMissing { name, command, cwd } => {
            enc.write_u8(ATTACH_TARGET_CREATE_IF_MISSING);
            enc.write_str(name);
            encode_optional_string_list(command.as_deref(), enc);
            encode_optional_str(cwd.as_deref(), enc);
        }
    }
}

pub(super) fn decode_attach_target(dec: &mut Decoder<'_>) -> Result<AttachTarget, DecodeError> {
    let tag = dec.read_u8()?;
    match tag {
        ATTACH_TARGET_LAST => Ok(AttachTarget::Last),
        ATTACH_TARGET_BY_NAME => Ok(AttachTarget::ByName(dec.read_str()?.to_owned())),
        ATTACH_TARGET_BY_ID => Ok(AttachTarget::ById(SessionId::new(dec.read_u32_be()?))),
        ATTACH_TARGET_CREATE_IF_MISSING => {
            let name = dec.read_str()?.to_owned();
            let command = decode_optional_string_list(dec)?;
            let cwd = decode_optional_str(dec)?.map(str::to_owned);
            Ok(AttachTarget::CreateIfMissing { name, command, cwd })
        }
        other => Err(DecodeError::UnknownEnumValue {
            field: "AttachTarget",
            value: u32::from(other),
        }),
    }
}

pub(super) fn encode_viewport_info(v: &ViewportInfo, enc: &mut Encoder<'_>) {
    enc.write_u16_be(v.cols);
    enc.write_u16_be(v.rows);
    encode_optional_u16(v.pixel_w, enc);
    encode_optional_u16(v.pixel_h, enc);
}

pub(super) fn decode_viewport_info(dec: &mut Decoder<'_>) -> Result<ViewportInfo, DecodeError> {
    let cols = dec.read_u16_be()?;
    let rows = dec.read_u16_be()?;
    let pixel_w = decode_optional_u16(dec)?;
    let pixel_h = decode_optional_u16(dec)?;
    Ok(ViewportInfo {
        cols,
        rows,
        pixel_w,
        pixel_h,
    })
}

pub(super) const fn encode_focus_event(event: FocusEvent) -> u8 {
    match event {
        FocusEvent::Gained => 0,
        FocusEvent::Lost => 1,
    }
}

pub(super) fn decode_focus_event(tag: u8) -> Result<FocusEvent, DecodeError> {
    match tag {
        0 => Ok(FocusEvent::Gained),
        1 => Ok(FocusEvent::Lost),
        other => Err(DecodeError::UnknownEnumValue {
            field: "FocusEvent",
            value: u32::from(other),
        }),
    }
}

pub(super) fn encode_key_event(event: &KeyEvent, enc: &mut Encoder<'_>) {
    // `KeyAction`/`PhysicalKey` are phux-owned `#[repr(u32)]` enums (ADR-0024);
    // cast to the discriminant; the decoder round-trips via `TryFrom<u32>`.
    enc.write_u32_be(event.action as u32);
    enc.write_u32_be(event.key as u32);
    enc.write_u16_be(event.mods.bits());
    enc.write_u16_be(event.consumed_mods.bits());
    enc.write_u8(u8::from(event.composing));
    encode_optional_str(event.text.as_deref(), enc);
    encode_optional_u32(event.unshifted_codepoint, enc);
}

pub(super) fn decode_key_event(dec: &mut Decoder<'_>) -> Result<KeyEvent, DecodeError> {
    use crate::input::key::{KeyAction, ModSet, PhysicalKey};

    let action_raw = dec.read_u32_be()?;
    let action = KeyAction::try_from(action_raw).map_err(|_| DecodeError::UnknownEnumValue {
        field: "KeyAction",
        value: action_raw,
    })?;
    let key_raw = dec.read_u32_be()?;
    let key = PhysicalKey::try_from(key_raw).map_err(|_| DecodeError::UnknownEnumValue {
        field: "PhysicalKey",
        value: key_raw,
    })?;
    let mods = ModSet::from_bits_truncate(dec.read_u16_be()?);
    let consumed_mods = ModSet::from_bits_truncate(dec.read_u16_be()?);
    let composing = dec.read_u8()? != 0;
    let text = decode_optional_str(dec)?.map(str::to_owned);
    let unshifted_codepoint = decode_optional_u32(dec)?;
    Ok(KeyEvent {
        action,
        key,
        mods,
        consumed_mods,
        composing,
        text,
        unshifted_codepoint,
    })
}

pub(super) fn encode_mouse_event(event: &MouseEvent, enc: &mut Encoder<'_>) {
    enc.write_u32_be(event.action as u32);
    enc.write_u32_be(event.button as u32);
    enc.write_u16_be(event.mods.bits());
    enc.write_f64_be(event.x);
    enc.write_f64_be(event.y);
}

pub(super) fn decode_mouse_event(dec: &mut Decoder<'_>) -> Result<MouseEvent, DecodeError> {
    use crate::input::key::ModSet;
    use crate::input::mouse::{MouseAction, MouseButton};

    let action_raw = dec.read_u32_be()?;
    let action = MouseAction::try_from(action_raw).map_err(|_| DecodeError::UnknownEnumValue {
        field: "MouseAction",
        value: action_raw,
    })?;
    let button_raw = dec.read_u32_be()?;
    let button = MouseButton::try_from(button_raw).map_err(|_| DecodeError::UnknownEnumValue {
        field: "MouseButton",
        value: button_raw,
    })?;
    let mods = ModSet::from_bits_truncate(dec.read_u16_be()?);
    let x = dec.read_f64_be()?;
    let y = dec.read_f64_be()?;
    Ok(MouseEvent {
        action,
        button,
        mods,
        x,
        y,
    })
}

pub(super) fn encode_paste_event(event: &PasteEvent, enc: &mut Encoder<'_>) {
    enc.write_u8(event.trust as u8);
    enc.write_bytes(&event.data);
}

pub(super) fn decode_paste_event(dec: &mut Decoder<'_>) -> Result<PasteEvent, DecodeError> {
    use crate::input::paste::PasteTrust;
    let trust_tag = dec.read_u8()?;
    let trust = match trust_tag {
        0 => PasteTrust::Trusted,
        1 => PasteTrust::Untrusted,
        other => {
            return Err(DecodeError::UnknownEnumValue {
                field: "PasteTrust",
                value: u32::from(other),
            });
        }
    };
    let data = dec.read_bytes()?.to_vec();
    Ok(PasteEvent { trust, data })
}

// -----------------------------------------------------------------------------
// `TerminalId` tagged-union codec — ADR-0016 §Decision (phux-vp0.4).
//
// Every `TerminalId` on the wire is prefixed with a 1-byte tag:
//
//   tag = 0  → Local      { id: u32 }
//   tag = 1  → Satellite  { host: str, id: u32 }
//
// v0.1 encoders only produce tag=0. v0.1 decoders MUST accept tag=1; the
// dispatch layer (in `phux-server`) responds with `ERROR
// { UnsupportedSatelliteRoute }` (SPEC §14) when the server is not a
// federation hub. Unknown tags surface as `DecodeError::UnknownEnumValue`.
// -----------------------------------------------------------------------------

/// Encode a [`TerminalId`] including its discriminant byte.
pub(super) fn encode_terminal_id(id: &TerminalId, enc: &mut Encoder<'_>) {
    match id {
        TerminalId::Local { id } => {
            enc.write_u8(TERMINAL_ID_TAG_LOCAL);
            enc.write_u32_be(*id);
        }
        TerminalId::Satellite { host, id } => {
            enc.write_u8(TERMINAL_ID_TAG_SATELLITE);
            enc.write_str(host.as_str());
            enc.write_u32_be(*id);
        }
    }
}

/// Decode a [`TerminalId`] previously written by [`encode_terminal_id`].
///
/// v0.1 decoders MUST accept the `Satellite` tag and surface it to the
/// dispatcher; the dispatcher responds with `ERROR
/// { UnsupportedSatelliteRoute }` when the server is not a federation hub.
pub(super) fn decode_terminal_id(dec: &mut Decoder<'_>) -> Result<TerminalId, DecodeError> {
    let tag = dec.read_u8()?;
    match tag {
        TERMINAL_ID_TAG_LOCAL => {
            let id = dec.read_u32_be()?;
            Ok(TerminalId::Local { id })
        }
        TERMINAL_ID_TAG_SATELLITE => {
            let host = SatelliteHost::new(dec.read_str()?);
            let id = dec.read_u32_be()?;
            Ok(TerminalId::Satellite { host, id })
        }
        other => Err(DecodeError::UnknownEnumValue {
            field: "TerminalId",
            value: u32::from(other),
        }),
    }
}

/// Encode an `Option<TerminalId>` with the standard `0 = None / 1 = Some`
/// presence tag, then the tagged [`TerminalId`] body for the `Some` arm.
///
/// Used by the agent-event frames (`SUBSCRIBE_EVENTS`, `EVENT`) where the
/// Terminal scope is optional (`None` = server-scoped).
pub(super) fn encode_optional_terminal_id(id: Option<&TerminalId>, enc: &mut Encoder<'_>) {
    match id {
        None => enc.write_u8(0),
        Some(id) => {
            enc.write_u8(1);
            encode_terminal_id(id, enc);
        }
    }
}

/// Decode an `Option<TerminalId>` previously written by
/// [`encode_optional_terminal_id`].
pub(super) fn decode_optional_terminal_id(
    dec: &mut Decoder<'_>,
) -> Result<Option<TerminalId>, DecodeError> {
    let tag = dec.read_u8()?;
    match tag {
        0 => Ok(None),
        1 => Ok(Some(decode_terminal_id(dec)?)),
        other => Err(DecodeError::UnknownEnumValue {
            field: "Option<TerminalId> tag",
            value: u32::from(other),
        }),
    }
}

// -----------------------------------------------------------------------------
// Small option-of-primitive helpers. Local to this module — `info.rs` has its
// own parallel set tuned to its types (id newtypes, layout nodes).
// -----------------------------------------------------------------------------

fn encode_optional_str(value: Option<&str>, enc: &mut Encoder<'_>) {
    match value {
        None => enc.write_u8(0),
        Some(s) => {
            enc.write_u8(1);
            enc.write_str(s);
        }
    }
}

pub(super) fn decode_optional_str<'a>(
    dec: &mut Decoder<'a>,
) -> Result<Option<&'a str>, DecodeError> {
    let tag = dec.read_u8()?;
    match tag {
        0 => Ok(None),
        1 => Ok(Some(dec.read_str()?)),
        other => Err(DecodeError::UnknownEnumValue {
            field: "Option<str> tag",
            value: u32::from(other),
        }),
    }
}

fn encode_optional_u16(value: Option<u16>, enc: &mut Encoder<'_>) {
    match value {
        None => enc.write_u8(0),
        Some(n) => {
            enc.write_u8(1);
            enc.write_u16_be(n);
        }
    }
}

fn decode_optional_u16(dec: &mut Decoder<'_>) -> Result<Option<u16>, DecodeError> {
    let tag = dec.read_u8()?;
    match tag {
        0 => Ok(None),
        1 => Ok(Some(dec.read_u16_be()?)),
        other => Err(DecodeError::UnknownEnumValue {
            field: "Option<u16> tag",
            value: u32::from(other),
        }),
    }
}

fn encode_optional_u32(value: Option<u32>, enc: &mut Encoder<'_>) {
    match value {
        None => enc.write_u8(0),
        Some(n) => {
            enc.write_u8(1);
            enc.write_u32_be(n);
        }
    }
}

pub(super) fn decode_optional_u32(dec: &mut Decoder<'_>) -> Result<Option<u32>, DecodeError> {
    let tag = dec.read_u8()?;
    match tag {
        0 => Ok(None),
        1 => Ok(Some(dec.read_u32_be()?)),
        other => Err(DecodeError::UnknownEnumValue {
            field: "Option<u32> tag",
            value: u32::from(other),
        }),
    }
}

fn encode_optional_string_list(value: Option<&[String]>, enc: &mut Encoder<'_>) {
    match value {
        None => enc.write_u8(0),
        Some(list) => {
            enc.write_u8(1);
            debug_assert!(
                u32::try_from(list.len()).is_ok(),
                "string list length exceeds u32",
            );
            let len = u32::try_from(list.len()).unwrap_or(u32::MAX);
            enc.write_u32_be(len);
            for s in list {
                enc.write_str(s);
            }
        }
    }
}

pub(super) fn decode_optional_string_list(
    dec: &mut Decoder<'_>,
) -> Result<Option<Vec<String>>, DecodeError> {
    let tag = dec.read_u8()?;
    match tag {
        0 => Ok(None),
        1 => {
            let len = dec.read_u32_be()?;
            let len_usize = usize::try_from(len).map_err(|_| DecodeError::LengthOverflow)?;
            // Clamp reservation to remaining bytes (each element is >=1 byte):
            // an over-declared length errors on EOF below rather than driving
            // an unbounded `Vec::with_capacity`.
            let mut out = dec.bounded_capacity(len_usize);
            for _ in 0..len_usize {
                out.push(dec.read_str()?.to_owned());
            }
            Ok(Some(out))
        }
        other => Err(DecodeError::UnknownEnumValue {
            field: "Option<list<str>> tag",
            value: u32::from(other),
        }),
    }
}

fn encode_optional_bytes(value: Option<&[u8]>, enc: &mut Encoder<'_>) {
    match value {
        None => enc.write_u8(0),
        Some(b) => {
            enc.write_u8(1);
            enc.write_bytes(b);
        }
    }
}

pub(super) fn decode_optional_bytes(dec: &mut Decoder<'_>) -> Result<Option<Vec<u8>>, DecodeError> {
    let tag = dec.read_u8()?;
    match tag {
        0 => Ok(None),
        1 => Ok(Some(dec.read_bytes()?.to_vec())),
        other => Err(DecodeError::UnknownEnumValue {
            field: "Option<bytes> tag",
            value: u32::from(other),
        }),
    }
}

// -----------------------------------------------------------------------------
// Scope codec — SPEC §7.4 (phux-4li.2).
//
// Layout: 1-byte tag + variant body.
//   0x00 Terminal   → tagged TerminalId (re-uses the L1 codec)
//   0x01 Collection → u32 (the inner CollectionId; once L2 ships a
//                     Local/Satellite tag will prefix this, mirroring the
//                     ADR-0016 TerminalId shape)
//   0x02 Global     → no body
// -----------------------------------------------------------------------------

pub(super) fn encode_scope(scope: &Scope, enc: &mut Encoder<'_>) {
    match scope {
        Scope::Terminal(terminal_id) => {
            enc.write_u8(SCOPE_TAG_TERMINAL);
            encode_terminal_id(terminal_id, enc);
        }
        Scope::Collection(collection_id) => {
            enc.write_u8(SCOPE_TAG_COLLECTION);
            enc.write_u32_be(collection_id.get());
        }
        Scope::Global => {
            enc.write_u8(SCOPE_TAG_GLOBAL);
        }
    }
}

pub(super) fn decode_scope(dec: &mut Decoder<'_>) -> Result<Scope, DecodeError> {
    let tag = dec.read_u8()?;
    match tag {
        SCOPE_TAG_TERMINAL => Ok(Scope::Terminal(decode_terminal_id(dec)?)),
        SCOPE_TAG_COLLECTION => Ok(Scope::Collection(CollectionId::new(dec.read_u32_be()?))),
        SCOPE_TAG_GLOBAL => Ok(Scope::Global),
        other => Err(DecodeError::UnknownEnumValue {
            field: "Scope",
            value: u32::from(other),
        }),
    }
}

// -----------------------------------------------------------------------------
// SpawnResult / SpawnError codec — SPEC §7.2 / §10.1 (phux-4li.10).
//
// Layout (outer SpawnResult, the body of `TERMINAL_SPAWNED.result`):
//   tag 0x00 Ok  → tagged TerminalId
//   tag 0x01 Err → SpawnError body:
//                    tag 0x00 CollectionNotFound → no further bytes
//                    tag 0x01 SpawnFailed        → length-prefixed UTF-8
//
// The `Ok = 0x00 / Err = 0x01` convention deliberately mirrors the
// `Option` tag convention (`None = 0x00 / Some = 0x01`) so hex-dump
// readers do not need a second per-shape table.
// -----------------------------------------------------------------------------

pub(super) fn encode_spawn_result(result: &SpawnResult, enc: &mut Encoder<'_>) {
    match result {
        SpawnResult::Ok(terminal_id) => {
            enc.write_u8(SPAWN_RESULT_OK);
            encode_terminal_id(terminal_id, enc);
        }
        SpawnResult::Err(err) => {
            enc.write_u8(SPAWN_RESULT_ERR);
            encode_spawn_error(err, enc);
        }
    }
}

pub(super) fn decode_spawn_result(dec: &mut Decoder<'_>) -> Result<SpawnResult, DecodeError> {
    let tag = dec.read_u8()?;
    match tag {
        SPAWN_RESULT_OK => Ok(SpawnResult::Ok(decode_terminal_id(dec)?)),
        SPAWN_RESULT_ERR => Ok(SpawnResult::Err(decode_spawn_error(dec)?)),
        other => Err(DecodeError::UnknownEnumValue {
            field: "SpawnResult",
            value: u32::from(other),
        }),
    }
}

fn encode_spawn_error(err: &SpawnError, enc: &mut Encoder<'_>) {
    match err {
        SpawnError::CollectionNotFound => {
            enc.write_u8(SPAWN_ERROR_TAG_COLLECTION_NOT_FOUND);
        }
        SpawnError::SpawnFailed(msg) => {
            enc.write_u8(SPAWN_ERROR_TAG_SPAWN_FAILED);
            enc.write_str(msg);
        }
    }
}

fn decode_spawn_error(dec: &mut Decoder<'_>) -> Result<SpawnError, DecodeError> {
    let tag = dec.read_u8()?;
    match tag {
        SPAWN_ERROR_TAG_COLLECTION_NOT_FOUND => Ok(SpawnError::CollectionNotFound),
        SPAWN_ERROR_TAG_SPAWN_FAILED => Ok(SpawnError::SpawnFailed(dec.read_str()?.to_owned())),
        other => Err(DecodeError::UnknownEnumValue {
            field: "SpawnError",
            value: u32::from(other),
        }),
    }
}

// -----------------------------------------------------------------------------
// Control-plane command codec — SPEC §5 (phux-k61 / ADR-0021).
//
// COMMAND body:        u32 request_id, then Command (tag + body).
// COMMAND_RESULT body: u32 request_id, then CommandResult (tag + body).
//
// Command tags follow the SPEC §5.1 catalog order; KILL_TERMINAL (0x03)
// and GET_STATE (0x05) are wired in v0.1, plus the appended GET_SCREEN
// (0x07, after RUN_HOOK's reserved 0x06), ROUTE_INPUT (0x08), and
// CREATE_SESSION (0x09). CommandResult / CommandValue tags use the same
// `Ok = 0x00` / sequential convention as the rest of the wire.
// -----------------------------------------------------------------------------

pub(super) fn encode_command(command: &Command, enc: &mut Encoder<'_>) {
    match command {
        Command::KillTerminal { terminal_id } => {
            enc.write_u8(COMMAND_TAG_KILL_TERMINAL);
            encode_terminal_id(terminal_id, enc);
        }
        Command::GetState { scope } => {
            enc.write_u8(COMMAND_TAG_GET_STATE);
            encode_state_scope(scope, enc);
        }
        Command::GetScreen {
            terminal_id,
            request_scrollback,
            cells,
        } => {
            enc.write_u8(COMMAND_TAG_GET_SCREEN);
            encode_terminal_id(terminal_id, enc);
            encode_optional_u32(*request_scrollback, enc);
            enc.write_u8(u8::from(*cells));
        }
        Command::RouteInput { terminal_id, event } => {
            enc.write_u8(COMMAND_TAG_ROUTE_INPUT);
            encode_terminal_id(terminal_id, enc);
            encode_input_event(event, enc);
        }
        Command::CreateSession {
            collection,
            name,
            command,
            cwd,
        } => {
            enc.write_u8(COMMAND_TAG_CREATE_SESSION);
            enc.write_u32_be(collection.get());
            enc.write_str(name);
            encode_optional_string_list(command.as_deref(), enc);
            encode_optional_str(cwd.as_deref(), enc);
        }
        Command::KillCollection { collection, name } => {
            enc.write_u8(COMMAND_TAG_KILL_COLLECTION);
            enc.write_u32_be(collection.get());
            enc.write_str(name);
        }
        Command::RenameSession {
            collection,
            name,
            new_name,
        } => {
            enc.write_u8(COMMAND_TAG_RENAME_SESSION);
            enc.write_u32_be(collection.get());
            enc.write_str(name);
            enc.write_str(new_name);
        }
        Command::GetTerminalState {
            terminal_id,
            include_scrollback,
            max_scrollback_lines,
        } => {
            enc.write_u8(COMMAND_TAG_GET_TERMINAL_STATE);
            encode_terminal_id(terminal_id, enc);
            enc.write_u8(u8::from(*include_scrollback));
            enc.write_u16_be(*max_scrollback_lines);
        }
        Command::SubscribeTerminalEvents {
            terminal_id,
            event_types,
        } => {
            enc.write_u8(COMMAND_TAG_SUBSCRIBE_TERMINAL_EVENTS);
            encode_terminal_id(terminal_id, enc);
            enc.write_u16_be(u16::try_from(event_types.len()).unwrap_or(0));
            for et in event_types {
                enc.write_u8(et.to_u8());
            }
        }
    }
}

fn encode_input_event(event: &InputEvent, enc: &mut Encoder<'_>) {
    match event {
        InputEvent::Key(event) => {
            enc.write_u8(INPUT_EVENT_TAG_KEY);
            encode_key_event(event, enc);
        }
        InputEvent::Mouse(event) => {
            enc.write_u8(INPUT_EVENT_TAG_MOUSE);
            encode_mouse_event(event, enc);
        }
        InputEvent::Focus(event) => {
            enc.write_u8(INPUT_EVENT_TAG_FOCUS);
            enc.write_u8(encode_focus_event(*event));
        }
        InputEvent::Paste(event) => {
            enc.write_u8(INPUT_EVENT_TAG_PASTE);
            encode_paste_event(event, enc);
        }
        InputEvent::Selection(event) => {
            enc.write_u8(INPUT_EVENT_TAG_SELECTION);
            enc.write_u8(event.mode.as_u8());
            enc.write_u8(u8::from(event.rectangle));
        }
    }
}

fn decode_input_event(dec: &mut Decoder<'_>) -> Result<InputEvent, DecodeError> {
    let tag = dec.read_u8()?;
    match tag {
        INPUT_EVENT_TAG_KEY => Ok(InputEvent::Key(decode_key_event(dec)?)),
        INPUT_EVENT_TAG_MOUSE => Ok(InputEvent::Mouse(decode_mouse_event(dec)?)),
        INPUT_EVENT_TAG_FOCUS => Ok(InputEvent::Focus(decode_focus_event(dec.read_u8()?)?)),
        INPUT_EVENT_TAG_PASTE => Ok(InputEvent::Paste(decode_paste_event(dec)?)),
        INPUT_EVENT_TAG_SELECTION => {
            let mode_u8 = dec.read_u8()?;
            let mode = SelectionMode::try_from_u8(mode_u8).ok_or_else(|| {
                DecodeError::UnknownEnumValue {
                    field: "SelectionMode",
                    value: u32::from(mode_u8),
                }
            })?;
            let rectangle = dec.read_u8()? != 0;
            Ok(InputEvent::Selection(SelectionEvent { mode, rectangle }))
        }
        other => Err(DecodeError::UnknownEnumValue {
            field: "InputEvent",
            value: u32::from(other),
        }),
    }
}

pub(super) fn decode_command(dec: &mut Decoder<'_>) -> Result<Command, DecodeError> {
    let tag = dec.read_u8()?;
    match tag {
        COMMAND_TAG_KILL_TERMINAL => Ok(Command::KillTerminal {
            terminal_id: decode_terminal_id(dec)?,
        }),
        COMMAND_TAG_GET_STATE => Ok(Command::GetState {
            scope: decode_state_scope(dec)?,
        }),
        COMMAND_TAG_GET_SCREEN => {
            let terminal_id = decode_terminal_id(dec)?;
            let request_scrollback = decode_optional_u32(dec)?;
            // `cells` is a trailing additive bool (`phux-8yl`): a
            // pre-`phux-8yl` body ends after `request_scrollback`, so an
            // absent byte (cursor already at the frame-body end) means
            // `false`. A present byte is read as a bool (non-zero is
            // `true`). `at_body_end` (not `remaining().is_empty()`) keeps a
            // following frame's bytes from being misread as `cells`.
            let cells = if dec.at_body_end() {
                false
            } else {
                dec.read_u8()? != 0
            };
            Ok(Command::GetScreen {
                terminal_id,
                request_scrollback,
                cells,
            })
        }
        COMMAND_TAG_ROUTE_INPUT => Ok(Command::RouteInput {
            terminal_id: decode_terminal_id(dec)?,
            event: decode_input_event(dec)?,
        }),
        COMMAND_TAG_CREATE_SESSION => {
            let collection = CollectionId::new(dec.read_u32_be()?);
            let name = dec.read_str()?.to_owned();
            let command = decode_optional_string_list(dec)?;
            let cwd = decode_optional_str(dec)?.map(str::to_owned);
            Ok(Command::CreateSession {
                collection,
                name,
                command,
                cwd,
            })
        }
        COMMAND_TAG_KILL_COLLECTION => {
            let collection = CollectionId::new(dec.read_u32_be()?);
            let name = dec.read_str()?.to_owned();
            Ok(Command::KillCollection { collection, name })
        }
        COMMAND_TAG_RENAME_SESSION => {
            let collection = CollectionId::new(dec.read_u32_be()?);
            let name = dec.read_str()?.to_owned();
            let new_name = dec.read_str()?.to_owned();
            Ok(Command::RenameSession {
                collection,
                name,
                new_name,
            })
        }
        COMMAND_TAG_GET_TERMINAL_STATE => {
            let terminal_id = decode_terminal_id(dec)?;
            let include_scrollback = dec.read_u8()? != 0;
            let max_scrollback_lines = dec.read_u16_be()?;
            Ok(Command::GetTerminalState {
                terminal_id,
                include_scrollback,
                max_scrollback_lines,
            })
        }
        COMMAND_TAG_SUBSCRIBE_TERMINAL_EVENTS => {
            let terminal_id = decode_terminal_id(dec)?;
            let count = dec.read_u16_be()? as usize;
            let mut event_types = Vec::with_capacity(count);
            for _ in 0..count {
                if let Some(et) = TerminalEventType::from_u8(dec.read_u8()?) {
                    event_types.push(et);
                }
            }
            Ok(Command::SubscribeTerminalEvents {
                terminal_id,
                event_types,
            })
        }
        other => Err(DecodeError::UnknownEnumValue {
            field: "Command",
            value: u32::from(other),
        }),
    }
}

fn encode_state_scope(scope: &StateScope, enc: &mut Encoder<'_>) {
    match scope {
        StateScope::Server => enc.write_u8(STATE_SCOPE_TAG_SERVER),
    }
}

fn decode_state_scope(dec: &mut Decoder<'_>) -> Result<StateScope, DecodeError> {
    let tag = dec.read_u8()?;
    match tag {
        STATE_SCOPE_TAG_SERVER => Ok(StateScope::Server),
        other => Err(DecodeError::UnknownEnumValue {
            field: "StateScope",
            value: u32::from(other),
        }),
    }
}

pub(super) fn encode_command_result(result: &CommandResult, enc: &mut Encoder<'_>) {
    match result {
        CommandResult::Ok => enc.write_u8(COMMAND_RESULT_TAG_OK),
        CommandResult::OkWith(value) => {
            enc.write_u8(COMMAND_RESULT_TAG_OK_WITH);
            encode_command_value(value, enc);
        }
        CommandResult::Error { code, message } => {
            enc.write_u8(COMMAND_RESULT_TAG_ERROR);
            enc.write_u16_be(code.as_wire());
            enc.write_str(message);
        }
    }
}

pub(super) fn decode_command_result(dec: &mut Decoder<'_>) -> Result<CommandResult, DecodeError> {
    let tag = dec.read_u8()?;
    match tag {
        COMMAND_RESULT_TAG_OK => Ok(CommandResult::Ok),
        COMMAND_RESULT_TAG_OK_WITH => Ok(CommandResult::OkWith(decode_command_value(dec)?)),
        COMMAND_RESULT_TAG_ERROR => {
            let code_raw = dec.read_u16_be()?;
            let code =
                ErrorCode::from_wire(code_raw).ok_or_else(|| DecodeError::UnknownEnumValue {
                    field: "ErrorCode",
                    value: u32::from(code_raw),
                })?;
            let message = dec.read_str()?.to_owned();
            Ok(CommandResult::Error { code, message })
        }
        other => Err(DecodeError::UnknownEnumValue {
            field: "CommandResult",
            value: u32::from(other),
        }),
    }
}

fn encode_command_value(value: &CommandValue, enc: &mut Encoder<'_>) {
    match value {
        CommandValue::TerminalId(id) => {
            enc.write_u8(COMMAND_VALUE_TAG_TERMINAL_ID);
            encode_terminal_id(id, enc);
        }
        CommandValue::CollectionId(id) => {
            enc.write_u8(COMMAND_VALUE_TAG_COLLECTION_ID);
            enc.write_u32_be(id.get());
        }
        CommandValue::State(snapshot) => {
            enc.write_u8(COMMAND_VALUE_TAG_STATE);
            encode_session_snapshot(snapshot, enc);
        }
        CommandValue::Json(s) => {
            enc.write_u8(COMMAND_VALUE_TAG_JSON);
            enc.write_str(s);
        }
        CommandValue::Bytes(b) => {
            enc.write_u8(COMMAND_VALUE_TAG_BYTES);
            enc.write_bytes(b);
        }
    }
}

fn decode_command_value(dec: &mut Decoder<'_>) -> Result<CommandValue, DecodeError> {
    let tag = dec.read_u8()?;
    match tag {
        COMMAND_VALUE_TAG_TERMINAL_ID => Ok(CommandValue::TerminalId(decode_terminal_id(dec)?)),
        COMMAND_VALUE_TAG_COLLECTION_ID => Ok(CommandValue::CollectionId(CollectionId::new(
            dec.read_u32_be()?,
        ))),
        COMMAND_VALUE_TAG_STATE => Ok(CommandValue::State(decode_session_snapshot(dec)?)),
        COMMAND_VALUE_TAG_JSON => Ok(CommandValue::Json(dec.read_str()?.to_owned())),
        COMMAND_VALUE_TAG_BYTES => Ok(CommandValue::Bytes(dec.read_bytes()?.to_vec())),
        other => Err(DecodeError::UnknownEnumValue {
            field: "CommandValue",
            value: u32::from(other),
        }),
    }
}

// -----------------------------------------------------------------------------
// `Option<i32>` codec — used by `TERMINAL_CLOSED.exit_status` (SPEC §10.1).
//
// Tag convention matches every other `Option` on the wire: `0 = None`,
// `1 = Some(value)`. The body is the two's-complement bit pattern
// reinterpreted as `u32` (matching how the `i64` encoder treats
// timestamps in `info.rs`).
// -----------------------------------------------------------------------------

fn encode_optional_i32(value: Option<i32>, enc: &mut Encoder<'_>) {
    match value {
        None => enc.write_u8(0),
        Some(n) => {
            enc.write_u8(1);
            // Two's-complement bit pattern reinterpreted as u32 — bit-
            // identical to the `i64` encoder treatment in `info.rs`. Using
            // `i32::to_be_bytes` avoids the sign-loss clippy lint that a
            // direct `n as u32` cast triggers (the in-memory bits are the
            // same; the lint is right that the *value* changes meaning).
            enc.write_u32_be(u32::from_be_bytes(n.to_be_bytes()));
        }
    }
}

pub(super) fn decode_optional_i32(dec: &mut Decoder<'_>) -> Result<Option<i32>, DecodeError> {
    let tag = dec.read_u8()?;
    match tag {
        0 => Ok(None),
        1 => {
            // Symmetric to the encoder: reinterpret the u32's big-endian
            // bytes as the i32 two's-complement bit pattern.
            let bits = dec.read_u32_be()?;
            Ok(Some(i32::from_be_bytes(bits.to_be_bytes())))
        }
        other => Err(DecodeError::UnknownEnumValue {
            field: "Option<i32> tag",
            value: u32::from(other),
        }),
    }
}

// -----------------------------------------------------------------------------
// `Option<Vec<(String, String)>>` codec — used by `SPAWN_TERMINAL.env`
// (SPEC §7.2 / §10.1).
//
// Mirrors `encode_optional_string_list`'s shape: outer 1-byte presence
// tag, then (when `Some`) a `u32` element count followed by N pairs of
// length-prefixed UTF-8 strings. Each pair is `(key, value)` in that
// order.
// -----------------------------------------------------------------------------

fn encode_optional_env(value: Option<&[(String, String)]>, enc: &mut Encoder<'_>) {
    match value {
        None => enc.write_u8(0),
        Some(list) => {
            enc.write_u8(1);
            debug_assert!(
                u32::try_from(list.len()).is_ok(),
                "env list length exceeds u32",
            );
            let len = u32::try_from(list.len()).unwrap_or(u32::MAX);
            enc.write_u32_be(len);
            for (k, v) in list {
                enc.write_str(k);
                enc.write_str(v);
            }
        }
    }
}

pub(super) fn decode_optional_env(
    dec: &mut Decoder<'_>,
) -> Result<Option<Vec<(String, String)>>, DecodeError> {
    let tag = dec.read_u8()?;
    match tag {
        0 => Ok(None),
        1 => {
            let len = dec.read_u32_be()?;
            let len_usize = usize::try_from(len).map_err(|_| DecodeError::LengthOverflow)?;
            // Clamp reservation to remaining bytes (each (k, v) pair is
            // >=8 bytes on the wire, so remaining bytes is a safe upper
            // bound): an over-declared length errors on EOF below.
            let mut out = dec.bounded_capacity(len_usize);
            for _ in 0..len_usize {
                let k = dec.read_str()?.to_owned();
                let v = dec.read_str()?.to_owned();
                out.push((k, v));
            }
            Ok(Some(out))
        }
        other => Err(DecodeError::UnknownEnumValue {
            field: "Option<list<(str, str)>> tag",
            value: u32::from(other),
        }),
    }
}

// -----------------------------------------------------------------------------
// AgentEvent codec — SPEC §7.5 / §10.3 (phux-y2t).
//
// TLV layout: `tag: u8`, then a length-prefixed `body: bytes`. The
// length prefix is the forward-compat lever — a decoder that doesn't
// recognise `tag` reads the body length, captures the bytes verbatim as
// `AgentEvent::Unknown { tag, body }`, and moves on without failing the
// frame. Known bodies decode from a sub-`Decoder` over the captured body
// slice, so a body that declares more fields than this version knows is
// still bounded by its own length (trailing additive fields inside a
// known event are likewise skippable).
//
// Body shapes by tag:
//   COMMAND_STARTED  (0x00) → empty
//   COMMAND_FINISHED (0x01) → optional<i32> exit_code
//   TITLE_CHANGED    (0x02) → str title
//   BELL             (0x03) → empty
//   PANE_SPAWNED     (0x04) → empty (the id rides the EVENT envelope)
//   PANE_CLOSED      (0x05) → optional<i32> exit_status
//   DIRTY            (0x06) → empty
//   IDLE             (0x07) → empty
// -----------------------------------------------------------------------------

pub(super) fn encode_agent_event(event: &AgentEvent, enc: &mut Encoder<'_>) {
    // Encode the variant body into a scratch buffer first, then write the
    // tag + the body as a single length-prefixed block. Keeping the body
    // length-delimited is what lets an older decoder skip an unknown tag.
    let mut body = BytesMut::new();
    let tag = {
        let mut body_enc = Encoder::new(&mut body);
        match event {
            AgentEvent::CommandStarted => EVENT_TAG_COMMAND_STARTED,
            AgentEvent::CommandFinished { exit_code } => {
                encode_optional_i32(*exit_code, &mut body_enc);
                EVENT_TAG_COMMAND_FINISHED
            }
            AgentEvent::TitleChanged { title } => {
                body_enc.write_str(title);
                EVENT_TAG_TITLE_CHANGED
            }
            AgentEvent::Bell => EVENT_TAG_BELL,
            AgentEvent::PaneSpawned => EVENT_TAG_PANE_SPAWNED,
            AgentEvent::PaneClosed { exit_status } => {
                encode_optional_i32(*exit_status, &mut body_enc);
                EVENT_TAG_PANE_CLOSED
            }
            AgentEvent::Dirty => EVENT_TAG_DIRTY,
            AgentEvent::Idle => EVENT_TAG_IDLE,
            // `Unknown` is decoder-only: an encoder that reaches here has
            // round-tripped an event this version did not understand.
            // Re-emit the captured body verbatim so a relay (a hub
            // forwarding a satellite's event, say) is lossless rather than
            // dropping the event or panicking. The raw bytes are appended
            // after this block to sidestep the `body`/`body_enc` borrow.
            AgentEvent::Unknown { tag, .. } => *tag,
        }
    };
    if let AgentEvent::Unknown { body: raw, .. } = event {
        body.extend_from_slice(raw);
    }
    enc.write_u8(tag);
    enc.write_bytes(&body);
}

pub(super) fn decode_agent_event(dec: &mut Decoder<'_>) -> Result<AgentEvent, DecodeError> {
    let tag = dec.read_u8()?;
    let body = dec.read_bytes()?;
    // Sub-decoder over just this event's body. A known body that declares
    // fewer bytes than expected errors with `UnexpectedEof`; an unknown
    // tag is captured verbatim and skipped.
    let mut body_dec = Decoder::new(body);
    let event = match tag {
        EVENT_TAG_COMMAND_STARTED => AgentEvent::CommandStarted,
        EVENT_TAG_COMMAND_FINISHED => AgentEvent::CommandFinished {
            exit_code: decode_optional_i32(&mut body_dec)?,
        },
        EVENT_TAG_TITLE_CHANGED => AgentEvent::TitleChanged {
            title: body_dec.read_str()?.to_owned(),
        },
        EVENT_TAG_BELL => AgentEvent::Bell,
        EVENT_TAG_PANE_SPAWNED => AgentEvent::PaneSpawned,
        EVENT_TAG_PANE_CLOSED => AgentEvent::PaneClosed {
            exit_status: decode_optional_i32(&mut body_dec)?,
        },
        EVENT_TAG_DIRTY => AgentEvent::Dirty,
        EVENT_TAG_IDLE => AgentEvent::Idle,
        // Unknown event tag: preserve the body verbatim and skip. This is
        // the forward-compat path — a v0.2.x server may add event kinds an
        // older client does not know.
        other => AgentEvent::Unknown {
            tag: other,
            body: body.to_vec(),
        },
    };
    Ok(event)
}
