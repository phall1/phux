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
/// `3` adds the additive [`ScreenState::cells`] field (`phux-8yl`). Both
/// fields carry `#[serde(default)]`, so an older-shaped JSON (missing the
/// `scrollback` or `cells` key) still deserializes; the bump is the signal
/// for consumers that want to *produce* or *require* the newer fields.
pub const SCHEMA_VERSION: u32 = 3;

/// A color drawn from a libghostty style attribute, projected to plain data
/// (`phux-8yl`).
///
/// Mirrors libghostty's `style::StyleColor`: a cell's foreground or
/// background is either unset (the terminal default), a palette index
/// (`0..=255`, the 16 ANSI names plus the 256-color cube), or a direct
/// 24-bit RGB triple. Kept as a tagged enum so the JSON distinguishes
/// "default" from "explicitly black", which a flattened RGB cannot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum CellColor {
    /// The terminal default (no explicit color set on the cell).
    #[default]
    Default,
    /// A palette index: `0..=15` are the ANSI names, `16..=255` the
    /// 256-color cube/greyscale ramp.
    Palette {
        /// The palette slot.
        index: u8,
    },
    /// A direct 24-bit truecolor value.
    Rgb {
        /// Red channel.
        r: u8,
        /// Green channel.
        g: u8,
        /// Blue channel.
        b: u8,
    },
}

/// OSC-133 semantic content classification of a cell (`phux-8yl`).
///
/// Set by shell integration via OSC-133 prompt-mark sequences; mirrors the
/// meaningful subset of libghostty's `screen::CellSemanticContent`. Lets an
/// agent tell shell prompt text apart from typed input without re-parsing
/// the screen heuristically.
///
/// The server collapses libghostty's `Output` (which is the *default* for
/// every cell, marked or not) to absence — [`CellInfo::semantic`] is `None`
/// for output and unmarked cells, and `Some` only for [`Self::Input`] /
/// [`Self::Prompt`]. [`Self::Output`] is retained in the enum for
/// forward-compatibility and explicit consumer matching, but the current
/// server never emits it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SemanticContent {
    /// Command output. Never emitted by the current server (collapsed to
    /// `None`); see the type-level note.
    Output,
    /// User-typed input on a command line.
    Input,
    /// Shell prompt text.
    Prompt,
}

/// Per-cell text-style attributes, projected to plain data (`phux-8yl`).
///
/// Mirrors the boolean attribute set of libghostty's `style::Style` plus
/// the resolved foreground/background colors. The SGR `underline` *style*
/// (single/double/curly/…) is intentionally collapsed to a single
/// [`Self::underline`] bool for now — the agent surface cares that a cell
/// is underlined, not which of six dash patterns; the richer enum can land
/// additively later.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "SGR attributes are an inherent bitset of independent flags, \
              mirroring libghostty's own `style::Style`; folding them into \
              two-variant enums would obscure the 1:1 mapping to SGR codes \
              and the JSON shape without buying anything"
)]
pub struct CellStyle {
    /// Bold (SGR 1).
    pub bold: bool,
    /// Faint / dim (SGR 2).
    pub faint: bool,
    /// Italic (SGR 3).
    pub italic: bool,
    /// Underlined (any SGR 4 variant).
    pub underline: bool,
    /// Blink (SGR 5).
    pub blink: bool,
    /// Inverse / reverse video (SGR 7).
    pub inverse: bool,
    /// Invisible / concealed (SGR 8).
    pub invisible: bool,
    /// Strikethrough (SGR 9).
    pub strikethrough: bool,
    /// Overline (SGR 53).
    pub overline: bool,
    /// Foreground color.
    pub fg: CellColor,
    /// Background color.
    pub bg: CellColor,
}

/// One cell's semantic + style projection (`phux-8yl`).
///
/// Cells are emitted in row-major order, skipping the right half of
/// double-width glyphs (libghostty's `SpacerTail`) — so a given `(row,
/// col)` appears at most once, and the base glyph carries the `(row, col)`
/// of its left edge. Blank cells are *not* emitted: the [`CellInfo`] vec is
/// sparse, carrying only cells with a non-default style or a semantic mark,
/// which keeps the JSON small for a mostly-empty grid.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CellInfo {
    /// Zero-based column, viewport-relative.
    pub col: u16,
    /// Zero-based row, viewport-relative.
    pub row: u16,
    /// OSC-133 semantic content, when the shell marked it; `None`
    /// otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub semantic: Option<SemanticContent>,
    /// Text-style attributes for the cell.
    pub style: CellStyle,
}

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
/// agents want. Per-cell styles and OSC-133 semantic marks ride the
/// additive [`Self::cells`] field (`--cells`, `phux-8yl`), not a new
/// struct (ADR-0022 §2); scrollback is the additive [`Self::scrollback`]
/// field (`--scrollback`, `phux-o1v`).
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
    /// Per-cell semantic marks + styles for the viewport, or `None` when
    /// the caller did not request them (`phux snapshot --cells`,
    /// `phux-8yl`). When `Some`, the vec is sparse: only cells carrying a
    /// non-default style or an OSC-133 semantic mark are emitted, in
    /// row-major order. See [`CellInfo`].
    ///
    /// `#[serde(default)]` plus `skip_serializing_if` keeps the contract
    /// back-compatible: a JSON without this key deserializes to `None`,
    /// and the common `cells = None` snapshot serializes to exactly the
    /// pre-`phux-8yl` shape (no `cells` key at all).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cells: Option<Vec<CellInfo>>,
}

/// Stable JSON contract version for [`RenderedFrame`] (`phux-l5xa`).
///
/// Independent of [`SCHEMA_VERSION`] (the per-pane [`ScreenState`] contract):
/// the composited-frame projection is a different shape with its own
/// evolution. Bump on any breaking change to [`RenderedFrame`].
pub const RENDERED_SCHEMA_VERSION: u32 = 1;

/// One cell of the client's composited frame (`phux-l5xa`).
///
/// Unlike [`CellInfo`] (sparse, per-pane, carries only non-default cells)
/// this is a *dense* cell: every column of the assembled frame has exactly
/// one, so a consumer can index `cells[row * cols + col]` and read the glyph
/// and style the human's glass actually shows — pane content, dividers, and
/// status bar alike, already composited.
///
/// `grapheme` is the cell's grapheme cluster:
/// * a normal glyph (`"a"`, `"世"`, a ZWJ emoji sequence) for a drawn cell;
/// * a single space (`" "`) for a blank cell;
/// * the empty string (`""`) for the right half of a double-width glyph
///   (libghostty's `SpacerTail`) — the preceding cell's wide glyph already
///   occupies this column, so emitting no glyph here preserves exact widths.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RenderedCell {
    /// The cell's grapheme cluster; `" "` when blank, `""` for a wide-glyph
    /// tail. See the type-level note.
    pub grapheme: String,
    /// Resolved text-style attributes for the cell.
    pub style: CellStyle,
}

/// The client's composited multi-pane view, as structured dense cells
/// (`phux snapshot --rendered`, `phux-l5xa`).
///
/// Where [`ScreenState`] projects a single server-side pane grid, this
/// projects the **assembled frame** the client renders: layout tiling,
/// dividers, and the status bar, composited exactly as painted to the
/// terminal — but returned as cells (grapheme + style + cursor) rather than
/// VT bytes, so an agent, a test, or an assistant debugging a render bug can
/// ask "what does the screen look like right now" and get an answer with no
/// external emulator in the loop (closing the symmetric-blindspot gap that
/// forced pyte before).
///
/// Cells are dense and row-major: `cells.len() == cols as usize * rows as
/// usize`, and the cell at `(row, col)` is `cells[row * cols + col]`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RenderedFrame {
    /// Contract version; see [`RENDERED_SCHEMA_VERSION`].
    pub schema_version: u32,
    /// Composited frame width in cells.
    pub cols: u16,
    /// Composited frame height in cells.
    pub rows: u16,
    /// The composited cursor (whichever pane's cursor the end-of-frame
    /// policy elects), or `None` when no pane contributes a visible
    /// viewport cursor.
    pub cursor: Option<CursorState>,
    /// Dense, row-major cells of the assembled frame. Length is exactly
    /// `cols * rows`; index `(row, col)` as `cells[row * cols + col]`.
    pub cells: Vec<RenderedCell>,
}

impl RenderedFrame {
    /// Build a blank frame of `cols * rows` space cells with the default
    /// style and no cursor — the canvas the compositor fills.
    #[must_use]
    pub fn blank(cols: u16, rows: u16) -> Self {
        let len = usize::from(cols) * usize::from(rows);
        Self {
            schema_version: RENDERED_SCHEMA_VERSION,
            cols,
            rows,
            cursor: None,
            cells: vec![
                RenderedCell {
                    grapheme: " ".to_owned(),
                    style: CellStyle::default(),
                };
                len
            ],
        }
    }

    /// Mutable access to the cell at `(row, col)`, or `None` when the
    /// coordinate is outside the frame.
    pub fn cell_mut(&mut self, row: u16, col: u16) -> Option<&mut RenderedCell> {
        if row >= self.rows || col >= self.cols {
            return None;
        }
        let idx = usize::from(row) * usize::from(self.cols) + usize::from(col);
        self.cells.get_mut(idx)
    }

    /// The cell at `(row, col)`, or `None` when out of range.
    #[must_use]
    pub fn cell(&self, row: u16, col: u16) -> Option<&RenderedCell> {
        if row >= self.rows || col >= self.cols {
            return None;
        }
        let idx = usize::from(row) * usize::from(self.cols) + usize::from(col);
        self.cells.get(idx)
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, reason = "tests")]
mod tests {
    use super::*;

    /// A v1-shaped JSON (no `scrollback`/`cells` keys, `schema_version =
    /// 1`) must still deserialize — both additive fields are
    /// `#[serde(default)]`, so older producers stay readable (`phux-o1v` /
    /// `phux-8yl` back-compat).
    #[test]
    fn deserializes_v1_json_without_scrollback_or_cells() {
        let v1 = r#"{
            "schema_version": 1,
            "pane": 3,
            "cols": 80,
            "rows": 2,
            "cursor": null,
            "lines": ["hello", "world"]
        }"#;
        let screen: ScreenState =
            serde_json::from_str(v1).expect("v1 JSON must deserialize into the current struct");
        assert_eq!(screen.schema_version, 1);
        assert_eq!(screen.lines, vec!["hello".to_owned(), "world".to_owned()]);
        assert!(
            screen.scrollback.is_empty(),
            "missing scrollback key defaults to empty",
        );
        assert!(screen.cells.is_none(), "missing cells key defaults to None",);
    }

    /// A round-trip carries scrollback through serialize/deserialize.
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
            cells: None,
        };
        let json = serde_json::to_string(&original).expect("serialize");
        let decoded: ScreenState = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(decoded, original);
    }

    /// A `cells = None` snapshot must serialize to exactly the pre-cells
    /// shape: no `cells` key at all (`skip_serializing_if`), so a consumer
    /// pinned to the older schema sees no surprise field (`phux-8yl`).
    #[test]
    fn omits_cells_key_when_none() {
        let screen = ScreenState {
            schema_version: SCHEMA_VERSION,
            pane: 1,
            cols: 2,
            rows: 1,
            cursor: None,
            lines: vec!["hi".to_owned()],
            scrollback: Vec::new(),
            cells: None,
        };
        let json = serde_json::to_string(&screen).expect("serialize");
        assert!(
            !json.contains("\"cells\""),
            "None cells must not emit a key, got: {json}",
        );
    }

    /// A populated `cells` field round-trips, including the semantic mark
    /// and the tagged color enum (`phux-8yl`).
    #[test]
    fn round_trips_cells_field() {
        let original = ScreenState {
            schema_version: SCHEMA_VERSION,
            pane: 2,
            cols: 4,
            rows: 1,
            cursor: None,
            lines: vec!["$ ls".to_owned()],
            scrollback: Vec::new(),
            cells: Some(vec![
                CellInfo {
                    col: 0,
                    row: 0,
                    semantic: Some(SemanticContent::Prompt),
                    style: CellStyle {
                        bold: true,
                        faint: false,
                        italic: false,
                        underline: false,
                        blink: false,
                        inverse: false,
                        invisible: false,
                        strikethrough: false,
                        overline: false,
                        fg: CellColor::Rgb { r: 1, g: 2, b: 3 },
                        bg: CellColor::Default,
                    },
                },
                CellInfo {
                    col: 2,
                    row: 0,
                    semantic: Some(SemanticContent::Input),
                    style: CellStyle {
                        bold: false,
                        faint: false,
                        italic: false,
                        underline: false,
                        blink: false,
                        inverse: false,
                        invisible: false,
                        strikethrough: false,
                        overline: false,
                        fg: CellColor::Palette { index: 7 },
                        bg: CellColor::Default,
                    },
                },
            ]),
        };
        let json = serde_json::to_string(&original).expect("serialize");
        let decoded: ScreenState = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(decoded, original);
    }

    /// `RenderedFrame::blank` allocates `cols * rows` dense space cells with
    /// the default style; `cell`/`cell_mut` index row-major and bound-check
    /// (`phux-l5xa`).
    #[test]
    fn rendered_frame_blank_is_dense_and_indexes_row_major() {
        let mut f = RenderedFrame::blank(3, 2);
        assert_eq!(f.schema_version, RENDERED_SCHEMA_VERSION);
        assert_eq!(f.cells.len(), 6);
        assert_eq!(f.cell(1, 2).expect("in range").grapheme, " ");
        assert_eq!(f.cell(1, 2).expect("in range").style, CellStyle::default());
        assert!(f.cell(2, 0).is_none(), "row past the frame is None");
        assert!(f.cell(0, 3).is_none(), "col past the frame is None");
        f.cell_mut(1, 2).expect("in range").grapheme = "X".to_owned();
        assert_eq!(f.cell(1, 2).expect("in range").grapheme, "X");
        // Row-major: (row 1, col 2) is index 1*3 + 2 = 5.
        assert_eq!(f.cells[5].grapheme, "X");
        assert!(f.cell_mut(2, 0).is_none(), "out-of-range mut is None");
    }

    /// A `RenderedFrame` survives a JSON round-trip, cursor and all
    /// (`phux-l5xa`).
    #[test]
    fn rendered_frame_json_round_trips() {
        let mut f = RenderedFrame::blank(2, 1);
        f.cell_mut(0, 1).expect("in range").grapheme = "Z".to_owned();
        f.cursor = Some(CursorState {
            x: 1,
            y: 0,
            visible: true,
        });
        let json = serde_json::to_string(&f).expect("serialize");
        let back: RenderedFrame = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(f, back);
    }
}
