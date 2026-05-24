//! Core domain types for phux.
//!
//! Defines sessions, windows, panes, and the layout tree as pure data —
//! no I/O, no terminal emulation, no PTY handling. The server crate
//! composes these with libghostty-vt and PTY plumbing.

#![forbid(unsafe_code)]
#![warn(missing_docs)]
