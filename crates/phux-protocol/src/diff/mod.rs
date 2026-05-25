//! Pane-state diff: cells, ops, grids, and the canonical diff algorithm.
//!
//! This module defines the protocol's *cell-level* view of terminal state.
//! Every cell is fully resolved — there is no SGR ambiguity in transit
//! (`SPEC.md` §8.2).
//!
//! The public surface:
//!
//! - [`Cell`], [`Color`], [`Underline`], [`CellFlags`] — what a cell *is*.
//! - [`CursorState`], [`CursorShape`] — cursor metadata.
//! - [`Grid`] — a complete pane state at a point in time.
//! - [`DiffOp`] — the wire-level diff operation set (`SPEC.md` §8.3).
//! - [`compute_diff`] — the canonical algorithm: `(Grid, Grid) -> Vec<DiffOp>`.

mod cell;
mod compute;
mod grid;
mod op;

pub use cell::{Cell, CellFlags, Color, PaletteIndex, RgbColor, Underline};
pub use compute::compute_diff;
pub use grid::Grid;
pub use op::{CursorShape, CursorState, DiffOp};
