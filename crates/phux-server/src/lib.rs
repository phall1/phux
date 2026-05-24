//! phux server: the daemon side.
//!
//! Owns the canonical state of every session, window, and pane for one
//! user. Hosts an IPC endpoint for clients (see `phux-protocol`), feeds
//! PTY output into per-pane `libghostty_vt::Terminal` instances, and
//! emits diffs to attached clients.

#![warn(missing_docs)]
