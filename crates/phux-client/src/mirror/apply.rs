//! [`DiffOp`] application to a [`DiffMirror`].
//!
//! Mirrors the semantics of `phux-server/examples/diff_spike.rs::apply_diff`
//! and the inverse of `phux_protocol::compute_diff`. The replay invariant
//! (see [`super`]) depends on these two staying in lockstep.

use phux_protocol::{Cell, DiffOp};

use super::state::DiffMirror;

/// Apply `ops` to `state` in order. Public via
/// [`DiffMirror::apply`](super::DiffMirror::apply).
pub fn apply(state: &mut DiffMirror, ops: &[DiffOp]) {
    for op in ops {
        apply_one(state, op);
    }
}

fn apply_one(state: &mut DiffMirror, op: &DiffOp) {
    match op {
        DiffOp::CellRun { row, col, cells } => {
            let row_idx = usize::from(*row);
            let col_idx = usize::from(*col);
            if let Some(target_row) = state.grid.cells.get_mut(row_idx) {
                for (i, cell) in cells.iter().enumerate() {
                    if let Some(slot) = target_row.get_mut(col_idx + i) {
                        *slot = cell.clone();
                    } else {
                        // Past the end of the row — silently clamp; the
                        // canonical algorithm never emits a CellRun that
                        // wraps a row, so this only fires on malformed
                        // input.
                        break;
                    }
                }
            }
        }
        DiffOp::Clear { row, col, count } => {
            let row_idx = usize::from(*row);
            let col_start = usize::from(*col);
            let count_n = usize::from(*count);
            if let Some(target_row) = state.grid.cells.get_mut(row_idx) {
                for slot in target_row.iter_mut().skip(col_start).take(count_n) {
                    *slot = Cell::blank();
                }
            }
        }
        DiffOp::CursorMove { row, col } => {
            state.grid.cursor.row = *row;
            state.grid.cursor.col = *col;
            state.cursor.row = *row;
            state.cursor.col = *col;
        }
        DiffOp::CursorStyle {
            visible,
            shape,
            blink,
        } => {
            state.grid.cursor.visible = *visible;
            state.grid.cursor.shape = *shape;
            state.grid.cursor.blink = *blink;
            state.cursor.visible = *visible;
            state.cursor.shape = *shape;
            state.cursor.blink = *blink;
        }
    }
}
