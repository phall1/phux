//! Wire protocol for phux.
//!
//! This crate defines the protocol described in [`SPEC.md`] at the workspace
//! root: framing, message catalog, version negotiation, and the cell-level
//! diff format used on the hot path.
//!
//! The protocol is the source of truth. Code in this crate is normative;
//! implementations elsewhere defer to it.
//!
//! Current state: in-memory types only. Wire codec is not yet implemented;
//! see `SPEC.md` Appendix A.
//!
//! [`SPEC.md`]: https://github.com/phall1/phux/blob/main/SPEC.md

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod diff;
pub mod ids;

pub use diff::{
    Cell, CellFlags, Color, CursorShape, CursorState, DiffOp, Grid, Underline, compute_diff,
};
pub use ids::{ClientId, FrameId, PaneId, SessionId, WindowId};

/// Protocol version this crate implements.
pub const PROTOCOL_VERSION: Version = Version {
    major: 0,
    minor: 1,
    patch: 0,
};

/// A semantic protocol version: `major.minor.patch`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Version {
    /// Wire-breaking changes bump this.
    pub major: u16,
    /// Additive changes bump this.
    pub minor: u16,
    /// Editorial; behavior unchanged.
    pub patch: u16,
}
