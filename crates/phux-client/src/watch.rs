//! Server-pushed agent-event stream — the push half of the agent surface
//! (SPEC §7.5, ADR-0022 'events', `phux-y2t`).
//!
//! Sends `SUBSCRIBE_EVENTS` and streams the `EVENT` frames the server
//! pushes back, invoking a caller-supplied sink per event until the
//! transport closes (server gone, or the caller drops the future). The
//! subscription neither attaches nor resizes the pane — an agent can
//! `watch` a Terminal without disturbing the live session.
//!
//! This is an *additive accelerator* of the [`crate::wait`] poll floor:
//! a `watch` consumer learns of activity immediately rather than on the
//! next poll tick. A consumer that only polls still converges; the event
//! stream just cuts latency.

use std::path::Path;
use std::time::Duration;

use phux_protocol::ids::TerminalId;
use phux_protocol::wire::frame::{AgentEvent, FrameKind};

use crate::attach::AttachError;
use crate::attach::connection::Connection;

/// One streamed agent event plus the Terminal it concerns.
///
/// `terminal` is `None` for a server-scoped event with no single owning
/// Terminal (none of the v0.2 events are server-scoped today, but the
/// envelope allows it). The CLI `phux watch` renders one of these per
/// line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WatchEvent {
    /// The Terminal the event concerns, or `None` if server-scoped.
    pub terminal: Option<TerminalId>,
    /// The event payload.
    pub event: AgentEvent,
}

/// Subscribe to the agent-event stream for `terminal` (or server-wide when
/// `None`) and invoke `sink` for every event until the transport closes.
///
/// Opens a fresh connection and sends a single `SUBSCRIBE_EVENTS`; no
/// `HELLO` and no `ATTACH` (the subscription stands alone, matching the
/// `GET_SCREEN` control path). Returns `Ok(())` on a clean server-side EOF
/// (the [`AttachError::Disconnected`] the framed reader yields), so a
/// caller that loops until the server exits sees a tidy success rather
/// than an error. Any other transport/protocol failure surfaces as
/// [`AttachError`].
///
/// `sink` returning `false` stops the stream early (the caller asked to
/// stop, e.g. on a Ctrl-C handler racing the recv); returning `true`
/// keeps streaming.
///
/// # Errors
///
/// Returns [`AttachError`] on connect/transport/protocol failure. A clean
/// EOF is NOT an error (returns `Ok(())`).
pub async fn watch_events<F>(
    socket: &Path,
    terminal: Option<TerminalId>,
    mut sink: F,
) -> Result<(), AttachError>
where
    F: FnMut(WatchEvent) -> bool,
{
    let mut conn = Connection::connect(socket).await?;
    conn.send(&FrameKind::SubscribeEvents {
        terminal: terminal.clone(),
    })
    .await?;
    loop {
        match conn.recv().await {
            Ok(FrameKind::Event { terminal, event }) => {
                if !sink(WatchEvent { terminal, event }) {
                    return Ok(());
                }
            }
            // Other frames the server might interleave are ignored — the
            // watch connection only ever subscribed to events, so in
            // practice only `EVENT` arrives, but be liberal.
            Ok(_other) => {}
            // A clean EOF means the server closed the connection (it
            // exited, or the pane's session ended). That is the normal
            // terminal state for `watch`, not a failure.
            Err(AttachError::Disconnected) => return Ok(()),
            Err(err) => return Err(err),
        }
    }
}

/// Bounded one-shot over [`watch_events`]: collect events until `max_events`
/// are seen, `timeout` elapses, or the server closes — then return them.
///
/// This is the request/response shape a non-streaming caller (the MCP
/// `phux_watch` tool) needs: streaming is great for a live CLI, but a tool
/// call must return a finite result. `timeout` elapsing is success, not an
/// error — the collected prefix is returned. With both bounds `None`, it
/// streams until the server exits.
///
/// # Errors
///
/// Returns [`AttachError`] on connect/transport failure before any timeout.
pub async fn collect_events(
    socket: &Path,
    terminal: Option<TerminalId>,
    max_events: Option<usize>,
    timeout: Option<Duration>,
) -> Result<Vec<WatchEvent>, AttachError> {
    let mut collected: Vec<WatchEvent> = Vec::new();
    {
        let sink = |ev: WatchEvent| {
            collected.push(ev);
            // Keep going until we reach the cap (if any).
            max_events.is_none_or(|m| collected.len() < m)
        };
        let fut = watch_events(socket, terminal, sink);
        match timeout {
            // Timeout is a clean stop: drop the future, keep the prefix.
            Some(d) => {
                let _ = tokio::time::timeout(d, fut).await;
            }
            None => fut.await?,
        }
    }
    Ok(collected)
}
