//! Hub-to-satellite frame relay (phux-v45.4, ADR-0007 §4).
//!
//! The routing layer that replaces the blanket `UnsupportedSatelliteRoute`
//! rejections on a `phux server --hub`: a frame targeting
//! `TerminalId::Satellite { host, id }` is resolved to the live outbound
//! link the dialer (phux-v45.3) maintains for `host`, its terminal id is
//! rewritten to `Local { id }`, and the frame is forwarded **verbatim** —
//! the hub never re-encodes VT bytes (ADR-0007: opaque relay). Responses
//! and subscribed streams coming back from the satellite are re-tagged
//! `Local { id }` -> `Satellite { host, id }` before they reach the
//! consumer, so the consumer only ever sees the hub-scoped address it
//! asked for. Satellites stay unaware of each other: a frame arriving
//! from a satellite already tagged `Satellite` is dropped, never chained.
//!
//! One `RelaySession` lives inside each link supervisor
//! (`super::link::run_link`) while its connection is up. It owns the
//! per-link `request_id` remap (the hub allocates its own id space toward
//! the satellite; the consumer's `request_id` never crosses the link) and
//! the proxy-subscription registry (which hub consumers observe which
//! satellite terminals). Consumers talk to it through a `RelayHandle` —
//! a bounded mailbox published in `HubRelays` on shared state.
//!
//! **Fail fast, never hang.** No live connection means a typed
//! `ErrorCode::SatelliteUnreachable` reply, immediately: the link
//! supervisor drains the relay mailbox during dial, backoff, and
//! fail-closed refusal phases, failing every queued request. A satellite
//! disconnect fails all in-flight commands the same way and pushes one
//! typed `ERROR { SatelliteUnreachable }` frame to every proxy-subscribed
//! consumer before the registry is cleared — teardown is observable, not
//! silence.
//!
//! **Backpressure.** The relay mailbox is bounded (`RELAY_MAILBOX`) and
//! every producer uses `try_send`: a saturated link fails commands with
//! `ResourceExhausted` and drops fire-and-forget frames with a warn,
//! mirroring the pane-input mailbox semantics. Return-leg fan-out to
//! consumers uses `try_send` into each consumer's bounded outbound
//! mailbox (the same discipline as `crate::runtime::client::broadcast_event`),
//! so one slow consumer never stalls the link or the hub's other work.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};

use bytes::BytesMut;
use phux_protocol::ids::{SatelliteHost, TerminalId};
use phux_protocol::wire::frame::{Command, CommandResult, ErrorCode, FrameKind};
use tokio::sync::{mpsc, oneshot};
use tracing::{debug, trace, warn};

use crate::state::{ClientId, Outbound};

/// Capacity of each per-satellite relay mailbox. Small and bounded: the
/// link is a single ordered stream, so queueing more than a burst behind
/// it only adds latency. Producers `try_send` and fail fast on `Full`.
pub(crate) const RELAY_MAILBOX: usize = 64;

/// A request from a hub-side consumer path to one satellite's relay.
#[derive(Debug)]
pub(crate) enum RelayRequest {
    /// Relay a `COMMAND` whose terminal ids are already rewritten to the
    /// satellite's `Local` space. The session allocates the link-side
    /// `request_id` and resolves `reply` with the correlated
    /// `COMMAND_RESULT` (or a typed error on disconnect).
    Command {
        /// The command to forward, ids already satellite-local.
        command: Command,
        /// Resolved with the satellite's result; dropping the receiver is
        /// legal (detached fire-and-forget relays do exactly that).
        reply: oneshot::Sender<CommandResult>,
    },
    /// Relay a fire-and-forget frame (`INPUT_*`, `FRAME_ACK`,
    /// `TERMINAL_RESIZE`, `SUBSCRIBE_EVENTS`), terminal ids already
    /// rewritten satellite-local.
    /// No reply; a dead link drops it (with the teardown notification
    /// covering the observable side).
    Forward {
        /// The frame to forward verbatim.
        frame: FrameKind,
    },
    /// Register `client`'s outbound mailbox as a proxy subscriber for the
    /// satellite-local terminal `terminal`: return-leg frames scoped to it
    /// (`EVENT`, `TERMINAL_OUTPUT`, `TERMINAL_CLOSED`, ...) are re-tagged and
    /// fanned out to `out_tx`. Idempotent per `(terminal, client)`.
    Subscribe {
        /// Satellite-local terminal id (the `id` of `Satellite { host, id }`).
        terminal: u32,
        /// Hub-side client identity, for teardown on detach.
        client: ClientId,
        /// The client's outbound mailbox.
        out_tx: mpsc::Sender<Outbound>,
    },
    /// Drop every proxy subscription `client` holds on this link
    /// (consumer detach / disconnect).
    UnsubscribeClient {
        /// The detaching hub-side client.
        client: ClientId,
    },
}

/// Cheaply-cloneable producer handle to one satellite's relay mailbox.
#[derive(Debug, Clone)]
pub(crate) struct RelayHandle {
    host: SatelliteHost,
    tx: mpsc::Sender<RelayRequest>,
}

impl RelayHandle {
    /// Pair a fresh handle with the receiver its link supervisor drains.
    pub(crate) fn new(host: SatelliteHost) -> (Self, mpsc::Receiver<RelayRequest>) {
        let (tx, rx) = mpsc::channel(RELAY_MAILBOX);
        (Self { host, tx }, rx)
    }

    /// Relay `command` and await the correlated result. Fails fast — a
    /// saturated mailbox, a dead link task, or a link lost mid-flight all
    /// produce a typed error instead of a hang (the session and the link
    /// supervisor's drain phases guarantee the oneshot always resolves or
    /// drops promptly).
    pub(crate) async fn command(&self, command: Command) -> CommandResult {
        let (reply, rx) = oneshot::channel();
        match self.tx.try_send(RelayRequest::Command { command, reply }) {
            Ok(()) => {}
            Err(mpsc::error::TrySendError::Full(_)) => {
                return CommandResult::Error {
                    code: ErrorCode::ResourceExhausted,
                    message: format!("satellite {} link is saturated; retry", self.host),
                };
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                return CommandResult::Error {
                    code: ErrorCode::SatelliteUnreachable,
                    message: format!("satellite {} link is down", self.host),
                };
            }
        }
        rx.await.unwrap_or_else(|_| CommandResult::Error {
            code: ErrorCode::SatelliteUnreachable,
            message: format!("satellite {} link dropped before the reply", self.host),
        })
    }

    /// Relay `command` without awaiting the result (the idempotent batch
    /// path — `KILL_TERMINALS` semantics tolerate a silent skip).
    pub(crate) fn command_detached(&self, command: Command) {
        let (reply, _rx) = oneshot::channel();
        if self
            .tx
            .try_send(RelayRequest::Command { command, reply })
            .is_err()
        {
            debug!(satellite = %self.host, "detached satellite command dropped (link down or saturated)");
        }
    }

    /// Relay a fire-and-forget frame. Drops with a warn on a saturated or
    /// dead link — the same contract those frames already have locally.
    pub(crate) fn forward(&self, frame: FrameKind) {
        if let Err(err) = self.tx.try_send(RelayRequest::Forward { frame }) {
            warn!(
                satellite = %self.host,
                reason = %trysend_reason(&err),
                "satellite relay frame dropped (fire-and-forget)"
            );
        }
    }

    /// Register a proxy subscription (see [`RelayRequest::Subscribe`]).
    pub(crate) fn subscribe(
        &self,
        terminal: u32,
        client: ClientId,
        out_tx: mpsc::Sender<Outbound>,
    ) {
        if let Err(err) = self.tx.try_send(RelayRequest::Subscribe {
            terminal,
            client,
            out_tx,
        }) {
            warn!(
                satellite = %self.host,
                terminal,
                reason = %trysend_reason(&err),
                "satellite proxy subscription dropped"
            );
        }
    }

    /// Drop every proxy subscription `client` holds on this link.
    pub(crate) fn unsubscribe_client(&self, client: ClientId) {
        // Best-effort: if the link task is gone its registry is gone too.
        let _ = self.tx.try_send(RelayRequest::UnsubscribeClient { client });
    }
}

const fn trysend_reason(err: &mpsc::error::TrySendError<RelayRequest>) -> &'static str {
    match err {
        mpsc::error::TrySendError::Full(_) => "mailbox full",
        mpsc::error::TrySendError::Closed(_) => "link task gone",
    }
}

/// Shared registry of per-satellite [`RelayHandle`]s, mirrored into
/// server state at hub bring-up (the sibling of
/// [`super::link::HubLinkStatuses`]). Empty on a non-hub server.
#[derive(Debug, Clone, Default)]
pub(crate) struct HubRelays {
    inner: Arc<Mutex<BTreeMap<SatelliteHost, RelayHandle>>>,
}

impl HubRelays {
    /// Register `handle` for its host (hub bring-up, one per table entry).
    pub(crate) fn insert(&self, handle: RelayHandle) {
        self.lock().insert(handle.host.clone(), handle);
    }

    /// The relay handle for `host`, if the hub dials that satellite.
    pub(crate) fn get(&self, host: &SatelliteHost) -> Option<RelayHandle> {
        self.lock().get(host).cloned()
    }

    /// Every registered handle (detach fan-out).
    pub(crate) fn all(&self) -> Vec<RelayHandle> {
        self.lock().values().cloned().collect()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, BTreeMap<SatelliteHost, RelayHandle>> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// Fail one queued request while the link is not connected (dial in
/// flight, backoff, fail-closed refusal). Used by the link supervisor's
/// drain arms so a consumer never hangs on a dead satellite.
pub(crate) fn fail_fast(request: RelayRequest, host: &SatelliteHost, why: &str) {
    match request {
        RelayRequest::Command { reply, .. } => {
            let _ = reply.send(CommandResult::Error {
                code: ErrorCode::SatelliteUnreachable,
                message: format!("satellite {host} is unreachable: {why}"),
            });
        }
        RelayRequest::Forward { frame } => {
            trace!(satellite = %host, kind = ?frame_label(&frame), why, "relay frame dropped while disconnected");
        }
        RelayRequest::Subscribe { client, out_tx, .. } => {
            // A subscription to an unreachable satellite gets the same
            // typed notification a disconnect would produce — observable,
            // not silence (SUBSCRIBE_EVENTS has no reply frame).
            let _ = out_tx.try_send(Outbound::Frame(unreachable_error(host, why)));
            trace!(satellite = %host, ?client, why, "proxy subscription refused while disconnected");
        }
        RelayRequest::UnsubscribeClient { .. } => {}
    }
}

/// The typed `ERROR` frame consumers receive when a satellite they observe
/// (or tried to observe) is unreachable.
fn unreachable_error(host: &SatelliteHost, why: &str) -> FrameKind {
    FrameKind::Error {
        request_id: None,
        code: ErrorCode::SatelliteUnreachable,
        message: format!("satellite {host} is unreachable: {why}"),
    }
}

/// A payload-free label for logging a relayed frame.
const fn frame_label(frame: &FrameKind) -> &'static str {
    match frame {
        FrameKind::InputKey { .. } => "INPUT_KEY",
        FrameKind::InputMouse { .. } => "INPUT_MOUSE",
        FrameKind::InputFocus { .. } => "INPUT_FOCUS",
        FrameKind::InputPaste { .. } => "INPUT_PASTE",
        FrameKind::FrameAck { .. } => "FRAME_ACK",
        FrameKind::TerminalResize { .. } => "TERMINAL_RESIZE",
        FrameKind::SubscribeEvents { .. } => "SUBSCRIBE_EVENTS",
        _ => "other",
    }
}

/// One proxy subscriber on the return leg.
#[derive(Debug)]
struct ProxySubscriber {
    client: ClientId,
    out_tx: mpsc::Sender<Outbound>,
}

/// The per-connection relay state a link supervisor drives while its
/// satellite connection is up (see [`super::link::run_link`]).
///
/// Owns the link-side `request_id` allocation, the pending-command map,
/// and the proxy-subscription registry. All state is session-scoped:
/// [`Self::teardown`] fails pending commands and notifies subscribers, so
/// a reconnected link starts clean (consumers re-issue and re-subscribe).
#[derive(Debug)]
pub(crate) struct RelaySession {
    host: SatelliteHost,
    next_request_id: u32,
    pending: HashMap<u32, oneshot::Sender<CommandResult>>,
    subscribers: HashMap<u32, Vec<ProxySubscriber>>,
    encode_buf: BytesMut,
}

impl RelaySession {
    /// Fresh session state for one established connection to `host`.
    pub(crate) fn new(host: SatelliteHost) -> Self {
        Self {
            host,
            next_request_id: 1,
            pending: HashMap::new(),
            subscribers: HashMap::new(),
            encode_buf: BytesMut::with_capacity(1024),
        }
    }

    /// Service one consumer request. Returns the encoded frame to put on
    /// the wire, or `None` when the request was registry-only.
    ///
    /// Split from the wire write so the caller (the link supervisor's
    /// select loop) owns all connection I/O in one place.
    pub(crate) fn handle_request(&mut self, request: RelayRequest) -> Option<Vec<u8>> {
        match request {
            RelayRequest::Command { command, reply } => {
                let request_id = self.allocate_request_id();
                self.pending.insert(request_id, reply);
                Some(self.encode(&FrameKind::Command {
                    request_id,
                    command,
                }))
            }
            RelayRequest::Forward { frame } => Some(self.encode(&frame)),
            RelayRequest::Subscribe {
                terminal,
                client,
                out_tx,
            } => {
                let subs = self.subscribers.entry(terminal).or_default();
                if let Some(existing) = subs.iter_mut().find(|s| s.client == client) {
                    existing.out_tx = out_tx;
                } else {
                    subs.push(ProxySubscriber { client, out_tx });
                }
                None
            }
            RelayRequest::UnsubscribeClient { client } => {
                self.subscribers.retain(|_, subs| {
                    subs.retain(|s| s.client != client);
                    !subs.is_empty()
                });
                None
            }
        }
    }

    /// Dispatch one frame arriving from the satellite: resolve relayed
    /// command replies and re-tag + fan out subscribed streams.
    pub(crate) fn handle_inbound(&mut self, framed: &[u8]) {
        let frame = match FrameKind::decode(framed) {
            Ok((frame, _rest)) => frame,
            Err(err) => {
                warn!(satellite = %self.host, error = ?err, "undecodable frame from satellite; dropping");
                return;
            }
        };
        match frame {
            FrameKind::CommandResult { request_id, result } => {
                self.resolve_pending(request_id, result);
            }
            FrameKind::Error {
                request_id: Some(request_id),
                code,
                message,
            } => {
                self.resolve_pending(request_id, CommandResult::Error { code, message });
            }
            FrameKind::Event { terminal, event } => {
                if let Some(id) = self.retag_inbound(terminal.as_ref()) {
                    self.fan_out(
                        id,
                        &FrameKind::Event {
                            terminal: Some(TerminalId::satellite(self.host.clone(), id)),
                            event,
                        },
                    );
                }
            }
            FrameKind::TerminalOutput {
                terminal_id,
                seq,
                bytes,
            } => {
                if let Some(id) = self.retag_inbound(Some(&terminal_id)) {
                    self.fan_out(
                        id,
                        &FrameKind::TerminalOutput {
                            terminal_id: TerminalId::satellite(self.host.clone(), id),
                            seq,
                            bytes,
                        },
                    );
                }
            }
            FrameKind::TerminalSnapshot {
                terminal_id,
                cols,
                rows,
                vt_replay_bytes,
                scrollback_bytes,
            } => {
                if let Some(id) = self.retag_inbound(Some(&terminal_id)) {
                    self.fan_out(
                        id,
                        &FrameKind::TerminalSnapshot {
                            terminal_id: TerminalId::satellite(self.host.clone(), id),
                            cols,
                            rows,
                            vt_replay_bytes,
                            scrollback_bytes,
                        },
                    );
                }
            }
            FrameKind::TerminalClosed {
                terminal_id,
                exit_status,
            } => {
                if let Some(id) = self.retag_inbound(Some(&terminal_id)) {
                    self.fan_out(
                        id,
                        &FrameKind::TerminalClosed {
                            terminal_id: TerminalId::satellite(self.host.clone(), id),
                            exit_status,
                        },
                    );
                    // The satellite terminal is gone; its proxy
                    // subscriptions go with it.
                    self.subscribers.remove(&id);
                }
            }
            FrameKind::Bell { terminal_id } => {
                if let Some(id) = self.retag_inbound(Some(&terminal_id)) {
                    self.fan_out(
                        id,
                        &FrameKind::Bell {
                            terminal_id: TerminalId::satellite(self.host.clone(), id),
                        },
                    );
                }
            }
            other => {
                // HELLO_OK, PONG, un-correlated errors, and anything a
                // newer satellite might push: not relayable (no terminal
                // scope), logged and dropped.
                trace!(satellite = %self.host, kind = ?other, "unrelayed frame from satellite");
            }
        }
    }

    /// Fail every in-flight command and notify every subscribed consumer,
    /// then clear the registries. Called exactly once per session, on
    /// disconnect or hub shutdown.
    pub(crate) fn teardown(&mut self, why: &str) {
        for (_, reply) in self.pending.drain() {
            let _ = reply.send(CommandResult::Error {
                code: ErrorCode::SatelliteUnreachable,
                message: format!("satellite {} is unreachable: {why}", self.host),
            });
        }
        // One typed ERROR per consumer (not per subscription): the frame
        // names the host, and every terminal of that host is gone at once.
        let mut notified: Vec<ClientId> = Vec::new();
        let error = unreachable_error(&self.host, why);
        for subs in self.subscribers.values() {
            for sub in subs {
                if notified.contains(&sub.client) {
                    continue;
                }
                notified.push(sub.client);
                let _ = sub.out_tx.try_send(Outbound::Frame(error.clone()));
            }
        }
        if !notified.is_empty() {
            debug!(
                satellite = %self.host,
                consumers = notified.len(),
                why,
                "notified proxy subscribers of satellite teardown"
            );
        }
        self.subscribers.clear();
    }

    /// Resolve a link-side `request_id` back to its waiting consumer.
    fn resolve_pending(&mut self, request_id: u32, result: CommandResult) {
        match self.pending.remove(&request_id) {
            Some(reply) => {
                // A dropped receiver (detached relay) is fine.
                let _ = reply.send(result);
            }
            None => {
                debug!(
                    satellite = %self.host,
                    request_id,
                    "satellite reply with no pending command; dropping"
                );
            }
        }
    }

    /// The satellite-local id of an inbound frame's terminal scope, or
    /// `None` when the frame is unscoped or (out of ADR-0007 topology)
    /// already satellite-tagged — satellites do not chain.
    fn retag_inbound(&self, terminal: Option<&TerminalId>) -> Option<u32> {
        match terminal {
            Some(TerminalId::Local { id }) => Some(*id),
            Some(TerminalId::Satellite { .. }) => {
                warn!(
                    satellite = %self.host,
                    "satellite forwarded a Satellite-tagged id; hub-and-spoke does not chain — dropping"
                );
                None
            }
            None => None,
        }
    }

    /// Push `frame` to every proxy subscriber of satellite-local terminal
    /// `id`. `try_send` per consumer: a slow consumer drops its copy, the
    /// link and its siblings keep flowing.
    fn fan_out(&self, id: u32, frame: &FrameKind) {
        let Some(subs) = self.subscribers.get(&id) else {
            trace!(satellite = %self.host, terminal = id, "inbound stream frame with no proxy subscribers");
            return;
        };
        for sub in subs {
            let _ = sub.out_tx.try_send(Outbound::Frame(frame.clone()));
        }
    }

    /// Allocate the next link-side request id, skipping ids still pending
    /// (u32 wrap-around safety, not a practical collision).
    fn allocate_request_id(&mut self) -> u32 {
        loop {
            let id = self.next_request_id;
            self.next_request_id = self.next_request_id.wrapping_add(1).max(1);
            if !self.pending.contains_key(&id) {
                return id;
            }
        }
    }

    fn encode(&mut self, frame: &FrameKind) -> Vec<u8> {
        self.encode_buf.clear();
        frame.encode(&mut self.encode_buf);
        self.encode_buf.to_vec()
    }
}

/// Split a satellite-tagged wire id into its host and satellite-local id.
pub(crate) fn satellite_route(terminal_id: &TerminalId) -> Option<(SatelliteHost, u32)> {
    match terminal_id {
        TerminalId::Satellite { host, id } => Some((host.clone(), *id)),
        TerminalId::Local { .. } => None,
    }
}

/// If `command` targets a single satellite-owned terminal, produce the
/// owning host and the command rewritten to the satellite's `Local` id
/// space (ADR-0007 outbound leg). `None` for local targets, unscoped
/// commands (`GET_STATE`, `UPGRADE` — hub-local by design), and
/// `KILL_TERMINALS` (a mixed batch, partitioned by its own handler).
#[allow(
    clippy::too_many_lines,
    reason = "one mechanical rewrite arm per per-terminal Command variant; splitting hides the catalog"
)]
pub(crate) fn route_to_satellite(command: &Command) -> Option<(SatelliteHost, Command)> {
    match command {
        Command::KillTerminal { terminal_id } => {
            let (host, id) = satellite_route(terminal_id)?;
            Some((
                host,
                Command::KillTerminal {
                    terminal_id: TerminalId::local(id),
                },
            ))
        }
        Command::GetScreen {
            terminal_id,
            request_scrollback,
            cells,
        } => {
            let (host, id) = satellite_route(terminal_id)?;
            Some((
                host,
                Command::GetScreen {
                    terminal_id: TerminalId::local(id),
                    request_scrollback: *request_scrollback,
                    cells: *cells,
                },
            ))
        }
        Command::RouteInput { terminal_id, event } => {
            let (host, id) = satellite_route(terminal_id)?;
            Some((
                host,
                Command::RouteInput {
                    terminal_id: TerminalId::local(id),
                    event: event.clone(),
                },
            ))
        }
        Command::GetTerminalState {
            terminal_id,
            include_scrollback,
            max_scrollback_lines,
        } => {
            let (host, id) = satellite_route(terminal_id)?;
            Some((
                host,
                Command::GetTerminalState {
                    terminal_id: TerminalId::local(id),
                    include_scrollback: *include_scrollback,
                    max_scrollback_lines: *max_scrollback_lines,
                },
            ))
        }
        Command::SubscribeTerminalEvents {
            terminal_id,
            event_types,
        } => {
            let (host, id) = satellite_route(terminal_id)?;
            Some((
                host,
                Command::SubscribeTerminalEvents {
                    terminal_id: TerminalId::local(id),
                    event_types: event_types.clone(),
                },
            ))
        }
        Command::AcquireInput {
            terminal_id,
            mode,
            ttl_ms,
        } => {
            let (host, id) = satellite_route(terminal_id)?;
            Some((
                host,
                Command::AcquireInput {
                    terminal_id: TerminalId::local(id),
                    mode: *mode,
                    ttl_ms: *ttl_ms,
                },
            ))
        }
        Command::ReleaseInput { terminal_id } => {
            let (host, id) = satellite_route(terminal_id)?;
            Some((
                host,
                Command::ReleaseInput {
                    terminal_id: TerminalId::local(id),
                },
            ))
        }
        Command::SignalTerminal {
            terminal_id,
            signal,
        } => {
            let (host, id) = satellite_route(terminal_id)?;
            Some((
                host,
                Command::SignalTerminal {
                    terminal_id: TerminalId::local(id),
                    signal: *signal,
                },
            ))
        }
        Command::ReportAsked {
            terminal_id,
            id: asked_id,
            question,
            suggestions,
            elapsed_seconds,
        } => {
            let (host, id) = satellite_route(terminal_id)?;
            Some((
                host,
                Command::ReportAsked {
                    terminal_id: TerminalId::local(id),
                    id: asked_id.clone(),
                    question: question.clone(),
                    suggestions: suggestions.clone(),
                    elapsed_seconds: *elapsed_seconds,
                },
            ))
        }
        // GET_STATE / UPGRADE are hub-local; KILL_TERMINALS partitions its
        // mixed batch in `handle_kill_terminals`; forward-compat commands
        // this hub does not know cannot be routed (their terminal scope is
        // unreadable) and fall through to the local INVALID_COMMAND path.
        _ => None,
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, reason = "tests")]
mod tests {
    use phux_protocol::wire::frame::{AgentEvent, CommandValue, StateScope};

    use super::*;

    fn host() -> SatelliteHost {
        SatelliteHost::new("devbox")
    }

    fn decode(bytes: &[u8]) -> FrameKind {
        FrameKind::decode(bytes).expect("frame").0
    }

    fn encode(frame: &FrameKind) -> Vec<u8> {
        let mut buf = BytesMut::new();
        frame.encode(&mut buf);
        buf.to_vec()
    }

    // --- outbound command rewrite ---------------------------------------

    #[test]
    fn route_to_satellite_rewrites_terminal_ids_to_local() {
        let command = Command::GetScreen {
            terminal_id: TerminalId::satellite("devbox", 7),
            request_scrollback: Some(10),
            cells: true,
        };
        let (routed_host, rewritten) = route_to_satellite(&command).expect("satellite target");
        assert_eq!(routed_host, host());
        assert_eq!(
            rewritten,
            Command::GetScreen {
                terminal_id: TerminalId::local(7),
                request_scrollback: Some(10),
                cells: true,
            }
        );
    }

    #[test]
    fn route_to_satellite_ignores_local_and_unscoped_commands() {
        assert!(
            route_to_satellite(&Command::GetScreen {
                terminal_id: TerminalId::local(7),
                request_scrollback: None,
                cells: false,
            })
            .is_none()
        );
        assert!(
            route_to_satellite(&Command::GetState {
                scope: StateScope::Server,
            })
            .is_none()
        );
        assert!(route_to_satellite(&Command::Upgrade).is_none());
        // Mixed batches partition in handle_kill_terminals, not here.
        assert!(
            route_to_satellite(&Command::KillTerminals {
                ids: vec![TerminalId::satellite("devbox", 1)],
            })
            .is_none()
        );
    }

    #[test]
    fn route_to_satellite_covers_every_per_terminal_command() {
        let sat = TerminalId::satellite("devbox", 3);
        let commands = [
            Command::KillTerminal {
                terminal_id: sat.clone(),
            },
            Command::GetTerminalState {
                terminal_id: sat.clone(),
                include_scrollback: false,
                max_scrollback_lines: 0,
            },
            Command::SubscribeTerminalEvents {
                terminal_id: sat.clone(),
                event_types: vec![],
            },
            Command::AcquireInput {
                terminal_id: sat.clone(),
                mode: phux_protocol::wire::frame::InputMode::Cooperative,
                ttl_ms: 0,
            },
            Command::ReleaseInput {
                terminal_id: sat.clone(),
            },
            Command::SignalTerminal {
                terminal_id: sat.clone(),
                signal: phux_protocol::wire::frame::TerminalSignal::Interrupt,
            },
            Command::ReportAsked {
                terminal_id: sat,
                id: "q".to_owned(),
                question: "?".to_owned(),
                suggestions: vec![],
                elapsed_seconds: None,
            },
        ];
        for command in commands {
            let (routed_host, rewritten) =
                route_to_satellite(&command).expect("per-terminal command routes");
            assert_eq!(routed_host, host());
            assert!(
                route_to_satellite(&rewritten).is_none(),
                "rewritten command must be local: {rewritten:?}"
            );
        }
    }

    // --- session: command remap ------------------------------------------

    #[test]
    fn session_remaps_request_ids_and_resolves_replies() {
        let mut session = RelaySession::new(host());
        let (reply_a, mut rx_a) = oneshot::channel();
        let (reply_b, mut rx_b) = oneshot::channel();

        let wire_a = session
            .handle_request(RelayRequest::Command {
                command: Command::GetState {
                    scope: StateScope::Server,
                },
                reply: reply_a,
            })
            .expect("command produces a wire frame");
        let wire_b = session
            .handle_request(RelayRequest::Command {
                command: Command::Upgrade,
                reply: reply_b,
            })
            .expect("command produces a wire frame");

        let FrameKind::Command {
            request_id: id_a, ..
        } = decode(&wire_a)
        else {
            panic!("expected COMMAND on the wire");
        };
        let FrameKind::Command {
            request_id: id_b, ..
        } = decode(&wire_b)
        else {
            panic!("expected COMMAND on the wire");
        };
        assert_ne!(id_a, id_b, "link-side request ids must be distinct");

        // Resolve out of order: the remap must correlate, not FIFO.
        session.handle_inbound(&encode(&FrameKind::CommandResult {
            request_id: id_b,
            result: CommandResult::OkWith(CommandValue::Json("b".to_owned())),
        }));
        session.handle_inbound(&encode(&FrameKind::CommandResult {
            request_id: id_a,
            result: CommandResult::Ok,
        }));

        assert_eq!(rx_a.try_recv().expect("a resolved"), CommandResult::Ok);
        assert_eq!(
            rx_b.try_recv().expect("b resolved"),
            CommandResult::OkWith(CommandValue::Json("b".to_owned()))
        );
    }

    #[test]
    fn session_maps_correlated_error_frames_to_command_errors() {
        let mut session = RelaySession::new(host());
        let (reply, mut rx) = oneshot::channel();
        let wire = session
            .handle_request(RelayRequest::Command {
                command: Command::Upgrade,
                reply,
            })
            .expect("wire frame");
        let FrameKind::Command { request_id, .. } = decode(&wire) else {
            panic!("expected COMMAND");
        };
        session.handle_inbound(&encode(&FrameKind::Error {
            request_id: Some(request_id),
            code: ErrorCode::TerminalNotFound,
            message: "nope".to_owned(),
        }));
        assert_eq!(
            rx.try_recv().expect("resolved"),
            CommandResult::Error {
                code: ErrorCode::TerminalNotFound,
                message: "nope".to_owned(),
            }
        );
    }

    // --- session: return-leg re-tagging ----------------------------------

    #[test]
    fn session_retags_subscribed_streams_local_to_satellite() {
        let mut session = RelaySession::new(host());
        let (out_tx, mut out_rx) = mpsc::channel(8);
        session.handle_request(RelayRequest::Subscribe {
            terminal: 9,
            client: ClientId(1),
            out_tx,
        });

        session.handle_inbound(&encode(&FrameKind::Event {
            terminal: Some(TerminalId::local(9)),
            event: AgentEvent::CommandStarted,
        }));
        session.handle_inbound(&encode(&FrameKind::TerminalOutput {
            terminal_id: TerminalId::local(9),
            seq: 42,
            bytes: bytes::Bytes::from_static(b"hi"),
        }));
        // A different terminal: nothing must reach the subscriber.
        session.handle_inbound(&encode(&FrameKind::Event {
            terminal: Some(TerminalId::local(10)),
            event: AgentEvent::CommandStarted,
        }));

        let Outbound::Frame(first) = out_rx.try_recv().expect("event fanned out");
        assert_eq!(
            first,
            FrameKind::Event {
                terminal: Some(TerminalId::satellite("devbox", 9)),
                event: AgentEvent::CommandStarted,
            }
        );
        let Outbound::Frame(second) = out_rx.try_recv().expect("output fanned out");
        assert!(matches!(
            second,
            FrameKind::TerminalOutput { terminal_id, seq: 42, .. }
                if terminal_id == TerminalId::satellite("devbox", 9)
        ));
        assert!(out_rx.try_recv().is_err(), "unsubscribed terminal leaked");
    }

    #[test]
    fn session_drops_chained_satellite_tags_from_the_return_leg() {
        let mut session = RelaySession::new(host());
        let (out_tx, mut out_rx) = mpsc::channel(8);
        session.handle_request(RelayRequest::Subscribe {
            terminal: 9,
            client: ClientId(1),
            out_tx,
        });
        // ADR-0007: satellites are unaware of each other; a nested
        // Satellite tag must never be re-relayed.
        session.handle_inbound(&encode(&FrameKind::Event {
            terminal: Some(TerminalId::satellite("nested", 9)),
            event: AgentEvent::CommandStarted,
        }));
        assert!(out_rx.try_recv().is_err());
    }

    #[test]
    fn terminal_closed_retags_and_drops_the_proxy_subscription() {
        let mut session = RelaySession::new(host());
        let (out_tx, mut out_rx) = mpsc::channel(8);
        session.handle_request(RelayRequest::Subscribe {
            terminal: 9,
            client: ClientId(1),
            out_tx,
        });
        session.handle_inbound(&encode(&FrameKind::TerminalClosed {
            terminal_id: TerminalId::local(9),
            exit_status: Some(0),
        }));
        let Outbound::Frame(frame) = out_rx.try_recv().expect("closed fanned out");
        assert_eq!(
            frame,
            FrameKind::TerminalClosed {
                terminal_id: TerminalId::satellite("devbox", 9),
                exit_status: Some(0),
            }
        );
        // Subscription is gone: further frames for id 9 do not fan out.
        session.handle_inbound(&encode(&FrameKind::Event {
            terminal: Some(TerminalId::local(9)),
            event: AgentEvent::CommandStarted,
        }));
        assert!(out_rx.try_recv().is_err());
    }

    // --- session: lifecycle teardown --------------------------------------

    #[test]
    fn teardown_fails_pending_and_notifies_each_consumer_once() {
        let mut session = RelaySession::new(host());
        let (reply, mut reply_rx) = oneshot::channel();
        let _ = session.handle_request(RelayRequest::Command {
            command: Command::Upgrade,
            reply,
        });
        let (out_tx, mut out_rx) = mpsc::channel(8);
        // Two subscriptions for the same client: one notification.
        session.handle_request(RelayRequest::Subscribe {
            terminal: 1,
            client: ClientId(7),
            out_tx: out_tx.clone(),
        });
        session.handle_request(RelayRequest::Subscribe {
            terminal: 2,
            client: ClientId(7),
            out_tx,
        });

        session.teardown("satellite went away");

        assert!(matches!(
            reply_rx.try_recv().expect("pending failed"),
            CommandResult::Error {
                code: ErrorCode::SatelliteUnreachable,
                ..
            }
        ));
        let Outbound::Frame(frame) = out_rx.try_recv().expect("consumer notified");
        assert!(matches!(
            frame,
            FrameKind::Error {
                request_id: None,
                code: ErrorCode::SatelliteUnreachable,
                ..
            }
        ));
        assert!(
            out_rx.try_recv().is_err(),
            "one typed notification per consumer, not per subscription"
        );
    }

    #[test]
    fn unsubscribe_client_stops_fan_out() {
        let mut session = RelaySession::new(host());
        let (out_tx, mut out_rx) = mpsc::channel(8);
        session.handle_request(RelayRequest::Subscribe {
            terminal: 9,
            client: ClientId(1),
            out_tx,
        });
        session.handle_request(RelayRequest::UnsubscribeClient {
            client: ClientId(1),
        });
        session.handle_inbound(&encode(&FrameKind::Event {
            terminal: Some(TerminalId::local(9)),
            event: AgentEvent::CommandStarted,
        }));
        assert!(out_rx.try_recv().is_err());
    }

    // --- fail-fast + handle backpressure -----------------------------------

    #[tokio::test]
    async fn fail_fast_resolves_commands_with_satellite_unreachable() {
        let (reply, rx) = oneshot::channel();
        fail_fast(
            RelayRequest::Command {
                command: Command::Upgrade,
                reply,
            },
            &host(),
            "backoff",
        );
        assert!(matches!(
            rx.await.expect("resolved"),
            CommandResult::Error {
                code: ErrorCode::SatelliteUnreachable,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn handle_command_fails_fast_when_mailbox_is_full_or_closed() {
        let (handle, mut rx) = RelayHandle::new(host());
        // Fill the bounded mailbox.
        for _ in 0..RELAY_MAILBOX {
            handle.forward(FrameKind::Detach);
        }
        let result = handle.command(Command::Upgrade).await;
        assert!(matches!(
            result,
            CommandResult::Error {
                code: ErrorCode::ResourceExhausted,
                ..
            }
        ));
        // Drop the receiver: the link task is gone.
        rx.close();
        while rx.try_recv().is_ok() {}
        drop(rx);
        let result = handle.command(Command::Upgrade).await;
        assert!(matches!(
            result,
            CommandResult::Error {
                code: ErrorCode::SatelliteUnreachable,
                ..
            }
        ));
    }
}
