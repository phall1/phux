//! phux server: the daemon side.
//!
//! Owns the canonical state of every session, window, and terminal for one
//! user. Hosts an IPC endpoint for clients (see `phux-protocol`), feeds
//! PTY output into per-terminal `libghostty_vt::Terminal` instances, and
//! forwards bytes to attached clients as `TERMINAL_OUTPUT` frames per
//! ADR-0013.

#![warn(missing_docs)]

pub mod cwd_query;
pub mod downsample;
pub mod grid;
pub mod id_bridge;
pub mod input;
pub mod runtime;
pub mod state;
pub mod telemetry;
pub mod transport;
pub mod terminal_actor;

pub use id_bridge::IdBridge;
pub use runtime::{ServerConfig, ServerError, ServerRuntime, default_socket_path};
pub use state::{
    AttachError, AttachedClient, ClientId, DEFAULT_COLLECTION_ID, Outbound, ServerState,
    SharedState, TerminalInput,
};
pub use terminal_actor::{
    SnapshotRequest, TerminalActor, TerminalActorBundle, TerminalActorError, TerminalHandle,
};
