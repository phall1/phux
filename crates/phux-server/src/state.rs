//! Server-side state shared by the listener loop and per-client tasks
//! (`phux-byc.4`).
//!
//! This module owns:
//!
//! * The [`Registry`] of sessions, windows, and panes (the canonical
//!   domain state from `phux-byc.1`/`phux-byc.2`).
//! * The set of currently attached clients ([`AttachedClient`]) keyed by a
//!   server-assigned monotonic [`ClientId`].
//! * The list of subscribers per pane — used to fan diffs out to every client
//!   that is currently observing a pane.
//! * A per-pane input log (`pane_inputs`) where every keystroke, mouse event,
//!   focus change, and paste recorded against a pane is appended. The PTY
//!   side of the pipeline (PTY writer task) reads from this log; for
//!   `phux-byc.4` it serves both as the merge point for multi-client input
//!   and as the inspection surface for tests.
//!
//! # Concurrency model
//!
//! The server runs on a `tokio::runtime::Builder::new_current_thread`
//! executor (see `runtime.rs`, ADR-0003 "one server per user, one event
//! loop"). Per-client tasks are spawned via `tokio::task::spawn_local`
//! onto a [`tokio::task::LocalSet`] (per ADR-0014), so per-client
//! futures are `!Send` and can hold `Rc<RefCell<_>>` if desired.
//!
//! [`ServerState`] itself stays behind `Arc<Mutex<_>>` because the
//! [`crate::pane_actor::PaneHandle`] held inside `panes` is `Send` and
//! the surrounding [`SharedState`] is used in a few sync contexts
//! (pre-seed before `LocalSet` entry, test scaffolding). Critical sections
//! are short (microseconds: a few `HashMap` ops), so atomic contention
//! is not a concern in steady state. The `std::sync::Mutex` avoids
//! `tokio::sync::Mutex`'s async-friendly futures-park machinery because
//! every section in this module is sync and finite — we never `.await`
//! while holding it.

use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, Mutex, MutexGuard};

use phux_core::ids::{PaneId, SessionId, WindowId};
use phux_core::registry::Registry;
use phux_core::session::Session;

use crate::id_bridge::IdBridge;
use crate::pane_actor::PaneHandle;
use phux_protocol::caps::ColorSupport;
use phux_protocol::ids::{PaneId as WirePaneId, WindowId as WireWindowId};
use phux_protocol::input::focus::FocusEvent;
use phux_protocol::input::key::KeyEvent;
use phux_protocol::input::mouse::MouseEvent;
use phux_protocol::input::paste::PasteEvent;
use thiserror::Error;
use tokio::sync::mpsc;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

/// Default per-client outbound mailbox depth.
///
/// Bounded on purpose: a stuck client must not let the server accumulate
/// unbounded backpressure. The exact number is small because outbound
/// frames are *coalesced byte chunks* (see `SPEC.md` §8 and ADR-0013),
/// not individual PTY reads; eight in-flight `PANE_OUTPUT` batches is
/// well above steady state.
pub const DEFAULT_CLIENT_MAILBOX: usize = 8;

/// Server-assigned identifier for an attached client.
///
/// Distinct from [`phux_protocol::ids::ClientId`] (which is the wire-level
/// identity carried in protocol messages): this one is allocated by the
/// server, monotonic from `1`, and used purely for routing inside
/// [`ServerState`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ClientId(pub u64);

/// Per-pane input event recorded against a pane.
///
/// `phux-byc.4` records these into a per-pane log; a future task will turn
/// them into PTY writes. The variant set tracks `SPEC.md` §9 (Input
/// events).
#[derive(Debug, Clone)]
pub enum PaneInput {
    /// A keystroke (`INPUT_KEY` on the wire — `SPEC.md` §9.1).
    Key(KeyEvent),
    /// A mouse event (`INPUT_MOUSE` — `SPEC.md` §9.2).
    Mouse(MouseEvent),
    /// A focus gained/lost notification (`INPUT_FOCUS` — `SPEC.md` §9.3).
    Focus(FocusEvent),
    /// A bracketed paste (`INPUT_PASTE` — `SPEC.md` §9.4).
    Paste(PasteEvent),
}

/// A frame queued on a client's outbound mailbox.
///
/// Aliased to `phux_protocol::wire::frame::FrameKind` so consumers can route
/// any variant — `Hello`, `PaneOutput`, `PaneSnapshot`, lifecycle frames —
/// without a parallel server-side enum. Per ADR-0008 / ADR-0013, the
/// protocol crate owns the wire types and the server defers to them.
///
/// PONG (reserved type byte `0xFF`) is not yet a `FrameKind` variant.
/// The writer task supports a side-channel for pre-encoded bytes via
/// [`OutboundMessage`] — see the runtime module.
pub type OutboundFrame = phux_protocol::wire::frame::FrameKind;

/// An attached client: routing identity plus outbound mailbox.
#[derive(Debug)]
pub struct AttachedClient {
    /// Server-assigned client id.
    pub id: ClientId,
    /// The session this client is observing.
    pub session: SessionId,
    /// Outbound mailbox; the per-client write task drains this and writes to
    /// the socket.
    pub tx: mpsc::Sender<OutboundFrame>,
    /// The client's advertised color tier (SPEC §6.2). The server MUST
    /// downsample outbound color values to this tier before fanout —
    /// see [`crate::downsample`] for the helper byc.5's fanout layer
    /// will plug into.
    ///
    /// Defaults to [`ColorSupport::TrueColor`] (most-permissive) for
    /// clients that have not yet advertised caps; this never silently
    /// downgrades. The HELLO/ClientCapabilities handshake (SPEC §6.1)
    /// is NOT wired through yet — see follow-up ticket "Wire
    /// `ColorSupport` through HELLO/ClientCapabilities per SPEC §6.1/§6.2".
    pub color_support: ColorSupport,
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

/// Single owner of all server-side state.
///
/// See the module-level doc for the concurrency model. Wrap this in
/// [`SharedState`] before sharing with per-client tasks.
#[derive(Debug)]
pub struct ServerState {
    /// Canonical domain state.
    pub registry: Registry,
    /// Currently attached clients, keyed by server-assigned id.
    pub attached: HashMap<ClientId, AttachedClient>,
    /// For each pane, the clients currently observing it (and thus eligible
    /// to receive `PANE_OUTPUT` frames for it).
    pub pane_subscribers: HashMap<PaneId, Vec<ClientId>>,
    /// Per-pane input log. Inputs from all attached clients are merged into
    /// the same vec in arrival order; the PTY writer task drains it.
    ///
    /// For `phux-byc.4` no draining consumer exists yet — the log
    /// accumulates and tests inspect it via
    /// [`Self::pane_input_log_for`].
    pane_inputs: HashMap<PaneId, Vec<PaneInput>>,
    /// Bridge between core slotmap [`SessionId`]s and wire-level
    /// `phux_protocol::ids::SessionId` (u32). Lives in this crate (and only
    /// this crate) because `phux-core` and `phux-protocol` must not depend
    /// on each other — see [`crate::id_bridge`] module docs.
    pub session_id_bridge: IdBridge,
    /// Per-pane actor handles, keyed by core [`PaneId`]. The
    /// `PaneHandle` is `Send`; the underlying `PaneActor` (which owns
    /// the `!Send` `Terminal`) lives on the `LocalSet` — see ADR-0014.
    ///
    /// Populated by [`Self::register_pane_handle`] after the actor is
    /// spawned. Looked up by the ATTACH handler to request snapshots
    /// and by future PTY-input branches to forward keystrokes.
    pub panes: HashMap<PaneId, PaneHandle>,
    /// Per-pane cancellation tokens. Cancelling a token fires the
    /// matching `PaneActor`'s shutdown branch (see
    /// `PaneActor::run`'s `select!`). Typically a child of the
    /// per-server root token, so a root cancel cascades to every
    /// pane in one step.
    ///
    /// Distinct from the prior `oneshot::Sender<()>` shutdown channel:
    /// dropping the token does NOT cancel — cancellation must be
    /// explicit (see [`Self::detach_pane_actor`]).
    pane_tokens: HashMap<PaneId, CancellationToken>,
    /// `JoinSet` collecting the `PaneActor::run` futures spawned via
    /// [`Self::spawn_pane_actor`]. Owned at this scope so cancellation
    /// of the per-server root token (or drop of `ServerState`) aborts
    /// every still-running pane actor in one go.
    ///
    /// **Drop-safety note:** `JoinSet<()>` is `Send`, but the futures it
    /// holds are `!Send` (pane actors own a `!Send` `Terminal` per
    /// ADR-0014). They were spawned via `JoinSet::spawn_local`, which
    /// is only legal inside a `LocalSet`. `ServerState` is dropped at
    /// the tail of `runtime::ServerRuntime::run_async` on the same
    /// thread that ran the `LocalSet`, so this `JoinSet`'s `Drop` is
    /// always on the spawning thread — no cross-thread poll of
    /// `!Send` futures occurs.
    pane_tasks: JoinSet<()>,
    /// Wire-side identifier for each core pane id. Allocated
    /// monotonically from `1` in [`Self::register_pane_handle`]. Mirrors
    /// the `IdBridge` shape used for session ids — kept inline because
    /// adding a second `IdBridge` generic over an arbitrary id pair is
    /// out of scope for `phux-byc.8` (the session bridge has its own
    /// reverse-lookup story; pane reverse lookup is needed too for
    /// future `INPUT_KEY` routing).
    pane_wire_forward: HashMap<PaneId, WirePaneId>,
    pane_wire_reverse: HashMap<WirePaneId, PaneId>,
    next_pane_wire_id: u32,
    /// Wire-side identifier for each core window id. Same shape as
    /// the pane bridge above; used to populate [`WindowInfo::id`] in
    /// the `ATTACHED` snapshot.
    window_wire_forward: HashMap<WindowId, WireWindowId>,
    window_wire_reverse: HashMap<WireWindowId, WindowId>,
    next_window_wire_id: u32,
    next_client_id: u64,
    /// The most-recently-attached session, used to resolve
    /// [`phux_protocol::wire::frame::AttachTarget::Last`].
    ///
    /// Recorded by the runtime ATTACH handler after a successful
    /// `attach()` (any variant — `ByName`, `ById`, eventually `Last`
    /// itself once chained re-attach exists). Stays `None` until the
    /// first successful attach.
    ///
    /// **Memory model: global, per-server.** A single slot, shared
    /// across all clients. Rationale: phux is single-user — the
    /// canonical workflow is "attach → detach → attach again later"
    /// from the same human. A global slot captures that intent with
    /// minimum state. A per-client model would be more accurate when
    /// multiple humans drive distinct connections, but that's not the
    /// shipping workload; the field can be migrated to a per-connection
    /// memory (e.g. carried on the per-client task's stack) without
    /// changing the wire surface.
    last_attached_session: Option<SessionId>,
}

impl Default for ServerState {
    fn default() -> Self {
        Self::new()
    }
}

impl ServerState {
    /// Build an empty state.
    #[must_use]
    pub fn new() -> Self {
        Self {
            registry: Registry::new(),
            attached: HashMap::new(),
            pane_subscribers: HashMap::new(),
            pane_inputs: HashMap::new(),
            session_id_bridge: IdBridge::new(),
            panes: HashMap::new(),
            pane_tokens: HashMap::new(),
            pane_tasks: JoinSet::new(),
            pane_wire_forward: HashMap::new(),
            pane_wire_reverse: HashMap::new(),
            next_pane_wire_id: 1,
            window_wire_forward: HashMap::new(),
            window_wire_reverse: HashMap::new(),
            next_window_wire_id: 1,
            next_client_id: 1,
            last_attached_session: None,
        }
    }

    /// Most-recently-attached session, if any. Resolves
    /// `AttachTarget::Last`. Returns the raw [`SessionId`] without
    /// validating that the session is still live — callers must
    /// re-check against the registry (a session may have been killed
    /// between the prior attach and this lookup).
    #[must_use]
    pub const fn last_attached_session(&self) -> Option<SessionId> {
        self.last_attached_session
    }

    /// Record `session` as the most-recently-attached session. Called
    /// by the runtime ATTACH handler after a successful attach.
    ///
    /// Overwrites unconditionally — the contract is "last", not
    /// "first" or "most-popular".
    pub const fn set_last_attached_session(&mut self, session: SessionId) {
        self.last_attached_session = Some(session);
    }

    /// Allocate the next monotonic [`ClientId`].
    ///
    /// Ids are never reused. `0` is intentionally skipped so log entries
    /// printing `client=0` are obviously a placeholder, not a real client.
    pub const fn new_client_id(&mut self) -> ClientId {
        let id = ClientId(self.next_client_id);
        self.next_client_id = self.next_client_id.saturating_add(1);
        id
    }

    /// Attach a client to the session with `session_name`.
    ///
    /// On success the client is recorded in `attached` and subscribed to the
    /// session's currently active pane (if any). Returns a borrow of the
    /// [`Session`] for callers that want to build an `ATTACHED` snapshot.
    pub fn attach(
        &mut self,
        client_id: ClientId,
        session_name: &str,
        tx: mpsc::Sender<OutboundFrame>,
    ) -> Result<SessionId, AttachError> {
        if self.attached.contains_key(&client_id) {
            return Err(AttachError::AlreadyAttached(client_id));
        }
        let session_id = self
            .find_session_by_name(session_name)
            .ok_or_else(|| AttachError::UnknownSession(session_name.to_owned()))?;

        self.attached.insert(
            client_id,
            AttachedClient {
                id: client_id,
                session: session_id,
                tx,
                // Default tier until HELLO wiring (SPEC §6.1) lands;
                // most-permissive so we never silently downgrade.
                color_support: ColorSupport::default(),
            },
        );

        // Subscribe to the session's active pane if there is one. This is the
        // first cut; richer subscription (every visible pane, dynamic
        // re-subscription on `FOCUS_CHANGED`) lives in `SUBSCRIBE` (§7.4)
        // and is deferred per SPEC.
        if let Some(active_pane) = self.active_pane_of_session(session_id) {
            self.pane_subscribers
                .entry(active_pane)
                .or_default()
                .push(client_id);
        }
        Ok(session_id)
    }

    /// Detach `client_id`, removing it from `attached` and from every
    /// `pane_subscribers` list it appears in.
    ///
    /// Silent no-op if the client is not currently attached — detach must be
    /// idempotent for the EOF cleanup path in `handle_client`.
    pub fn detach(&mut self, client_id: ClientId) {
        self.attached.remove(&client_id);
        for subs in self.pane_subscribers.values_mut() {
            subs.retain(|c| *c != client_id);
        }
        // Drop entries that became empty so the map doesn't grow unboundedly
        // across attach/detach churn.
        self.pane_subscribers.retain(|_, subs| !subs.is_empty());
    }

    /// Subscribers (snapshot) for `pane`. Returns an empty slice if no
    /// clients are currently observing the pane.
    #[must_use]
    pub fn subscribers_for_pane(&self, pane: PaneId) -> &[ClientId] {
        self.pane_subscribers.get(&pane).map_or(&[], Vec::as_slice)
    }

    /// Append `input` to the per-pane log. The log is shared across all
    /// attached clients of the pane's session; this is the merge point for
    /// multi-client keystrokes.
    pub fn record_pane_input(&mut self, pane: PaneId, input: PaneInput) {
        self.pane_inputs.entry(pane).or_default().push(input);
    }

    /// Look up the active pane of the active window of `session`, if any.
    #[must_use]
    pub fn active_pane_of_session(&self, session: SessionId) -> Option<PaneId> {
        let session = self.registry.session(session)?;
        let window_id = session.active?;
        let window = self.registry.window(window_id)?;
        window.active
    }

    /// Borrow the session named `name`, if it exists.
    #[must_use]
    pub fn session_by_name(&self, name: &str) -> Option<&Session> {
        let id = self.find_session_by_name(name)?;
        self.registry.session(id)
    }

    /// Look up the [`SessionId`] for a name by scanning the registry.
    ///
    /// Uses [`Registry::sessions`] directly — no side ledger required.
    fn find_session_by_name(&self, name: &str) -> Option<SessionId> {
        self.registry
            .sessions()
            .find(|(_, s)| s.name == name)
            .map(|(id, _)| id)
    }

    /// Seed a session+window+pane. Returns the new
    /// `(SessionId, WindowId, PaneId)`.
    ///
    /// This is the entry point `ServerConfig::pre_seeded_session` uses to
    /// pre-populate the registry before clients connect.
    ///
    /// # Panics
    ///
    /// Panics if the registry rejects the freshly-allocated session or
    /// window ids — both branches are unreachable because the parent
    /// entity was created on the line above. A panic here indicates a
    /// `phux-core::Registry` regression.
    #[allow(clippy::expect_used, reason = "unreachable: parent just created")]
    pub fn seed_session(&mut self, name: &str) -> (SessionId, phux_core::ids::WindowId, PaneId) {
        let sid = self.registry.new_session(name.to_owned());
        let wid = self.registry.new_window(sid).expect("session just created");
        let pid = self.registry.new_pane(wid).expect("window just created");
        (sid, wid, pid)
    }

    /// Test-only: snapshot the per-pane input log.
    ///
    /// Exposed behind `#[cfg(test)]` so integration tests can assert
    /// keystroke merge ordering and no-dup invariants without needing to
    /// drain a real PTY consumer.
    #[cfg(test)]
    #[must_use]
    pub fn pane_input_log_for(&self, pane: PaneId) -> Vec<PaneInput> {
        self.pane_inputs.get(&pane).cloned().unwrap_or_default()
    }

    /// Record a freshly-spawned [`PaneHandle`] against `pane` and
    /// allocate its wire id.
    ///
    /// Called by the runtime after `PaneActor::new` /
    /// `build_with_token`. Subsequent attaches use
    /// [`Self::pane_handle`] to look the handle up.
    ///
    /// `token` is stashed in `pane_tokens`; cancelling it (e.g. via
    /// [`Self::detach_pane_actor`]) fires the actor's shutdown branch.
    ///
    /// This method does NOT spawn the actor — pair it with
    /// [`Self::spawn_pane_actor`] when you also want the actor task
    /// registered against the per-server `JoinSet`.
    ///
    /// Idempotent on the wire-id allocation (a second call for the
    /// same `pane` returns the same wire id) but overwrites the
    /// `PaneHandle` / token. In practice the runtime calls this
    /// exactly once per pane lifetime.
    pub fn register_pane_handle(
        &mut self,
        pane: PaneId,
        handle: PaneHandle,
        token: CancellationToken,
    ) -> WirePaneId {
        let wire = self.intern_pane_wire(pane);
        self.panes.insert(pane, handle);
        self.pane_tokens.insert(pane, token);
        wire
    }

    /// One-shot helper: register `handle`/`token` AND spawn
    /// `actor_future` onto the per-server pane `JoinSet`. Must be
    /// called from inside a `LocalSet` (per ADR-0014; pane actors
    /// own `!Send` `Terminal`s and are spawned via
    /// `JoinSet::spawn_local`).
    ///
    /// Returns the wire pane id, matching [`Self::register_pane_handle`].
    pub fn spawn_pane_actor<F>(
        &mut self,
        pane: PaneId,
        handle: PaneHandle,
        token: CancellationToken,
        actor_future: F,
    ) -> WirePaneId
    where
        F: Future<Output = ()> + 'static,
    {
        let wire = self.register_pane_handle(pane, handle, token);
        self.pane_tasks.spawn_local(actor_future);
        wire
    }

    /// Cancel `pane`'s actor token, signalling the `PaneActor` to
    /// exit, and forget the token. Idempotent. Used by future
    /// pane-close lifecycle paths; not exercised by `phux-byc.8`.
    ///
    /// The actor task itself is drained from the per-server `JoinSet`
    /// when it returns from `run`; we don't need to touch
    /// `pane_tasks` here.
    pub fn detach_pane_actor(&mut self, pane: PaneId) {
        if let Some(token) = self.pane_tokens.remove(&pane) {
            token.cancel();
        }
    }

    /// Look up the [`PaneHandle`] for `pane`, if registered.
    #[must_use]
    pub fn pane_handle(&self, pane: PaneId) -> Option<&PaneHandle> {
        self.panes.get(&pane)
    }

    /// Wire pane id for `pane`, allocating one if needed.
    ///
    /// Mirrors [`IdBridge::intern`] but inline here for pane ids — see
    /// the field-level note on `pane_wire_forward` for why a second
    /// general-purpose `IdBridge` is deferred.
    pub fn intern_pane_wire(&mut self, pane: PaneId) -> WirePaneId {
        if let Some(w) = self.pane_wire_forward.get(&pane) {
            return *w;
        }
        let raw = self.next_pane_wire_id;
        self.next_pane_wire_id = self.next_pane_wire_id.saturating_add(1);
        let wire = WirePaneId(raw);
        self.pane_wire_forward.insert(pane, wire);
        self.pane_wire_reverse.insert(wire, pane);
        wire
    }

    /// Reverse lookup: which core pane id (if any) does `wire`
    /// resolve to?
    #[must_use]
    pub fn pane_from_wire(&self, wire: WirePaneId) -> Option<PaneId> {
        self.pane_wire_reverse.get(&wire).copied()
    }

    /// Wire window id for `window`, allocating one if needed.
    pub fn intern_window_wire(&mut self, window: WindowId) -> WireWindowId {
        if let Some(w) = self.window_wire_forward.get(&window) {
            return *w;
        }
        let raw = self.next_window_wire_id;
        self.next_window_wire_id = self.next_window_wire_id.saturating_add(1);
        let wire = WireWindowId(raw);
        self.window_wire_forward.insert(window, wire);
        self.window_wire_reverse.insert(wire, window);
        wire
    }

    /// Build a [`SessionSnapshot`] describing the entire registry plus
    /// the attaching client's initial focus.
    ///
    /// Used by the ATTACH handler in [`crate::runtime`] to populate the
    /// `ATTACHED` frame per SPEC §13. Allocates wire ids on demand so
    /// every entity in the registry gets one before this returns.
    ///
    /// `focus_session` is the resolved target of the ATTACH request;
    /// the attaching client's focused window/pane fall back to the
    /// session's `active` / window's `active` (tmux semantics).
    /// Returns `None` if `focus_session` has no active window or pane,
    /// since `SessionSnapshot::focused_window` / `focused_pane` are
    /// required fields on the wire.
    pub fn build_session_snapshot(
        &mut self,
        focus_session: SessionId,
    ) -> Option<phux_protocol::wire::info::SessionSnapshot> {
        use phux_protocol::wire::info::{PaneInfo, SessionInfo, SessionSnapshot, WindowInfo};

        let attached_counts: HashMap<SessionId, u16> = {
            let mut counts: HashMap<SessionId, u16> = HashMap::new();
            for c in self.attached.values() {
                *counts.entry(c.session).or_insert(0) = counts
                    .get(&c.session)
                    .copied()
                    .unwrap_or(0)
                    .saturating_add(1);
            }
            counts
        };

        let session_pairs: Vec<(SessionId, Session)> = self
            .registry
            .sessions()
            .map(|(id, s)| (id, s.clone()))
            .collect();

        let mut sessions = Vec::with_capacity(session_pairs.len());
        let mut windows = Vec::new();
        let mut panes = Vec::new();

        for (sid, session) in &session_pairs {
            let session_wire = self.session_id_bridge.intern(*sid);
            // Pre-intern the active window so `active_window` round-trips.
            let active_window_wire = session.active.map(|w| self.intern_window_wire(w));

            let created_at_unix_secs = session
                .created_at
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
                .unwrap_or(0);
            sessions.push(
                SessionInfo::new(session_wire, session.name.clone())
                    .with_active_window(active_window_wire)
                    .with_created_at_unix_secs(created_at_unix_secs)
                    .with_window_count(u16::try_from(session.windows.len()).unwrap_or(u16::MAX))
                    .with_attached_client_count(attached_counts.get(sid).copied().unwrap_or(0)),
            );

            for (index, wid) in session.windows.iter().enumerate() {
                let Some(window) = self.registry.window(*wid).cloned() else {
                    continue;
                };
                let window_wire = self.intern_window_wire(*wid);
                let active_pane_wire = window.active.map(|p| self.intern_pane_wire(p));

                // Layout-on-the-wire mirroring is its own concern;
                // for phux-byc.8 we ship `None` and let later tickets
                // translate `phux_core::LayoutNode` →
                // `phux_protocol::wire::info::LayoutNode`.
                windows.push(
                    WindowInfo::new(window_wire, session_wire, format!("window-{index}"))
                        .with_index(u16::try_from(index).unwrap_or(u16::MAX))
                        .with_active_pane(active_pane_wire),
                );

                for pid in &window.panes {
                    let Some(pane) = self.registry.pane(*pid).cloned() else {
                        continue;
                    };
                    let pane_wire = self.intern_pane_wire(*pid);
                    let cwd =
                        Some(pane.cwd.to_string_lossy().into_owned()).filter(|s| !s.is_empty());
                    panes.push(
                        PaneInfo::new(pane_wire, window_wire, pane.dims.0, pane.dims.1)
                            .with_title(pane.title.clone())
                            .with_cwd(cwd),
                    );
                }
            }
        }

        let session = self.registry.session(focus_session)?;
        let focused_window = session.active?;
        let focused_pane = self.registry.window(focused_window)?.active?;

        let focused_session_wire = self.session_id_bridge.intern(focus_session);
        let focused_window_wire = self.intern_window_wire(focused_window);
        let focused_pane_wire = self.intern_pane_wire(focused_pane);

        Some(
            SessionSnapshot::new(focused_session_wire, focused_window_wire, focused_pane_wire)
                .with_sessions(sessions)
                .with_windows(windows)
                .with_panes(panes),
        )
    }
}

/// Convenience newtype: `Arc<Mutex<ServerState>>`. This is the type
/// per-client tasks clone and hold.
///
/// Usage rules:
/// * Lock for as short as possible — never `.await` while the guard is
///   held. Every section in this crate is sync and finite.
/// * Use [`Self::with`] / [`Self::with_mut`] for scoped access; they
///   panic if the mutex is poisoned (i.e. a previous holder panicked),
///   which is the bug-finding behavior we want at this stage.
#[derive(Debug, Clone, Default)]
pub struct SharedState(Arc<Mutex<ServerState>>);

impl SharedState {
    /// Wrap a fresh [`ServerState`].
    #[must_use]
    pub fn new() -> Self {
        Self(Arc::new(Mutex::new(ServerState::new())))
    }

    /// Wrap a caller-built state. Useful when the caller has already pre-
    /// seeded sessions via `seed_session` and wants the prepared state
    /// shared across tasks.
    #[must_use]
    pub fn from_state(state: ServerState) -> Self {
        Self(Arc::new(Mutex::new(state)))
    }

    /// Lock the state. Prefer [`Self::with`] / [`Self::with_mut`] when
    /// possible.
    ///
    /// # Panics
    ///
    /// Panics if the mutex was poisoned (a previous holder panicked while
    /// holding the lock). In a current-thread tokio server that means a
    /// per-client task crashed mid-mutation; the conservative response is
    /// to crash the server rather than continue with potentially
    /// inconsistent state.
    #[allow(clippy::expect_used, reason = "poison panic is the intended behavior")]
    pub fn lock(&self) -> MutexGuard<'_, ServerState> {
        self.0.lock().expect("ServerState mutex poisoned")
    }

    /// Scoped immutable access.
    pub fn with<R>(&self, f: impl FnOnce(&ServerState) -> R) -> R {
        f(&self.lock())
    }

    /// Scoped mutable access.
    pub fn with_mut<R>(&self, f: impl FnOnce(&mut ServerState) -> R) -> R {
        f(&mut self.lock())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mk_tx() -> mpsc::Sender<OutboundFrame> {
        let (tx, _rx) = mpsc::channel::<OutboundFrame>(DEFAULT_CLIENT_MAILBOX);
        tx
    }

    #[test]
    fn new_client_id_is_monotonic_from_one() {
        let mut s = ServerState::new();
        assert_eq!(s.new_client_id(), ClientId(1));
        assert_eq!(s.new_client_id(), ClientId(2));
        assert_eq!(s.new_client_id(), ClientId(3));
    }

    #[test]
    fn attach_unknown_session_returns_error() {
        let mut s = ServerState::new();
        let cid = s.new_client_id();
        let err = s.attach(cid, "ghost", mk_tx()).unwrap_err();
        assert_eq!(err, AttachError::UnknownSession("ghost".to_owned()));
    }

    #[test]
    fn attach_records_client_and_subscribes_to_active_pane() {
        let mut s = ServerState::new();
        let (sid, _wid, pid) = s.seed_session("default");
        let cid = s.new_client_id();
        let returned_sid = s.attach(cid, "default", mk_tx()).unwrap();
        assert_eq!(returned_sid, sid);
        assert!(s.attached.contains_key(&cid));
        assert_eq!(s.subscribers_for_pane(pid), &[cid]);
    }

    #[test]
    fn second_attach_for_same_client_returns_already_attached() {
        let mut s = ServerState::new();
        let _ = s.seed_session("default");
        let cid = s.new_client_id();
        s.attach(cid, "default", mk_tx()).unwrap();
        let err = s.attach(cid, "default", mk_tx()).unwrap_err();
        assert_eq!(err, AttachError::AlreadyAttached(cid));
    }

    #[test]
    fn two_clients_attach_same_session_see_same_active_pane() {
        let mut s = ServerState::new();
        let (_sid, _wid, pid) = s.seed_session("default");
        let a = s.new_client_id();
        let b = s.new_client_id();
        s.attach(a, "default", mk_tx()).unwrap();
        s.attach(b, "default", mk_tx()).unwrap();
        let subs = s.subscribers_for_pane(pid);
        assert!(subs.contains(&a) && subs.contains(&b));
        assert_eq!(subs.len(), 2);
    }

    #[test]
    fn detach_removes_client_and_drops_empty_subscriber_lists() {
        let mut s = ServerState::new();
        let (_sid, _wid, pid) = s.seed_session("default");
        let cid = s.new_client_id();
        s.attach(cid, "default", mk_tx()).unwrap();
        assert!(!s.subscribers_for_pane(pid).is_empty());
        s.detach(cid);
        assert!(!s.attached.contains_key(&cid));
        assert!(s.subscribers_for_pane(pid).is_empty());
        assert!(s.pane_subscribers.is_empty(), "empty lists should be GC'd");
    }

    #[test]
    fn detach_is_idempotent() {
        let mut s = ServerState::new();
        let cid = ClientId(99);
        // Not attached at all — must not panic.
        s.detach(cid);
        s.detach(cid);
    }

    #[test]
    fn record_pane_input_appends_in_call_order() {
        use phux_protocol::input::key::{KeyAction, KeyEvent, ModSet, PhysicalKey};
        let mut s = ServerState::new();
        let (_sid, _wid, pid) = s.seed_session("default");

        let mk = |k: PhysicalKey, text: &str| KeyEvent {
            action: KeyAction::Press,
            key: k,
            mods: ModSet::empty(),
            consumed_mods: ModSet::empty(),
            composing: false,
            text: Some(text.to_owned()),
            unshifted_codepoint: Some(text.chars().next().unwrap() as u32),
        };

        s.record_pane_input(pid, PaneInput::Key(mk(PhysicalKey::A, "a")));
        s.record_pane_input(pid, PaneInput::Key(mk(PhysicalKey::B, "b")));
        s.record_pane_input(pid, PaneInput::Key(mk(PhysicalKey::C, "c")));

        let log = s.pane_input_log_for(pid);
        assert_eq!(log.len(), 3);
        let texts: Vec<String> = log
            .into_iter()
            .map(|pi| match pi {
                PaneInput::Key(k) => k.text.unwrap_or_default(),
                _ => unreachable!(),
            })
            .collect();
        assert_eq!(texts, vec!["a", "b", "c"]);
    }

    #[test]
    fn attached_client_color_support_defaults_to_truecolor() {
        // Default tier until HELLO wiring lands. If this assertion ever
        // needs to change, also update the comment in `attach()` and
        // the deferral note on `AttachedClient::color_support`.
        let mut s = ServerState::new();
        let _ = s.seed_session("default");
        let cid = s.new_client_id();
        s.attach(cid, "default", mk_tx()).unwrap();
        let client = s.attached.get(&cid).unwrap();
        assert_eq!(client.color_support, ColorSupport::TrueColor);
    }

    #[test]
    fn last_attached_session_starts_none_and_round_trips() {
        let mut s = ServerState::new();
        assert!(
            s.last_attached_session().is_none(),
            "fresh state has no prior-attach memory",
        );
        let (sid, _wid, _pid) = s.seed_session("default");
        s.set_last_attached_session(sid);
        assert_eq!(s.last_attached_session(), Some(sid));

        // Overwrite semantics: setting again replaces the previous slot.
        let (sid2, _w, _p) = s.seed_session("other");
        s.set_last_attached_session(sid2);
        assert_eq!(s.last_attached_session(), Some(sid2));
    }

    #[test]
    fn shared_state_with_and_with_mut_round_trip() {
        let shared = SharedState::new();
        let (_sid, _wid, pid) = shared.with_mut(|s| s.seed_session("default"));
        let count = shared.with(|s| s.subscribers_for_pane(pid).len());
        assert_eq!(count, 0);
    }
}
