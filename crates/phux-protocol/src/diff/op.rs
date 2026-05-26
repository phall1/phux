//! Diff operations (`SPEC.md` §8.3).
//!
//! Per SPEC §8.1, cursor state and pane modes are **struct fields** of
//! `PANE_DIFF`, not entries in this op stream. The cursor / modes types live
//! in [`super::cursor`].

use super::cell::Cell;

/// A single diff operation. Applied in order to evolve a pane's grid from one
/// frame to the next.
///
/// Cursor state and pane modes are intentionally **not** members of this enum;
/// SPEC §8.1/§8.5 carry them on `PANE_DIFF` as dedicated struct fields. See
/// [`super::cursor::CursorState`] and [`super::cursor::PaneModes`].
///
/// The set defined here is the minimum needed to round-trip the spike; the
/// remaining variants from `SPEC.md` §8.3 (`Repeat`, `EraseLine`, `ScrollUp`,
/// `ScrollDown`, `Hyperlink`, `Image`) will be added as the implementation
/// requires them.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
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
}
