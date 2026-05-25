//! phux server: the daemon side.
//!
//! Owns the canonical state of every session, window, and pane for one
//! user. Hosts an IPC endpoint for clients (see `phux-protocol`), feeds
//! PTY output into per-pane `libghostty_vt::Terminal` instances, and
//! emits diffs to attached clients.

#![warn(missing_docs)]

pub mod grid;
pub mod input;
pub mod runtime;
pub mod state;

pub use runtime::{ServerConfig, ServerError, ServerRuntime, default_socket_path};
pub use state::{
    AttachError, AttachedClient, ClientId, OutboundFrame, PaneInput, ServerState, SharedState,
};
