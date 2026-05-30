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
//! * A per-pane input log (`terminal_inputs`) where every keystroke, mouse event,
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
//! [`crate::terminal_actor::TerminalHandle`] held inside `panes` is `Send` and
//! the surrounding [`SharedState`] is used in a few sync contexts
//! (pre-seed before `LocalSet` entry, test scaffolding). Critical sections
//! are short (microseconds: a few `HashMap` ops), so atomic contention
//! is not a concern in steady state. The `std::sync::Mutex` avoids
//! `tokio::sync::Mutex`'s async-friendly futures-park machinery because
//! every section in this module is sync and finite — we never `.await`
//! while holding it.

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::sync::{Arc, Mutex, MutexGuard};

use bytes::BytesMut;
use phux_core::ids::{SessionId, TerminalId, WindowId};
use phux_core::registry::Registry;
use phux_core::session::Session;

use crate::id_bridge::IdBridge;
use crate::terminal_actor::TerminalHandle;
use phux_protocol::caps::{ClientCapabilities, ColorSupport, Layer, LayerSet};
use phux_protocol::ids::{CollectionId, TerminalId as WireTerminalId, WindowId as WireWindowId};
use phux_protocol::input::focus::FocusEvent;
use phux_protocol::input::key::KeyEvent;
use phux_protocol::input::mouse::MouseEvent;
use phux_protocol::input::paste::PasteEvent;
use phux_protocol::wire::frame::{FrameKind, Scope};
use portable_pty::CommandBuilder;
use thiserror::Error;
use tokio::sync::mpsc;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

/// Default per-client outbound mailbox depth.
///
/// Bounded on purpose: a stuck client must not let the server accumulate
/// unbounded backpressure. The exact number is small because outbound
/// frames are *coalesced byte chunks* (see `docs/spec/L1.md` §2 and ADR-0013),
/// not individual PTY reads; eight in-flight `TERMINAL_OUTPUT` batches is
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
/// them into PTY writes. The variant set tracks `docs/spec/input.md` (Input
/// events).
#[derive(Debug, Clone)]
pub enum TerminalInput {
    /// A keystroke (`INPUT_KEY` on the wire — `docs/spec/input.md` §2).
    Key(KeyEvent),
    /// A mouse event (`INPUT_MOUSE` — `docs/spec/input.md` §3).
    Mouse(MouseEvent),
    /// A focus gained/lost notification (`INPUT_FOCUS` — `docs/spec/input.md` §4).
    Focus(FocusEvent),
    /// A bracketed paste (`INPUT_PASTE` — `docs/spec/input.md` §5).
    Paste(PasteEvent),
}

/// A message queued on a client's outbound mailbox.
///
/// The writer task drains a single channel of [`Outbound`] and routes each
/// item to one of two write paths:
///
/// * [`Outbound::Frame`] carries a [`phux_protocol::wire::frame::FrameKind`]
///   and is encoded via `FrameKind::encode` before being written. Per
///   ADR-0008 / ADR-0013 the protocol crate owns the wire types and the
///   server defers to them for any variant — `Hello`, `TerminalOutput`,
///   `TerminalSnapshot`, lifecycle frames, and so on.
/// * [`Outbound::Raw`] carries pre-encoded bytes that bypass the encoder.
///   This is currently used only by the PONG path, because PONG (reserved
///   type byte `0xFF`) is not yet a `FrameKind` variant. Once the protocol
///   crate lifts `Pong` into the enum, this variant can go away and PONG
///   collapses to a structured send through `Outbound::Frame`.
#[derive(Debug)]
pub enum Outbound {
    /// A structured frame; the writer encodes it before writing.
    Frame(phux_protocol::wire::frame::FrameKind),
    /// A pre-encoded byte blob; the writer writes it as-is.
    Raw(BytesMut),
}

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
    /// to receive `TERMINAL_OUTPUT` frames for it).
    pub terminal_subscribers: HashMap<TerminalId, Vec<ClientId>>,
    /// Per-pane input log. Inputs from all attached clients are merged into
    /// the same vec in arrival order; the PTY writer task drains it.
    ///
    /// For `phux-byc.4` no draining consumer exists yet — the log
    /// accumulates and tests inspect it via
    /// [`Self::terminal_input_log_for`].
    terminal_inputs: HashMap<TerminalId, Vec<TerminalInput>>,
    /// Bridge between core slotmap [`SessionId`]s and wire-level
    /// `phux_protocol::ids::SessionId` (u32). Lives in this crate (and only
    /// this crate) because `phux-core` and `phux-protocol` must not depend
    /// on each other — see [`crate::id_bridge`] module docs.
    pub session_id_bridge: IdBridge,
    /// Per-pane actor handles, keyed by core [`TerminalId`]. The
    /// `TerminalHandle` is `Send`; the underlying `TerminalActor` (which owns
    /// the `!Send` `Terminal`) lives on the `LocalSet` — see ADR-0014.
    ///
    /// Populated by [`Self::register_terminal_handle`] after the actor is
    /// spawned. Looked up by the ATTACH handler to request snapshots
    /// and by future PTY-input branches to forward keystrokes.
    pub terminals: HashMap<TerminalId, TerminalHandle>,
    /// Per-pane cancellation tokens. Cancelling a token fires the
    /// matching `TerminalActor`'s shutdown branch (see
    /// `TerminalActor::run`'s `select!`). Typically a child of the
    /// per-server root token, so a root cancel cascades to every
    /// pane in one step.
    ///
    /// Distinct from the prior `oneshot::Sender<()>` shutdown channel:
    /// dropping the token does NOT cancel — cancellation must be
    /// explicit (see [`Self::detach_terminal_actor`]).
    terminal_tokens: HashMap<TerminalId, CancellationToken>,
    /// `JoinSet` collecting the `TerminalActor::run` futures spawned via
    /// [`Self::spawn_terminal_actor`]. Owned at this scope so cancellation
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
    terminal_tasks: JoinSet<()>,
    /// Wire-side identifier for each core pane id. Allocated
    /// monotonically from `1` in [`Self::register_terminal_handle`]. Mirrors
    /// the `IdBridge` shape used for session ids — kept inline because
    /// adding a second `IdBridge` generic over an arbitrary id pair is
    /// out of scope for `phux-byc.8` (the session bridge has its own
    /// reverse-lookup story; pane reverse lookup is needed too for
    /// future `INPUT_KEY` routing).
    terminal_wire_forward: HashMap<TerminalId, WireTerminalId>,
    terminal_wire_reverse: HashMap<WireTerminalId, TerminalId>,
    next_terminal_wire_id: u32,
    /// Wire-side identifier for each core window id. Same shape as
    /// the pane bridge above; used to populate [`WindowInfo::id`] in
    /// the `ATTACHED` snapshot.
    window_wire_forward: HashMap<WindowId, WireWindowId>,
    window_wire_reverse: HashMap<WireWindowId, WindowId>,
    next_window_wire_id: u32,
    next_client_id: u64,
    /// Per-session last-touch order used to resolve
    /// [`phux_protocol::wire::frame::AttachTarget::Last`].
    ///
    /// Updated by the runtime after successful attach and after valid
    /// input/focus dispatch. The value is a server-local monotonic
    /// timestamp: ordering is the only observable contract.
    session_last_touched: HashMap<SessionId, u64>,
    next_touch_timestamp: u64,
    /// Per-scope K/V store backing SPEC §7.4 / §11.L3 metadata.
    ///
    /// Three independently-keyed maps mirror the three `Scope` variants
    /// on the wire. Values are opaque `Vec<u8>`; the server enforces
    /// nothing beyond per-key size limits (currently un-enforced; the
    /// SPEC §11.L3 recommended 256 KiB cap is a follow-up).
    metadata: MetadataStore,
    /// Per-client cache of the negotiated [`LayerSet`] from HELLO (SPEC
    /// §6.2). The dispatcher consults this before emitting any L3
    /// frame; non-L3 consumers MUST NOT see `METADATA_CHANGED` (SPEC
    /// §16.4). Default for a client that never sent HELLO (test
    /// scaffolding) is [`LayerSet::all`] — the most-permissive default
    /// keeps test setups simple; production clients always advertise.
    client_layers: HashMap<ClientId, LayerSet>,
    /// Whether `AttachTarget::CreateIfMissing` (phux-k61.3) should spawn
    /// a real PTY-backed actor for the newly created session's seed
    /// pane. Mirrors [`crate::runtime::ServerConfig::seed_with_pty`] so
    /// the attach-time creation path matches the server's startup
    /// configuration. Set by the runtime via
    /// [`Self::set_attach_create_pty`] right after `SharedState::new`.
    ///
    /// Defaults to `false` so tests that never call the setter exercise
    /// the cheaper no-PTY actor (matches every existing integration
    /// test that uses `spawn_server`).
    attach_create_seeds_pty: bool,
    /// Optional pre-built `CommandBuilder` used when
    /// [`Self::attach_create_seeds_pty`] is `true` and `CreateIfMissing`
    /// fires. `None` falls back to
    /// [`crate::terminal_actor::default_shell_command`] (the user's
    /// `$SHELL`, or `/bin/sh`).
    attach_create_seed_command: Option<CommandBuilder>,
    /// Lines of scrollback retained per pane (`defaults.history-limit`).
    /// Mirrors [`crate::runtime::ServerConfig::history_limit`] so the
    /// attach-time creation path (`AttachTarget::CreateIfMissing`) and
    /// `SPAWN_TERMINAL` build their `TerminalActor`s with the configured
    /// cap without an extra channel to the runtime. Set by the runtime
    /// via [`Self::set_history_limit`] right after `SharedState::new`.
    ///
    /// Defaults to the `phux_config` schema default so tests that never
    /// call the setter still get a sane bound.
    history_limit: u32,
    /// Whether any client has ever attached to this server.
    ///
    /// Gates the tmux-model self-exit (phux-60s): the server only exits
    /// when its last session is reaped **after** it has served at least
    /// one client. A freshly auto-spawned server whose seed pane dies
    /// before anyone attaches therefore stays alive (empty) instead of
    /// vanishing mid-handshake — the launching `phux` then repopulates it
    /// via `CreateIfMissing`. Without this guard the auto-spawn → attach
    /// flow races the server's own self-exit.
    has_served_client: bool,
}

/// Default Collection identifier exposed by v0.1 servers.
///
/// L2 (Collection lifecycle, SPEC §7.3) is not yet wire-allocated; until
/// it ships, the server exposes a single static Collection that every
/// L3 metadata operation targeting `Scope::Collection` lands in. This is
/// load-bearing for the reference TUI's `phux.tui.layout/v1` key —
/// ADR-0019 ties layout persistence to a Collection scope and the TUI
/// needs a Collection to write into before L2 ships.
pub const DEFAULT_COLLECTION_ID: CollectionId = CollectionId::new(1);

/// Per-scope K/V store for L3 metadata (SPEC §7.4 / §11.L3) plus the
/// matching subscription registry.
///
/// Held inside [`ServerState`] but lifted into its own type so the
/// subscribe / set / delete / list operations live in a focused
/// surface — easier to test, easier to reason about ordering invariants,
/// and a natural home for the per-key size cap once that lands.
#[derive(Debug, Default)]
pub struct MetadataStore {
    /// Per-Terminal key → value. Cleared when the Terminal closes (the
    /// L1 lifecycle that owns the Terminal).
    terminal: HashMap<phux_protocol::ids::TerminalId, HashMap<String, Vec<u8>>>,
    /// Per-Collection key → value.
    collection: HashMap<CollectionId, HashMap<String, Vec<u8>>>,
    /// Global key → value.
    global: HashMap<String, Vec<u8>>,
    /// Active subscriptions: a flat set of `(client, scope, key)` tuples.
    /// Lookup on broadcast is linear in the number of subscriptions; that
    /// is acceptable while subscriptions are sparse (handful per client).
    /// A future ticket may switch this to a `HashMap<(scope, key), Vec<ClientId>>`
    /// if the dispatch path shows up in flame graphs.
    subscriptions: HashSet<(ClientId, Scope, String)>,
}

/// Outcome of a `SET_METADATA` call.
///
/// `Unchanged` means the key already held an identical value, so the
/// server SHOULD suppress the `METADATA_CHANGED` broadcast (it's a noop
/// from every subscriber's perspective).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetadataSetOutcome {
    /// Key did not exist or held a different value; value was written.
    Changed,
    /// Key already held the identical value; no broadcast needed.
    Unchanged,
}

impl MetadataStore {
    /// Get the value at `(scope, key)`, if any.
    #[must_use]
    pub fn get(&self, scope: &Scope, key: &str) -> Option<Vec<u8>> {
        match scope {
            Scope::Terminal(tid) => self.terminal.get(tid).and_then(|m| m.get(key)).cloned(),
            Scope::Collection(cid) => self.collection.get(cid).and_then(|m| m.get(key)).cloned(),
            Scope::Global => self.global.get(key).cloned(),
            // `Scope` is `#[non_exhaustive]`: a forward-compat variant we
            // don't know about returns None. The cleanest default for an
            // unknown scope is "no value present" — the caller's contract
            // is preserved without trapping on unknown bytes.
            _ => None,
        }
    }

    /// Set the value at `(scope, key)`. Returns whether the value
    /// actually changed (so the caller can suppress an unnecessary
    /// broadcast).
    pub fn set(&mut self, scope: &Scope, key: &str, value: Vec<u8>) -> MetadataSetOutcome {
        let bucket: &mut HashMap<String, Vec<u8>> = match scope {
            Scope::Terminal(tid) => self.terminal.entry(tid.clone()).or_default(),
            Scope::Collection(cid) => self.collection.entry(*cid).or_default(),
            Scope::Global => &mut self.global,
            // Unknown forward-compat variant: silently drop the write.
            // SPEC §6 lets newer encoders ship trailing field shapes;
            // here the surface area is "unknown scope, no bucket".
            _ => return MetadataSetOutcome::Unchanged,
        };
        if let Some(prev) = bucket.get(key)
            && prev == &value
        {
            return MetadataSetOutcome::Unchanged;
        }
        bucket.insert(key.to_owned(), value);
        MetadataSetOutcome::Changed
    }

    /// Delete `(scope, key)`. Returns whether the key existed (so the
    /// caller can suppress the broadcast on a true noop).
    pub fn delete(&mut self, scope: &Scope, key: &str) -> bool {
        match scope {
            Scope::Terminal(tid) => self
                .terminal
                .get_mut(tid)
                .and_then(|m| m.remove(key))
                .is_some(),
            Scope::Collection(cid) => self
                .collection
                .get_mut(cid)
                .and_then(|m| m.remove(key))
                .is_some(),
            Scope::Global => self.global.remove(key).is_some(),
            // Unknown forward-compat variant: nothing to delete.
            _ => false,
        }
    }

    /// List every key in `scope` (no values, sorted for determinism).
    #[must_use]
    pub fn list(&self, scope: &Scope) -> Vec<String> {
        let mut keys: Vec<String> = match scope {
            Scope::Terminal(tid) => self
                .terminal
                .get(tid)
                .map(|m| m.keys().cloned().collect())
                .unwrap_or_default(),
            Scope::Collection(cid) => self
                .collection
                .get(cid)
                .map(|m| m.keys().cloned().collect())
                .unwrap_or_default(),
            Scope::Global => self.global.keys().cloned().collect(),
            // Unknown forward-compat variant: empty listing.
            _ => Vec::new(),
        };
        keys.sort();
        keys
    }

    /// Drop every key scoped to `terminal`. Called when the Terminal
    /// closes (the L1 lifecycle that owns the per-Terminal scope — see
    /// the `terminal` field doc). Subscriptions targeting the dead
    /// Terminal are connection-scoped and are reaped on detach, so they
    /// are left untouched here.
    pub fn forget_terminal(&mut self, terminal: &phux_protocol::ids::TerminalId) {
        self.terminal.remove(terminal);
    }

    /// Register `(client, scope, key)` as an active subscription. The
    /// underlying set is idempotent: re-subscribing the same triple is
    /// a noop.
    pub fn subscribe(&mut self, client: ClientId, scope: Scope, key: String) {
        self.subscriptions.insert((client, scope, key));
    }

    /// Drop every subscription owned by `client`. Called on detach.
    pub fn drop_client(&mut self, client: ClientId) {
        self.subscriptions.retain(|(c, _, _)| *c != client);
    }

    /// Collect every client subscribed to `(scope, key)`. Order is
    /// unspecified — callers MUST NOT rely on subscriber iteration order.
    #[must_use]
    pub fn subscribers_for(&self, scope: &Scope, key: &str) -> Vec<ClientId> {
        self.subscriptions
            .iter()
            .filter(|(_, s, k)| s == scope && k == key)
            .map(|(c, _, _)| *c)
            .collect()
    }
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
            terminal_subscribers: HashMap::new(),
            terminal_inputs: HashMap::new(),
            session_id_bridge: IdBridge::new(),
            terminals: HashMap::new(),
            terminal_tokens: HashMap::new(),
            terminal_tasks: JoinSet::new(),
            terminal_wire_forward: HashMap::new(),
            terminal_wire_reverse: HashMap::new(),
            next_terminal_wire_id: 1,
            window_wire_forward: HashMap::new(),
            window_wire_reverse: HashMap::new(),
            next_window_wire_id: 1,
            next_client_id: 1,
            session_last_touched: HashMap::new(),
            next_touch_timestamp: 1,
            metadata: MetadataStore::default(),
            client_layers: HashMap::new(),
            attach_create_seeds_pty: false,
            attach_create_seed_command: None,
            history_limit: phux_config::DefaultsCfg::default().history_limit,
            has_served_client: false,
        }
    }

    /// Configure the PTY mode and seed command used by
    /// `crate::runtime::handle_attach`'s
    /// `AttachTarget::CreateIfMissing` branch (phux-k61.3).
    ///
    /// Called once at server startup to mirror
    /// [`crate::runtime::ServerConfig::seed_with_pty`] /
    /// [`crate::runtime::ServerConfig::seed_command`] into state, so the
    /// attach-time creation path can read them without an extra channel
    /// to the runtime.
    ///
    /// When `with_pty` is `false`, `cmd` is ignored — the create path
    /// spawns a no-PTY actor instead. Setting `cmd = None` with
    /// `with_pty = true` falls back to
    /// [`crate::terminal_actor::default_shell_command`] at create time.
    pub fn set_attach_create_pty(&mut self, with_pty: bool, cmd: Option<CommandBuilder>) {
        self.attach_create_seeds_pty = with_pty;
        self.attach_create_seed_command = cmd;
    }

    /// Read the PTY-mode flag set by [`Self::set_attach_create_pty`].
    #[must_use]
    pub const fn attach_create_seeds_pty(&self) -> bool {
        self.attach_create_seeds_pty
    }

    /// Clone the optional pre-built seed command. Used by the create
    /// path inside `handle_attach`: each `AttachTarget::CreateIfMissing`
    /// that fires gets a fresh clone, so the slot stays populated for
    /// future creates. `CommandBuilder` is `Clone` (per portable-pty
    /// 0.8), so this is cheap.
    #[must_use]
    pub fn attach_create_seed_command(&self) -> Option<CommandBuilder> {
        self.attach_create_seed_command.clone()
    }

    /// Set the per-pane scrollback cap (`defaults.history-limit`) used
    /// by the attach-time creation path and `SPAWN_TERMINAL`. Called
    /// once at server startup to mirror
    /// [`crate::runtime::ServerConfig::history_limit`] into state.
    pub const fn set_history_limit(&mut self, history_limit: u32) {
        self.history_limit = history_limit;
    }

    /// Read the per-pane scrollback cap set by [`Self::set_history_limit`].
    #[must_use]
    pub const fn history_limit(&self) -> u32 {
        self.history_limit
    }

    /// Borrow the L3 metadata store.
    #[must_use]
    pub const fn metadata(&self) -> &MetadataStore {
        &self.metadata
    }

    /// Mutably borrow the L3 metadata store. Use the higher-level
    /// [`Self::metadata_set`] / [`Self::metadata_delete`] /
    /// [`Self::metadata_subscribe`] helpers when you also want the
    /// subscriber-fanout side effects.
    pub const fn metadata_mut(&mut self) -> &mut MetadataStore {
        &mut self.metadata
    }

    /// Record the layer set advertised by `client_id` in HELLO. Called
    /// from the runtime's HELLO handler. Re-set is idempotent (the
    /// most recent HELLO wins, matching `ColorSupport`).
    pub fn set_client_layers(&mut self, client_id: ClientId, layers: LayerSet) {
        self.client_layers.insert(client_id, layers);
    }

    /// Look up the layer set advertised by `client_id`. Defaults to
    /// [`LayerSet::all`] for clients we never saw a HELLO from — the
    /// permissive default matches test scaffolding that skips HELLO.
    #[must_use]
    pub fn client_layers(&self, client_id: ClientId) -> LayerSet {
        self.client_layers
            .get(&client_id)
            .copied()
            .unwrap_or_else(LayerSet::all)
    }

    /// `true` iff `client_id` has L3 in its negotiated `HELLO.layers`.
    /// Gates emission of `METADATA_CHANGED` per SPEC §16.4.
    #[must_use]
    pub fn client_speaks_l3(&self, client_id: ClientId) -> bool {
        self.client_layers(client_id).contains(Layer::L3)
    }

    /// Atomic SET + broadcast: store `value` at `(scope, key)`, then
    /// enqueue a `MetadataChanged` to every L3-capable subscriber
    /// whose subscription matches `(scope, key)`. Silently skips
    /// subscribers that have been detached or whose mailbox is full
    /// (`try_send` semantics — backpressure is a flow-control concern
    /// SPEC §12 doesn't yet cover for L3).
    ///
    /// Returns the set of clients the broadcast was attempted against
    /// (after L3-capability filtering) so callers can assert fanout
    /// shape in tests.
    pub fn metadata_set(&mut self, scope: &Scope, key: &str, value: Vec<u8>) -> Vec<ClientId> {
        // Broadcast first so the borrow of `value` is finished by the time
        // the K/V store consumes it on `set`. The "set before broadcast"
        // ordering is preserved by checking the prior value: if the new
        // bytes equal what's already stored we return early *before*
        // mutating, so subscribers never observe a fake notification.
        let unchanged = self
            .metadata
            .get(scope, key)
            .is_some_and(|prev| prev == value);
        if unchanged {
            return Vec::new();
        }
        let subscribers = self.metadata.subscribers_for(scope, key);
        let delivered = self.broadcast_metadata_changed(&subscribers, scope, key, Some(&value));
        // Commit the write last; `MetadataSetOutcome` is now redundant
        // here but kept on the lower-level API for direct callers.
        let _ = self.metadata.set(scope, key, value);
        delivered
    }

    /// Atomic DELETE + tombstone broadcast. Idempotent: deleting a
    /// missing key returns an empty broadcast set.
    pub fn metadata_delete(&mut self, scope: &Scope, key: &str) -> Vec<ClientId> {
        let existed = self.metadata.delete(scope, key);
        if !existed {
            return Vec::new();
        }
        let subscribers = self.metadata.subscribers_for(scope, key);
        self.broadcast_metadata_changed(&subscribers, scope, key, None)
    }

    /// Register a subscription for `client_id`. The client MUST be
    /// L3-capable (call sites in the runtime gate on
    /// [`Self::client_speaks_l3`] before invoking this).
    pub fn metadata_subscribe(&mut self, client_id: ClientId, scope: Scope, key: String) {
        self.metadata.subscribe(client_id, scope, key);
    }

    /// Helper: fan a `MetadataChanged` frame out to every subscriber in
    /// `subscribers` that is (a) still attached, (b) L3-capable, and
    /// (c) drainable (mailbox not closed). Returns the actually-targeted
    /// client list.
    fn broadcast_metadata_changed(
        &self,
        subscribers: &[ClientId],
        scope: &Scope,
        key: &str,
        value: Option<&[u8]>,
    ) -> Vec<ClientId> {
        let mut delivered = Vec::with_capacity(subscribers.len());
        for client_id in subscribers {
            if !self.client_speaks_l3(*client_id) {
                continue;
            }
            let Some(client) = self.attached.get(client_id) else {
                continue;
            };
            let frame = FrameKind::MetadataChanged {
                scope: scope.clone(),
                key: key.to_owned(),
                value: value.map(<[u8]>::to_vec),
            };
            // `try_send`: the mailbox is bounded (DEFAULT_CLIENT_MAILBOX)
            // and we hold the state mutex synchronously; awaiting on a
            // full mailbox would deadlock the per-client read loop. A
            // dropped notification is acceptable per SPEC §7.4 — the
            // subscriber can re-`GET_METADATA` on next attach.
            if client.tx.try_send(Outbound::Frame(frame)).is_ok() {
                delivered.push(*client_id);
            }
        }
        delivered
    }

    /// Most-recently-touched live session, if any. Resolves
    /// `AttachTarget::Last`.
    #[must_use]
    pub fn most_recently_touched_session(&self) -> Option<SessionId> {
        self.session_last_touched
            .iter()
            .filter(|(sid, _)| self.registry.session(**sid).is_some())
            .max_by_key(|(_, touched_at)| *touched_at)
            .map(|(sid, _)| *sid)
    }

    /// Mark `session` as touched by attach/input/focus activity.
    pub fn touch_session(&mut self, session: SessionId) {
        let touched_at = self.next_touch_timestamp;
        self.next_touch_timestamp = self.next_touch_timestamp.saturating_add(1);
        self.session_last_touched.insert(session, touched_at);
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
    ///
    /// `client_caps` are the capabilities the client advertised in HELLO
    /// (SPEC §6.1/§6.2).
    /// Callers that never observed a HELLO (test scaffolding) MAY pass
    /// [`ClientCapabilities::default`]; the convenience wrapper
    /// [`Self::attach_default_caps`] does that for them.
    pub fn attach(
        &mut self,
        client_id: ClientId,
        session_name: &str,
        tx: mpsc::Sender<Outbound>,
        client_caps: ClientCapabilities,
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
                client_caps,
            },
        );
        // The server has now served at least one client, so the
        // tmux-model self-exit (phux-60s) is armed — see the
        // `has_served_client` field doc.
        self.has_served_client = true;

        // Subscribe to the session's active pane if there is one. This is the
        // first cut; richer subscription (every visible pane, dynamic
        // re-subscription on `FOCUS_CHANGED`) lives in `SUBSCRIBE` (§7.4)
        // and is deferred per SPEC.
        if let Some(active_pane) = self.active_pane_of_session(session_id) {
            self.terminal_subscribers
                .entry(active_pane)
                .or_default()
                .push(client_id);
        }
        Ok(session_id)
    }

    /// Convenience wrapper around [`Self::attach`] that passes
    /// [`ColorSupport::default`] for the client tier. Intended for test
    /// scaffolding and in-tree call sites that don't carry a HELLO-derived
    /// capability value.
    pub fn attach_default_caps(
        &mut self,
        client_id: ClientId,
        session_name: &str,
        tx: mpsc::Sender<Outbound>,
    ) -> Result<SessionId, AttachError> {
        self.attach(client_id, session_name, tx, ClientCapabilities::default())
    }

    /// Update the recorded [`ClientCapabilities`] for an already-attached
    /// client. Returns `false` if the client is not in [`Self::attached`].
    ///
    /// Used by the HELLO handler if a HELLO arrives after ATTACH (out of
    /// spec, but tolerated for forward-compat — the alternative is a
    /// protocol-error close that gives the operator no breadcrumbs).
    pub fn set_client_capabilities(
        &mut self,
        client_id: ClientId,
        client_caps: ClientCapabilities,
    ) -> bool {
        self.attached
            .get_mut(&client_id)
            .map(|c| {
                c.client_caps = client_caps;
            })
            .is_some()
    }

    /// Compatibility wrapper for tests that still update color only.
    pub fn set_client_color_support(
        &mut self,
        client_id: ClientId,
        color_support: ColorSupport,
    ) -> bool {
        self.attached
            .get_mut(&client_id)
            .map(|c| {
                c.client_caps = c.client_caps.with_color_support(color_support);
            })
            .is_some()
    }

    /// Detach `client_id`, removing it from `attached` and from every
    /// `terminal_subscribers` list it appears in.
    ///
    /// Silent no-op if the client is not currently attached — detach must be
    /// idempotent for the EOF cleanup path in `handle_client`.
    pub fn detach(&mut self, client_id: ClientId) {
        self.attached.remove(&client_id);
        for subs in self.terminal_subscribers.values_mut() {
            subs.retain(|c| *c != client_id);
        }
        // Drop entries that became empty so the map doesn't grow unboundedly
        // across attach/detach churn.
        self.terminal_subscribers.retain(|_, subs| !subs.is_empty());
        // Drop any L3 metadata subscriptions this client owned (SPEC §7.4
        // says subscriptions are connection-scoped) plus its cached layer
        // negotiation. Keeps the maps bounded across attach churn.
        self.metadata.drop_client(client_id);
        self.client_layers.remove(&client_id);
    }

    /// Whether any client has ever attached (arms the phux-60s self-exit).
    /// See the `has_served_client` field documentation for the rationale.
    #[must_use]
    pub const fn has_served_client(&self) -> bool {
        self.has_served_client
    }

    /// Reap a pane whose actor has exited, cascading the removal up the
    /// `pane → window → session` tree (phux-60s, the tmux server-lifecycle
    /// model). When the pane's window has no panes left the window is
    /// removed; when that window's session has no windows left the session
    /// is removed.
    ///
    /// Returns `true` iff the server now holds zero sessions — the signal
    /// the runtime uses to self-exit (nothing left to serve). Idempotent on
    /// an unknown or already-reaped pane: it touches nothing and reports the
    /// current emptiness.
    ///
    /// This is the structural counterpart to the `on_terminal_exited`
    /// path in `runtime.rs`: that path detaches clients focused on the
    /// dead pane; this one frees the domain entities and their server-side
    /// bookkeeping (actor handle, token, input log, subscribers, wire-id
    /// interning, and per-Terminal L3 metadata).
    pub fn reap_terminal(&mut self, pane: TerminalId) -> bool {
        // Resolve the parent window before the registry drops the pane.
        let window_id = self.registry.terminal(pane).map(|t| t.window);
        if self.registry.remove_terminal(pane).is_some() {
            self.forget_terminal_bookkeeping(pane);
        }
        let Some(window_id) = window_id else {
            return self.registry.session_count() == 0;
        };

        // Cascade up only while the parent has been emptied.
        let window_empty = self
            .registry
            .window(window_id)
            .is_some_and(|w| w.panes.is_empty());
        if window_empty {
            let session_id = self.registry.window(window_id).map(|w| w.session);
            if self.registry.remove_window(window_id).is_some() {
                self.forget_window_bookkeeping(window_id);
            }
            if let Some(session_id) = session_id {
                let session_empty = self
                    .registry
                    .session(session_id)
                    .is_some_and(|s| s.windows.is_empty());
                if session_empty && self.registry.remove_session(session_id).is_some() {
                    self.forget_session_bookkeeping(session_id);
                }
            }
        }

        self.registry.session_count() == 0
    }

    /// Drop every server-side map entry keyed on a now-removed pane.
    ///
    /// Cancels the actor token defensively (the actor has usually already
    /// exited by the time we reap, but a still-live token is cleanly
    /// resolved by the cancel) and retires the wire id without reuse.
    fn forget_terminal_bookkeeping(&mut self, pane: TerminalId) {
        self.terminals.remove(&pane);
        if let Some(token) = self.terminal_tokens.remove(&pane) {
            token.cancel();
        }
        self.terminal_inputs.remove(&pane);
        self.terminal_subscribers.remove(&pane);
        if let Some(wire) = self.terminal_wire_forward.remove(&pane) {
            self.terminal_wire_reverse.remove(&wire);
            self.metadata.forget_terminal(&wire);
        }
    }

    /// Retire a removed window's wire-id mapping (no reuse).
    fn forget_window_bookkeeping(&mut self, window: WindowId) {
        if let Some(wire) = self.window_wire_forward.remove(&window) {
            self.window_wire_reverse.remove(&wire);
        }
    }

    /// Forget a removed session's wire id and last-touch ordering entry.
    fn forget_session_bookkeeping(&mut self, session: SessionId) {
        self.session_id_bridge.forget(session);
        self.session_last_touched.remove(&session);
    }

    /// Subscribers (snapshot) for `pane`. Returns an empty slice if no
    /// clients are currently observing the pane.
    #[must_use]
    pub fn subscribers_for_terminal(&self, terminal: TerminalId) -> &[ClientId] {
        self.terminal_subscribers
            .get(&terminal)
            .map_or(&[], Vec::as_slice)
    }

    /// Clone the [`TerminalHandle`] of every pane `client_id` currently
    /// subscribes to (phux-0q8). The runtime uses this at DETACH /
    /// disconnect / EOF time to send a
    /// [`ConsumerDetachRequest`](crate::terminal_actor::ConsumerDetachRequest) to each
    /// pane actor so the per-consumer `RenderState` cache (ADR-0018) is
    /// freed, mirroring the `register_consumer` calls the ATTACH path
    /// made. Gathered under-lock; the sends happen off-lock in the
    /// runtime to avoid awaiting inside `with_mut`.
    #[must_use]
    pub fn subscribed_terminal_handles(&self, client_id: ClientId) -> Vec<TerminalHandle> {
        self.terminal_subscribers
            .iter()
            .filter(|(_, subs)| subs.contains(&client_id))
            .filter_map(|(terminal, _)| self.terminal_handle(*terminal).cloned())
            .collect()
    }

    /// Append `input` to the per-pane log. The log is shared across all
    /// attached clients of the pane's session; this is the merge point for
    /// multi-client keystrokes.
    pub fn record_terminal_input(&mut self, terminal: TerminalId, input: TerminalInput) {
        self.terminal_inputs
            .entry(terminal)
            .or_default()
            .push(input);
    }

    /// Look up the active pane of the active window of `session`, if any.
    #[must_use]
    pub fn active_pane_of_session(&self, session: SessionId) -> Option<TerminalId> {
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
    /// `(SessionId, WindowId, TerminalId)`.
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
    pub fn seed_session(
        &mut self,
        name: &str,
    ) -> (SessionId, phux_core::ids::WindowId, TerminalId) {
        let sid = self.registry.new_session(name.to_owned());
        let wid = self.registry.new_window(sid).expect("session just created");
        let pid = self
            .registry
            .new_terminal(wid)
            .expect("window just created");
        (sid, wid, pid)
    }

    /// Test-only: snapshot the per-pane input log.
    ///
    /// Exposed behind `#[cfg(test)]` so integration tests can assert
    /// keystroke merge ordering and no-dup invariants without needing to
    /// drain a real PTY consumer.
    #[cfg(test)]
    #[must_use]
    pub fn terminal_input_log_for(&self, terminal: TerminalId) -> Vec<TerminalInput> {
        self.terminal_inputs
            .get(&terminal)
            .cloned()
            .unwrap_or_default()
    }

    /// Record a freshly-spawned [`TerminalHandle`] against `pane` and
    /// allocate its wire id.
    ///
    /// Called by the runtime after `TerminalActor::new` /
    /// `build_with_token`. Subsequent attaches use
    /// [`Self::terminal_handle`] to look the handle up.
    ///
    /// `token` is stashed in `terminal_tokens`; cancelling it (e.g. via
    /// [`Self::detach_terminal_actor`]) fires the actor's shutdown branch.
    ///
    /// This method does NOT spawn the actor — pair it with
    /// [`Self::spawn_terminal_actor`] when you also want the actor task
    /// registered against the per-server `JoinSet`.
    ///
    /// Idempotent on the wire-id allocation (a second call for the
    /// same `pane` returns the same wire id) but overwrites the
    /// `TerminalHandle` / token. In practice the runtime calls this
    /// exactly once per pane lifetime.
    pub fn register_terminal_handle(
        &mut self,
        terminal: TerminalId,
        handle: TerminalHandle,
        token: CancellationToken,
    ) -> WireTerminalId {
        let wire = self.intern_terminal_wire(terminal);
        self.terminals.insert(terminal, handle);
        self.terminal_tokens.insert(terminal, token);
        wire
    }

    /// One-shot helper: register `handle`/`token` AND spawn
    /// `actor_future` onto the per-server pane `JoinSet`. Must be
    /// called from inside a `LocalSet` (per ADR-0014; pane actors
    /// own `!Send` `Terminal`s and are spawned via
    /// `JoinSet::spawn_local`).
    ///
    /// Returns the wire pane id, matching [`Self::register_terminal_handle`].
    pub fn spawn_terminal_actor<F>(
        &mut self,
        terminal: TerminalId,
        handle: TerminalHandle,
        token: CancellationToken,
        actor_future: F,
    ) -> WireTerminalId
    where
        F: Future<Output = ()> + 'static,
    {
        let wire = self.register_terminal_handle(terminal, handle, token);
        self.terminal_tasks.spawn_local(actor_future);
        wire
    }

    /// Cancel `pane`'s actor token, signalling the `TerminalActor` to
    /// exit, and forget the token. Idempotent. Used by future
    /// pane-close lifecycle paths; not exercised by `phux-byc.8`.
    ///
    /// The actor task itself is drained from the per-server `JoinSet`
    /// when it returns from `run`; we don't need to touch
    /// `terminal_tasks` here.
    pub fn detach_terminal_actor(&mut self, terminal: TerminalId) {
        if let Some(token) = self.terminal_tokens.remove(&terminal) {
            token.cancel();
        }
    }

    /// Look up the [`TerminalHandle`] for `pane`, if registered.
    #[must_use]
    pub fn terminal_handle(&self, terminal: TerminalId) -> Option<&TerminalHandle> {
        self.terminals.get(&terminal)
    }

    /// Wire pane id for `pane`, allocating one if needed.
    ///
    /// Mirrors [`IdBridge::intern`] but inline here for pane ids — see
    /// the field-level note on `terminal_wire_forward` for why a second
    /// general-purpose `IdBridge` is deferred.
    pub fn intern_terminal_wire(&mut self, terminal: TerminalId) -> WireTerminalId {
        if let Some(w) = self.terminal_wire_forward.get(&terminal) {
            return w.clone();
        }
        let raw = self.next_terminal_wire_id;
        self.next_terminal_wire_id = self.next_terminal_wire_id.saturating_add(1);
        let wire = WireTerminalId::local(raw);
        self.terminal_wire_forward.insert(terminal, wire.clone());
        self.terminal_wire_reverse.insert(wire.clone(), terminal);
        wire
    }

    /// Reverse lookup: which core pane id (if any) does `wire`
    /// resolve to?
    #[must_use]
    pub fn terminal_from_wire(&self, wire: &WireTerminalId) -> Option<TerminalId> {
        self.terminal_wire_reverse.get(wire).copied()
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

    /// Build a [`phux_protocol::wire::info::SessionSnapshot`] describing
    /// the entire registry plus the attaching client's initial focus.
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
        use phux_protocol::wire::info::{SessionInfo, SessionSnapshot, TerminalInfo, WindowInfo};

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
                let active_pane_wire = window.active.map(|p| self.intern_terminal_wire(p));

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
                    let Some(terminal) = self.registry.terminal(*pid).cloned() else {
                        continue;
                    };
                    let terminal_wire = self.intern_terminal_wire(*pid);
                    let cwd =
                        Some(terminal.cwd.to_string_lossy().into_owned()).filter(|s| !s.is_empty());
                    panes.push(
                        TerminalInfo::new(
                            terminal_wire,
                            window_wire,
                            terminal.dims.0,
                            terminal.dims.1,
                        )
                        .with_title(terminal.title.clone())
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
        let focused_pane_wire = self.intern_terminal_wire(focused_pane);

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

    fn mk_tx() -> mpsc::Sender<Outbound> {
        let (tx, _rx) = mpsc::channel::<Outbound>(DEFAULT_CLIENT_MAILBOX);
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
        let err = s.attach_default_caps(cid, "ghost", mk_tx()).unwrap_err();
        assert_eq!(err, AttachError::UnknownSession("ghost".to_owned()));
    }

    #[test]
    fn attach_records_client_and_subscribes_to_active_pane() {
        let mut s = ServerState::new();
        let (sid, _wid, pid) = s.seed_session("default");
        let cid = s.new_client_id();
        let returned_sid = s.attach_default_caps(cid, "default", mk_tx()).unwrap();
        assert_eq!(returned_sid, sid);
        assert!(s.attached.contains_key(&cid));
        assert_eq!(s.subscribers_for_terminal(pid), &[cid]);
    }

    #[test]
    fn second_attach_for_same_client_returns_already_attached() {
        let mut s = ServerState::new();
        let _ = s.seed_session("default");
        let cid = s.new_client_id();
        s.attach_default_caps(cid, "default", mk_tx()).unwrap();
        let err = s.attach_default_caps(cid, "default", mk_tx()).unwrap_err();
        assert_eq!(err, AttachError::AlreadyAttached(cid));
    }

    #[test]
    fn two_clients_attach_same_session_see_same_active_pane() {
        let mut s = ServerState::new();
        let (_sid, _wid, pid) = s.seed_session("default");
        let a = s.new_client_id();
        let b = s.new_client_id();
        s.attach_default_caps(a, "default", mk_tx()).unwrap();
        s.attach_default_caps(b, "default", mk_tx()).unwrap();
        let subs = s.subscribers_for_terminal(pid);
        assert!(subs.contains(&a) && subs.contains(&b));
        assert_eq!(subs.len(), 2);
    }

    #[test]
    fn detach_removes_client_and_drops_empty_subscriber_lists() {
        let mut s = ServerState::new();
        let (_sid, _wid, pid) = s.seed_session("default");
        let cid = s.new_client_id();
        s.attach_default_caps(cid, "default", mk_tx()).unwrap();
        assert!(!s.subscribers_for_terminal(pid).is_empty());
        s.detach(cid);
        assert!(!s.attached.contains_key(&cid));
        assert!(s.subscribers_for_terminal(pid).is_empty());
        assert!(
            s.terminal_subscribers.is_empty(),
            "empty lists should be GC'd"
        );
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
    fn reap_last_pane_empties_server() {
        let mut s = ServerState::new();
        let (sid, _wid, pid) = s.seed_session("default");
        assert_eq!(s.registry.session_count(), 1);

        let server_empty = s.reap_terminal(pid);

        assert!(server_empty, "reaping the only pane must empty the server");
        assert_eq!(s.registry.session_count(), 0);
        assert!(s.registry.session(sid).is_none(), "session cascaded away");
        assert!(s.registry.terminal(pid).is_none());
    }

    #[test]
    fn reap_one_of_two_sessions_keeps_server_alive() {
        let mut s = ServerState::new();
        let (sid_a, _wa, pid_a) = s.seed_session("a");
        let (sid_b, _wb, _pb) = s.seed_session("b");

        let server_empty = s.reap_terminal(pid_a);

        assert!(!server_empty, "a second session is still live");
        assert_eq!(s.registry.session_count(), 1);
        assert!(s.registry.session(sid_a).is_none(), "session a reaped");
        assert!(s.registry.session(sid_b).is_some(), "session b untouched");
    }

    #[test]
    fn reap_pane_in_multipane_window_keeps_session() {
        let mut s = ServerState::new();
        let (sid, wid, pid1) = s.seed_session("default");
        // Add a second pane to the same window so reaping one does not
        // empty the window.
        let pid2 = s.registry.new_terminal(wid).unwrap();

        let server_empty = s.reap_terminal(pid1);

        assert!(!server_empty);
        assert_eq!(s.registry.session_count(), 1);
        assert!(s.registry.session(sid).is_some());
        assert!(s.registry.terminal(pid1).is_none(), "reaped pane gone");
        assert!(s.registry.terminal(pid2).is_some(), "sibling pane survives");
        assert_eq!(
            s.registry.window(wid).map(|w| w.panes.len()),
            Some(1),
            "window keeps the surviving pane",
        );
    }

    #[test]
    fn reap_is_idempotent_on_unknown_pane() {
        let mut s = ServerState::new();
        let (_sid, _wid, pid) = s.seed_session("default");

        assert!(s.reap_terminal(pid), "first reap empties the server");
        // Second reap of the now-unknown pane must not panic and must
        // report the server is (still) empty.
        assert!(s.reap_terminal(pid));
        assert_eq!(s.registry.session_count(), 0);
    }

    #[test]
    fn reap_clears_pane_bookkeeping() {
        let mut s = ServerState::new();
        let (_sid, _wid, pid) = s.seed_session("default");
        let cid = s.new_client_id();
        s.attach_default_caps(cid, "default", mk_tx()).unwrap();
        let wire = s.intern_terminal_wire(pid);
        assert!(!s.subscribers_for_terminal(pid).is_empty());
        assert_eq!(s.terminal_from_wire(&wire), Some(pid));

        s.reap_terminal(pid);

        assert!(s.subscribers_for_terminal(pid).is_empty());
        assert!(
            s.terminal_from_wire(&wire).is_none(),
            "wire id retired on reap",
        );
    }

    #[test]
    fn record_terminal_input_appends_in_call_order() {
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

        s.record_terminal_input(pid, TerminalInput::Key(mk(PhysicalKey::A, "a")));
        s.record_terminal_input(pid, TerminalInput::Key(mk(PhysicalKey::B, "b")));
        s.record_terminal_input(pid, TerminalInput::Key(mk(PhysicalKey::C, "c")));

        let log = s.terminal_input_log_for(pid);
        assert_eq!(log.len(), 3);
        let texts: Vec<String> = log
            .into_iter()
            .map(|pi| match pi {
                TerminalInput::Key(k) => k.text.unwrap_or_default(),
                _ => unreachable!(),
            })
            .collect();
        assert_eq!(texts, vec!["a", "b", "c"]);
    }

    #[test]
    fn attached_client_color_support_defaults_to_truecolor() {
        // `attach_default_caps` keeps the most-permissive tier — used by
        // tests and any call site that doesn't have HELLO-derived caps
        // in hand.
        let mut s = ServerState::new();
        let _ = s.seed_session("default");
        let cid = s.new_client_id();
        s.attach_default_caps(cid, "default", mk_tx()).unwrap();
        let client = s.attached.get(&cid).unwrap();
        assert_eq!(client.client_caps.color_support, ColorSupport::TrueColor);
    }

    #[test]
    fn attach_records_advertised_color_support() {
        // Production path: HELLO advertised a tier, ATTACH consumes it.
        let mut s = ServerState::new();
        let _ = s.seed_session("default");
        let cid = s.new_client_id();
        s.attach(
            cid,
            "default",
            mk_tx(),
            ClientCapabilities::new().with_color_support(ColorSupport::Indexed16),
        )
        .unwrap();
        let client = s.attached.get(&cid).unwrap();
        assert_eq!(client.client_caps.color_support, ColorSupport::Indexed16);
    }

    #[test]
    fn set_client_color_support_updates_live_attached_client() {
        // Out-of-order HELLO after ATTACH (out of spec, but tolerated):
        // the setter patches the live record so downsample picks up the
        // newer tier.
        let mut s = ServerState::new();
        let _ = s.seed_session("default");
        let cid = s.new_client_id();
        s.attach_default_caps(cid, "default", mk_tx()).unwrap();
        assert!(s.set_client_color_support(cid, ColorSupport::Indexed256));
        let client = s.attached.get(&cid).unwrap();
        assert_eq!(client.client_caps.color_support, ColorSupport::Indexed256);
    }

    #[test]
    fn set_client_color_support_returns_false_for_unknown_client() {
        let mut s = ServerState::new();
        assert!(!s.set_client_color_support(ClientId(999), ColorSupport::Indexed16));
    }

    #[test]
    fn most_recently_touched_session_starts_none_and_tracks_touch_order() {
        let mut s = ServerState::new();
        assert!(
            s.most_recently_touched_session().is_none(),
            "fresh state has no prior activity memory",
        );
        let (sid, _wid, _pid) = s.seed_session("default");
        s.touch_session(sid);
        assert_eq!(s.most_recently_touched_session(), Some(sid));

        // Later touches win, regardless of attach order.
        let (sid2, _w, _p) = s.seed_session("other");
        s.touch_session(sid2);
        assert_eq!(s.most_recently_touched_session(), Some(sid2));
        s.touch_session(sid);
        assert_eq!(s.most_recently_touched_session(), Some(sid));
    }

    #[test]
    fn shared_state_with_and_with_mut_round_trip() {
        let shared = SharedState::new();
        let (_sid, _wid, pid) = shared.with_mut(|s| s.seed_session("default"));
        let count = shared.with(|s| s.subscribers_for_terminal(pid).len());
        assert_eq!(count, 0);
    }

    // -------------------------------------------------------------------------
    // L3 metadata tests — SPEC §7.4 / §11.L3 (phux-4li.2).
    //
    // Cover: SUBSCRIBE → SET → broadcast fanout, scope isolation (Terminal
    // vs Collection vs Global), non-L3 consumer filtering (§16.4), DELETE
    // tombstone semantics, and the `Unchanged` SET shortcut.
    // -------------------------------------------------------------------------

    fn attach_l3_client(s: &mut ServerState) -> (ClientId, mpsc::Receiver<Outbound>) {
        let _ = s.seed_session("default");
        let cid = s.new_client_id();
        let (tx, rx) = mpsc::channel::<Outbound>(DEFAULT_CLIENT_MAILBOX);
        s.attach_default_caps(cid, "default", tx).unwrap();
        s.set_client_layers(cid, LayerSet::all());
        (cid, rx)
    }

    fn attach_l1_only_client(s: &mut ServerState) -> (ClientId, mpsc::Receiver<Outbound>) {
        let cid = s.new_client_id();
        let (tx, rx) = mpsc::channel::<Outbound>(DEFAULT_CLIENT_MAILBOX);
        s.attach_default_caps(cid, "default", tx).unwrap();
        s.set_client_layers(cid, LayerSet::new());
        (cid, rx)
    }

    /// Pull every queued frame off `rx`. Returns the inner frames; raw
    /// PONG bytes (if any) are surfaced as `None` and skipped.
    fn drain_frames(rx: &mut mpsc::Receiver<Outbound>) -> Vec<FrameKind> {
        let mut out = Vec::new();
        while let Ok(msg) = rx.try_recv() {
            if let Outbound::Frame(f) = msg {
                out.push(f);
            }
        }
        out
    }

    #[test]
    fn metadata_subscribe_then_set_broadcasts_matching_key() {
        let mut s = ServerState::new();
        let (cid, mut rx) = attach_l3_client(&mut s);
        let scope = Scope::Collection(DEFAULT_COLLECTION_ID);

        s.metadata_subscribe(cid, scope.clone(), "phux.tui.layout/v1".to_owned());
        let delivered = s.metadata_set(&scope, "phux.tui.layout/v1", b"value-1".to_vec());

        assert_eq!(delivered, vec![cid]);
        let frames = drain_frames(&mut rx);
        assert_eq!(frames.len(), 1);
        match &frames[0] {
            FrameKind::MetadataChanged {
                scope: s2,
                key,
                value,
            } => {
                assert_eq!(s2, &scope);
                assert_eq!(key, "phux.tui.layout/v1");
                assert_eq!(value.as_deref(), Some(b"value-1".as_slice()));
            }
            other => panic!("expected MetadataChanged, got {other:?}"),
        }
    }

    #[test]
    fn metadata_set_on_different_key_does_not_fan_to_subscriber() {
        let mut s = ServerState::new();
        let (cid, mut rx) = attach_l3_client(&mut s);
        let scope = Scope::Collection(DEFAULT_COLLECTION_ID);

        s.metadata_subscribe(cid, scope.clone(), "phux.a/v1".to_owned());
        let delivered = s.metadata_set(&scope, "phux.b/v1", b"x".to_vec());

        assert!(delivered.is_empty(), "no subscriber for the b/v1 key");
        assert!(drain_frames(&mut rx).is_empty());
    }

    #[test]
    fn metadata_scope_isolation_terminal_vs_collection_vs_global() {
        let mut s = ServerState::new();
        let (cid, mut rx) = attach_l3_client(&mut s);
        let key = "phux.same/v1";
        let t_scope = Scope::Terminal(phux_protocol::ids::TerminalId::local(7));
        let c_scope = Scope::Collection(DEFAULT_COLLECTION_ID);
        let g_scope = Scope::Global;

        // Only subscribe to Collection.
        s.metadata_subscribe(cid, c_scope.clone(), key.to_owned());

        // Writes to Terminal and Global must NOT fire the subscriber.
        assert!(s.metadata_set(&t_scope, key, b"t".to_vec()).is_empty());
        assert!(s.metadata_set(&g_scope, key, b"g".to_vec()).is_empty());

        // Write to Collection MUST fire it.
        let delivered = s.metadata_set(&c_scope, key, b"c".to_vec());
        assert_eq!(delivered, vec![cid]);

        // And the receiver MUST see exactly one frame (for Collection).
        let frames = drain_frames(&mut rx);
        assert_eq!(frames.len(), 1);
        match &frames[0] {
            FrameKind::MetadataChanged { scope, value, .. } => {
                assert_eq!(scope, &c_scope);
                assert_eq!(value.as_deref(), Some(b"c".as_slice()));
            }
            other => panic!("expected MetadataChanged, got {other:?}"),
        }
    }

    #[test]
    fn metadata_delete_emits_tombstone_only_if_key_existed() {
        let mut s = ServerState::new();
        let (cid, mut rx) = attach_l3_client(&mut s);
        let scope = Scope::Global;

        s.metadata_subscribe(cid, scope.clone(), "phux.k/v1".to_owned());

        // Deleting a missing key is idempotent and silent.
        let delivered = s.metadata_delete(&scope, "phux.k/v1");
        assert!(delivered.is_empty());
        assert!(drain_frames(&mut rx).is_empty());

        // After a SET, DELETE fires the tombstone.
        s.metadata_set(&scope, "phux.k/v1", b"v".to_vec());
        drain_frames(&mut rx); // consume the SET broadcast

        let delivered = s.metadata_delete(&scope, "phux.k/v1");
        assert_eq!(delivered, vec![cid]);
        let frames = drain_frames(&mut rx);
        assert_eq!(frames.len(), 1);
        match &frames[0] {
            FrameKind::MetadataChanged {
                value: None,
                key,
                scope: s2,
            } => {
                assert_eq!(key, "phux.k/v1");
                assert_eq!(s2, &scope);
            }
            other => panic!("expected tombstone MetadataChanged, got {other:?}"),
        }
    }

    #[test]
    fn metadata_set_unchanged_value_does_not_broadcast() {
        let mut s = ServerState::new();
        let (cid, mut rx) = attach_l3_client(&mut s);
        let scope = Scope::Global;
        s.metadata_subscribe(cid, scope.clone(), "phux.k/v1".to_owned());

        let first = s.metadata_set(&scope, "phux.k/v1", b"v".to_vec());
        assert_eq!(first, vec![cid]);
        drain_frames(&mut rx);

        let second = s.metadata_set(&scope, "phux.k/v1", b"v".to_vec());
        assert!(second.is_empty(), "no broadcast on identical write");
        assert!(drain_frames(&mut rx).is_empty());
    }

    #[test]
    fn non_l3_consumer_does_not_receive_metadata_changed() {
        // SPEC §16.4: a non-L3 client (agent / recorder) MUST NOT see any
        // L3 frames. The fanout layer filters by `client_speaks_l3`.
        let mut s = ServerState::new();
        let (l3_cid, mut l3_rx) = attach_l3_client(&mut s);
        let (l1_cid, mut l1_rx) = attach_l1_only_client(&mut s);
        let scope = Scope::Global;

        s.metadata_subscribe(l3_cid, scope.clone(), "phux.k/v1".to_owned());
        // L1-only consumer might still TRY to subscribe via misbehaving
        // client; the dispatch in runtime.rs refuses it. Simulate that by
        // not subscribing through the gated path.
        s.metadata_subscribe(l1_cid, scope.clone(), "phux.k/v1".to_owned());

        let delivered = s.metadata_set(&scope, "phux.k/v1", b"v".to_vec());
        // Only the L3 client makes it through.
        assert_eq!(delivered, vec![l3_cid]);
        assert_eq!(drain_frames(&mut l3_rx).len(), 1);
        assert!(drain_frames(&mut l1_rx).is_empty());
    }

    #[test]
    fn detach_drops_metadata_subscriptions() {
        let mut s = ServerState::new();
        let (cid, mut rx) = attach_l3_client(&mut s);
        let scope = Scope::Global;
        s.metadata_subscribe(cid, scope.clone(), "phux.k/v1".to_owned());

        s.detach(cid);

        let delivered = s.metadata_set(&scope, "phux.k/v1", b"v".to_vec());
        assert!(delivered.is_empty());
        // Channel returns Err(Disconnected) eventually; just confirm no
        // frame arrived before detach cleanup.
        assert!(drain_frames(&mut rx).is_empty());
    }

    #[test]
    fn metadata_list_returns_keys_sorted_and_scope_isolated() {
        let mut s = ServerState::new();
        let scope_a = Scope::Collection(DEFAULT_COLLECTION_ID);
        let scope_b = Scope::Global;

        s.metadata_set(&scope_a, "zeta", b"z".to_vec());
        s.metadata_set(&scope_a, "alpha", b"a".to_vec());
        s.metadata_set(&scope_a, "mu", b"m".to_vec());
        s.metadata_set(&scope_b, "global-only", b"g".to_vec());

        let keys_a = s.metadata().list(&scope_a);
        assert_eq!(keys_a, vec!["alpha", "mu", "zeta"]);
        let keys_b = s.metadata().list(&scope_b);
        assert_eq!(keys_b, vec!["global-only"]);
    }

    #[test]
    fn metadata_get_returns_stored_value_or_none() {
        let mut s = ServerState::new();
        let scope = Scope::Collection(DEFAULT_COLLECTION_ID);
        s.metadata_set(&scope, "k", b"v".to_vec());
        assert_eq!(s.metadata().get(&scope, "k"), Some(b"v".to_vec()));
        assert_eq!(s.metadata().get(&scope, "missing"), None);
        // Wrong scope: same key returns None.
        assert_eq!(s.metadata().get(&Scope::Global, "k"), None);
    }
}
