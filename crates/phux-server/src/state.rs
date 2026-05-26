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
//! loop"). Per-client tasks are spawned via `tokio::spawn`, which —
//! regardless of runtime flavor — requires `Send + 'static` futures. That
//! rules out the otherwise-cheaper `Rc<RefCell<_>>` here; switching to a
//! local `LocalSet` would require refactoring `accept_loop` and is
//! out-of-scope for `phux-byc.4`.
//!
//! We therefore wrap state in `Arc<Mutex<ServerState>>`. Critical sections
//! are short (microseconds: an `attach` is a few `HashMap` ops), so atomic
//! contention is not a concern in steady state. The `std::sync::Mutex`
//! avoids `tokio::sync::Mutex`'s async-friendly futures-park machinery
//! because every section in this module is sync and finite — we never
//! `.await` while holding it.
//!
//! If a future task moves the server to a `LocalSet`-based per-client
//! task model, this is the file that needs to change: swap
//! `Arc<Mutex<_>>` for `Rc<RefCell<_>>` and drop the `Send` requirement.
//! The public surface of [`SharedState`] is the single seam.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};

use phux_core::ids::{PaneId, SessionId};
use phux_core::registry::Registry;
use phux_core::session::Session;

use crate::id_bridge::IdBridge;
use phux_protocol::diff::ColorSupport;
use phux_protocol::input::focus::FocusEvent;
use phux_protocol::input::key::KeyEvent;
use phux_protocol::input::mouse::MouseEvent;
use phux_protocol::input::paste::PasteEvent;
use thiserror::Error;
use tokio::sync::mpsc;

/// Default per-client outbound mailbox depth.
///
/// Bounded on purpose: a stuck client must not let the server accumulate
/// unbounded backpressure. The exact number is small because outbound
/// frames are *coalesced diffs* (see `SPEC.md` §8), not individual cell
/// updates; eight in-flight diffs is well above steady state.
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
/// `phux-byc.4` only routes [`Hello`] and [`PaneDiff`] for now (the
/// `FrameKind` variants for `ATTACHED` / `DETACHED` / input messages have
/// not yet been added in `phux-protocol`; see the report). The full enum
/// will be `phux_protocol::wire::frame::FrameKind` once those variants
/// land — by ADR-0008, the server does not maintain a parallel frame
/// type, so consumers can use `phux_protocol::wire::frame::FrameKind`
/// directly via this re-export-friendly alias.
///
/// [`Hello`]: phux_protocol::wire::frame::FrameKind::Hello
/// [`PaneDiff`]: phux_protocol::wire::frame::FrameKind::PaneDiff
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
    /// to receive `PANE_DIFF` frames for it).
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
    next_client_id: u64,
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
            next_client_id: 1,
        }
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
    fn shared_state_with_and_with_mut_round_trip() {
        let shared = SharedState::new();
        let (_sid, _wid, pid) = shared.with_mut(|s| s.seed_session("default"));
        let count = shared.with(|s| s.subscribers_for_pane(pid).len());
        assert_eq!(count, 0);
    }
}
