//! Pane-state diff: cells, ops, grids, and the canonical diff algorithm.
//!
//! This module defines the protocol's *cell-level* view of terminal state.
//! Every cell is fully resolved — there is no SGR ambiguity in transit
//! (`SPEC.md` §8.2).
//!
//! The public surface:
//!
//! - [`Cell`], [`Color`], [`Underline`], [`CellFlags`] — what a cell *is*.
//! - [`CursorState`], [`CursorShape`], [`PaneModes`] — per-frame cursor and
//!   mode metadata (carried on `PANE_DIFF` as struct fields per SPEC §8.1).
//! - [`Grid`] — a complete pane state at a point in time.
//! - [`DiffOp`] — the wire-level diff operation set (`SPEC.md` §8.3).
//! - [`compute_diff`] / [`PaneDiffResult`] — the canonical algorithm:
//!   `(Grid, Grid) -> (Vec<DiffOp>, CursorState, PaneModes)`.

mod cell;
mod compute;
mod cursor;
mod grid;
mod op;

pub use cell::{
    Cell, CellFlags, Color, ColorDownsample, ColorSupport, PaletteIndex, RgbColor, Underline,
    downsample_color, nearest_xterm_16, nearest_xterm_256, xterm_256_to_rgb,
};
pub use compute::{PaneDiffResult, compute_diff};
pub use cursor::{CursorShape, CursorState, PaneModes};
pub use grid::Grid;
pub use op::DiffOp;
