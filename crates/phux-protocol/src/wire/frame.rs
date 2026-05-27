//! Frame header and `FrameKind` enum.
//!
//! See `SPEC.md` §5 (framing) and §7 (message catalog).
//!
//! Wire layout (per `SPEC.md` §5):
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

use crate::caps::ClientCapabilities;
use crate::ids::{
    ClientId, CollectionId, SatelliteHost, SessionId, TERMINAL_ID_TAG_LOCAL,
    TERMINAL_ID_TAG_SATELLITE, TerminalId,
};
use crate::input::focus::FocusEvent;
use crate::input::key::KeyEvent;
use crate::input::mouse::MouseEvent;
use crate::input::paste::PasteEvent;

use super::decode::Decoder;
use super::encode::Encoder;
use super::error::DecodeError;
use super::info::{SessionSnapshot, encode_client_id, encode_session_snapshot};

/// Maximum permitted value of the wire-frame `length` field, per `SPEC.md` §5
/// ("at most `16_777_216` (16 MiB)").
pub const MAX_FRAME_LEN: u32 = 16 * 1024 * 1024;

// -----------------------------------------------------------------------------
// Message discriminants from SPEC §7. Only the variants implemented in this
// scaffold are exposed via `FrameKind`; the remaining IDs are recorded here so
// sibling tasks can wire them up without re-deriving the catalog.
// -----------------------------------------------------------------------------

/// Discriminant for `HELLO` (client to server, `SPEC.md` §6.1).
pub const TYPE_HELLO: u8 = 0x01;
/// Discriminant for `ATTACH` (client to server, `SPEC.md` §7.1 / §13).
pub const TYPE_ATTACH: u8 = 0x02;
/// Discriminant for `DETACH` (client to server, `SPEC.md` §7.1 / §7.3).
pub const TYPE_DETACH: u8 = 0x03;
/// Discriminant for `INPUT_KEY` (client to server, `SPEC.md` §9.1).
pub const TYPE_INPUT_KEY: u8 = 0x10;
/// Discriminant for `INPUT_PASTE` (client to server, `SPEC.md` §9.4).
pub const TYPE_INPUT_PASTE: u8 = 0x11;
/// Discriminant for `INPUT_MOUSE` (client to server, `SPEC.md` §9.2).
pub const TYPE_INPUT_MOUSE: u8 = 0x12;
/// Discriminant for `INPUT_FOCUS` (client to server, `SPEC.md` §9.3).
pub const TYPE_INPUT_FOCUS: u8 = 0x14;
/// Discriminant for `FRAME_ACK` (client to server, `SPEC.md` §7.proto.1 / §12.2).
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
/// Discriminant for `VIEWPORT_RESIZE` (client to server, `SPEC.md` §7.1 / §10.5).
///
/// The client emits this when its outer terminal changes size (SIGWINCH on
/// Unix, the GUI resize event on graphical hosts). Payload reuses the
/// [`ViewportInfo`] shape carried by `ATTACH` (§13) — phux-4hp keeps the wire
/// shape minimal and lets future tickets grow the per-cell pixel + padding
/// metrics from SPEC §10.5 when the mouse-encoder needs them.
pub const TYPE_VIEWPORT_RESIZE: u8 = 0x20;
/// Discriminant for `PING` (client to server, `SPEC.md` §7.5).
pub const TYPE_PING: u8 = 0x7F;
/// Discriminant for `HELLO_OK` (server to client, `SPEC.md` §6.1). Reserved.
pub const TYPE_HELLO_OK: u8 = 0x80;
/// Discriminant for `ATTACHED` (server to client, `SPEC.md` §7.2 / §13).
pub const TYPE_ATTACHED: u8 = 0x81;
/// Discriminant for `DETACHED` (server to client, `SPEC.md` §7.2 / §7.3).
pub const TYPE_DETACHED: u8 = 0x82;
/// Discriminant for `BELL` (server to client, `SPEC.md` §7.6).
pub const TYPE_BELL: u8 = 0xB0;
/// Discriminant for `ERROR` (server to client, `SPEC.md` §14).
///
/// Carries a structured [`ErrorCode`] plus a human-readable UTF-8 message
/// and an optional `request_id` correlating the error with a prior
/// `COMMAND` (per SPEC §14). Fatal errors MUST be followed by `DETACHED
/// { reason: PROTOCOL_ERROR }` and transport close.
pub const TYPE_ERROR: u8 = 0xC1;
/// Discriminant for `PONG` (server to client, `SPEC.md` §7.5). Reserved.
pub const TYPE_PONG: u8 = 0xFF;
/// Discriminant for `TERMINAL_OUTPUT` (server to client, `SPEC.md` §7.2 / §8.1).
///
/// Hot-path terminal content under [ADR-0013]: the server forwards PTY bytes
/// (possibly downsampled per the client's [`crate::caps::ColorSupport`])
/// in `TERMINAL_OUTPUT` frames. Supersedes the earlier `PANE_DIFF` slot;
/// `PANE_DIFF` is retired and its old discriminant (`0x40`) is no longer
/// recognised.
pub const TYPE_TERMINAL_OUTPUT: u8 = 0x90;
/// Discriminant for `TERMINAL_SNAPSHOT` (server to client, `SPEC.md` §7.2 / §8.4).
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

/// Discriminant for `GET_METADATA` (client to server, `SPEC.md` §7.4 / §11.L3).
pub const TYPE_GET_METADATA: u8 = 0x50;
/// Discriminant for `SET_METADATA` (client to server, `SPEC.md` §7.4 / §11.L3).
pub const TYPE_SET_METADATA: u8 = 0x51;
/// Discriminant for `DELETE_METADATA` (client to server, `SPEC.md` §7.4 / §11.L3).
pub const TYPE_DELETE_METADATA: u8 = 0x52;
/// Discriminant for `LIST_METADATA` (client to server, `SPEC.md` §7.4 / §11.L3).
pub const TYPE_LIST_METADATA: u8 = 0x53;
/// Discriminant for `SUBSCRIBE_METADATA` (client to server, `SPEC.md` §7.4).
pub const TYPE_SUBSCRIBE_METADATA: u8 = 0x54;

/// Discriminant for `METADATA_CHANGED` (server to client, `SPEC.md` §7.4).
pub const TYPE_METADATA_CHANGED: u8 = 0xD0;

// Wire tags for the `Scope` tagged union (SPEC §7.4 / §11.L3).
/// Wire tag for [`Scope::Terminal`].
pub(crate) const SCOPE_TAG_TERMINAL: u8 = 0;
/// Wire tag for [`Scope::Collection`].
pub(crate) const SCOPE_TAG_COLLECTION: u8 = 1;
/// Wire tag for [`Scope::Global`].
pub(crate) const SCOPE_TAG_GLOBAL: u8 = 2;

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

/// Decoded wire frame.
///
/// The phux-6yl.4 scaffold populated `Hello`, `Ping`, and `PaneDiff`. The
/// phux-4az pass added the message-catalog variants needed for the attach
/// lifecycle. The phux-i58 SPEC §13 conformance pass conforms ATTACH/ATTACHED
/// to spec and splits out `TERMINAL_SNAPSHOT` per SPEC §16. Under [ADR-0013] the
/// structured `PaneDiff` variant is replaced by `TerminalOutput` (raw VT bytes)
/// and `TerminalSnapshot` carries `vt_replay_bytes` instead of a `DiffOp` list.
/// The remaining SPEC §7 catalog (`Hello_Ok`, `Pong`, `TerminalEvent`,
/// `Alert`, resize/ack/command/etc.) lands in sibling tasks.
///
/// [ADR-0013]: https://github.com/phall1/phux/blob/main/ADR/0013-libghostty-bytes-on-wire.md
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum FrameKind {
    /// `HELLO` — client to server handshake (`SPEC.md` §6.1).
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
    /// Sibling tickets grow `ClientCapabilities` with the rest of
    /// SPEC §6.2 (keyboard / mouse / image protocols, layers bitset).
    /// Additional capability fields append after `color_support` using
    /// the same trailing-byte forward-compat trick.
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

    /// `PING` — liveness probe (`SPEC.md` §7.5). The peer MUST echo `nonce`
    /// back in a `PONG` frame.
    Ping {
        /// Opaque nonce echoed by the peer in `PONG`.
        nonce: u64,
    },

    /// `TERMINAL_OUTPUT` — server-to-client terminal content (`SPEC.md` §8.1).
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
        /// Monotonic per-terminal sequence id (`SPEC.md` §12).
        seq: u64,
        /// VT bytes from the PTY (possibly downsampled per
        /// [`crate::caps::ColorSupport`]).
        bytes: Vec<u8>,
    },

    /// `ATTACH` — client requests to attach to a session (`SPEC.md` §13).
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

    /// `DETACH` — client signals clean departure (`SPEC.md` §7.3).
    ///
    /// Carries no fields in the phux-4az scaffold; SPEC §7.3 also keeps it
    /// empty (the `DetachReason` is sent in `DETACHED` from the server).
    Detach,

    /// `INPUT_KEY` — client forwards a structured key event (`SPEC.md` §9.1).
    ///
    /// Wire shape: tagged [`TerminalId`] followed by the encoded [`KeyEvent`].
    InputKey {
        /// Target terminal.
        terminal_id: TerminalId,
        /// Structured key event; libghostty atoms inside.
        event: KeyEvent,
    },

    /// `INPUT_MOUSE` — client forwards a mouse event (`SPEC.md` §9.2).
    InputMouse {
        /// Target terminal.
        terminal_id: TerminalId,
        /// Structured mouse event; coordinates are terminal-local pixels.
        event: MouseEvent,
    },

    /// `INPUT_FOCUS` — client reports focus change on its host window
    /// (`SPEC.md` §9.3).
    InputFocus {
        /// Target terminal.
        terminal_id: TerminalId,
        /// Whether the client window gained or lost focus.
        event: FocusEvent,
    },

    /// `INPUT_PASTE` — client forwards a paste payload (`SPEC.md` §9.4).
    InputPaste {
        /// Target terminal.
        terminal_id: TerminalId,
        /// Paste payload plus trust classification.
        event: PasteEvent,
    },

    /// `FRAME_ACK` — client acknowledges a `TERMINAL_OUTPUT` it has applied
    /// (`SPEC.md` §7.proto.1 / §12.2).
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
    /// size (`SPEC.md` §7.1 / §10.5).
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
    /// (`SPEC.md` §13).
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
    /// (`SPEC.md` §7.3).
    ///
    /// Phux-4az scaffold carries no fields. SPEC §7.3 defines
    /// `{ reason: DetachReason, message: str }`; those land in a follow-up
    /// once the server actually distinguishes shutdown causes.
    Detached,

    /// `TERMINAL_SNAPSHOT` — initial state of a single terminal (`SPEC.md` §8.4).
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

    /// `BELL` — terminal received a bell character (`SPEC.md` §7.6).
    Bell {
        /// Terminal that bell'd.
        terminal_id: TerminalId,
    },

    /// `ERROR` — server-to-client structured error (`SPEC.md` §14).
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
    /// (`SPEC.md` §7.4 / §11.L3).
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
    /// (`SPEC.md` §7.4 / §11.L3).
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
    /// (`SPEC.md` §7.4 / §11.L3).
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
    /// (`SPEC.md` §7.4 / §11.L3).
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
    /// matching `(scope, key)` (`SPEC.md` §7.4).
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
    /// `(scope, key)` was written or deleted (`SPEC.md` §7.4).
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
}

impl FrameKind {
    /// Type discriminant from `SPEC.md` §7.
    #[must_use]
    pub const fn type_byte(&self) -> u8 {
        match self {
            Self::Hello { .. } => TYPE_HELLO,
            Self::Ping { .. } => TYPE_PING,
            Self::TerminalOutput { .. } => TYPE_TERMINAL_OUTPUT,
            Self::Attach { .. } => TYPE_ATTACH,
            Self::Detach => TYPE_DETACH,
            Self::InputKey { .. } => TYPE_INPUT_KEY,
            Self::InputMouse { .. } => TYPE_INPUT_MOUSE,
            Self::InputFocus { .. } => TYPE_INPUT_FOCUS,
            Self::InputPaste { .. } => TYPE_INPUT_PASTE,
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
                // Order: color_support (phux-7lf), layers (phux-4li.2).
                enc.write_u8(client_caps.color_support.as_wire());
                enc.write_u8(client_caps.layers.as_wire());
            }
            Self::Ping { nonce } => {
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
    // libghostty `Action` and `Key` are `#[repr(u32)]`; cast via `as u32` to
    // surface the discriminant. The decoder uses `TryFrom<u32>` (provided by
    // libghostty's `int_enum` derive) to round-trip.
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

fn decode_optional_str<'a>(dec: &mut Decoder<'a>) -> Result<Option<&'a str>, DecodeError> {
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

fn decode_optional_string_list(dec: &mut Decoder<'_>) -> Result<Option<Vec<String>>, DecodeError> {
    let tag = dec.read_u8()?;
    match tag {
        0 => Ok(None),
        1 => {
            let len = dec.read_u32_be()?;
            let len_usize = usize::try_from(len).map_err(|_| DecodeError::LengthOverflow)?;
            let mut out = Vec::with_capacity(len_usize);
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
