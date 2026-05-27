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
//!   `SPEC.md` §4 (Transport).
//! * Accept connections and spawn a per-client task on a
//!   [`tokio::task::LocalSet`] (per ADR-0014) that reads length-prefixed
//!   frames (`SPEC.md` §5), echoes `PING` with `PONG` (`SPEC.md` §7.5),
//!   and handles `ATTACH` / `DETACH` by talking to the per-terminal
//!   `TerminalActor`s (`phux-byc.8`). The
//!   remaining catalog (`INPUT_KEY`, etc.) is recorded against the
//!   terminal's input log but the PTY write side lands in `phux-byc.5`.
//! * Unlink the socket file on clean shutdown and refuse to start over an
//!   already-live socket.
//!
//! Frame types come from `phux_protocol::wire` (ADR-0008): the protocol crate
//! is the single source of truth for what bytes go on the wire.

use std::future::Future;
use std::io;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use bytes::BytesMut;
use futures_util::StreamExt;
use futures_util::stream::FuturesUnordered;
use phux_protocol::ids::CollectionId;
use phux_protocol::wire::frame::{
    AttachTarget, ErrorCode, FrameKind, MAX_FRAME_LEN, SpawnError, SpawnResult, TYPE_PONG,
    ViewportInfo,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};
use tokio::runtime::Builder;
use tokio::sync::oneshot;
use tokio::task::{JoinSet, LocalSet};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, trace, warn};

use crate::state::{ClientId, DEFAULT_CLIENT_MAILBOX, Outbound, SharedState, TerminalInput};
use crate::terminal_actor::{ConsumerAckRequest, SnapshotRequest, TerminalActor, TerminalHandle};

/// Per-byte-count of the length prefix on every wire frame (see `SPEC.md` §5).
const LENGTH_PREFIX: usize = 4;

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

        let listener = UnixListener::bind(&socket_path).map_err(ServerError::Bind)?;
        info!(path = %socket_path.display(), "phux-server listening on UDS");

        // The LocalSet hosts per-client tasks and per-pane actors —
        // both `!Send`. `LocalSet::run_until` drives the set to the
        // future's completion; tasks spawned via `spawn_local` from
        // inside the future are polled on the same thread.
        let pre_seeded = self.cfg.pre_seeded_session.clone();
        let seed_with_pty = self.cfg.seed_with_pty;
        let seed_command = self.cfg.seed_command.clone();
        // Mirror the PTY / seed-command preferences into shared state so
        // `handle_attach`'s `AttachTarget::CreateIfMissing` branch
        // (phux-k61.3) can spawn the new session's seed pane in the
        // same mode the server was configured with. We use `clone()`
        // (not `take`) on the local so the pre-seed path below still
        // gets its own copy.
        {
            let attach_create_cmd = seed_command.clone();
            state.with_mut(|s| s.set_attach_create_pty(seed_with_pty, attach_create_cmd));
        }
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
                        let cmd = seed_command
                            .unwrap_or_else(crate::terminal_actor::default_shell_command);
                        seed_session_with_pty(&state, name, cmd, &root_token)
                    } else {
                        seed_session_with_actor(&state, name, &root_token)
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
                accept_loop(&listener, state, root_token).await
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
    root_token: &CancellationToken,
) -> Result<phux_core::ids::TerminalId, crate::terminal_actor::TerminalActorError> {
    use phux_core::ids::TerminalId;
    let terminal: TerminalId = state.with_mut(|s| s.seed_session(name).2);
    // Default 80x24 — same as `phux_core::Pane::new`'s default dims.
    // Real resize wiring lands with VIEWPORT_RESIZE (phux-4hp).
    let terminal_token = root_token.child_token();
    let bundle = TerminalActor::build_with_token(80, 24, None, terminal_token.clone())?;
    let crate::terminal_actor::TerminalActorBundle {
        actor,
        handle,
        exit_notify,
        ..
    } = bundle;
    state.with_mut(|s| {
        let _ = s.spawn_terminal_actor(terminal, handle, terminal_token, actor.run());
    });
    spawn_terminal_exit_watcher(state.clone(), terminal, exit_notify);
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
    root_token: &CancellationToken,
) -> Result<phux_core::ids::TerminalId, crate::terminal_actor::TerminalActorError> {
    use phux_core::ids::TerminalId;
    let terminal: TerminalId = state.with_mut(|s| s.seed_session(name).2);
    let terminal_token = root_token.child_token();
    let bundle = TerminalActor::build_with_token(80, 24, Some(cmd), terminal_token.clone())?;
    let crate::terminal_actor::TerminalActorBundle {
        actor,
        handle,
        exit_notify,
        ..
    } = bundle;
    state.with_mut(|s| {
        let _ = s.spawn_terminal_actor(terminal, handle, terminal_token, actor.run());
    });
    spawn_terminal_exit_watcher(state.clone(), terminal, exit_notify);
    Ok(terminal)
}

/// Spawn the per-pane EOF watcher task (phux-it8).
///
/// Awaits the `TerminalActor`'s `exit_notify` oneshot. When the actor
/// observes PTY EOF (the child process has exited — typically the
/// shell typed `exit`), this watcher walks the attached-client table
/// and sends `FrameKind::Detached` to every client whose attached
/// session's currently-focused pane is the now-dead pane, then
/// detaches them server-side via [`ServerState::detach`]. Without
/// this signal the client sits in its `tokio::select!` waiting for
/// frames that never come and the user is stranded in an alt-screen
/// guard with no way out (the bug phux-it8 fixes).
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
/// the EOF branch — i.e. the pane is going away too. Detaching is
/// still the right response.
fn spawn_terminal_exit_watcher(
    state: SharedState,
    pane: phux_core::ids::TerminalId,
    exit_notify: Option<oneshot::Receiver<Option<i32>>>,
) {
    let Some(rx) = exit_notify else {
        return;
    };
    tokio::task::spawn_local(async move {
        // Recv error (sender dropped without firing) is treated the
        // same as a fired EOF with unknown exit status: in both cases
        // the pane is dead and every attached client focused on it
        // needs to be detached.
        let exit_status = rx.await.unwrap_or(None);
        // phux-4li.11: broadcast TERMINAL_CLOSED to every client that
        // has the dying pane in its subscription set before running
        // the legacy detach-on-EOF path. The two are stacked: clients
        // first learn the pane died (structured frame for L1
        // consumers); then the byc.8/it8 detach cascade fires for any
        // client whose focused pane was this one.
        broadcast_terminal_closed(&state, pane, exit_status).await;
        on_terminal_exited(&state, pane).await;
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
/// silently skipped — the downstream `on_terminal_exited` path
/// handles state cleanup.
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
    if targets.is_empty() {
        debug!(?pane, "TERMINAL_CLOSED: no subscribed clients to notify");
        return;
    }
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

/// Notify every client focused on `pane` that the session is closing,
/// then detach them. Idempotent: safe to call once per pane EOF.
///
/// We gather the doomed clients (id, outbound sender) inside one
/// `with_mut` critical section to avoid holding the state lock across
/// `await` points. Each client is then handed a `FrameKind::Detached`
/// asynchronously; on send failure (the writer task already exited)
/// we silently drop — the client is already gone.
///
/// The final `state.with_mut(|s| s.detach(id))` removes the client
/// from `attached` and clears its `pane_subscribers` entries, mirroring
/// the existing explicit-detach path in `handle_client`'s
/// `FrameKind::Detach` arm.
async fn on_terminal_exited(state: &SharedState, pane: phux_core::ids::TerminalId) {
    // Gather under-lock: which clients have this pane as their
    // currently-focused pane? See SPEC §13 — focused pane is the only
    // pane a single-pane session has, so this matches the practical
    // 1:1 case today and TODO(phux-9gw) extends to multi-pane.
    let doomed: Vec<(ClientId, tokio::sync::mpsc::Sender<Outbound>)> = state.with(|s| {
        s.attached
            .values()
            .filter_map(|client| {
                let active_pane = s.active_pane_of_session(client.session)?;
                if active_pane == pane {
                    Some((client.id, client.tx.clone()))
                } else {
                    None
                }
            })
            .collect()
    });
    if doomed.is_empty() {
        debug!(?pane, "TerminalActor EOF: no attached clients to detach");
        return;
    }
    debug!(
        ?pane,
        count = doomed.len(),
        "TerminalActor EOF: broadcasting DETACHED to attached clients",
    );
    for (client_id, tx) in doomed {
        // Best-effort: the writer task may already be gone if the
        // socket died. The subsequent detach() call covers state
        // cleanup either way.
        let _ = tx.send(Outbound::Frame(FrameKind::Detached)).await;
        state.with_mut(|s| s.detach(client_id));
    }
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
async fn accept_loop(
    listener: &UnixListener,
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
                    Ok((stream, _addr)) => {
                        debug!("client connected");
                        // Allocate the per-client routing id up-front so the
                        // task can detach itself cleanly on EOF.
                        let client_id = state.with_mut(crate::state::ServerState::new_client_id);
                        let task_state = state.clone();
                        let client_token = root_token.child_token();
                        let task_root_token = root_token.clone();
                        clients.spawn_local(async move {
                            if let Err(err) = handle_client(stream, task_state.clone(), client_id, client_token, task_root_token).await {
                                warn!(error = %err, "client task ended with error");
                            }
                            // Implicit detach on EOF / error path — matches
                            // the explicit `DETACH` semantics for the wire
                            // path that will land alongside the protocol
                            // variants.
                            task_state.with_mut(|s| s.detach(client_id));
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
/// [`Outbound`] so structured [`FrameKind`] sends and pre-encoded raw
/// byte blobs (today: PONG) share a single ordering domain.
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
async fn handle_client(
    stream: UnixStream,
    state: SharedState,
    client_id: ClientId,
    token: CancellationToken,
    root_token: CancellationToken,
) -> io::Result<()> {
    debug!(?client_id, "client task started");
    let (mut reader, writer) = stream.into_split();

    // Allocate the per-client outbound mailbox + spawn the writer task.
    // Both structured frames and pre-encoded raw byte blobs (currently
    // only PONG — see `encode_pong` and the wire-protocol comment on
    // `TYPE_PONG`) ride the same channel, tagged by the `Outbound`
    // variant. The writer task drains it with a single `recv()` loop;
    // closure of this one channel is the unambiguous signal for the
    // writer to exit.
    let (out_tx, out_rx) = tokio::sync::mpsc::channel::<Outbound>(DEFAULT_CLIENT_MAILBOX);
    // Per-client `JoinSet` for sibling tasks (today: just the writer).
    // Held in this scope so it drops with `handle_client` and the
    // writer aborts if it hasn't already exited via its own
    // close-on-EOF path. Keeps lifecycle plumbing local.
    let mut sibling_tasks: JoinSet<()> = JoinSet::new();
    sibling_tasks.spawn_local(writer_task(writer, out_rx, client_id));

    let mut header = [0u8; LENGTH_PREFIX];
    let mut payload = BytesMut::new();
    let mut framed = BytesMut::new();

    // Per-connection cache of the most-recently-advertised
    // [`ColorSupport`] (SPEC §6.2). HELLO populates this; ATTACH consumes
    // it when constructing the `AttachedClient`. Pre-HELLO it defaults to
    // [`ColorSupport::default`] (most-permissive) so a client that skips
    // HELLO (out of spec, but tolerated for forward-compat) still
    // attaches with sensible bytes-on-wire behavior.
    let mut negotiated_color_support = phux_protocol::caps::ColorSupport::default();

    loop {
        // Read the length prefix. EOF cleanly ends the session; a partial read
        // is treated as a malformed frame and also ends the session.
        // Cancellation token wins via biased select so a server-wide
        // shutdown can preempt a slow client read.
        let read_result = tokio::select! {
            biased;
            () = token.cancelled() => {
                debug!(?client_id, "client task cancelled by root token");
                return Ok(());
            }
            res = reader.read_exact(&mut header) => res,
        };
        match read_result {
            Ok(_) => {}
            Err(err) if err.kind() == io::ErrorKind::UnexpectedEof => {
                debug!("client disconnected (eof)");
                return Ok(());
            }
            Err(err) => {
                debug!(error = %err, "client read error on length prefix");
                return Ok(());
            }
        }
        let body_len = u32::from_be_bytes(header);
        if !(1..=MAX_FRAME_LEN).contains(&body_len) {
            warn!(body_len, "client sent oversized/empty frame; closing");
            return Ok(());
        }
        let body_len_usize = body_len as usize;

        payload.clear();
        payload.resize(body_len_usize, 0);
        if let Err(err) = reader.read_exact(&mut payload).await {
            debug!(error = %err, "client read error on body");
            return Ok(());
        }

        // Reassemble the wire frame so we can feed the existing decoder.
        framed.clear();
        framed.extend_from_slice(&header);
        framed.extend_from_slice(&payload);

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
                negotiated_color_support = client_caps.color_support;
                state.with_mut(|s| {
                    s.set_client_color_support(client_id, client_caps.color_support);
                    // SPEC §6.2: cache the negotiated layer set. The L3
                    // dispatch arms (METADATA_*) gate emission of
                    // `METADATA_CHANGED` on `client_speaks_l3` so non-L3
                    // consumers never see L3 frames (SPEC §16.4).
                    s.set_client_layers(client_id, client_caps.layers);
                });
                // SPEC §6.1: server replies with HELLO_OK. The
                // `HELLO_OK` `FrameKind` variant is not yet populated
                // (reserved type byte `0x80`); sibling work lifts it
                // in. Today the client proceeds optimistically without
                // waiting for HELLO_OK — see `phux-client::attach::driver::handshake`.
            }
            FrameKind::Ping { nonce } => {
                // PONG isn't a `FrameKind` variant yet (the type byte
                // `0xFF` is reserved). Ship the pre-encoded bytes
                // through the unified outbound channel as
                // `Outbound::Raw`. Once the protocol crate lifts
                // `Pong` into the enum, this collapses to a structured
                // `Outbound::Frame` send.
                debug!(nonce, "PING -> PONG");
                let mut buf = BytesMut::new();
                encode_pong(nonce, &mut buf);
                if out_tx.send(Outbound::Raw(buf)).await.is_err() {
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
                    negotiated_color_support,
                    &root_token,
                )
                .await;
            }
            FrameKind::Detach => {
                debug!(?client_id, "DETACH");
                // SPEC §7.3: server responds with DETACHED, then closes.
                // For byc.8 we emit DETACHED and let the read loop
                // continue — actual transport close lands when the
                // client drops, which is the path the existing
                // socket-lifecycle tests exercise.
                // Intentionally silent on send failure: we are about
                // to `detach()` this client on the next line, so the
                // writer being gone is the next thing to happen
                // anyway. Logging here would be pure noise.
                let _ = out_tx.send(Outbound::Frame(FrameKind::Detached)).await;
                state.with_mut(|s| s.detach(client_id));
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
            other => {
                debug!(kind = ?other, "unhandled message type (INPUT_* / etc.)");
            }
        }
    }
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

/// Writer task: drain the per-client outbound channel and write each
/// message to the socket. Encodes [`Outbound::Frame`] via
/// `FrameKind::encode`; [`Outbound::Raw`] pre-encoded byte blobs (PONG
/// today) go straight to the wire.
///
/// Exits when the channel closes — i.e. the client task drops its
/// sender. The unified `Outbound` enum collapses what used to be two
/// channels (one for structured frames, one for raw blobs) into a
/// single ordering domain, so a single `recv()` loop suffices.
async fn writer_task(
    mut writer: tokio::net::unix::OwnedWriteHalf,
    mut rx: tokio::sync::mpsc::Receiver<Outbound>,
    client_id: ClientId,
) {
    let mut buf = BytesMut::with_capacity(1024);
    while let Some(msg) = rx.recv().await {
        match msg {
            Outbound::Frame(frame) => {
                buf.clear();
                frame.encode(&mut buf);
                if let Err(err) = writer.write_all(&buf).await {
                    debug!(?client_id, error = %err, "writer error on frame; client task ending");
                    return;
                }
            }
            Outbound::Raw(bytes) => {
                if let Err(err) = writer.write_all(&bytes).await {
                    debug!(?client_id, error = %err, "writer error on raw; client task ending");
                    return;
                }
            }
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
    Vec<(
        phux_core::ids::TerminalId,
        crate::terminal_actor::TerminalHandle,
        phux_protocol::ids::TerminalId,
    )>,
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
    let (with_pty, override_cmd) =
        state.with(|s| (s.attach_create_seeds_pty(), s.attach_create_seed_command()));

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
        let cmd = override_cmd.unwrap_or_else(|| match command {
            Some(argv) if !argv.is_empty() => {
                let mut head = argv.into_iter();
                // Safe: argv is non-empty here.
                let program = head.next().unwrap_or_default();
                let mut builder = portable_pty::CommandBuilder::new(program);
                for arg in head {
                    builder.arg(arg);
                }
                builder.env("TERM", "xterm-256color");
                builder
            }
            _ => crate::terminal_actor::default_shell_command(),
        });
        seed_session_with_pty(state, &name, cmd, root_token)
    } else {
        // No-PTY path: the wire `command` is meaningless without a
        // child to exec it on. We still create the session+pane so
        // the snapshot path has a target — this is the shape every
        // existing `spawn_server` test uses.
        seed_session_with_actor(state, &name, root_token)
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
/// A fresh session is created to host the pane. v0.1 sessions are 1:1
/// with panes in practice (the multi-pane lifecycle work tracked under
/// phux-9gw lifts this); when L2 Collection wire frames ship, the
/// per-spawn session wrapper can collapse into a real Collection-scoped
/// container without rewriting this handler.
#[allow(
    clippy::too_many_arguments,
    reason = "1:1 with the SPAWN_TERMINAL wire frame (request_id + collection + command + cwd + env) plus the standard SharedState/client_id/out_tx/root_token threading the rest of this file uses"
)]
#[allow(
    clippy::too_many_lines,
    reason = "linear orchestration: validate collection → build CommandBuilder from wire frame → synthesize session name → spawn PTY-backed actor → auto-subscribe spawning client + spawn output pump → reply on the wire. Each step is small; splitting them scatters the SPAWN_TERMINAL contract without simplifying the logic."
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
            // Match `default_shell_command`'s baseline so terminfo
            // resolution doesn't silently degrade for explicit-command
            // spawns. Callers that want a different TERM can override
            // it via `env`.
            b.env("TERM", "xterm-256color");
            b
        }
        _ => crate::terminal_actor::default_shell_command(),
    };
    if let Some(path) = cwd {
        builder.cwd(path);
    }
    if let Some(pairs) = env {
        for (k, v) in pairs {
            builder.env(k, v);
        }
    }

    // Synthesize a per-spawn session name. The registry rejects nothing
    // about duplicate names (the lookup is by id, not name) but a
    // distinguishable name eases debugging and keeps the snapshot path's
    // by-name lookups deterministic. The wire `TerminalId` is what the
    // client correlates against, not this name.
    let session_name = state.with(|s| {
        let existing: std::collections::HashSet<String> = s
            .registry
            .sessions()
            .map(|(_, sess)| sess.name.clone())
            .collect();
        let mut idx: u32 = 1;
        loop {
            let candidate = format!("spawn-{idx}");
            if !existing.contains(&candidate) {
                return candidate;
            }
            idx = idx.saturating_add(1);
        }
    });

    let core_terminal_id = match seed_session_with_pty(state, &session_name, builder, root_token) {
        Ok(id) => id,
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
    )> = state.with_mut(|s| {
        let wire_terminal_id = s.intern_terminal_wire(core_terminal_id);
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
            .map(|h| (wire_terminal_id, h))
    });

    if let Some((wire_terminal_id, handle)) = wire_and_handle {
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
                        if pump_out_tx
                            .send(Outbound::Frame(FrameKind::TerminalOutput {
                                terminal_id: pump_wire_terminal_id.clone(),
                                seq,
                                bytes: bytes.to_vec(),
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
                result: SpawnResult::Ok(wire_terminal_id),
            }))
            .await;
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
        match handle.resize.try_send((cols, rows)) {
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
    color_support: phux_protocol::caps::ColorSupport,
) -> Result<AttachPrepared, crate::state::AttachError> {
    state.with_mut(|s| {
        let sid = s.attach(client_id, session_name, out_tx.clone(), color_support)?;
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
        let Some(session) = s.registry.session(sid).cloned() else {
            // Defensive: attach said yes but the session vanished.
            return Err(crate::state::AttachError::UnknownSession(
                session_name.to_owned(),
            ));
        };
        let mut panes_to_snapshot: Vec<(
            phux_core::ids::TerminalId,
            crate::terminal_actor::TerminalHandle,
            phux_protocol::ids::TerminalId,
        )> = Vec::new();
        for wid in &session.windows {
            let Some(window) = s.registry.window(*wid).cloned() else {
                continue;
            };
            for pid in &window.panes {
                if let Some(handle) = s.terminal_handle(*pid).cloned() {
                    let wire = s.intern_terminal_wire(*pid);
                    panes_to_snapshot.push((*pid, handle, wire));
                }
            }
        }
        let initial_client_id =
            phux_protocol::ids::ClientId::new(u32::try_from(client_id.0).unwrap_or(u32::MAX));
        Ok((snapshot, initial_client_id, panes_to_snapshot))
    })
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
async fn handle_attach(
    state: &SharedState,
    client_id: ClientId,
    target: AttachTarget,
    viewport: phux_protocol::wire::frame::ViewportInfo,
    _request_scrollback: bool,
    _scrollback_limit_lines: u32,
    out_tx: &tokio::sync::mpsc::Sender<Outbound>,
    color_support: phux_protocol::caps::ColorSupport,
    root_token: &CancellationToken,
) {
    let Some(session_name) = resolve_attach_target(state, target, out_tx, root_token).await else {
        return;
    };

    let (snapshot, initial_client_id, panes_to_snapshot) =
        match prepare_attach(state, client_id, &session_name, out_tx, color_support) {
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
    let mut pending: FuturesUnordered<_> = FuturesUnordered::new();
    for (terminal_id, handle, wire_terminal_id) in panes_to_snapshot {
        // Subscribe to live PTY output BEFORE requesting the snapshot.
        // Subscribing first means anything the TerminalActor broadcasts
        // after this point lands in our receiver; we then ask for a
        // snapshot so the client has a complete starting picture, and
        // any subsequent TerminalOutput we forward is "post-snapshot
        // delta" rather than racing against it.
        let mut output_rx = handle.output.subscribe();
        let pump_out_tx = out_tx.clone();
        let pump_wire_terminal_id = wire_terminal_id.clone();
        tokio::task::spawn_local(async move {
            let mut seq: u64 = 0;
            loop {
                match output_rx.recv().await {
                    Ok(bytes) => {
                        seq = seq.wrapping_add(1);
                        if pump_out_tx
                            .send(Outbound::Frame(FrameKind::TerminalOutput {
                                terminal_id: pump_wire_terminal_id.clone(),
                                seq,
                                bytes: bytes.to_vec(),
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
    panes_to_snapshot: &[(
        phux_core::ids::TerminalId,
        crate::terminal_actor::TerminalHandle,
        phux_protocol::ids::TerminalId,
    )],
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
        for (terminal_id, handle, _wire_terminal_id) in panes_to_snapshot {
            if let Some(pane) = s.registry.terminal_mut(*terminal_id) {
                pane.dims = (cols, rows);
            }
            match handle.resize.try_send((cols, rows)) {
                Ok(()) => {}
                Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                    warn!(
                        ?terminal_id,
                        cols,
                        rows,
                        "ATTACH viewport apply: pane resize mailbox full; dropping (next VIEWPORT_RESIZE will retry)",
                    );
                }
                Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                    debug!(
                        ?terminal_id,
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
            match handle.resize.try_send((viewport.cols, viewport.rows)) {
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

/// Encode a `PONG { nonce }` frame directly, since `phux-protocol`'s
/// `FrameKind` doesn't yet have a `Pong` variant (per the catalog comments in
/// `wire/frame.rs`, the type byte `0xFF` is reserved). This stays local to
/// the server until the protocol crate lifts it into a variant; see ADR-0008.
fn encode_pong(nonce: u64, out: &mut BytesMut) {
    // Body = type byte (1) + u64 nonce (8) = 9 bytes.
    let body_len: u32 = 9;
    out.extend_from_slice(&body_len.to_be_bytes());
    out.extend_from_slice(&[TYPE_PONG]);
    out.extend_from_slice(&nonce.to_be_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pong_frame_has_correct_length_prefix_and_type_byte() {
        let mut buf = BytesMut::new();
        encode_pong(0xDEAD_BEEF_CAFE_BABE, &mut buf);
        // length prefix (4) + type (1) + nonce (8) = 13 bytes
        assert_eq!(buf.len(), 13);
        assert_eq!(&buf[0..4], &9u32.to_be_bytes());
        assert_eq!(buf[4], TYPE_PONG);
        assert_eq!(&buf[5..13], &0xDEAD_BEEF_CAFE_BABE_u64.to_be_bytes());
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
        let (output_tx, _output_rx_seed) = broadcast::channel::<Bytes>(8);
        let (resize_tx, mut resize_rx) = mpsc::channel::<(u16, u16)>(8);
        let (consumer_attach_tx, _consumer_attach_rx) = mpsc::channel(8);
        let (consumer_detach_tx, _consumer_detach_rx) = mpsc::channel(8);
        let (consumer_ack_tx, _consumer_ack_rx) = mpsc::channel(8);
        let handle = TerminalHandle {
            input: input_tx,
            snapshot: snapshot_tx,
            output: output_tx,
            resize: resize_tx,
            consumer_attach: consumer_attach_tx,
            consumer_detach: consumer_detach_tx,
            consumer_ack: consumer_ack_tx,
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
        // already carry exactly one (cols, rows) tuple.
        let observed = resize_rx
            .try_recv()
            .expect("resize tuple must be queued on the channel");
        assert_eq!(
            observed,
            (132, 50),
            "TerminalHandle::resize must receive the new viewport dims",
        );
        assert!(
            resize_rx.try_recv().is_err(),
            "exactly one resize tuple should be queued — got more",
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
                    let (output_tx, _output_rx_seed) = broadcast::channel::<Bytes>(8);
                    let (resize_tx, _resize_rx) = mpsc::channel::<(u16, u16)>(8);
                    let (consumer_attach_tx, _consumer_attach_rx) = mpsc::channel(8);
                    let (consumer_detach_tx, _consumer_detach_rx) = mpsc::channel(8);
                    let (consumer_ack_tx, _consumer_ack_rx) = mpsc::channel(8);
                    let handle = TerminalHandle {
                        input: input_tx,
                        snapshot: snapshot_tx,
                        output: output_tx,
                        resize: resize_tx,
                        consumer_attach: consumer_attach_tx,
                        consumer_detach: consumer_detach_tx,
                        consumer_ack: consumer_ack_tx,
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
                    handle_attach(
                        &state_for_task,
                        client_id,
                        AttachTarget::ByName("multi".to_owned()),
                        ViewportInfo::new(80, 24),
                        false,
                        0,
                        &out_tx,
                        phux_protocol::caps::ColorSupport::default(),
                        &test_root_token,
                    )
                    .await;
                });

                // First the writer should see ATTACHED.
                let attached = tokio::time::timeout(Duration::from_secs(2), out_rx.recv())
                    .await
                    .expect("attached frame did not arrive")
                    .expect("out_rx closed before attached");
                match attached {
                    Outbound::Frame(FrameKind::Attached { .. }) => {}
                    other => panic!("expected Attached, got {other:?}"),
                }

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
                            panic!("snapshot request {i} never arrived — sequential loop?",)
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
