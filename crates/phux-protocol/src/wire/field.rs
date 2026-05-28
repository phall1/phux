//! TLV field-ID constants used inside message bodies.
//!
//! Owned by phux-6yl.4. See `docs/spec/proto.md` §7 (message catalog) and Appendix A
//! (encoding primitives). Field IDs are stable within a major protocol
//! version; additive minor-version changes append IDs but MUST NOT renumber
//! existing ones.

// -----------------------------------------------------------------------------
// `HELLO` / `HELLO_OK` — §6.1
// -----------------------------------------------------------------------------

/// `HELLO`: list of `VersionRange` the client supports.
pub const HELLO_VERSIONS: u32 = 1;
/// `HELLO`: `ClientCapabilities` blob.
pub const HELLO_CLIENT_CAPS: u32 = 2;

/// `HELLO_OK`: selected `Version`.
pub const HELLO_OK_VERSION: u32 = 1;
/// `HELLO_OK`: `ServerCapabilities` blob.
pub const HELLO_OK_SERVER_CAPS: u32 = 2;
/// `HELLO_OK`: opaque server identity bytes.
pub const HELLO_OK_SERVER_ID: u32 = 3;

// -----------------------------------------------------------------------------
// `PING` / `PONG` — §7.5
// -----------------------------------------------------------------------------

/// `PING` / `PONG`: nonce echoed back by the peer.
pub const PING_NONCE: u32 = 1;
/// `PING` / `PONG`: nonce echoed back by the peer (alias for symmetry).
pub const PONG_NONCE: u32 = 1;

// -----------------------------------------------------------------------------
// `INPUT_KEY` — §9.1
// -----------------------------------------------------------------------------

/// `INPUT_KEY`: target `TerminalId`.
pub const INPUT_KEY_TERMINAL: u32 = 1;
/// `INPUT_KEY`: physical/logical key code (libghostty `Key`).
pub const INPUT_KEY_KEY: u32 = 2;
/// `INPUT_KEY`: modifier bitset.
pub const INPUT_KEY_MODS: u32 = 3;
/// `INPUT_KEY`: key action (press/release/repeat).
pub const INPUT_KEY_ACTION: u32 = 4;
/// `INPUT_KEY`: optional UTF-8 text produced by the key event.
pub const INPUT_KEY_TEXT: u32 = 5;

// -----------------------------------------------------------------------------
// `INPUT_MOUSE` — §9.2
// -----------------------------------------------------------------------------

/// `INPUT_MOUSE`: target `TerminalId`.
pub const INPUT_MOUSE_TERMINAL: u32 = 1;
/// `INPUT_MOUSE`: action (press/release/motion) — libghostty `mouse::Action`.
pub const INPUT_MOUSE_ACTION: u32 = 2;
/// `INPUT_MOUSE`: button identity — libghostty `mouse::Button`.
pub const INPUT_MOUSE_BUTTON: u32 = 3;
/// `INPUT_MOUSE`: modifier bitset at event time.
pub const INPUT_MOUSE_MODS: u32 = 4;
/// `INPUT_MOUSE`: pane-local pixel `x` (f64, SPEC §9.2.1).
pub const INPUT_MOUSE_X: u32 = 5;
/// `INPUT_MOUSE`: pane-local pixel `y` (f64, SPEC §9.2.1).
pub const INPUT_MOUSE_Y: u32 = 6;

// -----------------------------------------------------------------------------
// `INPUT_FOCUS` — §9.3
// -----------------------------------------------------------------------------

/// `INPUT_FOCUS`: target `TerminalId`.
pub const INPUT_FOCUS_TERMINAL: u32 = 1;
/// `INPUT_FOCUS`: focus kind (gained=0, lost=1).
pub const INPUT_FOCUS_KIND: u32 = 2;

// -----------------------------------------------------------------------------
// `INPUT_PASTE` — §9.4
// -----------------------------------------------------------------------------

/// `INPUT_PASTE`: target `TerminalId`.
pub const INPUT_PASTE_TERMINAL: u32 = 1;
/// `INPUT_PASTE`: trust classification (0=untrusted, 1=trusted).
pub const INPUT_PASTE_TRUST: u32 = 2;
/// `INPUT_PASTE`: raw payload bytes.
pub const INPUT_PASTE_DATA: u32 = 3;

// -----------------------------------------------------------------------------
// `ATTACH` / `ATTACHED` / `DETACH` / `DETACHED` / `TERMINAL_SNAPSHOT` —
//   §7.1-§7.3, §8.4, §13. Field IDs are positional-codec-anticipatory
//   (unused today); TLV migration is tracked in phux-i58.
// -----------------------------------------------------------------------------

/// `ATTACH`: `AttachTarget` tagged union (SPEC §13).
pub const ATTACH_TARGET: u32 = 1;
/// `ATTACH`: `ViewportInfo { cols, rows, pixel_w?, pixel_h? }` (SPEC §13).
pub const ATTACH_VIEWPORT: u32 = 2;
/// `ATTACH`: `request_scrollback: bool` (SPEC §13).
pub const ATTACH_REQUEST_SCROLLBACK: u32 = 3;
/// `ATTACH`: `scrollback_limit_lines: u32` (SPEC §13).
pub const ATTACH_SCROLLBACK_LIMIT_LINES: u32 = 4;

/// `ATTACHED`: full `SessionSnapshot` (SPEC §13).
pub const ATTACHED_SNAPSHOT: u32 = 1;
/// `ATTACHED`: server-allocated `ClientId` for this attachment (SPEC §13).
pub const ATTACHED_INITIAL_CLIENT_ID: u32 = 2;

// `DETACH` and `DETACHED` are unit messages in the phux-4az scaffold;
// `DETACHED { reason, message }` from SPEC §7.3 lands in a follow-up.

// -----------------------------------------------------------------------------
// `TERMINAL_SNAPSHOT` body — §8.4 (separate frame per SPEC §13's attach sequence).
// -----------------------------------------------------------------------------

/// `TERMINAL_SNAPSHOT`: target `TerminalId`.
pub const TERMINAL_SNAPSHOT_TERMINAL: u32 = 1;
/// `TERMINAL_SNAPSHOT`: grid columns.
pub const TERMINAL_SNAPSHOT_COLS: u32 = 2;
/// `TERMINAL_SNAPSHOT`: grid rows.
pub const TERMINAL_SNAPSHOT_ROWS: u32 = 3;
/// `TERMINAL_SNAPSHOT`: opening sequence of `DiffOp` against a blank grid.
pub const TERMINAL_SNAPSHOT_OPS: u32 = 4;

// -----------------------------------------------------------------------------
// `BELL` — §7.6
// -----------------------------------------------------------------------------

/// `BELL`: terminal that received the bell character.
pub const BELL_TERMINAL: u32 = 1;

// -----------------------------------------------------------------------------
// `PANE_DIFF` / `TERMINAL_SNAPSHOT` — §8
// -----------------------------------------------------------------------------

/// Pane identifier for diff/snapshot frames.
pub const PANE_DIFF_PANE: u32 = 1;
/// Monotonic frame id (`FrameId`).
pub const PANE_DIFF_FRAME_ID: u32 = 2;
/// Encoded `DiffOp` sequence.
pub const PANE_DIFF_OPS: u32 = 3;
/// Base frame id this diff applies on top of (`docs/spec/L1.md` §2.1).
pub const PANE_DIFF_BASE_FRAME_ID: u32 = 4;
/// `CursorState` carried with every diff (`docs/spec/L1.md` §2.5).
pub const PANE_DIFF_CURSOR: u32 = 5;
/// `PaneModes` bitset carried with every diff (`docs/spec/L1.md` §2.5).
pub const PANE_DIFF_MODES: u32 = 6;
/// Revision tag, reserved for SPEC §8.1 compression schemes (`0` today).
pub const PANE_DIFF_REVISION: u32 = 7;

// -----------------------------------------------------------------------------
// `VIEWPORT_RESIZE` / `TERMINAL_RESIZED` — §10.5
// -----------------------------------------------------------------------------

/// Target terminal id for a resize.
pub const VIEWPORT_RESIZE_TERMINAL: u32 = 1;
/// New column count.
pub const VIEWPORT_RESIZE_COLS: u32 = 2;
/// New row count.
pub const VIEWPORT_RESIZE_ROWS: u32 = 3;

// -----------------------------------------------------------------------------
// `FRAME_ACK` — §12
// -----------------------------------------------------------------------------

/// Acked terminal id.
pub const FRAME_ACK_TERMINAL: u32 = 1;
/// Acked frame id.
pub const FRAME_ACK_FRAME_ID: u32 = 2;

// -----------------------------------------------------------------------------
// `ERROR` — §14
// -----------------------------------------------------------------------------

/// Error code discriminant.
pub const ERROR_CODE: u32 = 1;
/// Human-readable error message.
pub const ERROR_MESSAGE: u32 = 2;

// -----------------------------------------------------------------------------
// `SessionId` tagged union — ADR-0007 §3
// -----------------------------------------------------------------------------

/// `SessionId::Local` tag.
pub const SESSION_ID_TAG_LOCAL: u32 = 0;
/// `SessionId::Satellite` tag (reserved for v0.2+; decoders MUST reject).
pub const SESSION_ID_TAG_SATELLITE: u32 = 1;

// -----------------------------------------------------------------------------
// `TerminalId` tagged union — ADR-0016 §Decision (phux-vp0.4)
// -----------------------------------------------------------------------------
//
// The wire-side tag bytes (`u8`) live in `crate::ids` alongside the
// [`TerminalId`](crate::ids::TerminalId) definition. They are re-exported here
// so call sites that already import `wire::field::*` for TLV constants can
// spell the tags without an extra `use`. This is a re-export, not a renumber.

/// Wire tag byte for `TerminalId::Local` (see [`crate::ids::TERMINAL_ID_TAG_LOCAL`]).
pub const TERMINAL_ID_TAG_LOCAL: u8 = crate::ids::TERMINAL_ID_TAG_LOCAL;
/// Wire tag byte for `TerminalId::Satellite` (see [`crate::ids::TERMINAL_ID_TAG_SATELLITE`]).
pub const TERMINAL_ID_TAG_SATELLITE: u8 = crate::ids::TERMINAL_ID_TAG_SATELLITE;
