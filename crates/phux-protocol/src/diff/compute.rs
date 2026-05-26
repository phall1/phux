//! Canonical diff algorithm: `(Grid, Grid) -> PaneDiffResult`.
//!
//! Per SPEC §8.1, a `PANE_DIFF` carries `ops`, `cursor`, and `modes` as
//! separate fields — the cursor is **not** in the op stream. This module
//! returns all three together via [`PaneDiffResult`].
//!
//! The op-emission pass is intentionally simple. It scans row-by-row,
//! identifies maximal runs of changed cells, and emits one `CellRun` per run.
//! It does not yet detect scroll regions, run-length-encode identical cells,
//! or coalesce attribute-only changes. Those optimizations land as needed and
//! are observable as fewer bytes on the wire, not as different rendered
//! output — the algorithm's contract is correctness, not minimality.
//!
//! When grid dimensions differ between `prev` and `next`, this function
//! treats the change as a full repaint: it emits a single `Clear` covering
//! a generous span followed by `CellRun`s for the entire new grid. Real
//! servers will instead emit a `PANE_SNAPSHOT` per `SPEC.md` §8.4; that is
//! a wire-level distinction and not this function's concern.

use super::cell::Cell;
use super::cursor::{CursorState, PaneModes};
use super::grid::Grid;
use super::op::DiffOp;

/// Output of [`compute_diff`]: the op stream plus the cursor and mode state
/// that travel alongside it on `PANE_DIFF` per SPEC §8.1.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneDiffResult {
    /// Cell-level ops produced by the diff.
    pub ops: Vec<DiffOp>,
    /// Cursor state at the end of the new frame.
    pub cursor: CursorState,
    /// Pane-wide modes at the end of the new frame.
    pub modes: PaneModes,
}

/// Compute the diff that transforms `prev` into `next`.
///
/// The returned [`PaneDiffResult::ops`] applied in order to `prev` MUST
/// reproduce `next`'s cell grid. The cursor and modes are reported as
/// terminal-state fields (per SPEC §8.5), not as ops.
///
/// `modes` defaults to [`PaneModes::EMPTY`] for now — the `Grid` type does
/// not yet carry mode state. Once libghostty mode tracking lands on the
/// server side, callers will populate `modes` from there and this function
/// will surface it directly.
#[must_use]
pub fn compute_diff(prev: &Grid, next: &Grid) -> PaneDiffResult {
    let mut ops = Vec::new();

    // Dimensions differ → emit a full repaint. Wire-level callers should
    // emit a `PANE_SNAPSHOT` instead; this function returns a faithful
    // diff regardless.
    if prev.rows != next.rows || prev.cols != next.cols {
        ops.push(DiffOp::Clear {
            row: 0,
            col: 0,
            count: u16::MAX,
        });
        push_full_repaint(&mut ops, next);
        return PaneDiffResult {
            ops,
            cursor: next.cursor,
            modes: PaneModes::EMPTY,
        };
    }

    for (row_idx, (prev_row, next_row)) in prev.cells.iter().zip(next.cells.iter()).enumerate() {
        let row_u16 = u16::try_from(row_idx).unwrap_or(u16::MAX);
        let mut col = 0usize;
        while col < next_row.len() {
            if cells_eq(prev_row.get(col), next_row.get(col)) {
                col += 1;
                continue;
            }
            let start = col;
            while col < next_row.len() && !cells_eq(prev_row.get(col), next_row.get(col)) {
                col += 1;
            }
            let run: Vec<Cell> = next_row[start..col].to_vec();
            let start_u16 = u16::try_from(start).unwrap_or(u16::MAX);
            // If every cell in the run is blank, emit a Clear (fewer bytes
            // on the wire long-term, even though both are correct).
            if run.iter().all(Cell::is_blank) {
                let count = u16::try_from(run.len()).unwrap_or(u16::MAX);
                ops.push(DiffOp::Clear {
                    row: row_u16,
                    col: start_u16,
                    count,
                });
            } else {
                ops.push(DiffOp::CellRun {
                    row: row_u16,
                    col: start_u16,
                    cells: run,
                });
            }
        }
    }

    PaneDiffResult {
        ops,
        cursor: next.cursor,
        modes: PaneModes::EMPTY,
    }
}

fn cells_eq(a: Option<&Cell>, b: Option<&Cell>) -> bool {
    match (a, b) {
        (Some(x), Some(y)) => x == y,
        (None, None) => true,
        _ => false,
    }
}

fn push_full_repaint(ops: &mut Vec<DiffOp>, grid: &Grid) {
    for (row_idx, row) in grid.cells.iter().enumerate() {
        let row_u16 = u16::try_from(row_idx).unwrap_or(u16::MAX);
        if row.iter().all(Cell::is_blank) {
            continue;
        }
        ops.push(DiffOp::CellRun {
            row: row_u16,
            col: 0,
            cells: row.clone(),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff::cell::{Color, RgbColor};

    fn cell_with_text(s: &str) -> Cell {
        Cell {
            text: s.chars().collect(),
            ..Cell::blank()
        }
    }

    #[test]
    fn diff_of_equal_grids_has_no_ops() {
        let g = Grid::blank(3, 5);
        let result = compute_diff(&g, &g);
        assert!(result.ops.is_empty());
        assert_eq!(result.cursor, g.cursor);
        assert_eq!(result.modes, PaneModes::EMPTY);
    }

    #[test]
    fn diff_of_single_changed_cell_emits_one_op() {
        let prev = Grid::blank(2, 5);
        let mut next = prev.clone();
        next.cells[0][2] = cell_with_text("x");
        let result = compute_diff(&prev, &next);
        assert_eq!(result.ops.len(), 1);
        match &result.ops[0] {
            DiffOp::CellRun { row, col, cells } => {
                assert_eq!((*row, *col), (0, 2));
                assert_eq!(cells.len(), 1);
                assert_eq!(&cells[0].text[..], &['x'][..]);
            }
            other => panic!("expected CellRun, got {other:?}"),
        }
    }

    #[test]
    fn diff_collapses_adjacent_changes_into_one_run() {
        let prev = Grid::blank(1, 10);
        let mut next = prev.clone();
        for (i, ch) in "hello".chars().enumerate() {
            next.cells[0][i] = cell_with_text(&ch.to_string());
        }
        let result = compute_diff(&prev, &next);
        assert_eq!(result.ops.len(), 1);
        match &result.ops[0] {
            DiffOp::CellRun { row, col, cells } => {
                assert_eq!((*row, *col), (0, 0));
                assert_eq!(cells.len(), 5);
            }
            other => panic!("expected CellRun, got {other:?}"),
        }
    }

    #[test]
    fn blank_run_becomes_clear_not_cellrun() {
        // Start with a row of "x"; clear back to blank.
        let mut prev = Grid::blank(1, 5);
        for i in 0..5 {
            prev.cells[0][i] = cell_with_text("x");
        }
        let next = Grid::blank(1, 5);
        let result = compute_diff(&prev, &next);
        assert_eq!(result.ops.len(), 1);
        match &result.ops[0] {
            DiffOp::Clear { row, col, count } => {
                assert_eq!((*row, *col, *count), (0, 0, 5));
            }
            other => panic!("expected Clear, got {other:?}"),
        }
    }

    #[test]
    fn cursor_move_does_not_emit_an_op() {
        // Per SPEC §8.1/§8.5: cursor changes ride on the PANE_DIFF struct,
        // not in the op stream. The diff for a pure cursor move is empty.
        let prev = Grid::blank(3, 3);
        let mut next = prev.clone();
        next.cursor.col = 2;
        let result = compute_diff(&prev, &next);
        assert!(result.ops.is_empty());
        assert_eq!(result.cursor.col, 2);
        assert_eq!(result.cursor.row, 0);
    }

    #[test]
    fn color_change_is_a_diff() {
        let prev = Grid::blank(1, 1);
        let mut next = prev.clone();
        next.cells[0][0] = Cell {
            fg: Color::Rgb(RgbColor { r: 255, g: 0, b: 0 }),
            ..Cell::blank()
        };
        let result = compute_diff(&prev, &next);
        assert_eq!(result.ops.len(), 1);
        // It's a CellRun because the cell isn't "blank" — fg is non-default.
        assert!(matches!(result.ops[0], DiffOp::CellRun { .. }));
    }
}
