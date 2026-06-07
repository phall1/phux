use std::collections::HashSet;

use phux_protocol::ids::TerminalId as WireTerminalId;
use tokio::sync::mpsc;

use super::input_log::Outbound;

/// Scope of an agent-event subscription (SPEC §7.5, phux-y2t).
///
/// A client subscribes with [`Self::Server`] (every event the server
/// emits, including server-scoped events with no owning Terminal) or
/// [`Self::Terminal`] (only that Terminal's events). The two are stored
/// in a per-client `HashSet`, so a client MAY watch the whole server and
/// a specific pane simultaneously — fan-out de-duplicates by client.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum EventScope {
    /// Every event the server emits for any Terminal the client may
    /// observe, plus server-scoped events (`terminal: None`).
    Server,
    /// Only events concerning this specific Terminal.
    Terminal(WireTerminalId),
}

/// One client's agent-event subscription (SPEC §7.5, phux-y2t): its
/// outbound mailbox plus the set of scopes it watches.
///
/// The mailbox lives here so event fanout works for a pure `watch` client
/// that subscribed without attaching (no [`super::AttachedClient`] entry exists
/// for it). An attached client that also subscribes stores its same
/// mailbox here — fanout de-duplicates by [`super::ClientId`].
#[derive(Debug)]
pub struct EventSubscription {
    /// The client's outbound mailbox (the per-connection writer task
    /// drains it). Best-effort `try_send` target for `EVENT` frames.
    pub(crate) tx: mpsc::Sender<Outbound>,
    /// Scopes this client watches.
    pub(crate) scopes: HashSet<EventScope>,
}
