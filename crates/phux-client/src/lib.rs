//! phux TUI client.
//!
//! Receives cell-level diffs from a phux server (see `phux-protocol`),
//! composes pane grids with chrome (borders, status bar, command prompt),
//! and emits VT to the outer terminal. Knows nothing about PTYs.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod mirror;

pub use mirror::DiffMirror;
