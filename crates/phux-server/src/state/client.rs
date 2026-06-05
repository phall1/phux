use phux_core::ids::{SessionId, TerminalId};
use phux_protocol::caps::ClientCapabilities;
use phux_protocol::ids::TerminalId as WireTerminalId;
use tokio::sync::mpsc;
use thiserror::Error;

use crate::terminal_actor::TerminalHandle;
use super::input_log::Outbound;

/// Server-assigned identifier for an attached client.
///
/// Distinct from [`phux_protocol::ids::ClientId`] (which is the wire-level
/// identity carried in protocol messages): this one is allocated by the
/// server, monotonic from `1`, and used purely for routing inside
/// [`ServerState`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ClientId(pub u64);

/// An attached client: routing identity plus outbound mailbox.
#[derive(Debug)]
pub struct AttachedClient {
    /// Server-assigned client id.
    pub id: ClientId,
    /// The session this client is observing.
    pub session: SessionId,
    /// Outbound mailbox; the per-client write task drains this and writes to
    /// the socket.
    pub tx: mpsc::Sender<Outbound>,
    /// The client's advertised capabilities (SPEC §6.2). The server MUST
    /// downsample outbound terminal bytes to this set before fanout — see
    /// [`crate::downsample::rewrite_bytes_with_caps`] for the helper the
    /// fanout layer plugs into.
    ///
    /// Populated from the [`phux_protocol::caps::ClientCapabilities`] the
    /// client advertised in HELLO (SPEC §6.1) and forwarded into
    /// [`ServerState::attach`]. Test scaffolding that never observed a
    /// HELLO calls [`ServerState::attach_default_caps`] which falls back
    /// to [`ClientCapabilities::default`] (most-permissive — never silently
    /// downgrades).
    pub client_caps: ClientCapabilities,
}

/// One pane target in an ATTACH snapshot pass.
///
/// Bridges the protocol-facing attach flow (`runtime.rs`) to the
/// state-internal registry topology without exposing `Session`/`Window`
/// traversal details outside this module.
#[derive(Debug, Clone)]
pub struct AttachSnapshotPane {
    /// Core pane identifier.
    pub terminal_id: TerminalId,
    /// Cross-task handle for snapshot/input/resize requests.
    pub handle: TerminalHandle,
    /// Stable wire id to use in `TERMINAL_SNAPSHOT` / `TERMINAL_OUTPUT`.
    pub wire_terminal_id: WireTerminalId,
}

/// Errors returned by [`ServerState::attach`].
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum AttachError {
    /// No session with that name was found in the registry.
    #[error("unknown session: {0}")]
    UnknownSession(String),
    /// The given [`ClientId`] is already attached.
    #[error("client {0:?} is already attached")]
    AlreadyAttached(ClientId),
}
