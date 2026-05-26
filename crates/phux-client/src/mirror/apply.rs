//! [`DiffOp`] application to a [`DiffMirror`].
//!
//! Mirrors the semantics of the inverse of `phux_protocol::compute_diff`. The
//! replay invariant (see [`super`]) depends on these two staying in lockstep.
//!
//! Per SPEC §8.1/§8.5, cursor state and pane modes are NOT in the op stream;
//! callers update those via [`DiffMirror::apply_frame`] using the cursor and
//! modes fields carried by the `PANE_DIFF` frame.

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
        // `DiffOp` is `#[non_exhaustive]`; future additive variants are
        // silently dropped here until apply support lands for them. This
        // matches the spec's "tolerate unknown trailing fields" guidance
        // (SPEC §16) at the op-stream level.
        _ => {}
    }
}
