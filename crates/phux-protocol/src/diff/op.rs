//! Diff operations (`SPEC.md` §8.3) and cursor state (`SPEC.md` §8.5).

use super::cell::Cell;

/// A single diff operation. Applied in order to evolve a pane's grid from one
/// frame to the next.
///
/// The set defined here is the minimum needed to round-trip the spike; the
/// remaining variants from `SPEC.md` §8.3 (`Repeat`, `EraseLine`, `ScrollUp`,
/// `ScrollDown`, `Hyperlink`, `Image`) will be added as the implementation
/// requires them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiffOp {
    /// Set a contiguous horizontal run of cells.
    ///
    /// `cells.len()` cells starting at `(row, col)` and extending rightward
    /// are replaced. Wrapping past the row's last column is not implied;
    /// callers MUST split runs at row boundaries.
    CellRun {
        /// Zero-based row index.
        row: u16,
        /// Zero-based column index of the first cell.
        col: u16,
        /// Cells to write, in left-to-right order.
        cells: Vec<Cell>,
    },
    /// Clear `count` cells starting at `(row, col)` to blank
    /// (`Cell::blank()`) with default attributes.
    Clear {
        /// Zero-based row index.
        row: u16,
        /// Zero-based column index of the first cleared cell.
        col: u16,
        /// Number of cells to clear.
        count: u16,
    },
    /// Move the cursor to `(row, col)`.
    CursorMove {
        /// Zero-based row index.
        row: u16,
        /// Zero-based column index.
        col: u16,
    },
    /// Update cursor visibility and visual style.
    CursorStyle {
        /// Whether the cursor is currently visible.
        visible: bool,
        /// Cursor shape.
        shape: CursorShape,
        /// Whether the cursor is blinking.
        blink: bool,
    },
}

/// Cursor state, carried with every frame (`SPEC.md` §8.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CursorState {
    /// Zero-based row index.
    pub row: u16,
    /// Zero-based column index.
    pub col: u16,
    /// Whether the cursor is visible.
    pub visible: bool,
    /// Cursor shape.
    pub shape: CursorShape,
    /// Whether the cursor is blinking.
    pub blink: bool,
}

impl Default for CursorState {
    fn default() -> Self {
        Self {
            row: 0,
            col: 0,
            visible: true,
            shape: CursorShape::Block,
            blink: true,
        }
    }
}

/// Cursor shape.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum CursorShape {
    /// `DECSCUSR 1, 2` — block.
    #[default]
    Block = 0,
    /// `DECSCUSR 5, 6` — bar.
    Bar = 1,
    /// `DECSCUSR 3, 4` — underline.
    Underline = 2,
    /// Hollow block.
    BlockHollow = 3,
}
