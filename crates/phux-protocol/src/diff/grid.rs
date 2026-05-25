//! The grid: a complete pane state at a point in time.
//!
//! `Grid` is the protocol-level view of pane state — what diffs are computed
//! against. It is independent of any terminal-emulator library; producers
//! (server-side from libghostty-vt, client-side from accumulated diffs) build
//! it the same way.

use super::cell::Cell;
use super::op::CursorState;

/// A complete pane state at a point in time.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Grid {
    /// Width in cells.
    pub cols: u16,
    /// Height in cells.
    pub rows: u16,
    /// Row-major cell buffer. `cells[row][col]`. Always `rows` long; each
    /// row is always `cols` long.
    pub cells: Vec<Vec<Cell>>,
    /// Cursor position and style.
    pub cursor: CursorState,
}

impl Grid {
    /// Construct an empty `rows × cols` grid filled with blank cells.
    #[must_use]
    pub fn blank(rows: u16, cols: u16) -> Self {
        let cells = (0..rows)
            .map(|_| (0..cols).map(|_| Cell::blank()).collect())
            .collect();
        Self {
            cols,
            rows,
            cells,
            cursor: CursorState::default(),
        }
    }

    /// Total cell count, for sanity checks.
    #[must_use]
    pub fn len(&self) -> usize {
        usize::from(self.rows) * usize::from(self.cols)
    }

    /// True if `rows == 0` or `cols == 0`.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.rows == 0 || self.cols == 0
    }

    /// True if every cell is blank.
    #[must_use]
    pub fn is_all_blank(&self) -> bool {
        self.cells.iter().flatten().all(Cell::is_blank)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blank_grid_has_correct_shape() {
        let g = Grid::blank(5, 10);
        assert_eq!(g.rows, 5);
        assert_eq!(g.cols, 10);
        assert_eq!(g.cells.len(), 5);
        assert!(g.cells.iter().all(|r| r.len() == 10));
        assert!(g.is_all_blank());
    }
}
