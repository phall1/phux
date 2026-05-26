//! phux server: the daemon side.
//!
//! Owns the canonical state of every session, window, and pane for one
//! user. Hosts an IPC endpoint for clients (see `phux-protocol`), feeds
//! PTY output into per-pane `libghostty_vt::Terminal` instances, and
//! forwards bytes to attached clients as `PANE_OUTPUT` frames per
//! ADR-0013.

#![warn(missing_docs)]

pub mod downsample;
pub mod grid;
pub mod id_bridge;
pub mod input;
pub mod pane_actor;
pub mod runtime;
pub mod state;

pub use id_bridge::IdBridge;
pub use pane_actor::{PaneActor, PaneActorBundle, PaneActorError, PaneHandle, SnapshotRequest};
pub use runtime::{ServerConfig, ServerError, ServerRuntime, default_socket_path};
pub use state::{
    AttachError, AttachedClient, ClientId, Outbound, PaneInput, ServerState, SharedState,
};
