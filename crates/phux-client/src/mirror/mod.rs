//! Client-side diff mirror.
//!
//! Consumes the [`DiffOp`](phux_protocol::DiffOp) stream the server emits and
//! materialises it into a current-state [`Grid`](phux_protocol::Grid) plus
//! [`CursorState`](phux_protocol::CursorState) that the renderer reads.
//!
//! # Invariant
//!
//! Given a server-side trajectory `G0 -> G1` and the diff
//! `ops = compute_diff(G0, G1)`, a [`DiffMirror`] initialised so its grid
//! equals `G0` and then fed `ops` via [`DiffMirror::apply`] holds a grid
//! byte-identical to `G1`. This is the contract the predictive-echo layer
//! (phux-9gw.1) and the renderer rely on.
//!
//! See `SPEC.md` §8 (hot-path frame model), `SPEC.md` §8.3 (diff operations),
//! `SPEC.md` §8.4 (snapshots), and ADR-0002 / ADR-0008.

mod apply;
mod snapshot;
mod state;

pub use apply::apply;
pub use snapshot::ingest_snapshot;
pub use state::DiffMirror;
