//! TLV field-ID constants used inside message bodies.
//!
//! Owned by phux-6yl.4. See `SPEC.md` §7 (message catalog) and Appendix A
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

/// `INPUT_KEY`: target `PaneId`.
pub const INPUT_KEY_PANE: u32 = 1;
/// `INPUT_KEY`: physical/logical key code (libghostty `Key`).
pub const INPUT_KEY_KEY: u32 = 2;
/// `INPUT_KEY`: modifier bitset.
pub const INPUT_KEY_MODS: u32 = 3;
/// `INPUT_KEY`: key action (press/release/repeat).
pub const INPUT_KEY_ACTION: u32 = 4;
/// `INPUT_KEY`: optional UTF-8 text produced by the key event.
pub const INPUT_KEY_TEXT: u32 = 5;

// -----------------------------------------------------------------------------
// `PANE_DIFF` / `PANE_SNAPSHOT` — §8
// -----------------------------------------------------------------------------

/// Pane identifier for diff/snapshot frames.
pub const PANE_DIFF_PANE: u32 = 1;
/// Monotonic frame id (`FrameId`).
pub const PANE_DIFF_FRAME_ID: u32 = 2;
/// Encoded `DiffOp` sequence.
pub const PANE_DIFF_OPS: u32 = 3;

// -----------------------------------------------------------------------------
// `VIEWPORT_RESIZE` / `PANE_RESIZED` — §10.5
// -----------------------------------------------------------------------------

/// Target pane id for a resize.
pub const VIEWPORT_RESIZE_PANE: u32 = 1;
/// New column count.
pub const VIEWPORT_RESIZE_COLS: u32 = 2;
/// New row count.
pub const VIEWPORT_RESIZE_ROWS: u32 = 3;

// -----------------------------------------------------------------------------
// `FRAME_ACK` — §12
// -----------------------------------------------------------------------------

/// Acked pane id.
pub const FRAME_ACK_PANE: u32 = 1;
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
