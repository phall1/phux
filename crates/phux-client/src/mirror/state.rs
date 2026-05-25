//! [`DiffMirror`] — the client-side current-state container.

use phux_protocol::{CursorState, DiffOp, Grid};

/// Client-side mirror of a single pane's protocol-level state.
///
/// Holds the current [`Grid`] (cells + cursor), the last applied frame
/// counter, and exposes [`apply`](DiffMirror::apply) for incremental updates
/// plus [`ingest_snapshot`](DiffMirror::ingest_snapshot) for full-state
/// resets (`SPEC.md` §8.4).
///
/// The `cursor` field on this struct is a convenience mirror of
/// `grid.cursor`; both are kept in sync by [`apply`](DiffMirror::apply) and
/// [`ingest_snapshot`](DiffMirror::ingest_snapshot).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffMirror {
    /// Current pane grid. Renderer reads this directly.
    pub grid: Grid,
    /// Current cursor state. Mirrors `grid.cursor`.
    pub cursor: CursorState,
    /// Monotonic frame counter of the most recently applied update.
    pub frame_id: u64,
}

impl DiffMirror {
    /// Construct a blank mirror of the given dimensions.
    #[must_use]
    pub fn new(rows: u16, cols: u16) -> Self {
        let grid = Grid::blank(rows, cols);
        let cursor = grid.cursor;
        Self {
            grid,
            cursor,
            frame_id: 0,
        }
    }

    /// Apply a sequence of [`DiffOp`] in order to the current state.
    ///
    /// Out-of-bounds writes (a `row` >= `grid.rows`, a `col` past the row
    /// length, or a clear / cell run that extends past the row's last
    /// column) are silently clamped — they would indicate a server bug,
    /// but the mirror keeps the renderer alive rather than panicking.
    pub fn apply(&mut self, ops: &[DiffOp]) {
        super::apply::apply(self, ops);
    }

    /// Replace the entire state with `snap`, resetting `frame_id` to
    /// `frame_id`.
    ///
    /// Used for full-state resets per `SPEC.md` §8.4 (`PANE_SNAPSHOT`).
    pub fn ingest_snapshot(&mut self, snap: &Grid, frame_id: u64) {
        super::snapshot::ingest_snapshot(self, snap, frame_id);
    }
}
