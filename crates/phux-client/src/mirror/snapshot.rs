//! Full-state ingestion. See `SPEC.md` §8.4 (`PANE_SNAPSHOT`).
//!
//! A snapshot replaces the entire pane state: cells, cursor, dimensions.
//! Used at attach time and any time the client's frame counter drifts past
//! a recoverable horizon (the server emits a snapshot rather than backfill
//! diffs).

use phux_protocol::Grid;

use super::state::DiffMirror;

/// Replace the mirror's state with `snap` and bump `frame_id`. Public via
/// [`DiffMirror::ingest_snapshot`](super::DiffMirror::ingest_snapshot).
pub fn ingest_snapshot(state: &mut DiffMirror, snap: &Grid, frame_id: u64) {
    state.grid = snap.clone();
    state.cursor = snap.cursor;
    state.frame_id = frame_id;
}
