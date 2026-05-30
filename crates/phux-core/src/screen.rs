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
///
/// `2` adds the additive [`ScreenState::scrollback`] field (`phux-o1v`).
/// The field carries `#[serde(default)]`, so a v1-shaped JSON (no
/// `scrollback` key) still deserializes; the bump is the signal for
/// consumers that want to *produce* or *require* scrollback.
pub const SCHEMA_VERSION: u32 = 2;

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
/// is the additive [`Self::scrollback`] field (`--scrollback`,
/// `phux-o1v`).
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
    /// Scrollback history rows above the viewport, oldest first,
    /// right-trimmed. Populated only when the caller requests it
    /// (`phux snapshot --scrollback[=N]`); empty otherwise (`phux-o1v`).
    ///
    /// `#[serde(default)]` keeps the contract back-compatible: a v1-shaped
    /// JSON without this key deserializes to an empty `Vec`, and a v1
    /// consumer reading a v2 payload simply ignores the extra key.
    #[serde(default)]
    pub scrollback: Vec<String>,
}

#[cfg(test)]
#[allow(clippy::expect_used, reason = "tests")]
mod tests {
    use super::*;

    /// A v1-shaped JSON (no `scrollback` key, `schema_version = 1`) must
    /// still deserialize — the additive field is `#[serde(default)]`, so
    /// older producers stay readable (`phux-o1v` back-compat).
    #[test]
    fn deserializes_v1_json_without_scrollback() {
        let v1 = r#"{
            "schema_version": 1,
            "pane": 3,
            "cols": 80,
            "rows": 2,
            "cursor": null,
            "lines": ["hello", "world"]
        }"#;
        let screen: ScreenState =
            serde_json::from_str(v1).expect("v1 JSON must deserialize into the v2 struct");
        assert_eq!(screen.schema_version, 1);
        assert_eq!(screen.lines, vec!["hello".to_owned(), "world".to_owned()]);
        assert!(
            screen.scrollback.is_empty(),
            "missing scrollback key defaults to empty",
        );
    }

    /// A v2 round-trip carries scrollback through serialize/deserialize.
    #[test]
    fn round_trips_scrollback_field() {
        let original = ScreenState {
            schema_version: SCHEMA_VERSION,
            pane: 1,
            cols: 10,
            rows: 1,
            cursor: None,
            lines: vec!["live".to_owned()],
            scrollback: vec!["old1".to_owned(), "old2".to_owned()],
        };
        let json = serde_json::to_string(&original).expect("serialize");
        let decoded: ScreenState = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(decoded, original);
    }
}
