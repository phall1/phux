//! phux server: the daemon side.
//!
//! Owns the canonical state of every session, window, and terminal for one
//! user. Hosts an IPC endpoint for clients (see `phux-protocol`), feeds
//! PTY output into per-terminal `libghostty_vt::Terminal` instances, and
//! forwards bytes to attached clients as `TERMINAL_OUTPUT` frames per
//! ADR-0013.

#![deny(missing_docs)]

pub mod auth;
pub mod cwd_query;
pub mod downsample;
pub mod extract;
pub mod grid;
pub mod id_bridge;
pub mod input;
pub mod policy;
pub mod runtime;
pub mod search;
pub mod state;
pub mod telemetry;
pub mod terminal_actor;
pub mod transport;
pub mod upgrade;

pub use id_bridge::IdBridge;
pub use runtime::{ServerConfig, ServerError, ServerRuntime, default_socket_path};
pub use state::{
    AttachError, AttachedClient, ClientId, DEFAULT_GROUP_ID, Outbound, ServerState, SharedState,
    TerminalInput,
};
pub use terminal_actor::{
    SnapshotRequest, TerminalActor, TerminalActorBundle, TerminalActorError, TerminalHandle,
};
