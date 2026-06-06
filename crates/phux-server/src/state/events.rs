use std::collections::HashSet;

use libghostty_vt::terminal::{Point, PointCoordinate};
use phux_protocol::ids::TerminalId as WireTerminalId;
use tokio::sync::mpsc;

use super::input_log::Outbound;

/// Selection span data for a client on a terminal (phux-dh4).
///
/// Stores the coordinate endpoints of a selection in either the active
/// (viewport) or history (scrollback) point space. The actual [`Selection`]
/// type from libghostty holds borrowed references to grid cells and cannot
/// be stored long-term; this struct captures the span metadata needed to
/// reconstruct it on-demand.
///
/// [`Selection`]: libghostty_vt::selection::Selection
#[derive(Debug, Clone, PartialEq)]
pub struct SelectionSpan {
    /// Start point: either Active (viewport) or History (scrollback).
    pub start: Point,
    /// End point: must be in the same point space as start.
    pub end: Point,
    /// Whether the selection is rectangular (false = linear).
    /// Currently only linear selections are used; rectangular is deferred.
    pub rectangular: bool,
}

impl SelectionSpan {
    /// Create a new selection span from start and end points.
    #[must_use]
    pub fn new(start: Point, end: Point) -> Self {
        Self {
            start,
            end,
            rectangular: false,
        }
    }

    /// Create a selection span in the active (viewport) point space.
    #[must_use]
    pub fn active(start_x: u16, start_y: u32, end_x: u16, end_y: u32) -> Self {
        Self {
            start: Point::Active(PointCoordinate {
                x: start_x,
                y: start_y,
            }),
            end: Point::Active(PointCoordinate { x: end_x, y: end_y }),
            rectangular: false,
        }
    }

    /// Create a selection span in the history (scrollback) point space.
    #[must_use]
    pub fn history(start_x: u16, start_y: u32, end_x: u16, end_y: u32) -> Self {
        Self {
            start: Point::History(PointCoordinate {
                x: start_x,
                y: start_y,
            }),
            end: Point::History(PointCoordinate { x: end_x, y: end_y }),
            rectangular: false,
        }
    }
}

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
