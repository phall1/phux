//! Server runtime: tokio current-thread executor + Unix-domain-socket
//! listener (`phux-byc.3`).
//!
//! This module wires the minimum surface needed to host clients:
//!
//! * Construct a single-threaded tokio runtime
//!   (`tokio::runtime::Builder::new_current_thread`) per ADR-0003 (one server
//!   per user, one event loop).
//! * Bind a `SOCK_STREAM` Unix domain socket at a resolved path under
//!   `$XDG_RUNTIME_DIR` (falling back to `/tmp/phux-$UID/`), as described in
//!   `docs/spec/proto.md` §4 (Transport).
//! * Accept connections and spawn a per-client task on a
//!   [`tokio::task::LocalSet`] (per ADR-0014) that reads length-prefixed
//!   frames (`docs/spec/proto.md` §5), echoes `PING` with `PONG` (`docs/spec/proto.md` §7.4),
//!   and handles `ATTACH` / `DETACH` by talking to the per-terminal
//!   `TerminalActor`s (`phux-byc.8`). The
//!   remaining catalog (`INPUT_KEY`, etc.) is recorded against the
//!   terminal's input log but the PTY write side lands in `phux-byc.5`.
//! * Unlink the socket file on clean shutdown and refuse to start over an
//!   already-live socket.
//!
//! Frame types come from `phux_protocol::wire` (ADR-0008): the protocol crate
//! is the single source of truth for what bytes go on the wire.
#![allow(
    clippy::future_not_send,
    reason = "single-threaded tokio runtime per ADR-0003; Send/Sync not required"
)]

use std::future::Future;
use std::io;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use bytes::BytesMut;
use futures_util::StreamExt;
use futures_util::stream::FuturesUnordered;
use phux_protocol::PROTOCOL_VERSION;
use phux_protocol::caps::{ClientCapabilities, LayerSet, ServerCapabilities};
use phux_protocol::ids::CollectionId;
use phux_protocol::input::InputEvent;
use phux_protocol::wire::frame::{
    AgentEvent, AttachTarget, Command, CommandResult, CommandValue, ErrorCode, FrameKind,
    SpawnError, SpawnResult, StateScope, ViewportInfo,
};
use tokio::net::{UnixListener, UnixStream};
use tokio::runtime::Builder;
use tokio::sync::oneshot;
use tokio::task::{JoinSet, LocalSet};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, trace, warn};

use crate::state::{
    AttachSnapshotPane, ClientId, DEFAULT_CLIENT_MAILBOX, Outbound, SharedState, TerminalInput,
};
use crate::terminal_actor::{
    ConsumerAckRequest, ConsumerAttachRequest, ConsumerDetachRequest, PwdRequest, ResizeRequest,
    ScreenRequest, SnapshotRequest, TerminalActor, TerminalHandle,
};
use crate::transport::{FrameReader, FrameWriter, Incoming};

/// Timeout for the "is the socket still live?" liveness probe used when an
/// existing socket file is encountered during bind.
const STALE_PROBE_TIMEOUT: Duration = Duration::from_millis(50);

/// Configuration for [`ServerRuntime`].
#[derive(Debug, Clone)]
pub struct ServerConfig {
    /// Filesystem path to bind the Unix domain socket at.
    pub socket_path: PathBuf,
    /// Optional session name to pre-seed in the registry before clients
    /// connect. When `Some(name)`, the server creates a session by that
    /// name with one window and one pane during startup (`phux-byc.4`).
    ///
    /// Tests use this to launch a server whose registry already contains
    /// a known session to attach to without first issuing a `COMMAND` (the
    /// `COMMAND` message is not implemented yet).
    pub pre_seeded_session: Option<String>,
    /// When `true`, the pre-seeded session's initial pane spawns the user's
    /// default shell (`$SHELL`, falling back to `/bin/sh`) inside a real
    /// PTY (see [`seed_session_with_pty`] / [`crate::terminal_actor::TerminalActor::new_with_default_shell`]).
    /// When `false`, the pre-seeded session's pane has a no-PTY actor —
    /// the actor exists for snapshot/input plumbing but no child process
    /// runs and no bytes flow.
    ///
    /// The PTY path is what the `phux server` binary subcommand needs to
    /// actually be useful to a human attacher; tests and example code
    /// keep the default (no-PTY) so they can exercise the registry/wire
    /// surface without forking shells.
    pub seed_with_pty: bool,
    /// When `Some` and [`Self::seed_with_pty`] is `true`, the pre-seeded
    /// pane spawns this command instead of the user's default shell.
    /// Mostly useful for integration tests that need a deterministic
    /// PTY-backed actor (e.g. `cat`, which echoes input → output for a
    /// crisp wire round-trip assertion).
    ///
    /// Ignored when `seed_with_pty` is `false`. `None` (the default)
    /// falls back to [`crate::terminal_actor::default_shell_command`].
    pub seed_command: Option<portable_pty::CommandBuilder>,
    /// Lines of scrollback retained per pane (`defaults.history-limit`,
    /// SPEC DESIGN.md §4.2). Threaded into every `TerminalActor`'s
    /// scrollback cap at construction — both the pre-seeded session and
    /// any session created later via `AttachTarget::CreateIfMissing` or
    /// `SPAWN_TERMINAL`. The binary populates this from
    /// `phux_config`'s `defaults.history-limit`;
    /// [`Self::with_default_socket`] uses the schema default.
    pub history_limit: u32,
    /// How a freshly-spawned pane chooses its working directory
    /// (`defaults.cwd-inheritance`, SPEC DESIGN.md). Threaded into
    /// shared state so `SPAWN_TERMINAL` resolves the new pane's CWD when
    /// the wire frame leaves `cwd` unset:
    /// [`phux_config::CwdInheritance::InheritFocused`] reads the spawning
    /// client's focused pane's live PTY working directory via a kernel
    /// query ([`crate::cwd_query`]); the other modes pick `$HOME` or the
    /// session root. The binary populates this from
    /// `phux_config`'s `defaults.cwd-inheritance`;
    /// [`Self::with_default_socket`] uses the schema default.
    pub cwd_inheritance: phux_config::CwdInheritance,
    /// `TERM` advertised to the inner program of every server-spawned pane
    /// (`defaults.term`, phux-ign). Threaded into shared state so the seed
    /// session, attach-time `CreateIfMissing`, and `SPAWN_TERMINAL` apply
    /// it as the PTY's `TERM` baseline. A per-spawn `SPAWN_TERMINAL.env`
    /// entry for `TERM` overrides it. The binary populates this from
    /// `phux_config`'s `defaults.term`; [`Self::with_default_socket`] uses
    /// the schema default (`xterm-256color`).
    pub term: String,
}

impl ServerConfig {
    /// Build a config with `socket_path` resolved via [`default_socket_path`]
    /// and no pre-seeded session.
    #[must_use]
    pub fn with_default_socket() -> Self {
        Self {
            socket_path: default_socket_path(),
            pre_seeded_session: None,
            seed_with_pty: false,
            seed_command: None,
            history_limit: phux_config::DefaultsCfg::default().history_limit,
            cwd_inheritance: phux_config::CwdInheritance::default(),
            term: phux_config::DefaultsCfg::default().term,
        }
    }
}

/// Resolve the default Unix-domain-socket path per the convention documented
/// in this module: `$XDG_RUNTIME_DIR/phux/phux.sock` if `XDG_RUNTIME_DIR` is
/// set, otherwise `/tmp/phux-$UID/phux.sock`.
#[must_use]
pub fn default_socket_path() -> PathBuf {
    if let Some(dir) = std::env::var_os("XDG_RUNTIME_DIR") {
        let mut p = PathBuf::from(dir);
        p.push("phux");
        p.push("phux.sock");
        return p;
    }
    // SAFETY-free: `getuid` is a `libc` call we'd rather not depend on here.
    // Read the effective UID from `/proc` is Linux-only; instead use the
    // `USER` env var as a stable, portable fallback when crafting the path.
    // The exact directory name is cosmetic — it only needs to be unique per
    // user.
    let uid_segment = std::env::var("UID")
        .ok()
        .or_else(|| std::env::var("USER").ok())
        .unwrap_or_else(|| "default".to_owned());
    let mut p = PathBuf::from("/tmp");
    p.push(format!("phux-{uid_segment}"));
    p.push("phux.sock");
    p
}

/// Errors surfaced by [`ServerRuntime`].
#[derive(Debug, thiserror::Error)]
pub enum ServerError {
    /// The Unix domain socket could not be bound.
    #[error("failed to bind unix socket: {0}")]
    Bind(#[source] io::Error),

    /// Another server appears to be live at this socket path. The path is
    /// returned so callers can present a useful diagnostic.
    #[error("socket {0} is already in use by a live server")]
    SocketBusy(PathBuf),

    /// The parent directory of the socket path could not be prepared.
    #[error("failed to prepare socket directory {path}: {source}")]
    PrepareDir {
        /// Directory that could not be created or had wrong permissions.
        path: PathBuf,
        /// Underlying I/O error.
        #[source]
        source: io::Error,
    },

    /// An I/O error not otherwise classified.
    #[error("io error: {0}")]
    Io(#[from] io::Error),

    /// Failed to build the tokio runtime.
    #[error("failed to build tokio runtime: {0}")]
    Runtime(#[source] io::Error),
}

/// Server runtime owning the listener loop and per-client task scaffolding.
#[derive(Debug)]
pub struct ServerRuntime {
    cfg: ServerConfig,
}

impl ServerRuntime {
    /// Create a runtime ready to be `run`. Does not perform I/O.
    #[must_use]
    pub const fn new(cfg: ServerConfig) -> Self {
        Self { cfg }
    }

    /// Run the server until `shutdown` resolves.
    ///
    /// Builds a `new_current_thread` tokio runtime internally and blocks on
    /// [`Self::run_async`].
    pub fn run<F>(self, shutdown: F) -> Result<(), ServerError>
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let rt = Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(ServerError::Runtime)?;
        rt.block_on(self.run_async(shutdown))
    }

    /// Async variant for tests and embedders that already own a runtime.
    ///
    /// Per ADR-0014, the accept loop and every per-client task run on a
    /// [`tokio::task::LocalSet`] driven by the current async context.
    /// `!Send` futures are legal — and required — because pane actors
    /// own a [`libghostty_vt::Terminal`], which carries no `Send`/`Sync`
    /// impls.
    #[allow(
        clippy::future_not_send,
        reason = "ADR-0014: server runs on a LocalSet; per-pane actors are !Send"
    )]
    pub async fn run_async<F>(self, shutdown: F) -> Result<(), ServerError>
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let socket_path = self.cfg.socket_path.clone();
        prepare_socket_dir(&socket_path)?;
        handle_existing_socket(&socket_path).await?;

        // Build shared state. The state is the merge point for multi-
        // client input and the routing table for fanout (see
        // `state.rs`). Cloning the `SharedState` is cheap (`Arc::clone`).
        let state = SharedState::new();

        let listener = crate::transport::UdsListener::new(
            UnixListener::bind(&socket_path).map_err(ServerError::Bind)?,
        );
        info!(path = %socket_path.display(), "phux-server listening on UDS");

        // The LocalSet hosts per-client tasks and per-pane actors —
        // both `!Send`. `LocalSet::run_until` drives the set to the
        // future's completion; tasks spawned via `spawn_local` from
        // inside the future are polled on the same thread.
        let pre_seeded = self.cfg.pre_seeded_session.clone();
        let seed_with_pty = self.cfg.seed_with_pty;
        let seed_command = self.cfg.seed_command.clone();
        // Mirror the PTY *mode* into shared state so `handle_attach`'s
        // `AttachTarget::CreateIfMissing` branch (phux-k61.3) spawns new
        // sessions' seed panes with PTYs when the server runs with them.
        //
        // phux-07y: the seed *command* is deliberately NOT mirrored as
        // the CreateIfMissing override. `seed_command` is the pre-seeded
        // session's program (e.g. `defaults.spawn-on-attach`, the thing
        // naked `phux` opens with); a CreateIfMissing-created session —
        // `phux new`, `phux new -- vim` — must instead honor its own
        // wire `command` (or fall back to `default_shell_command`), not
        // inherit naked-`phux`'s launcher. So the override stays `None`.
        state.with_mut(|s| s.set_attach_create_pty(seed_with_pty, None));
        // Mirror `defaults.history-limit` into shared state so the
        // attach-time creation path (`CreateIfMissing`) and
        // `SPAWN_TERMINAL` build their panes with the configured cap.
        let history_limit = self.cfg.history_limit;
        state.with_mut(|s| s.set_history_limit(history_limit));
        // Mirror `defaults.cwd-inheritance` into shared state so the
        // `SPAWN_TERMINAL` handler resolves a new pane's working directory
        // from the configured policy.
        let cwd_inheritance = self.cfg.cwd_inheritance;
        state.with_mut(|s| s.set_cwd_inheritance(cwd_inheritance));
        // Mirror `defaults.term` into shared state so the seed session,
        // attach-time `CreateIfMissing`, and `SPAWN_TERMINAL` apply the
        // configured `TERM` baseline.
        let term = self.cfg.term.clone();
        state.with_mut(|s| s.set_term(term));
        let local = LocalSet::new();
        // Hierarchical cancellation: a single root token is the parent
        // of every per-client / per-pane child. The external `shutdown`
        // future is folded into this token by a small task spawned on
        // the LocalSet (see below). On `root_token.cancel()`:
        //   * `accept_loop` returns from its select! → its per-client
        //     `JoinSet` drops → in-flight client tasks abort.
        //   * Every `TerminalActor`'s child token fires → actors exit
        //     cleanly via their own `select!` (shutdown_pty runs).
        let root_token = CancellationToken::new();
        let result = local
            .run_until(async move {
                // Fold the external shutdown future into the root
                // token. `spawn_local` (not `tokio::spawn`) because
                // the runtime is current-thread with no worker pool.
                {
                    let token = root_token.clone();
                    tokio::task::spawn_local(async move {
                        shutdown.await;
                        debug!("shutdown future resolved; cancelling root token");
                        token.cancel();
                    });
                }

                // Pre-seed inside the LocalSet so we can `spawn_local`
                // the pane actor. Without this, the pre-seed path
                // would have to call `tokio::spawn`, which requires
                // `Send` futures — exactly what `TerminalActor` is not.
                if let Some(name) = pre_seeded.as_deref() {
                    let seeded = if seed_with_pty {
                        let mut cmd = seed_command
                            .unwrap_or_else(crate::terminal_actor::default_shell_command);
                        // Apply the configured `defaults.term` over the
                        // builder's baseline so the seed pane advertises the
                        // server-wide `TERM` (phux-ign).
                        let term = state.with(|s| s.term().to_owned());
                        crate::terminal_actor::apply_term(&mut cmd, &term);
                        seed_session_with_pty(&state, name, cmd, history_limit, &root_token)
                    } else {
                        seed_session_with_actor(&state, name, history_limit, &root_token)
                    };
                    if let Err(err) = seeded {
                        warn!(
                            session = name,
                            error = %err,
                            "failed to spawn pane actor for pre-seeded session",
                        );
                    } else {
                        debug!(
                            session = name,
                            pty = seed_with_pty,
                            "pre-seeded session in registry"
                        );
                    }
                }
                // Optionally also accept WebSocket connections (phux-486.4) so
                // browser consumers (`phux-web`) can speak the identical wire.
                // Opt-in via `PHUX_WS_ADDR` (e.g. "127.0.0.1:8787"); UDS is
                // always on.
                let ws_addr = std::env::var("PHUX_WS_ADDR").ok().and_then(|raw| {
                    match raw.parse::<std::net::SocketAddr>() {
                        Ok(addr) => Some(addr),
                        Err(err) => {
                            warn!(addr = %raw, error = %err, "invalid PHUX_WS_ADDR; WebSocket transport disabled");
                            None
                        }
                    }
                });
                match ws_addr {
                    Some(addr) => match crate::transport::WsListener::bind(addr).await {
                        Ok(ws) => {
                            let bound = ws.local_addr().map(|a| a.to_string()).unwrap_or_default();
                            info!(addr = %bound, "phux-server also listening on WebSocket");
                            // Both loops run until the root token cancels;
                            // whichever returns first ends the server (on
                            // shutdown both observe the cancellation).
                            tokio::select! {
                                r = accept_loop(&listener, state.clone(), root_token.clone()) => r,
                                r = accept_loop(&ws, state, root_token) => r,
                            }
                        }
                        Err(err) => {
                            warn!(addr = %addr, error = %err, "failed to bind WebSocket; UDS only");
                            accept_loop(&listener, state, root_token).await
                        }
                    },
                    None => accept_loop(&listener, state, root_token).await,
                }
            })
            .await;

        // Always try to unlink the socket on the way out; ignore NotFound.
        if let Err(err) = std::fs::remove_file(&socket_path)
            && err.kind() != io::ErrorKind::NotFound
        {
            warn!(path = %socket_path.display(), error = %err, "failed to unlink socket");
        }

        result
    }
}

/// Seed `(session, window, pane)` and spawn a **no-PTY** `TerminalActor`
/// on the current `LocalSet`. Used by tests that pre-seed a session
/// to exercise the ATTACH path without spawning a real subprocess.
///
/// For the real server path (which will spawn `$SHELL` once a binary
/// entry point exists), see [`seed_session_with_pty`].
///
/// Public-ish (`pub(crate)`) so tests can drive it directly inside
/// their own `LocalSet`.
pub(crate) fn seed_session_with_actor(
    state: &SharedState,
    name: &str,
    history_limit: u32,
    root_token: &CancellationToken,
) -> Result<phux_core::ids::TerminalId, crate::terminal_actor::TerminalActorError> {
    use phux_core::ids::TerminalId;
    let terminal: TerminalId = state.with_mut(|s| s.seed_session(name).2);
    // Default 80x24 — same as `phux_core::Pane::new`'s default dims.
    // Real resize wiring lands with VIEWPORT_RESIZE (phux-4hp).
    let terminal_token = root_token.child_token();
    let bundle =
        TerminalActor::build_with_token(80, 24, None, history_limit, terminal_token.clone())?;
    let crate::terminal_actor::TerminalActorBundle {
        actor,
        handle,
        exit_notify,
        ..
    } = bundle;
    state.with_mut(|s| {
        let _ = s.spawn_terminal_actor(terminal, handle, terminal_token, actor.run());
    });
    spawn_terminal_exit_watcher(state.clone(), terminal, exit_notify, root_token.clone());
    Ok(terminal)
}

/// Seed `(session, window, pane)` and spawn a **PTY-backed**
/// `TerminalActor` running `cmd`. Sibling of the private
/// `seed_session_with_actor` helper for the real server path
/// (`phux-byc.5`).
///
/// Call sites:
///
/// * The `phux server` binary entry point, via
///   [`ServerConfig::seed_with_pty`] (with [`ServerConfig::seed_command`]
///   left `None` to fall back to
///   [`crate::terminal_actor::default_shell_command`] — the user's `$SHELL`,
///   or `/bin/sh` per the byc.5 convention).
/// * Anything embedding `phux-server` and wanting a specific command
///   (e.g. an integration test driving a known fixture; see the
///   `input_dispatch.rs` test, which seeds with `cat` to get
///   deterministic echo).
pub fn seed_session_with_pty(
    state: &SharedState,
    name: &str,
    cmd: portable_pty::CommandBuilder,
    history_limit: u32,
    root_token: &CancellationToken,
) -> Result<phux_core::ids::TerminalId, crate::terminal_actor::TerminalActorError> {
    use phux_core::ids::TerminalId;
    let terminal: TerminalId = state.with_mut(|s| s.seed_session(name).2);
    let terminal_token = root_token.child_token();
    let bundle =
        TerminalActor::build_with_token(80, 24, Some(cmd), history_limit, terminal_token.clone())?;
    let crate::terminal_actor::TerminalActorBundle {
        mut actor,
        handle,
        exit_notify,
        ..
    } = bundle;
    // phux-y2t: wire the actor's agent-event sink and spawn a drain task
    // that fans bell / title / dirty / idle events out to event-stream
    // subscribers scoped to this pane. The wire `TerminalId` is interned
    // up front (stable for the pane's lifetime) and captured by the drain.
    let (event_tx, event_rx) = tokio::sync::mpsc::channel(EVENT_SINK_CAPACITY);
    actor.set_event_sink(event_tx);
    let wire_terminal_id = state.with_mut(|s| {
        let _ = s.spawn_terminal_actor(terminal, handle, terminal_token, actor.run());
        s.intern_terminal_wire(terminal)
    });
    spawn_pane_event_drain(state.clone(), wire_terminal_id, event_rx);
    spawn_terminal_exit_watcher(state.clone(), terminal, exit_notify, root_token.clone());
    Ok(terminal)
}

/// Add a **PTY-backed** pane to an existing `session`'s window and spawn its
/// `TerminalActor` — the split counterpart to [`seed_session_with_pty`]
/// (phux-i9zl).
///
/// Identical to `seed_session_with_pty` except the new pane joins
/// `session`'s window via [`ServerState::add_pane_to_session`] instead of
/// creating a fresh `spawn-N` session. A TUI split routes here so the new
/// L1 Terminal stays in the spawning client's current session.
///
/// Returns `Ok(None)` when `session` has no window to host the pane
/// (unreachable for a seeded session); the caller maps that to a wire
/// `SpawnError`. `Err` is an actor-build failure, same as the seed path.
pub fn spawn_pane_with_pty(
    state: &SharedState,
    session: phux_core::ids::SessionId,
    cmd: portable_pty::CommandBuilder,
    history_limit: u32,
    root_token: &CancellationToken,
) -> Result<Option<phux_core::ids::TerminalId>, crate::terminal_actor::TerminalActorError> {
    use phux_core::ids::TerminalId;
    let Some(terminal): Option<TerminalId> = state.with_mut(|s| s.add_pane_to_session(session))
    else {
        return Ok(None);
    };
    let terminal_token = root_token.child_token();
    let bundle =
        TerminalActor::build_with_token(80, 24, Some(cmd), history_limit, terminal_token.clone())?;
    let crate::terminal_actor::TerminalActorBundle {
        mut actor,
        handle,
        exit_notify,
        ..
    } = bundle;
    // Same agent-event wiring as the seed path (phux-y2t): intern the wire id
    // up front and spawn the per-pane event drain.
    let (event_tx, event_rx) = tokio::sync::mpsc::channel(EVENT_SINK_CAPACITY);
    actor.set_event_sink(event_tx);
    let wire_terminal_id = state.with_mut(|s| {
        let _ = s.spawn_terminal_actor(terminal, handle, terminal_token, actor.run());
        s.intern_terminal_wire(terminal)
    });
    spawn_pane_event_drain(state.clone(), wire_terminal_id, event_rx);
    spawn_terminal_exit_watcher(state.clone(), terminal, exit_notify, root_token.clone());
    Ok(Some(terminal))
}

/// Bounded capacity of the per-pane agent-event sink (SPEC §7.5,
/// phux-y2t). Small: events are coalesced (one `dirty` per burst, one
/// `idle` to close it) and the stream tolerates loss — a full sink drops
/// the event rather than stalling the actor's hot PTY-pump loop.
const EVENT_SINK_CAPACITY: usize = 64;

/// Drain a pane actor's agent-event channel and fan each event out to
/// event-stream subscribers scoped to `wire_terminal_id` (SPEC §7.5,
/// phux-y2t). Runs until the actor drops its event sender (pane gone).
///
/// `spawn_local` to co-locate with the actor on the `LocalSet` (the
/// cancellation story rides the root-token `JoinSet` cascade, same as the
/// EOF watcher).
fn spawn_pane_event_drain(
    state: SharedState,
    wire_terminal_id: phux_protocol::ids::TerminalId,
    mut event_rx: tokio::sync::mpsc::Receiver<AgentEvent>,
) {
    tokio::task::spawn_local(async move {
        while let Some(event) = event_rx.recv().await {
            broadcast_event(&state, Some(&wire_terminal_id), &event);
        }
    });
}

/// Spawn the per-pane EOF watcher task (phux-it8, reshaped by phux-4r1).
///
/// Awaits the `TerminalActor`'s `exit_notify` oneshot. When the actor
/// observes PTY EOF (the child process has exited — typically the
/// shell typed `exit`), this watcher broadcasts the L1 lifecycle event
/// `FrameKind::TerminalClosed { terminal_id, exit_status }` to every
/// client subscribed to the now-dead pane, then reaps the pane's
/// server-side state.
///
/// The watcher does NOT decide whether any client should detach:
/// "no Terminals left in my attached collection ⇒ detach" is a
/// *consumer* policy (ADR-0015 L1: lifecycle events are facts, detach
/// is interpretation), now owned by the TUI's `attach::driver`
/// main loop, which folds the closed pane out of its layout and
/// detaches itself when the last pane closes. The server stops
/// sending `FrameKind::Detached` on EOF.
///
/// The watcher is `spawn_local` because `SharedState` is `Send` but
/// we want the task to live on the same `LocalSet` that owns the
/// pane actor — co-locating the lifecycle keeps the cancellation
/// story tidy (root-token cascade still applies via `JoinSet` drop
/// when the runtime exits).
///
/// No-op when `exit_notify` is `None` (the bundle's receiver was
/// already taken) or when the actor exits without ever firing EOF
/// (cancellation via the root token, for example). Errors on the
/// oneshot recv side are treated identically to "EOF observed":
/// they only happen if the sender was dropped without firing, which
/// in current code means the actor was dropped without going through
/// the EOF branch — i.e. the pane is going away too. Broadcasting
/// `TERMINAL_CLOSED` is still the right response.
fn spawn_terminal_exit_watcher(
    state: SharedState,
    pane: phux_core::ids::TerminalId,
    exit_notify: Option<oneshot::Receiver<Option<i32>>>,
    root_token: CancellationToken,
) {
    let Some(rx) = exit_notify else {
        return;
    };
    tokio::task::spawn_local(async move {
        // Recv error (sender dropped without firing) is treated the
        // same as a fired EOF with unknown exit status: in both cases
        // the pane is dead and every subscribed client must be told.
        let exit_status = rx.await.unwrap_or(None);
        // phux-4li.11 / phux-4r1: broadcast the L1 lifecycle event
        // TERMINAL_CLOSED to every client subscribed to the dying pane.
        // The server's job ends here — it reports the fact. The detach
        // policy ("no Terminals left in my collection ⇒ detach") is the
        // consumer's (the TUI driver folds the pane out of its layout and
        // detaches itself when the last pane closes); the server no longer
        // sends `Detached` on EOF (ADR-0015 L1).
        broadcast_terminal_closed(&state, pane, exit_status).await;
        // phux-60s: reap the dead pane, cascading to its window and
        // session when they empty. When the last session is gone the
        // server has nothing left to serve, so fire the root token —
        // the tmux server-exit model. Without this the server lingers
        // forever after every shell exits.
        //
        // Two guards keep this from misfiring:
        //   * `has_served_client`: a freshly auto-spawned server whose
        //     seed pane dies before anyone attaches must NOT vanish — the
        //     launching `phux` is still racing to connect and will
        //     repopulate it via `CreateIfMissing`. Only self-exit once
        //     we've actually served someone.
        //   * `!root_token.is_cancelled()`: a Ctrl-C shutdown cancels the
        //     pane actor too, routing through here; don't log a spurious
        //     "self-exit" or double-cancel during normal teardown.
        let (server_empty, served) =
            state.with_mut(|s| (s.reap_terminal(pane), s.has_served_client()));
        if server_empty && served && !root_token.is_cancelled() {
            info!("last session reaped after serving clients; server self-exit");
            root_token.cancel();
        }
    });
}

/// Emit `TERMINAL_CLOSED { terminal_id, exit_status }` to every client
/// subscribed to `pane` (phux-4li.11, SPEC §7.2 / §10.1).
///
/// Fanout uses the per-pane subscriber list maintained by
/// [`ServerState::attach`] / [`ServerState::detach`]. The wire
/// `TerminalId` is interned via [`ServerState::intern_terminal_wire`]
/// so the frame carries the same id the client saw on
/// `TERMINAL_SPAWNED` / `TERMINAL_SNAPSHOT`. The send is best-effort:
/// a client whose mailbox has closed (it dropped the socket) is
/// silently skipped — the `reap_terminal` call in the caller handles
/// server-side state cleanup.
async fn broadcast_terminal_closed(
    state: &SharedState,
    pane: phux_core::ids::TerminalId,
    exit_status: Option<i32>,
) {
    let targets: Vec<(
        phux_protocol::ids::TerminalId,
        tokio::sync::mpsc::Sender<Outbound>,
    )> = state.with_mut(|s| {
        let wire_terminal_id = s.intern_terminal_wire(pane);
        s.subscribers_for_terminal(pane)
            .iter()
            .filter_map(|cid| {
                s.attached
                    .get(cid)
                    .map(|c| (wire_terminal_id.clone(), c.tx.clone()))
            })
            .collect()
    });
    // The wire id is stable for the pane's lifetime; intern it once so
    // both the L1 `TERMINAL_CLOSED` fanout and the `pane_closed` agent
    // event below carry the same id the client saw on spawn/snapshot.
    let wire_terminal_id = state.with_mut(|s| s.intern_terminal_wire(pane));
    if targets.is_empty() {
        debug!(?pane, "TERMINAL_CLOSED: no L1-subscribed clients to notify");
    } else {
        debug!(
            ?pane,
            count = targets.len(),
            ?exit_status,
            "TERMINAL_CLOSED: broadcasting to subscribed clients",
        );
        for (wire_terminal_id, tx) in targets {
            let _ = tx
                .send(Outbound::Frame(FrameKind::TerminalClosed {
                    terminal_id: wire_terminal_id,
                    exit_status,
                }))
                .await;
        }
    }
    // phux-y2t: fan a `pane_closed` agent event to event-stream
    // subscribers (SPEC §7.5) regardless of L1 subscribers — a
    // `watch`-only client that never attached must still learn the pane
    // died, so this MUST run even when the L1 fanout above was empty.
    broadcast_event(
        state,
        Some(&wire_terminal_id),
        &AgentEvent::PaneClosed { exit_status },
    );
}

/// Free the per-consumer state-sync entries (ADR-0018, phux-0q8) this
/// client holds across every pane it subscribes to, then remove the
/// client from `ServerState`.
///
/// Counterpart to the `consumer_attach` registration the ATTACH path
/// performs per pane. Run at every client-teardown site (explicit
/// DETACH, transport disconnect, PTY EOF) so the per-consumer
/// `RenderState` cache the actor allocated at attach is dropped rather
/// than leaked until pane teardown.
///
/// Handles are gathered under-lock (`subscribed_terminal_handles`); the
/// `consumer_detach` sends happen off-lock to avoid awaiting inside
/// `with_mut`. `try_send` is non-blocking and best-effort: a full or
/// closed mailbox just means the actor is gone or saturated. A dropped
/// detach on a *live* actor is no longer a leak — `state.detach` below
/// drops the client's outbound receiver, so the actor's `tick_emit`
/// observes the mailbox as `Closed` on its next tick and reaps the
/// orphaned per-consumer entry itself (phux-ddg, the self-healing path).
fn detach_and_release_consumer_state(state: &SharedState, client_id: ClientId) {
    let wire_client_id =
        phux_protocol::ids::ClientId::new(u32::try_from(client_id.0).unwrap_or(u32::MAX));
    let handles = state.with(|s| s.subscribed_terminal_handles(client_id));
    for handle in handles {
        let (reply_tx, _reply_rx) = oneshot::channel();
        match handle.consumer_detach.try_send(ConsumerDetachRequest {
            client_id: wire_client_id,
            reply: reply_tx,
        }) {
            Ok(()) => {}
            Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                trace!(
                    ?client_id,
                    "consumer_detach mailbox full; entry reaped by tick_emit when its mailbox closes",
                );
            }
            Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                trace!(
                    ?client_id,
                    "consumer_detach: pane actor gone; nothing to free"
                );
            }
        }
    }
    state.with_mut(|s| s.detach(client_id));
}

/// Prepare the parent directory of `socket_path` with mode `0o700`.
fn prepare_socket_dir(socket_path: &Path) -> Result<(), ServerError> {
    let Some(parent) = socket_path.parent() else {
        return Ok(());
    };
    if parent.as_os_str().is_empty() {
        return Ok(());
    }
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true).mode(0o700);
    builder
        .create(parent)
        .map_err(|source| ServerError::PrepareDir {
            path: parent.to_path_buf(),
            source,
        })
}

/// Handle the case where `socket_path` already exists. If something accepts a
/// connection on it within the probe timeout, treat it as live and refuse to
/// start. Otherwise unlink the stale entry so `bind` can succeed.
async fn handle_existing_socket(socket_path: &Path) -> Result<(), ServerError> {
    let metadata = match std::fs::symlink_metadata(socket_path) {
        Ok(m) => m,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(ServerError::Io(err)),
    };
    // Anything sitting in the way — socket, file, symlink — gets probed and
    // either rejected or removed.
    let connect = tokio::time::timeout(STALE_PROBE_TIMEOUT, UnixStream::connect(socket_path)).await;
    if let Ok(Ok(_stream)) = connect {
        return Err(ServerError::SocketBusy(socket_path.to_path_buf()));
    }
    debug!(
        path = %socket_path.display(),
        file_type = ?metadata.file_type(),
        "removing stale socket entry",
    );
    std::fs::remove_file(socket_path).map_err(ServerError::Io)?;
    Ok(())
}

/// Core accept loop. Pulled out to keep `run_async` flat.
///
/// Per ADR-0014, every per-client task spawns via
/// [`tokio::task::JoinSet::spawn_local`]; the futures we hand it are
/// `!Send` because they call into pane actors that own `!Send`
/// `Terminal`s.
///
/// `root_token` is the per-server root cancellation token. Cancellation
/// drives a clean return from this loop (the `JoinSet` of per-client
/// tasks then drops, aborting any in-flight client tasks).
#[allow(
    clippy::future_not_send,
    reason = "ADR-0014: the server runs on a LocalSet; per-connection transports (L::Reader/Writer) are !Send by design"
)]
async fn accept_loop<L: Incoming>(
    listener: &L,
    state: SharedState,
    root_token: CancellationToken,
) -> Result<(), ServerError> {
    // JoinSet of per-client tasks. Dropping this set on loop exit
    // aborts every still-running client task in one step — much
    // shorter than waiting for each task's own `select!` to observe
    // its child token's cancellation.
    let mut clients: JoinSet<()> = JoinSet::new();
    loop {
        tokio::select! {
            () = root_token.cancelled() => {
                info!("root cancellation token fired; accept loop exiting");
                return Ok(());
            }
            accept = listener.accept() => {
                match accept {
                    Ok((reader, writer)) => {
                        debug!(transport = listener.kind(), "client connected");
                        // Allocate the per-client routing id up-front so the
                        // task can detach itself cleanly on EOF.
                        let client_id = state.with_mut(crate::state::ServerState::new_client_id);
                        let task_state = state.clone();
                        let client_token = root_token.child_token();
                        let task_root_token = root_token.clone();
                        clients.spawn_local(async move {
                            if let Err(err) = handle_client(reader, writer, task_state.clone(), client_id, client_token, task_root_token).await {
                                warn!(error = %err, "client task ended with error");
                            }
                            // Implicit detach on EOF / error path — matches
                            // the explicit `DETACH` semantics for the wire
                            // path that will land alongside the protocol
                            // variants.
                            detach_and_release_consumer_state(&task_state, client_id);
                        });
                    }
                    Err(err) => {
                        // Accept errors are typically transient (EMFILE,
                        // ECONNABORTED). Log and continue rather than killing
                        // the server.
                        error!(error = %err, "accept failed");
                    }
                }
            }
        }
    }
}

/// Per-client task. Reads frames in a loop and dispatches each one.
///
/// Outbound messages are routed through a per-client `mpsc` channel
/// drained by a sibling writer task (also `spawn_local`'d). This gives
/// us one place to back-pressure on slow clients without entangling
/// the read side, and matches the `tx: mpsc::Sender<Outbound>` shape
/// `ServerState::attach` already wants. The channel carries
/// [`Outbound`] so every typed [`FrameKind`] send shares one ordering
/// domain.
///
/// `phux-byc.8`: implements the ATTACH path. Resolves the target,
/// builds a [`SessionSnapshot`](phux_protocol::wire::info::SessionSnapshot)
/// from the registry, requests a snapshot from each pane's
/// [`TerminalActor`](crate::terminal_actor::TerminalActor), and emits
/// `ATTACHED` + `TERMINAL_SNAPSHOT` frames per SPEC §13. On unknown
/// session, emits an `ERROR` frame with `SessionNotFound` (SPEC §14).
#[allow(
    clippy::too_many_lines,
    reason = "single per-client dispatch loop; each frame arm is small and the catalog grows linearly. Extracting arms hides the wire→state seam without simplifying it."
)]
#[allow(
    clippy::cognitive_complexity,
    reason = "see `clippy::too_many_lines` rationale above: the dispatch shape is one match arm per wire frame variant, where each arm is small and self-contained. Splitting on the arm boundary fragments the wire→state seam; merging arms across variants is what generated the complexity score in the first place."
)]
async fn handle_client<R, W>(
    mut reader: R,
    writer: W,
    state: SharedState,
    client_id: ClientId,
    token: CancellationToken,
    root_token: CancellationToken,
) -> io::Result<()>
where
    R: FrameReader + 'static,
    W: FrameWriter + 'static,
{
    debug!(?client_id, "client task started");

    // Allocate the per-client outbound mailbox + spawn the writer task.
    // The writer drains one `Outbound` channel; closure of this one
    // channel is the unambiguous signal for the writer to exit.
    let (out_tx, out_rx) = tokio::sync::mpsc::channel::<Outbound>(DEFAULT_CLIENT_MAILBOX);
    // Per-client `JoinSet` for sibling tasks (today: just the writer).
    // Held in this scope so it drops with `handle_client` and the
    // writer aborts if it hasn't already exited via its own
    // close-on-EOF path. Keeps lifecycle plumbing local.
    let mut sibling_tasks: JoinSet<()> = JoinSet::new();
    sibling_tasks.spawn_local(writer_task(writer, out_rx, client_id));

    // Per-attach raw-output pumps. These are deliberately separate from
    // `sibling_tasks`: DETACH/session switch must abort pane output pumps
    // without killing the writer, because the writer still needs to emit
    // DETACHED and may serve a later ATTACH on the same connection.
    let mut output_pumps: JoinSet<()> = JoinSet::new();

    // Per-connection cache of the most-recently-advertised
    // [`ClientCapabilities`] (SPEC §6.2). HELLO populates this; ATTACH
    // consumes it when constructing the `AttachedClient`. Pre-HELLO it
    // defaults to [`ClientCapabilities::default`] (most-permissive) so a
    // client that skips HELLO (out of spec, but tolerated for
    // forward-compat) still attaches with sensible bytes-on-wire behavior.
    let mut negotiated_client_caps = ClientCapabilities::default();

    loop {
        // Pull the next complete frame from the transport — length-prefixed on
        // UDS, one binary message on WebSocket (see `transport.rs`). EOF ends
        // the session cleanly; cancellation preempts a slow read via the biased
        // select so a server-wide shutdown isn't blocked behind it.
        let framed = tokio::select! {
            biased;
            () = token.cancelled() => {
                debug!(?client_id, "client task cancelled by root token");
                return Ok(());
            }
            res = reader.read_frame() => match res {
                Ok(Some(framed)) => framed,
                Ok(None) => {
                    debug!("client disconnected (eof)");
                    return Ok(());
                }
                Err(err) => {
                    debug!(error = %err, "client read error; closing");
                    return Ok(());
                }
            },
        };

        let frame = match FrameKind::decode(&framed) {
            Ok((frame, _rest)) => frame,
            Err(err) => {
                warn!(error = ?err, "client sent undecodable frame; closing");
                return Ok(());
            }
        };

        match frame {
            FrameKind::Hello {
                client_name,
                protocol_major,
                protocol_minor,
                protocol_patch,
                client_caps,
            } => {
                debug!(
                    ?client_id,
                    %client_name,
                    protocol_major,
                    protocol_minor,
                    protocol_patch,
                    color_support = ?client_caps.color_support,
                    "HELLO",
                );
                // SPEC §6.1: HELLO arrives before ATTACH. Cache the
                // advertised tier on the per-task stack; the ATTACH
                // branch consumes it when building the `AttachedClient`.
                // If a client (mis-)sends HELLO post-ATTACH we also
                // patch the live `AttachedClient` so downsample picks
                // up the change — the alternative (protocol error
                // close) gives the operator nothing to debug.
                negotiated_client_caps = client_caps;
                state.with_mut(|s| {
                    s.set_client_capabilities(client_id, client_caps);
                    // SPEC §6.2: cache the negotiated layer set. The L3
                    // dispatch arms (METADATA_*) gate emission of
                    // `METADATA_CHANGED` on `client_speaks_l3` so non-L3
                    // consumers never see L3 frames (SPEC §16.4).
                    s.set_client_layers(client_id, client_caps.layers);
                });
                // SPEC §6.1: server replies with HELLO_OK before ATTACH
                // is processed on this connection. The single-version
                // reference server echoes its own PROTOCOL_VERSION as the
                // selected version (no `VERSION_INCOMPATIBLE` negotiation
                // yet) and advertises the full tier set it mounts (L1+L2+L3);
                // the negotiated set is the intersection with the client's
                // `layers`. `server_id` is the opaque process identity.
                let hello_ok = FrameKind::HelloOk {
                    protocol_major: PROTOCOL_VERSION.major,
                    protocol_minor: PROTOCOL_VERSION.minor,
                    protocol_patch: PROTOCOL_VERSION.patch,
                    server_caps: ServerCapabilities::new().with_layers(LayerSet::all()),
                    server_id: std::process::id().to_be_bytes().to_vec(),
                };
                if out_tx.send(Outbound::Frame(hello_ok)).await.is_err() {
                    trace!(?client_id, "HELLO_OK send dropped: writer gone");
                }
            }
            FrameKind::Ping { nonce } => {
                // SPEC §7.4: echo nonce in PONG.
                debug!(nonce, "PING -> PONG");
                if out_tx
                    .send(Outbound::Frame(FrameKind::Pong { nonce }))
                    .await
                    .is_err()
                {
                    trace!(?client_id, nonce, "PONG send dropped: writer gone");
                }
            }
            FrameKind::Attach {
                target,
                viewport,
                request_scrollback,
                scrollback_limit_lines,
            } => {
                handle_attach(
                    &state,
                    client_id,
                    target,
                    viewport,
                    request_scrollback,
                    scrollback_limit_lines,
                    &out_tx,
                    negotiated_client_caps,
                    &root_token,
                    &mut output_pumps,
                )
                .await;
            }
            FrameKind::Detach => {
                // Lifecycle event at info so it shows under the default
                // capture filter — DETACH is a per-client lifecycle edge a
                // trace reader wants to see without enabling debug.
                info!(?client_id, "DETACH");
                // SPEC §7.3: server responds with DETACHED, then closes.
                // For byc.8 we emit DETACHED and let the read loop
                // continue — actual transport close lands when the
                // client drops, which is the path the existing
                // socket-lifecycle tests exercise.
                // Intentionally silent on send failure: we are about
                // to `detach()` this client on the next line, so the
                // writer being gone is the next thing to happen
                // anyway. Logging here would be pure noise.
                abort_output_pumps(&mut output_pumps, client_id, "DETACH").await;
                let _ = out_tx.send(Outbound::Frame(FrameKind::Detached)).await;
                detach_and_release_consumer_state(&state, client_id);
            }
            FrameKind::ViewportResize { viewport } => {
                debug!(
                    ?client_id,
                    cols = viewport.cols,
                    rows = viewport.rows,
                    "VIEWPORT_RESIZE"
                );
                handle_viewport_resize(&state, client_id, &viewport);
            }
            FrameKind::InputKey { terminal_id, event } => {
                handle_terminal_input(
                    &state,
                    client_id,
                    &terminal_id,
                    TerminalInput::Key(event),
                    "INPUT_KEY",
                );
            }
            FrameKind::InputMouse { terminal_id, event } => {
                handle_terminal_input(
                    &state,
                    client_id,
                    &terminal_id,
                    TerminalInput::Mouse(event),
                    "INPUT_MOUSE",
                );
            }
            FrameKind::InputFocus { terminal_id, event } => {
                handle_terminal_input(
                    &state,
                    client_id,
                    &terminal_id,
                    TerminalInput::Focus(event),
                    "INPUT_FOCUS",
                );
            }
            FrameKind::FrameAck { terminal_id, seq } => {
                handle_frame_ack(&state, client_id, &terminal_id, seq);
            }
            FrameKind::GetMetadata {
                request_id,
                scope,
                key,
            } => {
                handle_get_metadata(&state, client_id, request_id, &scope, &key, &out_tx).await;
            }
            FrameKind::SetMetadata {
                request_id,
                scope,
                key,
                value,
            } => {
                handle_set_metadata(&state, client_id, request_id, &scope, &key, value);
            }
            FrameKind::DeleteMetadata {
                request_id,
                scope,
                key,
            } => {
                handle_delete_metadata(&state, client_id, request_id, &scope, &key);
            }
            FrameKind::ListMetadata { request_id, scope } => {
                handle_list_metadata(&state, client_id, request_id, &scope, &out_tx).await;
            }
            FrameKind::SubscribeMetadata { scope, key } => {
                handle_subscribe_metadata(&state, client_id, scope, key);
            }
            FrameKind::SubscribeEvents { terminal } => {
                handle_subscribe_events(&state, client_id, terminal, &out_tx);
            }
            FrameKind::SpawnTerminal {
                request_id,
                collection,
                command,
                cwd,
                env,
            } => {
                handle_spawn_terminal(
                    &state,
                    client_id,
                    request_id,
                    collection,
                    command,
                    cwd,
                    env,
                    &out_tx,
                    &root_token,
                )
                .await;
            }
            FrameKind::TerminalResize {
                terminal_id,
                cols,
                rows,
            } => {
                handle_terminal_resize(&state, client_id, &terminal_id, cols, rows);
            }
            FrameKind::Command {
                request_id,
                command,
            } => {
                handle_command(&state, client_id, request_id, command, &out_tx, &root_token).await;
            }
            other => {
                debug!(kind = ?other, "unhandled message type (INPUT_* / etc.)");
            }
        }
    }
}

async fn abort_output_pumps(
    output_pumps: &mut JoinSet<()>,
    client_id: ClientId,
    reason: &'static str,
) {
    if output_pumps.is_empty() {
        return;
    }
    debug!(
        ?client_id,
        pump_count = output_pumps.len(),
        reason,
        "aborting per-attach output pumps",
    );
    output_pumps.abort_all();
    while output_pumps.join_next().await.is_some() {}
}

// -----------------------------------------------------------------------------
// L3 metadata dispatch — SPEC §7.4 / §11.L3 (phux-4li.2 / phux-4li.8).
//
// GET / LIST replies ride dedicated `METADATA_VALUE` / `METADATA_KEYS`
// S→C frames (allocated by phux-4li.8) correlated to the originating
// request by `request_id`. Reply emission, like `METADATA_CHANGED`
// fan-out, is gated on `client_speaks_l3` (SPEC §16.4): a non-L3
// consumer that nevertheless ships an L3 request gets silence.
// -----------------------------------------------------------------------------

async fn handle_get_metadata(
    state: &SharedState,
    client_id: ClientId,
    request_id: u32,
    scope: &phux_protocol::wire::frame::Scope,
    key: &str,
    out_tx: &tokio::sync::mpsc::Sender<Outbound>,
) {
    let (value, speaks_l3) =
        state.with(|s| (s.metadata().get(scope, key), s.client_speaks_l3(client_id)));
    debug!(
        ?client_id,
        request_id,
        ?scope,
        %key,
        present = value.is_some(),
        speaks_l3,
        "GET_METADATA",
    );
    if !speaks_l3 {
        // SPEC §16.4: out-of-tier traffic from a non-L3 consumer is
        // dropped silently, matching the SUBSCRIBE_METADATA arm above.
        // A future ticket may switch to ERROR { OUT_OF_TIER } once the
        // error code lands.
        return;
    }
    if out_tx
        .send(Outbound::Frame(FrameKind::MetadataValue {
            request_id,
            value,
        }))
        .await
        .is_err()
    {
        trace!(
            ?client_id,
            request_id, "METADATA_VALUE send dropped: writer gone"
        );
    }
}

fn handle_set_metadata(
    state: &SharedState,
    client_id: ClientId,
    request_id: u32,
    scope: &phux_protocol::wire::frame::Scope,
    key: &str,
    value: Vec<u8>,
) {
    debug!(?client_id, request_id, ?scope, %key, "SET_METADATA");
    let delivered = state.with_mut(|s| s.metadata_set(scope, key, value));
    trace!(
        ?client_id,
        request_id,
        subscriber_count = delivered.len(),
        "SET_METADATA delivered"
    );
}

fn handle_delete_metadata(
    state: &SharedState,
    client_id: ClientId,
    request_id: u32,
    scope: &phux_protocol::wire::frame::Scope,
    key: &str,
) {
    debug!(?client_id, request_id, ?scope, %key, "DELETE_METADATA");
    let delivered = state.with_mut(|s| s.metadata_delete(scope, key));
    trace!(
        ?client_id,
        request_id,
        subscriber_count = delivered.len(),
        "DELETE_METADATA delivered"
    );
}

async fn handle_list_metadata(
    state: &SharedState,
    client_id: ClientId,
    request_id: u32,
    scope: &phux_protocol::wire::frame::Scope,
    out_tx: &tokio::sync::mpsc::Sender<Outbound>,
) {
    let (keys, speaks_l3) =
        state.with(|s| (s.metadata().list(scope), s.client_speaks_l3(client_id)));
    debug!(
        ?client_id,
        request_id,
        ?scope,
        key_count = keys.len(),
        speaks_l3,
        "LIST_METADATA",
    );
    if !speaks_l3 {
        // SPEC §16.4: same out-of-tier gating as `handle_get_metadata`.
        return;
    }
    if out_tx
        .send(Outbound::Frame(FrameKind::MetadataKeys {
            request_id,
            keys,
        }))
        .await
        .is_err()
    {
        trace!(
            ?client_id,
            request_id, "METADATA_KEYS send dropped: writer gone"
        );
    }
}

fn handle_subscribe_metadata(
    state: &SharedState,
    client_id: ClientId,
    scope: phux_protocol::wire::frame::Scope,
    key: String,
) {
    state.with_mut(|s| {
        if !s.client_speaks_l3(client_id) {
            // SPEC §16.4: out-of-tier traffic from a non-L3 consumer.
            // The L3 dispatch is best-effort: we drop the subscribe
            // rather than tear the connection down, on the theory that
            // a misbehaving client should learn from silence faster
            // than from a protocol error. A future ticket may swap
            // this for an explicit `ERROR { OUT_OF_TIER }` once the
            // error code lands.
            debug!(?client_id, ?scope, %key, "SUBSCRIBE_METADATA refused (non-L3)");
            return;
        }
        debug!(?client_id, ?scope, %key, "SUBSCRIBE_METADATA");
        s.metadata_subscribe(client_id, scope, key);
    });
}

/// Record an agent-event subscription for `client_id` (SPEC §7.5,
/// phux-y2t). `terminal = None` subscribes server-wide; `Some(id)`
/// subscribes per-pane. Idempotent (the per-client scope set absorbs
/// duplicates) and connection-scoped (cleared on detach). Unlike the L3
/// metadata path this is not tier-gated — the event stream is part of L1
/// and any consumer may opt in.
fn handle_subscribe_events(
    state: &SharedState,
    client_id: ClientId,
    terminal: Option<phux_protocol::ids::TerminalId>,
    out_tx: &tokio::sync::mpsc::Sender<Outbound>,
) {
    debug!(?client_id, ?terminal, "SUBSCRIBE_EVENTS");
    // Capture the client's mailbox in the subscription so event fanout
    // reaches it even without an ATTACH (a pure `watch` client never
    // attaches).
    state.with_mut(|s| s.subscribe_events(client_id, terminal, out_tx.clone()));
}

/// Push an [`AgentEvent`] to every client subscribed to events scoped to
/// `terminal` (SPEC §7.5, phux-y2t).
///
/// `terminal` is the wire id the event concerns, or `None` for a
/// server-scoped event with no owning Terminal. Fan-out uses
/// [`ServerState::event_targets`], which matches server-wide subscribers
/// plus (when `terminal` is `Some`) per-pane subscribers for that id.
/// Best-effort: a client whose mailbox is full or closed is silently
/// skipped — the event stream is an accelerator, never a guarantee
/// (a dropped event just means the consumer falls back to the poll floor).
///
/// Synchronous: fanout uses non-blocking `try_send`, so there is nothing
/// to await — the caller need not be in an async context to push an event.
fn broadcast_event(
    state: &SharedState,
    terminal: Option<&phux_protocol::ids::TerminalId>,
    event: &AgentEvent,
) {
    let targets = state.with(|s| s.event_targets(terminal));
    if targets.is_empty() {
        return;
    }
    trace!(
        ?terminal,
        ?event,
        count = targets.len(),
        "EVENT: broadcasting"
    );
    for tx in targets {
        // `try_send` is non-blocking: a full mailbox drops the event
        // rather than stalling the emitter. The accelerator contract
        // tolerates loss (the CLI poll floor still converges).
        let _ = tx.try_send(Outbound::Frame(FrameKind::Event {
            terminal: terminal.cloned(),
            event: event.clone(),
        }));
    }
}

/// Writer task: drain the per-client outbound channel and write each
/// message to the socket. Encodes [`Outbound::Frame`] via
/// `FrameKind::encode`.
///
/// Exits when the channel closes — i.e. the client task drops its
/// sender.
async fn writer_task<W: FrameWriter>(
    mut writer: W,
    mut rx: tokio::sync::mpsc::Receiver<Outbound>,
    client_id: ClientId,
) {
    let mut buf = BytesMut::with_capacity(1024);
    while let Some(msg) = rx.recv().await {
        let Outbound::Frame(frame) = msg;
        buf.clear();
        frame.encode(&mut buf);
        if let Err(err) = writer.write_frame(&buf).await {
            debug!(?client_id, error = %err, "writer error on frame; client task ending");
            return;
        }
    }
    debug!(?client_id, "writer task exiting (channel closed)");
}

/// Tuple bundling everything `handle_attach` needs after it's done
/// touching `ServerState`. Cloned out of the critical section so the
/// remaining awaits don't hold the state lock.
type AttachPrepared = (
    phux_protocol::wire::info::SessionSnapshot,
    phux_protocol::ids::ClientId,
    Vec<AttachSnapshotPane>,
);

/// Resolve `target` to a session name. SPEC §13: `ByName` is the only
/// fully-implemented mode in byc.8; the others fail with
/// `SessionNotFound` until follow-up tickets land.
async fn resolve_attach_target(
    state: &SharedState,
    target: AttachTarget,
    out_tx: &tokio::sync::mpsc::Sender<Outbound>,
    root_token: &CancellationToken,
) -> Option<String> {
    match target {
        AttachTarget::ByName(name) => Some(name),
        AttachTarget::ById(id) => {
            let resolved = state
                .with(|s| s.session_id_bridge.resolve(id))
                .and_then(|sid| {
                    state.with(|s| s.registry.session(sid).map(|sess| sess.name.clone()))
                });
            if resolved.is_none() {
                send_error(
                    out_tx,
                    ErrorCode::SessionNotFound,
                    &format!("session id {} not found", id.get()),
                )
                .await;
            }
            resolved
        }
        AttachTarget::Last => {
            // Resolve against the global per-server "last touched
            // session" order (see ServerState::touch_session). If a
            // prior touch exists and that session is still live in the
            // registry, return its name; otherwise treat as "not found"
            // — matches SPEC §13's allowance that "implementations
            // without prior-attach memory MAY return SESSION_NOT_FOUND".
            // We follow the same code path when the prior session has
            // been killed since the last touch.
            //
            // TODO(error-codes): introduce ErrorCode::NoLastSession
            // (and a sibling variant for "last session killed") so
            // clients can distinguish "no history" from "history is
            // stale" without parsing the message string. Additive
            // ErrorCode work is intentionally out of scope here.
            let resolved = state.with(|s| {
                s.most_recently_touched_session()
                    .and_then(|sid| s.registry.session(sid).map(|sess| sess.name.clone()))
            });
            if resolved.is_none() {
                send_error(
                    out_tx,
                    ErrorCode::SessionNotFound,
                    "no prior session activity: AttachTarget::Last has nothing to resolve",
                )
                .await;
            }
            resolved
        }
        AttachTarget::CreateIfMissing { name, command, cwd } => {
            resolve_create_if_missing(state, name, command, cwd, out_tx, root_token).await
        }
        _ => {
            send_error(
                out_tx,
                ErrorCode::SessionNotFound,
                "unknown AttachTarget variant",
            )
            .await;
            None
        }
    }
}

/// Handle [`AttachTarget::CreateIfMissing`] (phux-k61.3, SPEC §13).
///
/// Behavior:
///
/// * If a session with `name` already exists in the registry, return
///   its name unchanged — the caller's `prepare_attach` then runs the
///   normal `ByName` attach path against it. No duplicate session is
///   created.
/// * Otherwise, seed a fresh `(session, window, pane)` triple, spawn
///   the seed pane's actor in the mode the server was configured
///   with (PTY-backed via [`seed_session_with_pty`] when
///   [`crate::state::ServerState::attach_create_seeds_pty`] is `true`,
///   or no-PTY via [`seed_session_with_actor`] otherwise), and return
///   the name so the caller proceeds with the normal attach path.
///
/// `command` and `cwd` from the wire frame are honored only when the
/// PTY mode is on AND no explicit
/// [`crate::state::ServerState::attach_create_seed_command`] preempts
/// them: an explicit per-server seed command always wins (it's how the
/// `phux server` binary pins `default_shell_command()` for the user).
/// The PTY path also currently ignores `cwd` — pre-seeded PTY launches
/// land in the server's CWD today, and lifting that into a
/// per-`CreateIfMissing` override is filed as a follow-up rather than
/// snuck in here. The no-PTY path ignores both, matching the existing
/// `seed_session_with_actor` shape.
///
/// On terminal-actor spawn failure (e.g. PTY allocation fails on a
/// host with no remaining ptys), emits a `SessionNotFound` error
/// frame (mirroring how the pre-seed path logs-and-continues at
/// startup) and returns `None` so the attach fails atomically. We
/// reuse `SessionNotFound` rather than introducing a new error code:
/// the user-visible effect is "the requested session is not available
/// to attach to", which is what `SessionNotFound` already means on
/// the wire. A richer error code (e.g. `SessionCreateFailed`) is a
/// SPEC-level follow-up.
async fn resolve_create_if_missing(
    state: &SharedState,
    name: String,
    command: Option<Vec<String>>,
    _cwd: Option<String>,
    out_tx: &tokio::sync::mpsc::Sender<Outbound>,
    root_token: &CancellationToken,
) -> Option<String> {
    // Fast path: a session with this name already exists. Fall through
    // to the normal `ByName(name)` attach by returning `name` as-is.
    // The lookup is read-only so we hold only an immutable borrow.
    if state.with(|s| s.session_by_name(&name).is_some()) {
        debug!(session = %name, "CreateIfMissing: session already exists, attaching");
        return Some(name);
    }

    // Slow path: create the session + seed pane. Snapshot the server's
    // configured PTY mode and (optional) override command before
    // releasing the state borrow.
    let (with_pty, override_cmd, history_limit, term) = state.with(|s| {
        (
            s.attach_create_seeds_pty(),
            s.attach_create_seed_command(),
            s.history_limit(),
            s.term().to_owned(),
        )
    });

    let seed_result = if with_pty {
        // Resolve the command. Precedence:
        //   1. The server-wide override stashed via
        //      `set_attach_create_pty(_, Some(cmd))`. Set explicitly by
        //      the runtime (or by tests that want a deterministic
        //      child like `cat`).
        //   2. The wire-level `command` from the CreateIfMissing
        //      variant. This is the per-attach command knob clients
        //      use to spawn (e.g.) `phux new -- vim foo.txt`.
        //   3. `default_shell_command()` (the user's `$SHELL`, or
        //      `/bin/sh`) — same fallback the pre-seed path uses.
        let mut cmd = override_cmd.unwrap_or_else(|| match command {
            Some(argv) if !argv.is_empty() => {
                let mut head = argv.into_iter();
                // Safe: argv is non-empty here.
                let program = head.next().unwrap_or_default();
                let mut builder = portable_pty::CommandBuilder::new(program);
                for arg in head {
                    builder.arg(arg);
                }
                builder
            }
            _ => crate::terminal_actor::default_shell_command(),
        });
        // Apply the server-wide `defaults.term` (phux-ign); this overrides
        // whatever baseline the builder carried.
        crate::terminal_actor::apply_term(&mut cmd, &term);
        seed_session_with_pty(state, &name, cmd, history_limit, root_token)
    } else {
        // No-PTY path: the wire `command` is meaningless without a
        // child to exec it on. We still create the session+pane so
        // the snapshot path has a target — this is the shape every
        // existing `spawn_server` test uses.
        seed_session_with_actor(state, &name, history_limit, root_token)
    };

    if let Err(err) = seed_result {
        warn!(
            session = %name,
            error = %err,
            "CreateIfMissing: failed to spawn pane actor for newly-created session",
        );
        send_error(
            out_tx,
            ErrorCode::SessionNotFound,
            &format!("CreateIfMissing: failed to create session {name:?}: {err}"),
        )
        .await;
        return None;
    }

    debug!(
        session = %name,
        pty = with_pty,
        "CreateIfMissing: created session and seeded pane"
    );
    Some(name)
}

/// Resolve a freshly-spawned pane's working directory from
/// `defaults.cwd-inheritance` (phux-cs6) when the `SPAWN_TERMINAL` wire
/// frame left `cwd` unset.
///
/// Returns the directory to seed the new pane's `CommandBuilder.cwd`
/// with, or `None` to inherit the server process's CWD (no override) —
/// the same effect the wire-`cwd = None` path had before this policy
/// existed.
///
/// Policy mapping:
/// * [`InheritFocused`](phux_config::CwdInheritance::InheritFocused) —
///   look up the spawning client's focused pane and ask its actor for
///   the live PTY CWD (a kernel query on the PTY child, see
///   [`crate::cwd_query`]). `None` when the client is not attached, has
///   no focused pane, the pane has no live handle, or the query is
///   unsupported/denied — each falls through to no override.
/// * [`Home`](phux_config::CwdInheritance::Home) — `$HOME`, or `None`
///   when unset.
/// * [`SessionRoot`](phux_config::CwdInheritance::SessionRoot) — the
///   session's creation directory: the live CWD of the session's seed
///   (oldest) pane, captured once and frozen in
///   [`crate::state::ServerState::record_session_root`] so a later `cd`
///   in the seed pane does not move the root. `None` when the client is
///   not attached, the session has no live seed pane, or the query is
///   unsupported/denied (with no previously frozen value to fall back on).
/// * [`LastCwdPerWindow`](phux_config::CwdInheritance::LastCwdPerWindow) —
///   the most-recent CWD observed in the spawning client's active window.
///   Resolved from the active pane's live CWD, recorded into
///   [`crate::state::ServerState::record_window_last_cwd`], and reused as
///   the fallback when a subsequent live query fails. `None` when there is
///   no active window and nothing was ever recorded.
async fn resolve_inherited_cwd(state: &SharedState, client_id: ClientId) -> Option<String> {
    let mode = state.with(crate::state::ServerState::cwd_inheritance);
    match mode {
        phux_config::CwdInheritance::InheritFocused => {
            // Find the spawning client's focused pane's actor handle in a
            // single critical section, then query it off-lock (the actor
            // runs on the same LocalSet; `with` must not be held across
            // the await).
            let handle = state.with(|s| {
                let session = s.attached.get(&client_id)?.session;
                let focused = s.active_pane_of_session(session)?;
                s.terminal_handle(focused).cloned()
            })?;
            query_pane_cwd(handle).await
        }
        phux_config::CwdInheritance::Home => std::env::var("HOME").ok().filter(|h| !h.is_empty()),
        phux_config::CwdInheritance::SessionRoot => {
            // The session root is the seed pane's directory at session
            // creation, frozen on first observation. Query the seed pane
            // live; if a root was already frozen, reuse it (and the live
            // query is redundant). The freeze happens in `with_mut` after
            // the off-lock query so a concurrent spawn cannot move it.
            let (session, handle) = state.with(|s| {
                let session = s.attached.get(&client_id)?.session;
                if let Some(root) = s.session_root(session) {
                    // Already frozen — return it without a live query.
                    return Some((session, FrozenOrQuery::Frozen(path_to_string(root)?)));
                }
                let seed = s.seed_pane_of_session(session)?;
                let handle = s.terminal_handle(seed).cloned()?;
                Some((session, FrozenOrQuery::Query(handle)))
            })?;
            match handle {
                FrozenOrQuery::Frozen(root) => Some(root),
                FrozenOrQuery::Query(handle) => {
                    let resolved = query_pane_cwd(handle).await?;
                    // Freeze the first observed root; reuse any value a
                    // racing spawn already inserted.
                    let frozen = state.with_mut(|s| {
                        path_to_string(
                            s.record_session_root(session, std::path::PathBuf::from(&resolved)),
                        )
                    });
                    frozen.or(Some(resolved))
                }
            }
        }
        phux_config::CwdInheritance::LastCwdPerWindow => {
            // Resolve the active window and its active pane's handle. If the
            // window has no live active pane, fall back to the last value we
            // recorded for that window.
            let (window, handle) = state.with(|s| {
                let session = s.attached.get(&client_id)?.session;
                let window = s.active_window_of_session(session)?;
                let handle = s
                    .active_pane_of_session(session)
                    .and_then(|p| s.terminal_handle(p).cloned());
                Some((window, handle))
            })?;
            let resolved = match handle {
                Some(handle) => query_pane_cwd(handle).await,
                None => None,
            };
            if let Some(cwd) = resolved {
                // Record the freshly observed CWD and seed the new pane with
                // it.
                state.with_mut(|s| {
                    s.record_window_last_cwd(window, std::path::PathBuf::from(&cwd));
                });
                return Some(cwd);
            }
            // Live query unavailable — reuse the most recent recorded value
            // for this window, if any.
            state.with(|s| s.window_last_cwd(window).and_then(|p| path_to_string(p)))
        }
    }
}

/// Either a directory already frozen as a session root or the actor handle
/// to query for it. Lets `resolve_inherited_cwd` decide whether a live PTY
/// query is needed inside a single `with` critical section without holding
/// the lock across the `await`.
enum FrozenOrQuery {
    Frozen(String),
    Query(crate::terminal_actor::TerminalHandle),
}

/// Render `path` as a UTF-8 string, or `None` if it is not valid UTF-8 — the
/// wire `cwd` and `CommandBuilder.cwd` plumbing are string-based, so a
/// non-UTF-8 directory simply yields no override.
fn path_to_string(path: &std::path::Path) -> Option<String> {
    path.to_str().map(ToOwned::to_owned)
}

/// Ask `handle`'s actor for its live PTY child CWD (a kernel query, see
/// [`crate::cwd_query`]). `None` when the actor has gone away or the query
/// is unsupported/denied. The handle must be cloned out of state before the
/// call: `with` must not be held across the `await`.
async fn query_pane_cwd(handle: crate::terminal_actor::TerminalHandle) -> Option<String> {
    let (reply, rx) = tokio::sync::oneshot::channel();
    handle.pwd.send(PwdRequest { reply }).await.ok()?;
    rx.await.ok().flatten()
}

/// Handle `SPAWN_TERMINAL` (phux-4li.11, SPEC §7.2 / §10.1).
///
/// v0.1 servers expose a single default Collection at
/// [`crate::state::DEFAULT_COLLECTION_ID`] (= `CollectionId(1)`). Any
/// other id is rejected with [`SpawnError::CollectionNotFound`] inside
/// the [`SpawnResult::Err`] arm of the reply frame — separate from
/// the catch-all `Error` channel so command-correlated failures stay
/// typed end-to-end (the same precedent the metadata reply path uses).
///
/// On success the spawn reuses the same PTY primitive
/// [`seed_session_with_pty`] that
/// [`resolve_create_if_missing`] threads through. We always go PTY-
/// backed: a `SPAWN_TERMINAL` with no PTY would be functionally
/// indistinguishable from "nothing happened," and the wire frame
/// commits to a runnable Terminal (the `command = None` ↔ "use the
/// server's default shell" contract from
/// `FrameKind::SpawnTerminal`'s doc).
///
/// `command`/`cwd`/`env` from the wire frame populate the
/// `portable_pty::CommandBuilder`:
///   * `command = None`  → fall back to
///     [`crate::terminal_actor::default_shell_command`] (same as
///     `AttachTarget::CreateIfMissing.command = None`).
///   * `cwd = Some(p)`    → `builder.cwd(p)`.
///   * `env = Some(v)`    → each `(k, v)` set via `builder.env(k, v)`,
///     additive over the parent environment. `env = Some(vec![])` is
///     distinct from `None` per the wire schema but has no observable
///     effect on the resulting child today (we don't `env_clear`).
///
/// The spawning client is auto-subscribed to the new pane and gets an
/// output-pump task fanning the actor's broadcast into its outbound
/// mailbox — the same machinery `handle_attach` uses for the session's
/// initial panes. Without that, an `INPUT_KEY` to the freshly-spawned
/// id would be rejected at [`handle_terminal_input`]'s subscription
/// gate and the user would see nothing.
///
/// The pane joins the spawning client's CURRENT session's window
/// (phux-i9zl): a TUI split keeps the session intact so `phux ls` shows one
/// session and a reattach resolves every split pane. The session is
/// resolved from the client's attachment; a `SPAWN_TERMINAL` from a
/// non-attached client is refused (no session to host the pane).
#[allow(
    clippy::too_many_arguments,
    reason = "1:1 with the SPAWN_TERMINAL wire frame (request_id + collection + command + cwd + env) plus the standard SharedState/client_id/out_tx/root_token threading the rest of this file uses"
)]
#[allow(
    clippy::too_many_lines,
    reason = "linear orchestration: validate collection → build CommandBuilder from wire frame → resolve spawning client's session → spawn PTY-backed pane into its window → auto-subscribe spawning client + spawn output pump → reply on the wire. Each step is small; splitting them scatters the SPAWN_TERMINAL contract without simplifying the logic."
)]
async fn handle_spawn_terminal(
    state: &SharedState,
    client_id: ClientId,
    request_id: u32,
    collection: CollectionId,
    command: Option<Vec<String>>,
    cwd: Option<String>,
    env: Option<Vec<(String, String)>>,
    out_tx: &tokio::sync::mpsc::Sender<Outbound>,
    root_token: &CancellationToken,
) {
    debug!(
        ?client_id,
        request_id,
        collection = ?collection,
        command = ?command,
        cwd = ?cwd,
        env_count = env.as_ref().map_or(0, Vec::len),
        "SPAWN_TERMINAL",
    );

    if collection != crate::state::DEFAULT_COLLECTION_ID {
        let _ = out_tx
            .send(Outbound::Frame(FrameKind::TerminalSpawned {
                request_id,
                result: SpawnResult::Err(SpawnError::CollectionNotFound),
            }))
            .await;
        return;
    }

    // Build the `CommandBuilder` from the wire frame. `command = None`
    // mirrors `AttachTarget::CreateIfMissing.command = None`: fall back
    // to the user's default shell (or `/bin/sh`).
    let mut builder = match command {
        Some(argv) if !argv.is_empty() => {
            let mut head = argv.into_iter();
            let program = head.next().unwrap_or_default();
            let mut b = portable_pty::CommandBuilder::new(program);
            for arg in head {
                b.arg(arg);
            }
            b
        }
        _ => crate::terminal_actor::default_shell_command(),
    };
    // TERM precedence (phux-ign): the server-wide `defaults.term` is the
    // baseline for every spawn (overriding `default_shell_command`'s
    // compiled-in default so explicit-command spawns don't silently
    // degrade); a per-spawn `SPAWN_TERMINAL.env` entry for `TERM` then
    // wins, because the wire `env` loop below runs last and
    // `CommandBuilder::env` overwrites. So the order is:
    //   1. compiled-in DEFAULT_TERM (from `default_shell_command`)
    //   2. server `defaults.term` (here)
    //   3. wire `env` (below) — authoritative for the Terminal it creates.
    let term = state.with(|s| s.term().to_owned());
    crate::terminal_actor::apply_term(&mut builder, &term);
    // Working directory precedence (phux-cs6): an explicit wire `cwd`
    // always wins; otherwise fall back to `defaults.cwd-inheritance`. The
    // inherit-focused policy reads the spawning client's focused pane's
    // live PTY CWD via a kernel query, so `C-a |` from a pane cd'd to
    // /tmp opens the new pane in /tmp.
    if let Some(path) = cwd {
        builder.cwd(path);
    } else if let Some(path) = resolve_inherited_cwd(state, client_id).await {
        builder.cwd(path);
    }
    if let Some(pairs) = env {
        for (k, v) in pairs {
            builder.env(k, v);
        }
    }

    // phux-i9zl: a split spawns into the spawning client's CURRENT session's
    // window, not a fresh `spawn-N` wrapper session. Resolve that session
    // from the client's attachment (the same `s.attached` lookup the cwd
    // inheritance above uses). A `SPAWN_TERMINAL` from a non-attached client
    // has no session to host the pane — reject it rather than orphan a PTY.
    let Some(session) = state.with(|s| s.attached.get(&client_id).map(|c| c.session)) else {
        let _ = out_tx
            .send(Outbound::Frame(FrameKind::TerminalSpawned {
                request_id,
                result: SpawnResult::Err(SpawnError::SpawnFailed(
                    "spawning client is not attached to a session".to_owned(),
                )),
            }))
            .await;
        return;
    };

    let history_limit = state.with(crate::state::ServerState::history_limit);
    let core_terminal_id =
        match spawn_pane_with_pty(state, session, builder, history_limit, root_token) {
            Ok(Some(id)) => id,
            Ok(None) => {
                warn!(
                    ?client_id,
                    request_id,
                    ?session,
                    "SPAWN_TERMINAL: attached session has no window to host the pane",
                );
                let _ = out_tx
                    .send(Outbound::Frame(FrameKind::TerminalSpawned {
                        request_id,
                        result: SpawnResult::Err(SpawnError::SpawnFailed(
                            "attached session has no window to host the pane".to_owned(),
                        )),
                    }))
                    .await;
                return;
            }
            Err(err) => {
                warn!(
                    ?client_id,
                    request_id,
                    error = %err,
                    "SPAWN_TERMINAL: failed to spawn pane actor",
                );
                let _ = out_tx
                    .send(Outbound::Frame(FrameKind::TerminalSpawned {
                        request_id,
                        result: SpawnResult::Err(SpawnError::SpawnFailed(format!("{err}"))),
                    }))
                    .await;
                return;
            }
        };

    // Auto-subscribe the spawning client to the new pane and snapshot
    // its `TerminalHandle` so we can spawn an output pump. Without
    // subscription the `INPUT_*` dispatch path's
    // `subscribers_for_terminal(...).contains(&client_id)` gate would
    // reject every keystroke the spawning client sends to the new id.
    //
    // The subscribe-and-handle lookup happens in a single `with_mut`
    // critical section so the wire-id allocation and the subscriber
    // append observe the same registry state.
    let wire_and_handle: Option<(
        phux_protocol::ids::TerminalId,
        crate::terminal_actor::TerminalHandle,
        ClientCapabilities,
    )> = state.with_mut(|s| {
        let wire_terminal_id = s.intern_terminal_wire(core_terminal_id);
        let client_caps = s
            .attached
            .get(&client_id)
            .map(|c| c.client_caps)
            .unwrap_or_default();
        // Only auto-subscribe if the client is currently attached —
        // a bare `SPAWN_TERMINAL` from a non-attached client is legal
        // wire-wise (the frame doesn't require ATTACH first) but the
        // subscription would have no `attached` slot to live in.
        if s.attached.contains_key(&client_id) {
            let subs = s.terminal_subscribers.entry(core_terminal_id).or_default();
            if !subs.contains(&client_id) {
                subs.push(client_id);
            }
        }
        s.terminal_handle(core_terminal_id)
            .cloned()
            .map(|h| (wire_terminal_id, h, client_caps))
    });

    if let Some((wire_terminal_id, handle, client_caps)) = wire_and_handle {
        // Spawn the output pump BEFORE replying with `TerminalSpawned`
        // so any bytes the freshly-spawned PTY emits in the gap between
        // exec and the client's first read are queued on the broadcast
        // channel (broadcasts buffer per subscriber). Mirrors the
        // subscribe-before-snapshot ordering in `handle_attach`.
        let mut output_rx = handle.output.subscribe();
        let pump_out_tx = out_tx.clone();
        let pump_wire_terminal_id = wire_terminal_id.clone();
        tokio::task::spawn_local(async move {
            let mut seq: u64 = 0;
            loop {
                match output_rx.recv().await {
                    Ok(bytes) => {
                        seq = seq.wrapping_add(1);
                        let bytes = crate::downsample::rewrite_bytes_with_caps(&bytes, client_caps);
                        if pump_out_tx
                            .send(Outbound::Frame(FrameKind::TerminalOutput {
                                terminal_id: pump_wire_terminal_id.clone(),
                                seq,
                                bytes,
                            }))
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        warn!(
                            terminal_id = ?pump_wire_terminal_id,
                            dropped = n,
                            "SPAWN_TERMINAL output pump lagged; consider larger broadcast capacity",
                        );
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        });

        let _ = out_tx
            .send(Outbound::Frame(FrameKind::TerminalSpawned {
                request_id,
                result: SpawnResult::Ok(wire_terminal_id.clone()),
            }))
            .await;
        // phux-y2t: fan a `pane_spawned` agent event to event-stream
        // subscribers (SPEC §7.5). The new pane's wire id rides the
        // `EVENT` envelope; server-wide subscribers and any per-pane
        // subscribers for this id receive it.
        broadcast_event(state, Some(&wire_terminal_id), &AgentEvent::PaneSpawned);
    } else {
        // Defensive: seed_session_with_pty succeeded but the handle
        // somehow vanished before we could clone it. Treat as a spawn
        // failure on the wire so the client doesn't hang on a reply
        // that will never arrive.
        warn!(
            ?client_id,
            request_id,
            ?core_terminal_id,
            "SPAWN_TERMINAL: spawn succeeded but TerminalHandle vanished",
        );
        let _ = out_tx
            .send(Outbound::Frame(FrameKind::TerminalSpawned {
                request_id,
                result: SpawnResult::Err(SpawnError::SpawnFailed(
                    "internal state inconsistency: handle missing after spawn".to_owned(),
                )),
            }))
            .await;
    }
}

/// Handle `TERMINAL_RESIZE` (phux-4li.11, SPEC §7.2 / §10.2).
///
/// Look up the target Terminal by its wire id, then `try_send` the new
/// `(cols, rows)` into the actor's resize mailbox. The actor's existing
/// `handle_resize` (built for `VIEWPORT_RESIZE` in phux-byc.5) drives
/// both `libghostty_vt::Terminal::resize` and the PTY
/// `ioctl(TIOCSWINSZ)` from one place — we reuse it verbatim so the
/// per-Terminal resize and the per-Viewport resize stay in lockstep.
///
/// Silent on every "not found" path per the wire frame's
/// no-reply-by-design contract. The frame label distinguishes this
/// path from `VIEWPORT_RESIZE` in logs.
///
/// `client_id` is unused today (the wire frame is unauthenticated;
/// SATELLITE-routed ids are rejected before we get here). It's wired
/// through anyway so future per-client validation (e.g. checking that
/// the client is subscribed to the pane) doesn't require widening the
/// helper signature.
fn handle_terminal_resize(
    state: &SharedState,
    client_id: ClientId,
    wire_terminal_id: &phux_protocol::ids::TerminalId,
    cols: u16,
    rows: u16,
) {
    if !wire_terminal_id.is_local() {
        warn!(
            ?client_id,
            ?wire_terminal_id,
            cols,
            rows,
            "TERMINAL_RESIZE: SATELLITE-routed pane id rejected on non-federation-hub server",
        );
        return;
    }
    state.with_mut(|s| {
        let Some(terminal) = s.terminal_from_wire(wire_terminal_id) else {
            debug!(
                ?client_id,
                ?wire_terminal_id,
                cols,
                rows,
                "TERMINAL_RESIZE: unknown pane; dropping (no-reply per wire frame design)",
            );
            return;
        };
        // Keep the registry's recorded dims in sync so future
        // `TERMINAL_SNAPSHOT` payloads report the post-resize cols/rows.
        // Mirrors what `handle_viewport_resize` does for VIEWPORT_RESIZE.
        if let Some(pane) = s.registry.terminal_mut(terminal) {
            pane.dims = (cols, rows);
        }
        let Some(handle) = s.terminals.get(&terminal) else {
            debug!(
                ?client_id,
                ?terminal,
                cols,
                rows,
                "TERMINAL_RESIZE: no TerminalHandle registered for pane; dropping",
            );
            return;
        };
        // Live per-pane resize (TERMINAL_RESIZE): resync clients so their
        // mirrors reconverge after reflow (phux-8v1).
        match handle.resize.try_send(ResizeRequest {
            cols,
            rows,
            resync_clients: true,
        }) {
            Ok(()) => {}
            Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                warn!(
                    ?client_id,
                    ?terminal,
                    cols,
                    rows,
                    "TERMINAL_RESIZE: pane resize mailbox full; dropping",
                );
            }
            Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                debug!(
                    ?client_id,
                    ?terminal,
                    "TERMINAL_RESIZE: pane actor gone; dropping resize",
                );
            }
        }
    });
}

/// Perform the attach mutation in one critical section: call
/// [`crate::state::ServerState::attach`], build the snapshot, collect
/// the per-pane handles + wire ids to snapshot.
///
/// Pulled out so [`handle_attach`] stays under clippy's
/// `too_many_lines` ceiling.
fn prepare_attach(
    state: &SharedState,
    client_id: ClientId,
    session_name: &str,
    out_tx: &tokio::sync::mpsc::Sender<Outbound>,
    client_caps: ClientCapabilities,
) -> Result<AttachPrepared, crate::state::AttachError> {
    state.with_mut(|s| {
        let sid = s.attach(client_id, session_name, out_tx.clone(), client_caps)?;
        // Record successful attach as session activity before we build
        // the snapshot. The order doesn't matter for
        // correctness (we're still inside the with_mut critical
        // section), but doing it here keeps the recording adjacent to
        // the attach call that justified it — easier to reason about
        // when reading the code.
        s.touch_session(sid);
        let snapshot = s
            .build_session_snapshot(sid)
            .ok_or_else(|| crate::state::AttachError::UnknownSession(session_name.to_owned()))?;
        let panes_to_snapshot = s.attach_snapshot_panes(sid);
        let initial_client_id =
            phux_protocol::ids::ClientId::new(u32::try_from(client_id.0).unwrap_or(u32::MAX));
        Ok((snapshot, initial_client_id, panes_to_snapshot))
    })
}

// -----------------------------------------------------------------------------
// Control-plane command dispatch — SPEC §5 (phux-k61 / ADR-0021).
// -----------------------------------------------------------------------------

/// Dispatch a `COMMAND` envelope and reply with `COMMAND_RESULT`
/// correlated by `request_id`. The control plane for the CLI's `ls` /
/// `kill` verbs. Per SPEC §5 a command is asynchronous: the result MAY
/// follow other frames the command triggered (e.g. `KILL_TERMINAL`'s
/// `TERMINAL_CLOSED`).
/// Stable, payload-free label for a [`Command`] variant — the `kind` field
/// on the `handle_command` lifecycle span. A hand-written map (rather than
/// `?command`) keeps the trace line small and free of user payloads
/// (session names, env, input bytes) while still localizing which control
/// command ran. `Command` is `#[non_exhaustive]`, hence the wildcard; a new
/// variant logs as `"other"` until an arm is added here.
const fn command_kind(command: &Command) -> &'static str {
    match command {
        Command::KillTerminal { .. } => "kill_terminal",
        Command::GetState { .. } => "get_state",
        Command::GetScreen { .. } => "get_screen",
        Command::RouteInput { .. } => "route_input",
        Command::CreateSession { .. } => "create_session",
        Command::KillCollection { .. } => "kill_collection",
        Command::RenameSession { .. } => "rename_session",
        _ => "other",
    }
}

// Lifecycle span (info): one per L2 COMMAND. `kind` is a payload-free
// label so the trace localizes which control command ran without leaking
// session names / env / input bytes; the CLOSE duration times the command
// (some, e.g. GET_SCREEN, round-trip to an actor).
#[tracing::instrument(
    level = "info",
    name = "handle_command",
    skip_all,
    fields(?client_id, request_id, kind = command_kind(&command)),
)]
async fn handle_command(
    state: &SharedState,
    client_id: ClientId,
    request_id: u32,
    command: Command,
    out_tx: &tokio::sync::mpsc::Sender<Outbound>,
    root_token: &CancellationToken,
) {
    let result = match command {
        Command::GetState { scope } => handle_get_state(state, &scope),
        Command::GetScreen {
            terminal_id,
            request_scrollback,
            cells,
        } => handle_get_screen(state, &terminal_id, request_scrollback, cells).await,
        Command::RouteInput { terminal_id, event } => {
            handle_route_input(state, client_id, &terminal_id, event)
        }
        Command::CreateSession {
            collection,
            name,
            command,
            cwd,
        } => handle_create_session(
            state,
            collection,
            &name,
            command,
            cwd.as_deref(),
            root_token,
        ),
        Command::KillCollection { collection, name } => {
            handle_kill_collection(state, collection, &name)
        }
        Command::RenameSession {
            collection,
            name,
            new_name,
        } => handle_rename_session(state, collection, &name, &new_name),
        Command::KillTerminal { terminal_id } => {
            // Resolve the wire id to the core pane, then cancel its actor.
            // Cancellation drops the actor's `exit_notify`, which the
            // per-pane EOF watcher (phux-it8) treats identically to PTY
            // EOF: it broadcasts `TERMINAL_CLOSED` and reaps the pane
            // (phux-60s), cascading to session removal + server self-exit
            // when the last session empties. So KILL_TERMINAL reuses the
            // exact teardown a natural shell exit takes — no separate
            // kill plumbing, and the async `TERMINAL_CLOSED` still fires.
            state
                .with(|s| s.terminal_from_wire(&terminal_id))
                .map_or_else(
                    || CommandResult::Error {
                        code: ErrorCode::TerminalNotFound,
                        message: format!("no such terminal: {terminal_id:?}"),
                    },
                    |core_id| {
                        state.with_mut(|s| s.detach_terminal_actor(core_id));
                        CommandResult::Ok
                    },
                )
        }
        Command::GetTerminalState {
            terminal_id,
            include_scrollback,
            max_scrollback_lines,
        } => {
            handle_get_terminal_state(
                state,
                &terminal_id,
                include_scrollback,
                max_scrollback_lines,
            )
            .await
        }
        Command::SubscribeTerminalEvents {
            terminal_id,
            event_types,
        } => handle_subscribe_terminal_events(state, client_id, &terminal_id, event_types, out_tx),
        // `Command` is `#[non_exhaustive]`: a forward-compat command this
        // server doesn't implement decodes only if a newer peer sent a
        // tag we allocated but haven't wired (the decoder rejects truly
        // unknown tags). Refuse it per SPEC §5 with `INVALID_COMMAND`.
        _ => CommandResult::Error {
            code: ErrorCode::InvalidCommand,
            message: "command not supported by this server".to_owned(),
        },
    };
    debug!(
        ?client_id,
        request_id, "COMMAND dispatched; sending COMMAND_RESULT"
    );
    let _ = out_tx
        .send(Outbound::Frame(FrameKind::CommandResult {
            request_id,
            result,
        }))
        .await;
}

/// Build the `OK_WITH(TerminalId(..))` reply for `CREATE_SESSION`
/// (`phux-fdh`, ADR-0021 §3).
///
/// Creates a named session under `collection` and seeds its pane *without*
/// attaching, subscribing, or resizing — the create-only counterpart to the
/// always-attaching `ATTACH { CreateIfMissing }`. The existence check and
/// the seed both run inside this `handle_client`-driven task on the
/// single-threaded runtime, so the lookup→create sequence is atomic with
/// respect to other clients: two racing `CREATE_SESSION { name }` callers
/// cannot both succeed (the second sees the first's session and is rejected),
/// which is the TOCTOU fix the client-side `GET_STATE`→`ATTACH` always-new
/// path could not offer.
///
/// A name already in use is rejected with `INVALID_COMMAND` (create-only,
/// never create-or-attach). An unknown `collection` is rejected likewise;
/// v0.1 servers host only the default [`DEFAULT_COLLECTION_ID`].
///
/// The reply carries the seed pane's wire [`TerminalId`] so the caller
/// (`phux new --json`) can print it without attaching.
fn handle_create_session(
    state: &SharedState,
    collection: CollectionId,
    name: &str,
    command: Option<Vec<String>>,
    cwd: Option<&str>,
    root_token: &CancellationToken,
) -> CommandResult {
    if collection != crate::state::DEFAULT_COLLECTION_ID {
        return CommandResult::Error {
            code: ErrorCode::InvalidCommand,
            message: format!("unknown collection: {collection:?}"),
        };
    }
    if state.with(|s| s.session_by_name(name).is_some()) {
        return CommandResult::Error {
            code: ErrorCode::InvalidCommand,
            message: format!("session {name:?} already exists"),
        };
    }

    let (with_pty, override_cmd, history_limit, term) = state.with(|s| {
        (
            s.attach_create_seeds_pty(),
            s.attach_create_seed_command(),
            s.history_limit(),
            s.term().to_owned(),
        )
    });

    let seed_result = if with_pty {
        // Command precedence mirrors `resolve_create_if_missing`: an explicit
        // server-wide override (set by tests for a deterministic child) wins,
        // then the wire `command`, then the default shell.
        let mut seed_cmd = override_cmd.unwrap_or_else(|| match command {
            Some(argv) if !argv.is_empty() => {
                let mut head = argv.into_iter();
                let program = head.next().unwrap_or_default();
                let mut builder = portable_pty::CommandBuilder::new(program);
                for arg in head {
                    builder.arg(arg);
                }
                if let Some(path) = cwd {
                    builder.cwd(path);
                }
                builder
            }
            _ => {
                let mut builder = crate::terminal_actor::default_shell_command();
                if let Some(path) = cwd {
                    builder.cwd(path);
                }
                builder
            }
        });
        // Apply the server-wide `defaults.term` (phux-ign).
        crate::terminal_actor::apply_term(&mut seed_cmd, &term);
        seed_session_with_pty(state, name, seed_cmd, history_limit, root_token)
    } else {
        // No-PTY path: the wire `command`/`cwd` are meaningless without a
        // child to exec, but the session+pane still need to exist so the
        // reply can carry a real seed-pane id.
        seed_session_with_actor(state, name, history_limit, root_token)
    };

    match seed_result {
        Ok(core_terminal) => {
            let wire = state.with_mut(|s| s.intern_terminal_wire(core_terminal));
            CommandResult::OkWith(CommandValue::TerminalId(wire))
        }
        Err(err) => {
            warn!(
                session = %name,
                error = %err,
                "CREATE_SESSION: failed to seed pane for new session",
            );
            CommandResult::Error {
                code: ErrorCode::ResourceExhausted,
                message: format!("failed to create session {name:?}: {err}"),
            }
        }
    }
}

/// Build the `Ok` reply for `KILL_COLLECTION` — the teardown counterpart to
/// `CREATE_SESSION` (`phux-h9s`, ADR-0021 §3).
///
/// Destroys the session named `name` under `collection` by cancelling every
/// pane actor it owns, in one round-trip. Each cancellation drops the
/// actor's `exit_notify`, which the per-pane EOF watcher (phux-it8) treats
/// like PTY EOF: it broadcasts `TERMINAL_CLOSED` and reaps the pane
/// (phux-60s), cascading to session removal and — when the last session
/// empties — server self-exit. So this reuses the exact teardown a per-pane
/// `KILL_TERMINAL` (or a natural shell exit) takes, but resolves the whole
/// session's panes in one pass rather than over N client round-trips.
///
/// The reply is `Ok` the moment the actors are cancelled; the
/// `TERMINAL_CLOSED` frames follow asynchronously as the panes reap (SPEC
/// §5). An unknown `collection` or an unknown `name` is rejected with
/// `INVALID_COMMAND` — symmetric with `CREATE_SESSION`'s refusals.
///
/// Detach is idempotent (cancelling an already-cancelled token is a no-op),
/// so a pane that exits concurrently with this teardown carries no
/// double-close risk.
fn handle_kill_collection(
    state: &SharedState,
    collection: CollectionId,
    name: &str,
) -> CommandResult {
    if collection != crate::state::DEFAULT_COLLECTION_ID {
        return CommandResult::Error {
            code: ErrorCode::InvalidCommand,
            message: format!("unknown collection: {collection:?}"),
        };
    }

    // Resolve the session to the core pane ids it owns, under a single
    // `with` borrow. `None` means the name is unknown — refuse it rather
    // than silently ack a no-op teardown.
    let Some(panes) = state.with(|s| {
        let session = s.session_by_name(name)?;
        let panes: Vec<phux_core::ids::TerminalId> = session
            .windows
            .iter()
            .filter_map(|wid| s.registry.window(*wid))
            .flat_map(|w| w.panes.iter().copied())
            .collect();
        Some(panes)
    }) else {
        return CommandResult::Error {
            code: ErrorCode::SessionNotFound,
            message: format!("no such session: {name:?}"),
        };
    };

    state.with_mut(|s| {
        for pane in panes {
            s.detach_terminal_actor(pane);
        }
    });
    CommandResult::Ok
}

/// Build the reply for `RENAME_SESSION` — the rename counterpart to
/// `CREATE_SESSION` (ADR-0021 §3).
///
/// Resolves the session named `name` under `collection` (the same registry
/// scan `KILL_COLLECTION` uses for name resolution) and reassigns its
/// human-readable name to `new_name` in one pass. The rename is a single
/// field write on the registry's `Session`; there is no name-keyed side
/// index to update — every lookup scans the registry directly
/// (`ServerState::find_session_by_name`).
///
/// An unknown `collection` or `new_name` already in use is refused with
/// `INVALID_COMMAND` (symmetric with `CREATE_SESSION`); an unknown `name`
/// with `SESSION_NOT_FOUND` (symmetric with `KILL_COLLECTION`). On success
/// the reply is `Ok` — the server is authoritative, and each attached
/// client reconciles the new name on its next `ATTACHED` snapshot (a live
/// `SESSION_RENAMED` push to other clients is out of scope for this pass).
fn handle_rename_session(
    state: &SharedState,
    collection: CollectionId,
    name: &str,
    new_name: &str,
) -> CommandResult {
    if collection != crate::state::DEFAULT_COLLECTION_ID {
        return CommandResult::Error {
            code: ErrorCode::InvalidCommand,
            message: format!("unknown collection: {collection:?}"),
        };
    }

    match state.with_mut(|s| s.rename_session(name, new_name)) {
        crate::state::RenameOutcome::Renamed => CommandResult::Ok,
        crate::state::RenameOutcome::NotFound => CommandResult::Error {
            code: ErrorCode::SessionNotFound,
            message: format!("no such session: {name:?}"),
        },
        crate::state::RenameOutcome::NameTaken => CommandResult::Error {
            code: ErrorCode::InvalidCommand,
            message: format!("session {new_name:?} already exists"),
        },
    }
}

/// Build the `OK_WITH(STATE(..))` reply for `GET_STATE`.
///
/// v0.1 supports only [`StateScope::Server`] (the whole-server snapshot).
/// The snapshot reuses the `ATTACHED` [`SessionSnapshot`] shape; `phux ls`
/// and client-side selector resolution read its `sessions` list and ignore
/// the focused-* fields. An empty server yields an empty session list with
/// sentinel focus ids (the wire requires the focus fields to be present).
fn handle_get_state(state: &SharedState, scope: &StateScope) -> CommandResult {
    match scope {
        StateScope::Server => {
            let snapshot = state.with_mut(|s| {
                let focus = s
                    .most_recently_touched_session()
                    .or_else(|| s.registry.sessions().next().map(|(id, _)| id));
                focus.and_then(|sid| s.build_session_snapshot(sid))
            });
            CommandResult::OkWith(CommandValue::State(
                snapshot.unwrap_or_else(empty_session_snapshot),
            ))
        }
        // `StateScope` is `#[non_exhaustive]`; a narrower scope a newer
        // peer requests is not yet supported.
        _ => CommandResult::Error {
            code: ErrorCode::InvalidCommand,
            message: "unsupported GET_STATE scope".to_owned(),
        },
    }
}

/// Build the `OK_WITH(JSON(..))` reply for `GET_SCREEN`.
///
/// Resolves the wire id to its pane actor, then asks the actor to project
/// its own `Terminal` grid into a [`phux_core::screen::ScreenState`]
/// serialized as JSON — the stable agent-surface contract (ADR-0022 §2).
/// This is side-effect-free: it neither attaches nor resizes, so polling
/// it (the `phux wait`/`run` floor) never disturbs the live pane.
async fn handle_get_screen(
    state: &SharedState,
    terminal_id: &phux_protocol::ids::TerminalId,
    request_scrollback: Option<u32>,
    cells: bool,
) -> CommandResult {
    // Clone the (Send) handle out of the lock; the actor reply is awaited
    // outside the critical section.
    let handle = state.with(|s| {
        s.terminal_from_wire(terminal_id)
            .and_then(|core| s.terminal_handle(core).cloned())
    });
    let Some(handle) = handle else {
        return CommandResult::Error {
            code: ErrorCode::TerminalNotFound,
            message: format!("no such terminal: {terminal_id:?}"),
        };
    };
    let pane = terminal_id.local_id().unwrap_or(0);
    let (reply_tx, reply_rx) = oneshot::channel();
    if handle
        .screen
        .send(ScreenRequest {
            pane,
            scrollback: request_scrollback,
            cells,
            reply: reply_tx,
        })
        .await
        .is_err()
    {
        return CommandResult::Error {
            code: ErrorCode::InternalError,
            message: "pane actor unavailable for GET_SCREEN".to_owned(),
        };
    }
    reply_rx.await.map_or_else(
        |_| CommandResult::Error {
            code: ErrorCode::InternalError,
            message: "pane actor dropped the GET_SCREEN reply".to_owned(),
        },
        |screen| {
            serde_json::to_string(&screen).map_or_else(
                |err| CommandResult::Error {
                    code: ErrorCode::InternalError,
                    message: format!("screen serialization failed: {err}"),
                },
                |json| CommandResult::OkWith(CommandValue::Json(json)),
            )
        },
    )
}

/// Build the `Ok_With(Json(TerminalState))` reply for `GET_TERMINAL_STATE`.
///
/// L2 Collection-aware counterpart to [`handle_get_screen`]: returns a
/// comprehensive snapshot of terminal state (grid, scrollback, cursor, shell
/// metadata, sequence number, and timestamp) in a structured JSON format.
/// Backs agent polling and state inspection without requiring an attach or
/// subscription (ADR-0022, ADR-0015 L2).
///
/// Unlike `GET_SCREEN` which returns raw `ScreenState` with only grid
/// dimensions and viewport text, `GET_TERMINAL_STATE` returns structured
/// JSON with:
/// - Grid cells with text and styling
/// - Cursor position and visibility
/// - Optional scrollback history (if `include_scrollback` is true)
/// - Shell process metadata (PID, name, jobs, copy-mode state)
/// - Pending command tracking (overlay layer)
/// - Logical sequence number (for change detection)
/// - Timestamp (for agent polling)
///
/// Handler flow:
/// 1. Resolve `terminal_id` to a `TerminalActor` handle (reuse same pattern as
///    `handle_get_screen`)
/// 2. Query screen state via `ScreenRequest` (reuse existing path)
/// 3. Walk grid cells: parse `ScreenState.lines` and merge styling from
///    `ScreenState.cells` (`CellInfo`)
/// 4. Extract cursor, scrollback, and dimensions
/// 5. Query shell state (gracefully degrade to None if unavailable)
/// 6. Build JSON and encode as JSON
/// 7. Return as `COMMAND_RESULT Ok_With(Json(TerminalState))`
///
/// Error cases:
/// - Unknown `terminal_id` → `TERMINAL_NOT_FOUND`
/// - Actor unavailable → `INTERNAL_ERROR`
/// - Shell query fails → populate `shell_state: None`, continue gracefully
#[allow(clippy::too_many_lines)]
async fn handle_get_terminal_state(
    state: &SharedState,
    terminal_id: &phux_protocol::ids::TerminalId,
    include_scrollback: bool,
    max_scrollback_lines: u16,
) -> CommandResult {
    use std::time::{SystemTime, UNIX_EPOCH};

    // Step 1: Resolve terminal_id to TerminalActor handle (same pattern as
    // handle_get_screen).
    let handle = state.with(|s| {
        s.terminal_from_wire(terminal_id)
            .and_then(|core| s.terminal_handle(core).cloned())
    });

    let Some(handle) = handle else {
        return CommandResult::Error {
            code: ErrorCode::TerminalNotFound,
            message: format!("no such terminal: {terminal_id:?}"),
        };
    };

    let pane = terminal_id.local_id().unwrap_or(0);

    // Step 2: Query screen state via ScreenRequest (reuse existing path).
    // This gives us canonical grid snapshot, scrollback (if requested), and
    // cell styling information.
    let (reply_tx, reply_rx) = oneshot::channel();
    if handle
        .screen
        .send(ScreenRequest {
            pane,
            scrollback: if include_scrollback {
                Some(u32::from(max_scrollback_lines))
            } else {
                None
            },
            cells: true, // Always request cells for semantic info (styles, OSC-133 marks)
            reply: reply_tx,
        })
        .await
        .is_err()
    {
        return CommandResult::Error {
            code: ErrorCode::InternalError,
            message: "pane actor unavailable for GET_TERMINAL_STATE".to_owned(),
        };
    }

    let Ok(screen_state) = reply_rx.await else {
        return CommandResult::Error {
            code: ErrorCode::InternalError,
            message: "pane actor dropped the GET_TERMINAL_STATE reply".to_owned(),
        };
    };

    // Step 3: Convert ScreenState viewport to JSON cells array.
    // ScreenState carries:
    // - lines: Vec<String> — viewport text, one row per element, right-trimmed
    // - cells: Option<Vec<CellInfo>> — sparse: only cells with non-default
    //   style or OSC-133 semantic marks, in row-major order
    //
    // We parse each line into characters and emit cells as JSON objects.
    // Note: a full implementation using unicode-segmentation::Graphemes
    // would handle combining marks, emoji, and wide glyphs more precisely;
    // for now we estimate width based on ASCII vs. non-ASCII.

    let mut viewport_cells = Vec::new();

    // Emit viewport cells by parsing each line.
    // Each line is right-trimmed, so we don't need to emit trailing blanks.
    #[allow(clippy::cast_possible_truncation)]
    for (row_idx, line_text) in screen_state.lines.iter().enumerate() {
        let row = row_idx as u16;
        let mut col = 0u16;

        for ch in line_text.chars() {
            // Estimate cell width: ASCII is 1 column, everything else is 2
            // (emoji, CJK). libghostty tracks actual widths; we approximate.
            let width = if ch.is_ascii() { 1u16 } else { 2u16 };

            // Emit this cell as JSON.
            viewport_cells.push(serde_json::json!({
                "col": col,
                "row": row,
                "text": ch.to_string(),
                "width": width as u8,
                "selected": false,
            }));

            col += width;
            // Stop if we exceed grid width (shouldn't happen in right-trimmed lines)
            if col >= screen_state.cols {
                break;
            }
        }
    }

    // Extract cursor state as JSON.
    let cursor = screen_state.cursor.map(|cs| {
        serde_json::json!({
            "x": cs.x,
            "y": cs.y,
            "visible": cs.visible,
        })
    });

    // Step 4: Convert scrollback lines to JSON.
    let mut scrollback_lines = Vec::new();
    #[allow(clippy::cast_possible_truncation)]
    let scrollback_count_total = screen_state.scrollback.len() as u32;

    if include_scrollback {
        for line_text in &screen_state.scrollback {
            scrollback_lines.push(serde_json::json!({
                "text": line_text,
                "cells": [],
            }));
        }
    }

    // Step 5: Query shell state.
    // The TerminalActor could provide shell PID (child of PTY master),
    // shell name, job list, and in_copy_mode. For now, set to None;
    // a future iteration adds a GetShellStateRequest channel and wires
    // shell state queries (phux-y2t Phase 2).
    //
    // Graceful degrade: if the actor has no PTY (no-PTY test actor),
    // or the query fails, leave shell_state as None. Agents can work
    // with partial snapshots.
    let shell_state: Option<serde_json::Value> = None;

    // Step 6: Compute timestamp and sequence number.
    let timestamp_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());

    // Sequence number is a logical clock maintained per terminal for change
    // detection. For now, placeholder; should be sourced from actor's state
    // in a future iteration (phux-y2t Phase 2). See ADR-0015 for the versioning model.
    let seq = 0u64;

    // Step 7: Build the TerminalState as JSON.
    let terminal_state_json = serde_json::json!({
        "cols": screen_state.cols,
        "rows": screen_state.rows,
        "cells": viewport_cells,
        "cursor": cursor,
        "scrollback": scrollback_lines,
        "scrollback_count_total": scrollback_count_total,
        "shell_state": shell_state,
        "pending_command": serde_json::Value::Null,
        "timestamp_secs": timestamp_secs,
        "seq": seq,
    });

    // Step 8: Serialize to JSON string and return.
    match serde_json::to_string(&terminal_state_json) {
        Ok(json) => CommandResult::OkWith(CommandValue::Json(json)),
        Err(err) => CommandResult::Error {
            code: ErrorCode::InternalError,
            message: format!("terminal state serialization failed: {err}"),
        },
    }
}

/// Build the `Ok` reply for `ROUTE_INPUT`.
///
/// The write counterpart to [`handle_get_screen`]: it resolves the wire id
/// to its pane actor and feeds the already-built input event straight into
/// the pane's input mailbox — the same mailbox `handle_terminal_input`
/// targets, but with no attach / subscription gate and, crucially, no
/// resize. So unlike the ATTACH-then-`INPUT_KEY` path, routing input here
/// never transiently shrinks the pane to the caller's viewport; the live
/// dimensions are preserved (ADR-0022, `phux-3j3`).
///
/// `ROUTE_INPUT` is the side-effect-free agent path (ADR-0022): it
/// delivers input to a Terminal WITHOUT an attach or subscription, which is
/// exactly how `phux run` / `send-keys` drive a pane headlessly. It must
/// therefore NOT require the caller to be a subscriber. An earlier interim
/// gate (phux-nlo) approximated "PRIMARY" by subscription and rejected any
/// unsubscribed caller — but that is precisely the headless agent, so it
/// broke the agent surface; it is removed. v0.1 is single-trust-domain (one
/// server per user, ADR-0003), so there is no untrusted observer to fence
/// off here. Genuine viewer-vs-primary authority (SPEC `input.md` §7 /
/// `L1.md` §7.1) returns when per-connection roles are materialized, and
/// must gate an *attached read-only viewer*, never the headless
/// control-plane caller. `client_id` is kept for that future policy and for
/// the observability trace below.
///
/// `try_send` is non-blocking for the same single-threaded-runtime reason
/// as `handle_terminal_input`: input is fire-and-forget per SPEC §9, so a
/// full mailbox drops the event rather than blocking the read loop. The
/// command still acks `Ok` (the event was accepted for delivery); an
/// unknown Terminal or a gone actor produces an `Error`.
fn handle_route_input(
    state: &SharedState,
    client_id: ClientId,
    terminal_id: &phux_protocol::ids::TerminalId,
    event: InputEvent,
) -> CommandResult {
    // v0.1 non-federation-hub servers reject SATELLITE-routed input
    // (ADR-0016 / SPEC §10.1), matching `handle_terminal_input`.
    if !terminal_id.is_local() {
        return CommandResult::Error {
            code: ErrorCode::UnsupportedSatelliteRoute,
            message: format!("ROUTE_INPUT to satellite route unsupported: {terminal_id:?}"),
        };
    }
    // Resolve the wire id to its (Send) Terminal handle in one lock; we
    // never await inside the lock. No subscription/role gate: ROUTE_INPUT is
    // the headless agent path (see the doc comment) and must work without an
    // attach.
    let handle = state.with(|s| {
        let core = s.terminal_from_wire(terminal_id)?;
        s.terminal_handle(core).cloned()
    });
    let Some(handle) = handle else {
        return CommandResult::Error {
            code: ErrorCode::TerminalNotFound,
            message: format!("no such terminal: {terminal_id:?}"),
        };
    };
    debug!(?client_id, ?terminal_id, "ROUTE_INPUT delivering input");
    let input = match event {
        InputEvent::Key(event) => TerminalInput::Key(event),
        InputEvent::Mouse(event) => TerminalInput::Mouse(event),
        InputEvent::Focus(event) => TerminalInput::Focus(event),
        InputEvent::Paste(event) => TerminalInput::Paste(event),
        // `InputEvent` is `#[non_exhaustive]`; a future atom a newer peer
        // sends is not yet routable here.
        _ => {
            return CommandResult::Error {
                code: ErrorCode::InvalidCommand,
                message: "unsupported ROUTE_INPUT event".to_owned(),
            };
        }
    };
    match handle.input.try_send(input) {
        Ok(()) => CommandResult::Ok,
        Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
            warn!(
                ?terminal_id,
                "ROUTE_INPUT mailbox full; dropping (fire-and-forget per SPEC §9)"
            );
            CommandResult::Ok
        }
        Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => CommandResult::Error {
            code: ErrorCode::InternalError,
            message: "pane actor unavailable for ROUTE_INPUT".to_owned(),
        },
    }
}

/// Handle `SUBSCRIBE_TERMINAL_EVENTS` command.
///
/// Resolves the wire `terminal_id` to a pane actor and registers the caller
/// as an event subscriber. The server will broadcast semantic events
/// (`CommandStarted`, `CommandEnded`, `GridChanged`, etc.) as they occur, filtered
/// by `event_types` (empty = all types). The subscription persists until the
/// client detaches or the connection closes.
///
/// Replies `CommandResult::Ok` immediately; events flow asynchronously as
/// `Event` frames to the client's outbound mailbox. `try_send` semantics:
/// a full subscriber mailbox drops events (accelerator semantics, not
/// guaranteed delivery).
fn handle_subscribe_terminal_events(
    state: &SharedState,
    client_id: ClientId,
    terminal_id: &phux_protocol::ids::TerminalId,
    event_types: Vec<phux_protocol::wire::frame::TerminalEventType>,
    out_tx: &tokio::sync::mpsc::Sender<Outbound>,
) -> CommandResult {
    use crate::terminal_actor::{SubscribeToEventsRequest, TerminalEventSubscriber};

    // Resolve the wire id to its pane actor (same pattern as handle_route_input).
    let handle = state.with(|s| {
        let core = s.terminal_from_wire(terminal_id)?;
        s.terminal_handle(core).cloned()
    });

    let Some(handle) = handle else {
        return CommandResult::Error {
            code: ErrorCode::TerminalNotFound,
            message: format!("no such terminal: {terminal_id:?}"),
        };
    };

    debug!(
        ?client_id,
        ?terminal_id,
        "SUBSCRIBE_TERMINAL_EVENTS registering"
    );

    // Get the wire terminal id for use in Event frames.
    let wire_terminal_id = terminal_id.local_id().unwrap_or(0);

    // Build the subscriber request and send to the actor.
    // The subscriber receives the client's outbound mailbox directly,
    // so events are forwarded straight to the client without an intermediary.
    let req = SubscribeToEventsRequest {
        subscriber: TerminalEventSubscriber {
            outbound: out_tx.clone(),
            event_types,
        },
        wire_terminal_id,
    };

    if handle.subscribe_to_events.try_send(req).is_err() {
        return CommandResult::Error {
            code: ErrorCode::InternalError,
            message: "pane actor unavailable for SUBSCRIBE_TERMINAL_EVENTS".to_owned(),
        };
    }

    debug!(
        ?client_id,
        ?terminal_id,
        "SUBSCRIBE_TERMINAL_EVENTS: subscriber registered"
    );
    CommandResult::Ok
}

/// A `SessionSnapshot` describing a server with no sessions: empty lists,
/// sentinel focus ids. Used by `GET_STATE` when the registry is empty.
const fn empty_session_snapshot() -> phux_protocol::wire::info::SessionSnapshot {
    use phux_protocol::ids::{SessionId, TerminalId, WindowId};
    phux_protocol::wire::info::SessionSnapshot::new(
        SessionId::new(0),
        WindowId::new(0),
        TerminalId::local(0),
    )
}

/// Resolve `target`, call [`prepare_attach`], and queue the
/// `ATTACHED` + per-pane `TERMINAL_SNAPSHOT` frames on `out_tx`.
///
/// On any failure path, emits an `ERROR` frame and returns. We never
/// partially-attach: either every frame queues or none does.
#[allow(
    clippy::too_many_lines,
    reason = "linear attach orchestration: resolve target -> prepare -> spawn per-pane output pumps -> fan out snapshot requests via FuturesUnordered -> drain; splitting it would scatter context"
)]
#[allow(
    clippy::too_many_arguments,
    reason = "the ATTACH branch in handle_client pre-decomposes the FrameKind::Attach payload (target/viewport/request_scrollback/scrollback_limit_lines) and threads the negotiated ColorSupport alongside the SharedState + client_id + out_tx; rebundling into a struct would just move the arity from the call site to a builder"
)]
// Lifecycle span (info): one ATTACH per client. Its CLOSE duration is the
// attach-handshake timing (snapshot fan-out is the slow part); the fields
// correlate it to a client + target + requested dims. `skip_all` keeps the
// large arg list (state handle, channels, token) out of the span.
#[tracing::instrument(
    level = "info",
    name = "handle_attach",
    skip_all,
    fields(?client_id, target = ?target, cols = viewport.cols, rows = viewport.rows),
)]
async fn handle_attach(
    state: &SharedState,
    client_id: ClientId,
    target: AttachTarget,
    viewport: phux_protocol::wire::frame::ViewportInfo,
    _request_scrollback: bool,
    _scrollback_limit_lines: u32,
    out_tx: &tokio::sync::mpsc::Sender<Outbound>,
    client_caps: ClientCapabilities,
    root_token: &CancellationToken,
    output_pumps: &mut JoinSet<()>,
) {
    let Some(session_name) = resolve_attach_target(state, target, out_tx, root_token).await else {
        return;
    };

    let (snapshot, initial_client_id, panes_to_snapshot) =
        match prepare_attach(state, client_id, &session_name, out_tx, client_caps) {
            Ok(t) => t,
            Err(crate::state::AttachError::UnknownSession(name)) => {
                send_error(
                    out_tx,
                    ErrorCode::SessionNotFound,
                    &format!("session {name:?} not found"),
                )
                .await;
                return;
            }
            Err(crate::state::AttachError::AlreadyAttached(_)) => {
                send_error(
                    out_tx,
                    ErrorCode::AlreadyAttached,
                    "client is already attached",
                )
                .await;
                return;
            }
        };

    // phux-2lj: apply the client's ATTACH viewport to every pane so
    // freshly-spawned PTYs (currently built at hardcoded 80x24, see
    // `seed_session_with_pty`) are resized to match the attaching
    // client's host terminal. Without this, e.g. `vim` running in a
    // 120x48 host terminal only fills the top 24 rows of the screen
    // until SIGWINCH or an explicit VIEWPORT_RESIZE drives a resize.
    //
    // SPEC §10.5: ATTACH.viewport is the outer client viewport. Single-
    // pane: the server applies it directly as the PTY's winsize (matches
    // the existing `handle_viewport_resize` convention; the off-by-one
    // for a host-side status bar is the client's concern via the
    // post-attach `TERMINAL_RESIZE` reflow path used by multi-pane).
    apply_attach_viewport(state, &panes_to_snapshot, viewport);

    if out_tx
        .send(Outbound::Frame(FrameKind::Attached {
            snapshot,
            initial_client_id,
        }))
        .await
        .is_err()
    {
        return;
    }

    // Fan out all `SnapshotRequest`s concurrently. The mpsc sends below
    // are fast (they just push into each actor's mailbox); the slow part
    // is awaiting the oneshot reply once the actor synthesizes. Doing
    // this sequentially made attach latency scale with the SUM of pane
    // reply times. With `FuturesUnordered` it scales with the MAX —
    // one slow pane no longer stalls the rest.
    // Bridge `state::ClientId` (u64 newtype) -> `phux_protocol::ClientId`
    // (u32), matching `handle_frame_ack`'s conversion so the
    // per-consumer state map keys line up across attach / ack / detach.
    let wire_client_id =
        phux_protocol::ids::ClientId::new(u32::try_from(client_id.0).unwrap_or(u32::MAX));

    let mut pending: FuturesUnordered<_> = FuturesUnordered::new();
    for pane in panes_to_snapshot {
        let terminal_id = pane.terminal_id;
        let handle = pane.handle;
        let wire_terminal_id = pane.wire_terminal_id;
        // ADR-0018 / phux-0q8: register the per-consumer state-sync entry
        // so the actor allocates and primes a per-consumer `RenderState`
        // cache for this client/pane, keyed by `wire_client_id`. We do
        // this BEFORE emitting the snapshot so the per-consumer cache is
        // primed against the same canonical state the snapshot installs
        // on the client mirror (see `register_consumer`'s doc).
        //
        // phux-3uv: the register reply reports whether the actor is
        // tick-managing this consumer (`consumer_tick_emits == true`). If
        // so, the actor's `tick_emit` is the sole emitter and we MUST
        // suppress the broadcast pump below — otherwise two independent
        // `seq` streams land on one consumer mailbox (double-paint, SPEC
        // §12.2 monotonic-per-consumer violation). If not tick-managed
        // (gate off, or register failed / actor gone / no local id), the
        // broadcast pump stays the live emitter and the per-consumer
        // entry just drives the dormant `FRAME_ACK` eviction loop.
        //
        // Awaited (not fire-and-forget) so the cache is primed before the
        // pump starts streaming deltas; a dropped reply or actor-gone is
        // logged and we fall back to the broadcast path.
        let mut tick_managed = false;
        if let Some(wire_id) = wire_terminal_id.local_id() {
            let (attach_reply_tx, attach_reply_rx) = oneshot::channel();
            if handle
                .consumer_attach
                .send(ConsumerAttachRequest {
                    client_id: wire_client_id,
                    outbound: out_tx.clone(),
                    wire_terminal_id: wire_id,
                    // phux-fseo: honor the consumer's negotiated output mode.
                    // StateSync ⇒ the actor's tick is this consumer's emitter
                    // and the broadcast pump below is suppressed for it; Raw
                    // (the human-TUI default) keeps the pump.
                    wants_state_sync: matches!(
                        client_caps.output_mode,
                        phux_protocol::caps::OutputMode::StateSync
                    ),
                    reply: attach_reply_tx,
                })
                .await
                .is_ok()
            {
                match attach_reply_rx.await {
                    Ok(Ok(outcome)) => {
                        tick_managed = outcome.tick_managed;
                        trace!(
                            ?terminal_id,
                            tick_managed, "per-consumer state-sync entry registered",
                        );
                    }
                    Ok(Err(err)) => {
                        warn!(
                            ?terminal_id,
                            error = %err,
                            "per-consumer state-sync register failed; broadcast path still serves this pane",
                        );
                    }
                    Err(_) => {
                        warn!(
                            ?terminal_id,
                            "per-consumer state-sync register: actor dropped reply",
                        );
                    }
                }
            } else {
                warn!(
                    ?terminal_id,
                    "per-consumer state-sync register: actor mailbox closed",
                );
            }
        }

        // phux-3uv: suppress the broadcast pump for a tick-managed
        // consumer — the actor's `tick_emit` is the single emitter for
        // this pane. Non-tick-managed consumers keep the broadcast pump.
        if !tick_managed {
            // Subscribe to live PTY output BEFORE requesting the snapshot.
            // Subscribing first means anything the TerminalActor broadcasts
            // after this point lands in our receiver; we then ask for a
            // snapshot so the client has a complete starting picture, and
            // any subsequent TerminalOutput we forward is "post-snapshot
            // delta" rather than racing against it.
            let mut output_rx = handle.output.subscribe();
            let pump_out_tx = out_tx.clone();
            let pump_wire_terminal_id = wire_terminal_id.clone();
            let pump_client_caps = client_caps;
            output_pumps.spawn_local(async move {
                let mut seq: u64 = 0;
                loop {
                    match output_rx.recv().await {
                        Ok(bytes) => {
                            seq = seq.wrapping_add(1);
                            let bytes = crate::downsample::rewrite_bytes_with_caps(
                                &bytes,
                                pump_client_caps,
                            );
                            if pump_out_tx
                                .send(Outbound::Frame(FrameKind::TerminalOutput {
                                    terminal_id: pump_wire_terminal_id.clone(),
                                    seq,
                                    bytes,
                                }))
                                .await
                                .is_err()
                            {
                                // Client mailbox closed (detach or disconnect).
                                break;
                            }
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                            warn!(
                                terminal_id = ?pump_wire_terminal_id,
                                dropped = n,
                                "TerminalOutput pump lagged; consider larger broadcast capacity",
                            );
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    }
                }
            });
        }

        let (reply_tx, reply_rx) = oneshot::channel();
        if handle
            .snapshot
            .send(SnapshotRequest { reply: reply_tx })
            .await
            .is_err()
        {
            warn!(?terminal_id, "pane actor dropped; skipping snapshot");
            continue;
        }
        // Tag each in-flight receiver with its identifiers so the drain
        // loop can warn / build a frame without re-deriving them.
        pending.push(async move { (terminal_id, wire_terminal_id, reply_rx.await) });
    }

    while let Some((terminal_id, wire_terminal_id, reply)) = pending.next().await {
        let Ok(snap) = reply else {
            warn!(?terminal_id, "pane actor failed to reply with snapshot");
            continue;
        };
        if out_tx
            .send(Outbound::Frame(FrameKind::TerminalSnapshot {
                terminal_id: wire_terminal_id,
                cols: snap.cols,
                rows: snap.rows,
                vt_replay_bytes: snap.bytes,
                // Scrollback negotiation per ATTACH viewport metrics
                // lands with the PTY pump; byc.8 always sends None.
                scrollback_bytes: None,
            }))
            .await
            .is_err()
        {
            return;
        }
    }
}

/// phux-2lj: Apply the ATTACH viewport to every pane in the freshly-
/// attached session.
///
/// Panes are spawned at a hardcoded 80x24 default ([`seed_session_with_pty`]
/// / [`seed_session_with_actor`]) because the session may exist before any
/// client attaches (e.g. `phux-server` pre-seeding). On the first attach
/// we have to size the PTY to match the client's outer viewport, otherwise
/// full-screen TUIs (vim, htop) think they're running in 24 rows and
/// render into a fraction of the visible area. This mirrors what
/// [`handle_viewport_resize`] does for a live `VIEWPORT_RESIZE` frame.
///
/// The resize is fire-and-forget on the per-actor mpsc channel — same
/// primitive `handle_viewport_resize` and `handle_terminal_resize` use.
/// We `try_send` rather than `.await` so we can stay in a sync helper
/// (no impact on `handle_attach`'s lock ordering) and because the
/// resize channel is sized at `DEFAULT_INPUT_MAILBOX = 64`, which is
/// well above the worst-case number of panes per attach (1 today; would
/// stay << 64 even with multi-window sessions).
///
/// The `pane.dims` update is wrapped in `with_mut` once so the registry
/// stays consistent with what future `TERMINAL_SNAPSHOT` payloads will
/// report; the resize sends are emitted while holding the same lock,
/// matching `handle_viewport_resize`'s pattern (the actor's mailbox is
/// independent of the state lock).
fn apply_attach_viewport(
    state: &SharedState,
    panes_to_snapshot: &[AttachSnapshotPane],
    viewport: phux_protocol::wire::frame::ViewportInfo,
) {
    let cols = viewport.cols;
    let rows = viewport.rows;
    if cols == 0 || rows == 0 {
        // SPEC §10.5: zero-dimension viewports are treated as no-ops
        // rather than kernel errors. Skip the resize entirely.
        return;
    }
    state.with_mut(|s| {
        for pane in panes_to_snapshot {
            if let Some(pane_entry) = s.registry.terminal_mut(pane.terminal_id) {
                pane_entry.dims = (cols, rows);
            }
            // ATTACH-time resize: do NOT resync — the attach handshake
            // already sends an authoritative TERMINAL_SNAPSHOT, and a
            // resync broadcast here would race ahead of it (phux-8v1).
            match pane.handle.resize.try_send(ResizeRequest {
                cols,
                rows,
                resync_clients: false,
            }) {
                Ok(()) => {}
                Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                    warn!(
                        terminal_id = ?pane.terminal_id,
                        cols,
                        rows,
                        "ATTACH viewport apply: pane resize mailbox full; dropping (next VIEWPORT_RESIZE will retry)",
                    );
                }
                Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                    debug!(
                        terminal_id = ?pane.terminal_id,
                        "ATTACH viewport apply: pane actor gone; dropping resize",
                    );
                }
            }
        }
    });
}

/// Handle a client's `VIEWPORT_RESIZE` (SPEC §7.1 / §10.5).
///
/// Look up the client's currently-focused pane and update the in-memory
/// `dims` so future `TERMINAL_SNAPSHOT` frames reflect the new size. This is
/// the additive surface for phux-4hp: we deliberately do NOT push a
/// resize into the [`TerminalActor`] (or call `Terminal::set_size` /
/// `pty.resize(...)`) because byc.5's PTY pump owns the actor-side
/// `Terminal` / `portable-pty` resize integration. The follow-up there
/// will consume this state change (or, if it prefers a direct channel,
/// can add a new `TerminalHandle` channel without touching this code).
///
/// Per SPEC §10.5, when multiple clients are attached with different
/// sizes the server uses the smallest common bounding box per window.
/// That negotiation lives with byc.5 too; today the last writer wins,
/// which matches single-attach behavior (the only path exercised).
///
/// Silent on every "not-found" path. A `VIEWPORT_RESIZE` from an
/// unattached client is a benign race (the client may have sent it
/// before its ATTACH completed); logging at `debug!` is enough.
fn handle_viewport_resize(state: &SharedState, client_id: ClientId, viewport: &ViewportInfo) {
    state.with_mut(|s| {
        let Some(client) = s.attached.get(&client_id) else {
            debug!(
                ?client_id,
                "VIEWPORT_RESIZE from non-attached client; ignoring"
            );
            return;
        };
        let session_id = client.session;
        let Some(session) = s.registry.session(session_id) else {
            debug!(?client_id, "VIEWPORT_RESIZE: client's session vanished");
            return;
        };
        let Some(window_id) = session.active else {
            debug!(?client_id, "VIEWPORT_RESIZE: no active window in session");
            return;
        };
        let Some(window) = s.registry.window(window_id) else {
            return;
        };
        let Some(terminal_id) = window.active else {
            return;
        };
        if let Some(pane) = s.registry.terminal_mut(terminal_id) {
            pane.dims = (viewport.cols, viewport.rows);
        }
        // Fan the resize out to the TerminalActor so libghostty's
        // `Terminal::set_size` and the PTY `winsize` ioctl get
        // updated. byc.5 added the `resize` channel on `TerminalHandle`;
        // this is the missing connector (4hp ↔ byc.5).
        //
        // We hold the state lock here so `try_send` is the right
        // primitive: VIEWPORT_RESIZE is fire-and-forget per SPEC §10.5,
        // and an `.await` inside `with_mut` would deadlock the
        // single-threaded runtime. On send failure (actor terminated,
        // mailbox full — both rare; the resize mailbox is sized at
        // `DEFAULT_INPUT_MAILBOX` = 64), we log and continue: a
        // dropped resize is recoverable (the next resize, or the
        // next snapshot, re-syncs) and SPEC §10.5 explicitly classes
        // VIEWPORT_RESIZE as best-effort.
        if let Some(handle) = s.terminals.get(&terminal_id) {
            // Live viewport resize (SIGWINCH): resync clients (phux-8v1).
            match handle.resize.try_send(ResizeRequest {
                cols: viewport.cols,
                rows: viewport.rows,
                resync_clients: true,
            }) {
                Ok(()) => {}
                Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                    warn!(
                        ?client_id,
                        ?terminal_id,
                        cols = viewport.cols,
                        rows = viewport.rows,
                        "VIEWPORT_RESIZE: pane resize mailbox full; dropping (fire-and-forget per SPEC §10.5)",
                    );
                }
                Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                    debug!(
                        ?client_id,
                        ?terminal_id,
                        "VIEWPORT_RESIZE: pane actor gone; dropping resize",
                    );
                }
            }
        } else {
            debug!(
                ?client_id,
                ?terminal_id,
                "VIEWPORT_RESIZE: no TerminalHandle registered for pane; dropping resize",
            );
        }
    });
}

/// Route an `INPUT_*` frame body to the target pane's [`TerminalActor`].
///
/// SPEC §9: input frames are fire-and-forget — no `Outbound` reply.
/// On the wire the pane is identified by its `WireTerminalId` (`u32`); we
/// resolve it back to a core [`TerminalId`] via [`ServerState::terminal_from_wire`],
/// then locate the [`TerminalHandle`] and `try_send` the encoded
/// [`TerminalInput`] onto the actor's input mailbox.
///
/// Validation: we drop with `warn!` (not `debug!`, this is observable
/// misbehavior worth surfacing) on:
///   * Unknown wire pane id (no [`TerminalId`] mapping).
///   * Client not attached (the per-client task should not be reading
///     frames from a detached identity, but we re-check defensively).
///   * Client attached but not subscribed to this pane — prevents one
///     client from steering another's pane (SPEC §9 leaves multi-client
///     subscription rules to per-pane policy; for now subscription is
///     the gate).
///   * Pane has no registered [`TerminalHandle`] (actor never spawned, or
///     spawned but evicted).
///
/// `try_send` is used because we hold the `with_mut` lock while routing:
/// awaiting inside a `with_mut` would deadlock the single-threaded
/// runtime, and an unbounded queue would let a slow PTY producer push
/// memory through the roof. `Full` is treated as a backpressure event
/// (warn-drop); `Closed` is logged at debug and dropped (actor gone).
fn handle_terminal_input(
    state: &SharedState,
    client_id: ClientId,
    wire_terminal_id: &phux_protocol::ids::TerminalId,
    input: TerminalInput,
    frame_label: &'static str,
) {
    // v0.1 non-federation-hub servers reject SATELLITE-routed input frames
    // (per ADR-0016 / SPEC §10.1). The protocol-level response is `ERROR
    // { UnsupportedSatelliteRoute }`; this dispatch helper just drops the
    // frame with a warn — the surrounding read loop will surface the
    // error response in a follow-up tied to phux-byc.9.
    if !wire_terminal_id.is_local() {
        warn!(
            ?client_id,
            ?wire_terminal_id,
            frame_label,
            "input frame carried a SATELLITE TerminalId on a non-federation-hub server; dropping",
        );
        return;
    }
    state.with_mut(|s| {
        let Some(pane) = s.terminal_from_wire(wire_terminal_id) else {
            warn!(
                ?client_id,
                ?wire_terminal_id,
                frame_label,
                "input frame for unknown pane; dropping",
            );
            return;
        };
        let Some(attached) = s.attached.get(&client_id) else {
            warn!(
                ?client_id,
                ?wire_terminal_id,
                frame_label,
                "input frame from non-attached client; dropping",
            );
            return;
        };
        // Subscription gate: the pane must be one the client is observing.
        // For byc.8's "active pane only" subscription model this is the
        // same as "is the pane in the client's attached session"; a
        // richer SUBSCRIBE story (SPEC §7.4) will refine this without
        // changing the dispatch shape.
        let session = attached.session;
        let is_subscribed = s.subscribers_for_terminal(pane).contains(&client_id);
        if !is_subscribed {
            warn!(
                ?client_id,
                ?wire_terminal_id,
                ?session,
                frame_label,
                "client not subscribed to pane; dropping input",
            );
            return;
        }
        s.touch_session(session);
        let Some(handle): Option<&TerminalHandle> = s.terminal_handle(pane) else {
            warn!(
                ?client_id,
                ?wire_terminal_id,
                frame_label,
                "no TerminalHandle for pane; dropping input",
            );
            return;
        };
        match handle.input.try_send(input) {
            Ok(()) => {
                trace!(
                    ?client_id,
                    ?wire_terminal_id,
                    frame_label,
                    "input routed to TerminalActor"
                );
            }
            Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                warn!(
                    ?client_id,
                    ?wire_terminal_id,
                    frame_label,
                    "pane input mailbox full; dropping (fire-and-forget per SPEC §9)",
                );
            }
            Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                debug!(
                    ?client_id,
                    ?wire_terminal_id,
                    frame_label,
                    "pane actor gone; dropping input",
                );
            }
        }
    });
}

/// Route an inbound `FRAME_ACK` (SPEC §7.proto.1 / §12.2) to the
/// owning `TerminalActor` so it can evict the per-consumer dirty cache
/// under ADR-0018 lazy state synchronization (phux-q0e.4).
///
/// Validation:
///   * Unknown wire pane id → drop (warn). The client is acking a
///     terminal the server has no mapping for; this is observable
///     misbehavior worth surfacing.
///   * Client not attached → drop (warn). Acks make no sense without
///     an attachment.
///   * Client not subscribed to this pane → drop (warn). Same gate as
///     `handle_terminal_input`: a client cannot ack a pane it does not
///     observe.
///   * No `TerminalHandle` (actor evicted) → drop (debug — race against
///     teardown).
///
/// `try_send` is non-blocking by the same `with_mut` locking rationale
/// as `handle_terminal_input`: awaiting inside `with_mut` would
/// deadlock the single-threaded runtime, and `FRAME_ACK` is hint-shaped
/// per ADR-0018 — dropping under backpressure is correct (the next
/// ack the client sends will catch up the per-consumer reference,
/// and unacked diffs stay re-emittable in the meantime).
fn handle_frame_ack(
    state: &SharedState,
    client_id: ClientId,
    wire_terminal_id: &phux_protocol::ids::TerminalId,
    seq: u64,
) {
    // v0.1 servers reject SATELLITE-routed acks for the same reason input
    // frames are dropped above: this server is not a federation hub.
    if !wire_terminal_id.is_local() {
        warn!(
            ?client_id,
            ?wire_terminal_id,
            seq,
            "FRAME_ACK carried a SATELLITE TerminalId on a non-federation-hub server; dropping",
        );
        return;
    }
    state.with_mut(|s| {
        let Some(pane) = s.terminal_from_wire(wire_terminal_id) else {
            warn!(
                ?client_id,
                ?wire_terminal_id,
                seq,
                "FRAME_ACK for unknown pane; dropping",
            );
            return;
        };
        let Some(attached) = s.attached.get(&client_id) else {
            warn!(
                ?client_id,
                ?wire_terminal_id,
                seq,
                "FRAME_ACK from non-attached client; dropping",
            );
            return;
        };
        let session = attached.session;
        let is_subscribed = s.subscribers_for_terminal(pane).contains(&client_id);
        if !is_subscribed {
            warn!(
                ?client_id,
                ?wire_terminal_id,
                ?session,
                seq,
                "FRAME_ACK from client not subscribed to pane; dropping",
            );
            return;
        }
        let Some(handle): Option<&TerminalHandle> = s.terminal_handle(pane) else {
            warn!(
                ?client_id,
                ?wire_terminal_id,
                seq,
                "FRAME_ACK with no TerminalHandle for pane; dropping",
            );
            return;
        };
        // Bridge `state::ClientId` (u64 newtype) → `phux_protocol::ClientId`
        // (u32), matching the conversion `handle_attach` already does for
        // the per-consumer state map keys. The wire ClientId space caps at
        // u32::MAX; widening would require a protocol bump.
        let wire_client_id =
            phux_protocol::ids::ClientId::new(u32::try_from(client_id.0).unwrap_or(u32::MAX));
        match handle.consumer_ack.try_send(ConsumerAckRequest {
            client_id: wire_client_id,
            seq,
        }) {
            Ok(()) => {
                trace!(
                    ?client_id,
                    ?wire_terminal_id,
                    seq,
                    "FRAME_ACK routed to TerminalActor"
                );
            }
            Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                trace!(
                    ?client_id,
                    ?wire_terminal_id,
                    seq,
                    "FRAME_ACK mailbox full; dropping (ADR-0018: next ack catches up)",
                );
            }
            Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                debug!(
                    ?client_id,
                    ?wire_terminal_id,
                    seq,
                    "FRAME_ACK: pane actor gone; dropping",
                );
            }
        }
    });
}

/// Queue an `ERROR` frame on `out_tx`. Used by attach failure paths.
async fn send_error(out_tx: &tokio::sync::mpsc::Sender<Outbound>, code: ErrorCode, message: &str) {
    if out_tx
        .send(Outbound::Frame(FrameKind::Error {
            request_id: None,
            code,
            message: message.to_owned(),
        }))
        .await
        .is_err()
    {
        trace!(?code, "ERROR send dropped: writer gone");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detach_aborts_raw_output_pumps_without_closing_writer_mailbox() {
        let rt = Builder::new_current_thread().enable_all().build().unwrap();
        let local = LocalSet::new();
        local.block_on(&rt, async {
            let client_id = ClientId(7);
            let (out_tx, mut out_rx) = tokio::sync::mpsc::channel::<Outbound>(8);
            let (output_tx, _seed_rx) = tokio::sync::broadcast::channel::<bytes::Bytes>(8);
            let mut output_rx = output_tx.subscribe();
            let mut output_pumps = JoinSet::new();
            let terminal_id = phux_protocol::ids::TerminalId::local(42);

            let pump_out_tx = out_tx.clone();
            let pump_terminal_id = terminal_id.clone();
            output_pumps.spawn_local(async move {
                let mut seq: u64 = 0;
                while let Ok(bytes) = output_rx.recv().await {
                    seq = seq.wrapping_add(1);
                    if pump_out_tx
                        .send(Outbound::Frame(FrameKind::TerminalOutput {
                            terminal_id: pump_terminal_id.clone(),
                            seq,
                            bytes: bytes.to_vec(),
                        }))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
            });

            output_tx
                .send(bytes::Bytes::from_static(b"before-detach"))
                .unwrap();
            let first = tokio::time::timeout(Duration::from_secs(1), out_rx.recv())
                .await
                .expect("first output timed out")
                .expect("writer mailbox closed");
            assert!(matches!(
                first,
                Outbound::Frame(FrameKind::TerminalOutput { seq: 1, .. })
            ));

            abort_output_pumps(&mut output_pumps, client_id, "test-detach").await;
            assert!(output_pumps.is_empty());

            // The writer mailbox remains usable after DETACH so the server
            // can still emit DETACHED or serve a later ATTACH on the same
            // connection, but the old per-pane pump no longer forwards bytes.
            assert!(
                out_tx
                    .send(Outbound::Frame(FrameKind::Detached))
                    .await
                    .is_ok()
            );
            assert!(
                output_tx
                    .send(bytes::Bytes::from_static(b"after-detach"))
                    .is_ok()
            );

            let detached = tokio::time::timeout(Duration::from_secs(1), out_rx.recv())
                .await
                .expect("DETACHED timed out")
                .expect("writer mailbox closed");
            assert!(matches!(detached, Outbound::Frame(FrameKind::Detached)));
            tokio::task::yield_now().await;
            assert!(
                out_rx.try_recv().is_err(),
                "old output pump forwarded after detach"
            );
        });
    }

    /// `VIEWPORT_RESIZE` updates the focused pane's stored dims on the
    /// canonical `Registry`. byc.5's PTY-resize integration will read
    /// this state when it lands; today we just observe the mutation.
    #[test]
    fn viewport_resize_updates_focused_pane_dims() {
        use phux_core::ids::TerminalId as CoreTerminalId;

        let state = SharedState::new();
        // Seed a session with a pane, then attach a client. Mirrors what
        // `seed_session_with_actor` does on the real path, minus the
        // TerminalActor spawn (we're not exercising the actor here — just
        // the state-side dim update).
        let (sid, _wid, pid): (_, _, CoreTerminalId) =
            state.with_mut(|s| s.seed_session("test-session"));
        let client_id = state.with_mut(crate::state::ServerState::new_client_id);
        let (tx, _rx) = tokio::sync::mpsc::channel(8);
        state
            .with_mut(|s| s.attach_default_caps(client_id, "test-session", tx))
            .expect("attach");

        // Sanity: starts at 80x24 (default core::Pane::dims).
        let before = state
            .with(|s| s.registry.terminal(pid).map(|p| p.dims))
            .expect("pane exists");
        assert_eq!(before, (80, 24));

        let viewport = ViewportInfo::new(132, 50).with_pixels(Some(1320), Some(750));
        handle_viewport_resize(&state, client_id, &viewport);

        let after = state
            .with(|s| s.registry.terminal(pid).map(|p| p.dims))
            .expect("pane exists");
        assert_eq!(after, (132, 50));

        // Sanity: the session linkage didn't get clobbered.
        let attached_session = state.with(|s| s.attached.get(&client_id).map(|c| c.session));
        assert_eq!(attached_session, Some(sid));
    }

    /// `VIEWPORT_RESIZE` fans the new (cols, rows) tuple onto the
    /// `TerminalHandle::resize` channel byc.5 added. We inject a hand-
    /// built `TerminalHandle` (no real actor) so the test can observe the
    /// receiver side directly — this pins the wire from
    /// `handle_viewport_resize` into the actor without needing to
    /// stand up libghostty or a PTY pair.
    #[test]
    fn viewport_resize_sends_to_terminal_actor_resize_channel() {
        use crate::terminal_actor::TerminalHandle;
        use bytes::Bytes;
        use phux_core::ids::TerminalId as CoreTerminalId;
        use tokio::sync::{broadcast, mpsc};

        let state = SharedState::new();
        let (_sid, _wid, pid): (_, _, CoreTerminalId) =
            state.with_mut(|s| s.seed_session("test-session"));

        // Build a `TerminalHandle` directly. The actor side is not running;
        // we only care that `handle.resize.try_send` lands. The other
        // channels exist purely to satisfy the struct shape.
        let (input_tx, _input_rx) = mpsc::channel(8);
        let (snapshot_tx, _snapshot_rx) = mpsc::channel(8);
        let (screen_tx, _screen_rx) = mpsc::channel(8);
        let (pwd_tx, _pwd_rx) = mpsc::channel(8);
        let (output_tx, _output_rx_seed) = broadcast::channel::<Bytes>(8);
        let (resize_tx, mut resize_rx) = mpsc::channel::<ResizeRequest>(8);
        let (consumer_attach_tx, _consumer_attach_rx) = mpsc::channel(8);
        let (consumer_detach_tx, _consumer_detach_rx) = mpsc::channel(8);
        let (consumer_ack_tx, _consumer_ack_rx) = mpsc::channel(8);
        let (subscribe_to_events_tx, _subscribe_to_events_rx) = mpsc::channel(8);
        let (unsubscribe_from_events_tx, _unsubscribe_from_events_rx) = mpsc::channel(8);
        let handle = TerminalHandle {
            input: input_tx,
            snapshot: snapshot_tx,
            screen: screen_tx,
            pwd: pwd_tx,
            output: output_tx,
            resize: resize_tx,
            consumer_attach: consumer_attach_tx,
            consumer_detach: consumer_detach_tx,
            consumer_ack: consumer_ack_tx,
            subscribe_to_events: subscribe_to_events_tx,
            unsubscribe_from_events: unsubscribe_from_events_tx,
            cols: 80,
            rows: 24,
        };
        state.with_mut(|s| {
            // `register_terminal_handle` wants a CancellationToken; build
            // a fresh one. We don't keep a clone — no actor is running
            // for this test, so cancellation is moot.
            let token = CancellationToken::new();
            let _ = s.register_terminal_handle(pid, handle, token);
        });

        let client_id = state.with_mut(crate::state::ServerState::new_client_id);
        let (tx, _rx) = tokio::sync::mpsc::channel(8);
        state
            .with_mut(|s| s.attach_default_caps(client_id, "test-session", tx))
            .expect("attach");

        let viewport = ViewportInfo::new(132, 50);
        handle_viewport_resize(&state, client_id, &viewport);

        // The connector ran inside the same task; the channel must
        // already carry exactly one resize request.
        let observed = resize_rx
            .try_recv()
            .expect("resize request must be queued on the channel");
        assert_eq!(
            (observed.cols, observed.rows),
            (132, 50),
            "TerminalHandle::resize must receive the new viewport dims",
        );
        assert!(
            observed.resync_clients,
            "a live VIEWPORT_RESIZE must request a client resync (phux-8v1)",
        );
        assert!(
            resize_rx.try_recv().is_err(),
            "exactly one resize request should be queued — got more",
        );
    }

    /// Concurrency proof for the ATTACH per-pane snapshot fan-out.
    ///
    /// Builds N hand-crafted `TerminalHandle`s (no real `TerminalActor`) whose
    /// `snapshot_rx` ends the test holds. Registers them against a
    /// session, then drives `handle_attach`. With the sequential loop
    /// the test would deadlock: the handler would `await` pane 0's
    /// reply, but the test only replies after observing all N requests
    /// land on their receivers. The `FuturesUnordered` fan-out unsticks
    /// it by sending all N requests up front, then awaiting replies as
    /// they arrive in any order.
    #[tokio::test(flavor = "current_thread")]
    #[allow(
        clippy::too_many_lines,
        reason = "linear setup-then-act-then-assert test body; splitting would obscure the concurrency proof"
    )]
    async fn handle_attach_fans_out_snapshot_requests_concurrently() {
        use std::time::Duration;

        use bytes::Bytes;
        use phux_core::ids::TerminalId as CoreTerminalId;
        use tokio::sync::{broadcast, mpsc, oneshot};
        use tokio::task::LocalSet;

        use crate::grid::SnapshotBytes;
        use crate::terminal_actor::{SnapshotRequest, TerminalHandle};

        const N: usize = 4;

        let local = LocalSet::new();
        local
            .run_until(async {
                let state = SharedState::new();
                // Seed one session with one window and N panes.
                let (sid, wid, _first_pane) = state.with_mut(|s| s.seed_session("multi"));
                // `seed_session` made one pane already; we want N total.
                let mut terminal_ids: Vec<CoreTerminalId> = Vec::with_capacity(N);
                state.with_mut(|s| {
                    let session = s.registry.session(sid).cloned().expect("session");
                    let window = s
                        .registry
                        .window(session.windows[0])
                        .cloned()
                        .expect("window");
                    terminal_ids.push(window.panes[0]);
                    for _ in 1..N {
                        let pid = s.registry.new_terminal(wid).expect("new_pane");
                        terminal_ids.push(pid);
                    }
                });

                // Build N TerminalHandles; keep the snapshot receivers in the test.
                let mut snapshot_rxs: Vec<mpsc::Receiver<SnapshotRequest>> = Vec::with_capacity(N);
                for &pid in &terminal_ids {
                    let (input_tx, _input_rx) = mpsc::channel(8);
                    let (snapshot_tx, snapshot_rx) = mpsc::channel(8);
                    let (screen_tx, _screen_rx) = mpsc::channel(8);
                    let (pwd_tx, _pwd_rx) = mpsc::channel(8);
                    let (output_tx, _output_rx_seed) = broadcast::channel::<Bytes>(8);
                    let (resize_tx, _resize_rx) = mpsc::channel::<ResizeRequest>(8);
                    let (consumer_attach_tx, _consumer_attach_rx) = mpsc::channel(8);
                    let (consumer_detach_tx, _consumer_detach_rx) = mpsc::channel(8);
                    let (consumer_ack_tx, _consumer_ack_rx) = mpsc::channel(8);
                    let (subscribe_to_events_tx, _subscribe_to_events_rx) = mpsc::channel(8);
                    let (unsubscribe_from_events_tx, _unsubscribe_from_events_rx) =
                        mpsc::channel(8);
                    let handle = TerminalHandle {
                        input: input_tx,
                        snapshot: snapshot_tx,
                        screen: screen_tx,
                        pwd: pwd_tx,
                        output: output_tx,
                        resize: resize_tx,
                        consumer_attach: consumer_attach_tx,
                        consumer_detach: consumer_detach_tx,
                        consumer_ack: consumer_ack_tx,
                        subscribe_to_events: subscribe_to_events_tx,
                        unsubscribe_from_events: unsubscribe_from_events_tx,
                        cols: 80,
                        rows: 24,
                    };
                    state.with_mut(|s| {
                        let _ = s.register_terminal_handle(pid, handle, CancellationToken::new());
                    });
                    snapshot_rxs.push(snapshot_rx);
                }

                // Outbound channel for the would-be writer task; we read
                // TERMINAL_SNAPSHOT frames out of `out_rx` to verify all N
                // shipped.
                let (out_tx, mut out_rx) =
                    mpsc::channel::<Outbound>(crate::state::DEFAULT_CLIENT_MAILBOX);
                let client_id = state.with_mut(crate::state::ServerState::new_client_id);

                // Spawn `handle_attach` on the LocalSet so the test
                // body can interleave with it.
                let state_for_task = state.clone();
                let test_root_token = CancellationToken::new();
                let attach_task = tokio::task::spawn_local(async move {
                    let mut output_pumps = JoinSet::new();
                    handle_attach(
                        &state_for_task,
                        client_id,
                        AttachTarget::ByName("multi".to_owned()),
                        ViewportInfo::new(80, 24),
                        false,
                        0,
                        &out_tx,
                        ClientCapabilities::default(),
                        &test_root_token,
                        &mut output_pumps,
                    )
                    .await;
                });

                // First the writer should see ATTACHED.
                let attached = tokio::time::timeout(Duration::from_secs(2), out_rx.recv())
                    .await
                    .expect("attached frame did not arrive")
                    .expect("out_rx closed before attached");
                let Outbound::Frame(frame) = attached;
                assert!(
                    matches!(frame, FrameKind::Attached { .. }),
                    "expected Attached, got {frame:?}",
                );

                // Now collect all N SnapshotRequests BEFORE replying to
                // any of them. Under the old sequential loop the
                // handler would block on pane 0's reply forever (we
                // haven't replied yet), so only the first request
                // would land. With the concurrent fan-out all N land
                // up front.
                let mut replies: Vec<oneshot::Sender<SnapshotBytes>> = Vec::with_capacity(N);
                for (i, rx) in snapshot_rxs.iter_mut().enumerate() {
                    let req = tokio::time::timeout(Duration::from_secs(2), rx.recv())
                        .await
                        .unwrap_or_else(|_| {
                            panic!("snapshot request {i} never arrived — sequential loop?")
                        })
                        .expect("snapshot channel closed");
                    replies.push(req.reply);
                }

                // Reply on all N oneshots. Order should not matter to the
                // fan-out; deliberately reply in reverse to underscore that.
                for (i, reply) in replies.into_iter().enumerate().rev() {
                    let payload = SnapshotBytes {
                        cols: 80,
                        rows: 24,
                        bytes: format!("snap-{i}").into_bytes(),
                    };
                    let _ = reply.send(payload);
                }

                // Drain N TERMINAL_SNAPSHOT frames out of the writer channel.
                let mut snaps_seen = 0usize;
                for _ in 0..N {
                    let frame = tokio::time::timeout(Duration::from_secs(2), out_rx.recv())
                        .await
                        .expect("pane snapshot frame did not arrive")
                        .expect("out_rx closed before snapshot");
                    if matches!(frame, Outbound::Frame(FrameKind::TerminalSnapshot { .. })) {
                        snaps_seen += 1;
                    } else {
                        panic!("expected TerminalSnapshot, got {frame:?}");
                    }
                }
                assert_eq!(snaps_seen, N, "expected one TERMINAL_SNAPSHOT per pane");

                attach_task.await.expect("attach task panicked");
            })
            .await;
    }

    /// phux-0q8: ATTACH wires the per-consumer state-sync lifecycle.
    /// `handle_attach` must send a `ConsumerAttachRequest` (carrying the
    /// resolved wire terminal id) and await its reply before streaming;
    /// the DETACH-class teardown helper must send a matching
    /// `ConsumerDetachRequest` so the actor frees the per-consumer
    /// `RenderState`. We inject a hand-built `TerminalHandle` and hold the
    /// consumer-lifecycle receivers so the test observes both ends without
    /// standing up a libghostty actor.
    #[tokio::test(flavor = "current_thread")]
    #[allow(
        clippy::too_many_lines,
        reason = "linear setup-attach-observe-detach-observe body; splitting would scatter the lifecycle proof"
    )]
    async fn attach_registers_and_detach_unregisters_consumer_lifecycle() {
        use bytes::Bytes;
        use phux_core::ids::TerminalId as CoreTerminalId;
        use tokio::sync::{broadcast, mpsc};
        use tokio::task::LocalSet;

        use crate::grid::SnapshotBytes;
        use crate::terminal_actor::{
            ConsumerAttachRequest, ConsumerDetachRequest, SnapshotRequest, TerminalHandle,
        };

        let local = LocalSet::new();
        local
            .run_until(async {
                let state = SharedState::new();
                let (_sid, _wid, pid): (_, _, CoreTerminalId) =
                    state.with_mut(|s| s.seed_session("lifecycle"));

                let (input_tx, _input_rx) = mpsc::channel(8);
                let (snapshot_tx, mut snapshot_rx) = mpsc::channel::<SnapshotRequest>(8);
                let (screen_tx, _screen_rx) = mpsc::channel(8);
                let (pwd_tx, _pwd_rx) = mpsc::channel(8);
                let (output_tx, _output_rx_seed) = broadcast::channel::<Bytes>(8);
                let (resize_tx, _resize_rx) = mpsc::channel::<ResizeRequest>(8);
                let (consumer_attach_tx, mut consumer_attach_rx) =
                    mpsc::channel::<ConsumerAttachRequest>(8);
                let (consumer_detach_tx, mut consumer_detach_rx) =
                    mpsc::channel::<ConsumerDetachRequest>(8);
                let (consumer_ack_tx, _consumer_ack_rx) = mpsc::channel(8);
                let (subscribe_to_events_tx, _subscribe_to_events_rx) = mpsc::channel(8);
                let (unsubscribe_from_events_tx, _unsubscribe_from_events_rx) = mpsc::channel(8);
                let handle = TerminalHandle {
                    input: input_tx,
                    snapshot: snapshot_tx,
                    screen: screen_tx,
                    pwd: pwd_tx,
                    output: output_tx,
                    resize: resize_tx,
                    consumer_attach: consumer_attach_tx,
                    consumer_detach: consumer_detach_tx,
                    consumer_ack: consumer_ack_tx,
                    subscribe_to_events: subscribe_to_events_tx,
                    unsubscribe_from_events: unsubscribe_from_events_tx,
                    cols: 80,
                    rows: 24,
                };
                state.with_mut(|s| {
                    let _ = s.register_terminal_handle(pid, handle, CancellationToken::new());
                });

                let (out_tx, mut out_rx) =
                    mpsc::channel::<Outbound>(crate::state::DEFAULT_CLIENT_MAILBOX);
                let client_id = state.with_mut(crate::state::ServerState::new_client_id);

                let state_for_task = state.clone();
                let token = CancellationToken::new();
                let attach_task = tokio::task::spawn_local(async move {
                    let mut output_pumps = JoinSet::new();
                    handle_attach(
                        &state_for_task,
                        client_id,
                        AttachTarget::ByName("lifecycle".to_owned()),
                        ViewportInfo::new(80, 24),
                        false,
                        0,
                        &out_tx,
                        ClientCapabilities::default(),
                        &token,
                        &mut output_pumps,
                    )
                    .await;
                });

                // ATTACHED first.
                let attached = tokio::time::timeout(Duration::from_secs(2), out_rx.recv())
                    .await
                    .expect("attached frame did not arrive")
                    .expect("out_rx closed");
                assert!(matches!(
                    attached,
                    Outbound::Frame(FrameKind::Attached { .. })
                ));

                // The consumer-attach request must land, carrying the wire
                // terminal id. Reply Ok so `handle_attach` proceeds.
                let attach_req =
                    tokio::time::timeout(Duration::from_secs(2), consumer_attach_rx.recv())
                        .await
                        .expect("ConsumerAttachRequest never arrived — register not wired?")
                        .expect("consumer_attach channel closed");
                assert_eq!(
                    attach_req.client_id,
                    phux_protocol::ids::ClientId::new(
                        u32::try_from(client_id.0).unwrap_or(u32::MAX)
                    ),
                    "consumer attach keyed by the wire client id",
                );
                assert!(
                    attach_req.wire_terminal_id >= 1,
                    "wire terminal id assigned"
                );
                // phux-3uv: reply `tick_managed: false` so `handle_attach`
                // keeps its broadcast pump (this stub actor never tick-
                // emits). The test exercises the register/snapshot/detach
                // lifecycle, not the emitter-selection branch.
                attach_req
                    .reply
                    .send(Ok(crate::terminal_actor::ConsumerAttachOutcome {
                        tick_managed: false,
                    }))
                    .expect("send attach reply");

                // Service the snapshot request so the attach task completes.
                let snap_req = tokio::time::timeout(Duration::from_secs(2), snapshot_rx.recv())
                    .await
                    .expect("snapshot request did not arrive")
                    .expect("snapshot channel closed");
                snap_req
                    .reply
                    .send(SnapshotBytes {
                        cols: 80,
                        rows: 24,
                        bytes: b"snap".to_vec(),
                    })
                    .expect("send snapshot reply");

                attach_task.await.expect("attach task panicked");

                // Now tear the client down. The helper must send a
                // ConsumerDetachRequest for the subscribed pane.
                detach_and_release_consumer_state(&state, client_id);
                let detach_req =
                    tokio::time::timeout(Duration::from_secs(2), consumer_detach_rx.recv())
                        .await
                        .expect("ConsumerDetachRequest never arrived — detach not wired?")
                        .expect("consumer_detach channel closed");
                assert_eq!(
                    detach_req.client_id,
                    phux_protocol::ids::ClientId::new(
                        u32::try_from(client_id.0).unwrap_or(u32::MAX)
                    ),
                    "consumer detach keyed by the same wire client id",
                );

                // And the client is gone from ServerState.
                assert!(
                    state.with(|s| !s.attached.contains_key(&client_id)),
                    "detach helper must remove the client from ServerState",
                );
            })
            .await;
    }

    /// A `VIEWPORT_RESIZE` from a non-attached client is a benign no-op —
    /// the handler must not panic or mutate state.
    #[test]
    fn viewport_resize_from_unattached_client_is_noop() {
        let state = SharedState::new();
        let (_sid, _wid, pid) = state.with_mut(|s| s.seed_session("session"));
        let bogus_client = ClientId(9999);
        let before = state
            .with(|s| s.registry.terminal(pid).map(|p| p.dims))
            .expect("pane exists");
        handle_viewport_resize(&state, bogus_client, &ViewportInfo::new(200, 60));
        let after = state
            .with(|s| s.registry.terminal(pid).map(|p| p.dims))
            .expect("pane exists");
        assert_eq!(before, after, "no mutation expected for unattached client");
    }
}
