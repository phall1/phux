//! [`DiffMirror`] — the client-side current-state container.

use phux_protocol::{CursorState, DiffOp, Grid, PaneModes};

/// Client-side mirror of a single pane's protocol-level state.
///
/// Holds the current [`Grid`] (cells + cursor), pane-wide [`PaneModes`], the
/// last applied frame counter, and exposes [`apply`](DiffMirror::apply) for
/// incremental updates plus
/// [`ingest_snapshot`](DiffMirror::ingest_snapshot) for full-state resets
/// (`SPEC.md` §8.4).
///
/// The `cursor` field on this struct is a convenience mirror of
/// `grid.cursor`; both are kept in sync by [`apply_frame`](Self::apply_frame)
/// and [`ingest_snapshot`](DiffMirror::ingest_snapshot).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffMirror {
    /// Current pane grid. Renderer reads this directly.
    pub grid: Grid,
    /// Current cursor state. Mirrors `grid.cursor`.
    pub cursor: CursorState,
    /// Pane-wide modes (alt screen, bracketed paste, mouse protocol, etc.).
    pub modes: PaneModes,
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
            modes: PaneModes::EMPTY,
            frame_id: 0,
        }
    }

    /// Apply a sequence of [`DiffOp`] in order to the current state.
    ///
    /// Cursor and modes are NOT touched by this call — they ride alongside
    /// the op list on `PANE_DIFF` per SPEC §8.5. Use
    /// [`Self::apply_frame`] to apply ops together with the cursor + modes
    /// from a single `PANE_DIFF`.
    ///
    /// Out-of-bounds writes (a `row` >= `grid.rows`, a `col` past the row
    /// length, or a clear / cell run that extends past the row's last
    /// column) are silently clamped — they would indicate a server bug,
    /// but the mirror keeps the renderer alive rather than panicking.
    pub fn apply(&mut self, ops: &[DiffOp]) {
        super::apply::apply(self, ops);
    }

    /// Apply a full `PANE_DIFF` payload: ops plus the SPEC §8.5 cursor and
    /// modes fields and the SPEC §8.1 `frame_id`.
    ///
    /// This is the canonical entry point for the wire path; tests that want
    /// to exercise op application in isolation can still use [`Self::apply`].
    pub fn apply_frame(
        &mut self,
        ops: &[DiffOp],
        cursor: CursorState,
        modes: PaneModes,
        frame_id: u64,
    ) {
        self.apply(ops);
        self.cursor = cursor;
        self.grid.cursor = cursor;
        self.modes = modes;
        self.frame_id = frame_id;
    }

    /// Replace the entire state with `snap`, resetting `frame_id` to
    /// `frame_id`.
    ///
    /// Used for full-state resets per `SPEC.md` §8.4 (`PANE_SNAPSHOT`).
    pub fn ingest_snapshot(&mut self, snap: &Grid, frame_id: u64) {
        super::snapshot::ingest_snapshot(self, snap, frame_id);
    }
}
