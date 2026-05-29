//! Structured screen projection — the agent surface's read shape
//! (ADR-0022 §2, `phux-oki`).
//!
//! A [`ScreenState`] is a point-in-time projection of one pane's grid as
//! plain data: dims, cursor, and the viewport rows as text. It is the
//! stable JSON contract the CLI emits (`phux snapshot --json`) and the
//! payload the server returns from the side-effect-free `GET_SCREEN`
//! control command.
//!
//! This type lives in `phux-core` (not the server or client) precisely so
//! both ends share one definition: the server *produces* it by walking its
//! own libghostty `Terminal`; the CLI *consumes* it by deserializing the
//! `COMMAND_RESULT` JSON. Keeping it pure data here — no libghostty, no
//! I/O — is what lets the walk run server-side without dragging emulator
//! types across the crate boundary.

use serde::{Deserialize, Serialize};

/// Stable JSON contract version (ADR-0022 §2). Bump on any breaking change
/// to the [`ScreenState`] shape so consumers can pin or branch.
pub const SCHEMA_VERSION: u32 = 1;

/// Cursor position + visibility, viewport-relative, zero-based.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CursorState {
    /// Column, zero-based, viewport-relative.
    pub x: u16,
    /// Row, zero-based, viewport-relative.
    pub y: u16,
    /// Whether the cursor is currently visible (DECTCEM).
    pub visible: bool,
}

/// A point-in-time projection of one pane's grid as structured data.
///
/// The default shape is plain text lines + cursor + dims — what most
/// agents want. Per-cell styles and OSC-133 semantic marks are a future
/// additive field (`--cells`), not a new struct (ADR-0022 §2); scrollback
/// is a future additive field (`--scrollback`, `phux-o1v`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScreenState {
    /// Contract version; see [`SCHEMA_VERSION`].
    pub schema_version: u32,
    /// Wire-local terminal id of the captured pane.
    pub pane: u32,
    /// Grid width in cells.
    pub cols: u16,
    /// Grid height in cells.
    pub rows: u16,
    /// Cursor state, or `None` when the emulator can't resolve a
    /// viewport-resident cursor (e.g. it is in scrollback or hidden).
    pub cursor: Option<CursorState>,
    /// Viewport rows, top to bottom, right-trimmed.
    pub lines: Vec<String>,
}
