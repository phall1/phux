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
//!   and handles `ATTACH` / `DETACH` by talking to the per-pane
//!   [`PaneActor`](crate::pane_actor::PaneActor)s (`phux-byc.8`). The
//!   remaining catalog (`INPUT_KEY`, etc.) is recorded against the
//!   pane's input log but the PTY write side lands in `phux-byc.5`.
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
use phux_protocol::wire::frame::{
    AttachTarget, ErrorCode, FrameKind, MAX_FRAME_LEN, TYPE_PONG, ViewportInfo,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};
use tokio::runtime::Builder;
use tokio::sync::oneshot;
use tokio::task::{JoinSet, LocalSet};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, trace, warn};

use crate::pane_actor::{PaneActor, PaneHandle, SnapshotRequest};
use crate::state::{ClientId, DEFAULT_CLIENT_MAILBOX, Outbound, PaneInput, SharedState};

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
    /// PTY (see [`seed_session_with_pty`] / [`crate::pane_actor::PaneActor::new_with_default_shell`]).
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
    /// falls back to [`crate::pane_actor::default_shell_command`].
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
        let local = LocalSet::new();
        // Hierarchical cancellation: a single root token is the parent
        // of every per-client / per-pane child. The external `shutdown`
        // future is folded into this token by a small task spawned on
        // the LocalSet (see below). On `root_token.cancel()`:
        //   * `accept_loop` returns from its select! → its per-client
        //     `JoinSet` drops → in-flight client tasks abort.
        //   * Every `PaneActor`'s child token fires → actors exit
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
                // `Send` futures — exactly what `PaneActor` is not.
                if let Some(name) = pre_seeded.as_deref() {
                    let seeded = if seed_with_pty {
                        let cmd =
                            seed_command.unwrap_or_else(crate::pane_actor::default_shell_command);
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

/// Seed `(session, window, pane)` and spawn a **no-PTY** `PaneActor`
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
) -> Result<phux_core::ids::PaneId, crate::pane_actor::PaneActorError> {
    use phux_core::ids::PaneId;
    let pane: PaneId = state.with_mut(|s| s.seed_session(name).2);
    // Default 80x24 — same as `phux_core::Pane::new`'s default dims.
    // Real resize wiring lands with VIEWPORT_RESIZE (phux-4hp).
    let pane_token = root_token.child_token();
    let bundle = PaneActor::build_with_token(80, 24, None, pane_token.clone())?;
    let crate::pane_actor::PaneActorBundle {
        actor,
        handle,
        exit_notify,
        ..
    } = bundle;
    state.with_mut(|s| {
        let _ = s.spawn_pane_actor(pane, handle, pane_token, actor.run());
    });
    spawn_pane_exit_watcher(state.clone(), pane, exit_notify);
    Ok(pane)
}

/// Seed `(session, window, pane)` and spawn a **PTY-backed**
/// `PaneActor` running `cmd`. Sibling of [`seed_session_with_actor`]
/// for the real server path (`phux-byc.5`).
///
/// Call sites:
///
/// * The `phux server` binary entry point, via
///   [`ServerConfig::seed_with_pty`] (with [`ServerConfig::seed_command`]
///   left `None` to fall back to
///   [`crate::pane_actor::default_shell_command`] — the user's `$SHELL`,
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
) -> Result<phux_core::ids::PaneId, crate::pane_actor::PaneActorError> {
    use phux_core::ids::PaneId;
    let pane: PaneId = state.with_mut(|s| s.seed_session(name).2);
    let pane_token = root_token.child_token();
    let bundle = PaneActor::build_with_token(80, 24, Some(cmd), pane_token.clone())?;
    let crate::pane_actor::PaneActorBundle {
        actor,
        handle,
        exit_notify,
        ..
    } = bundle;
    state.with_mut(|s| {
        let _ = s.spawn_pane_actor(pane, handle, pane_token, actor.run());
    });
    spawn_pane_exit_watcher(state.clone(), pane, exit_notify);
    Ok(pane)
}

/// Spawn the per-pane EOF watcher task (phux-it8).
///
/// Awaits the `PaneActor`'s `exit_notify` oneshot. When the actor
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
fn spawn_pane_exit_watcher(
    state: SharedState,
    pane: phux_core::ids::PaneId,
    exit_notify: Option<oneshot::Receiver<()>>,
) {
    let Some(rx) = exit_notify else {
        return;
    };
    tokio::task::spawn_local(async move {
        // Recv error (sender dropped without firing) is treated the
        // same as a fired EOF: in both cases the pane is dead and
        // every attached client focused on it needs to be detached.
        let _ = rx.await;
        on_pane_exited(&state, pane).await;
    });
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
async fn on_pane_exited(state: &SharedState, pane: phux_core::ids::PaneId) {
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
        debug!(?pane, "PaneActor EOF: no attached clients to detach");
        return;
    }
    debug!(
        ?pane,
        count = doomed.len(),
        "PaneActor EOF: broadcasting DETACHED to attached clients",
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
                        clients.spawn_local(async move {
                            if let Err(err) = handle_client(stream, task_state.clone(), client_id, client_token).await {
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
/// [`PaneActor`](crate::pane_actor::PaneActor), and emits
/// `ATTACHED` + `PANE_SNAPSHOT` frames per SPEC §13. On unknown
/// session, emits an `ERROR` frame with `SessionNotFound` (SPEC §14).
#[allow(
    clippy::too_many_lines,
    reason = "single per-client dispatch loop; each frame arm is small and the catalog grows linearly. Extracting arms hides the wire→state seam without simplifying it."
)]
async fn handle_client(
    stream: UnixStream,
    state: SharedState,
    client_id: ClientId,
    token: CancellationToken,
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
            FrameKind::InputKey { pane_id, event } => {
                handle_pane_input(
                    &state,
                    client_id,
                    pane_id,
                    PaneInput::Key(event),
                    "INPUT_KEY",
                );
            }
            FrameKind::InputMouse { pane_id, event } => {
                handle_pane_input(
                    &state,
                    client_id,
                    pane_id,
                    PaneInput::Mouse(event),
                    "INPUT_MOUSE",
                );
            }
            FrameKind::InputFocus { pane_id, event } => {
                handle_pane_input(
                    &state,
                    client_id,
                    pane_id,
                    PaneInput::Focus(event),
                    "INPUT_FOCUS",
                );
            }
            other => {
                debug!(kind = ?other, "unhandled message type (INPUT_* / etc.)");
            }
        }
    }
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
        phux_core::ids::PaneId,
        crate::pane_actor::PaneHandle,
        phux_protocol::ids::PaneId,
    )>,
);

/// Resolve `target` to a session name. SPEC §13: `ByName` is the only
/// fully-implemented mode in byc.8; the others fail with
/// `SessionNotFound` until follow-up tickets land.
async fn resolve_attach_target(
    state: &SharedState,
    target: AttachTarget,
    out_tx: &tokio::sync::mpsc::Sender<Outbound>,
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
            // Resolve against the global per-server "last attached
            // session" slot (see ServerState::last_attached_session).
            // If a prior attach exists and that session is still live
            // in the registry, return its name; otherwise treat as
            // "not found" — matches SPEC §13's allowance that
            // "implementations without prior-attach memory MAY return
            // SESSION_NOT_FOUND". We follow the same code path when
            // the prior session has been killed since the last attach.
            //
            // TODO(error-codes): introduce ErrorCode::NoLastSession
            // (and a sibling variant for "last session killed") so
            // clients can distinguish "no history" from "history is
            // stale" without parsing the message string. Additive
            // ErrorCode work is intentionally out of scope here.
            let resolved = state.with(|s| {
                s.last_attached_session()
                    .and_then(|sid| s.registry.session(sid).map(|sess| sess.name.clone()))
            });
            if resolved.is_none() {
                send_error(
                    out_tx,
                    ErrorCode::SessionNotFound,
                    "no prior-attach memory: AttachTarget::Last has nothing to resolve",
                )
                .await;
            }
            resolved
        }
        AttachTarget::CreateIfMissing { .. } => {
            send_error(
                out_tx,
                ErrorCode::SessionNotFound,
                "AttachTarget::CreateIfMissing is not yet implemented",
            )
            .await;
            None
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
) -> Result<AttachPrepared, crate::state::AttachError> {
    state.with_mut(|s| {
        let sid = s.attach(client_id, session_name, out_tx.clone())?;
        // Record success into the global "last attached" slot before
        // we build the snapshot. The order doesn't matter for
        // correctness (we're still inside the with_mut critical
        // section), but doing it here keeps the recording adjacent to
        // the attach call that justified it — easier to reason about
        // when reading the code.
        s.set_last_attached_session(sid);
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
            phux_core::ids::PaneId,
            crate::pane_actor::PaneHandle,
            phux_protocol::ids::PaneId,
        )> = Vec::new();
        for wid in &session.windows {
            let Some(window) = s.registry.window(*wid).cloned() else {
                continue;
            };
            for pid in &window.panes {
                if let Some(handle) = s.pane_handle(*pid).cloned() {
                    let wire = s.intern_pane_wire(*pid);
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
/// `ATTACHED` + per-pane `PANE_SNAPSHOT` frames on `out_tx`.
///
/// On any failure path, emits an `ERROR` frame and returns. We never
/// partially-attach: either every frame queues or none does.
#[allow(
    clippy::too_many_lines,
    reason = "linear attach orchestration: resolve target -> prepare -> spawn per-pane output pumps -> fan out snapshot requests via FuturesUnordered -> drain; splitting it would scatter context"
)]
async fn handle_attach(
    state: &SharedState,
    client_id: ClientId,
    target: AttachTarget,
    _viewport: phux_protocol::wire::frame::ViewportInfo,
    _request_scrollback: bool,
    _scrollback_limit_lines: u32,
    out_tx: &tokio::sync::mpsc::Sender<Outbound>,
) {
    let Some(session_name) = resolve_attach_target(state, target, out_tx).await else {
        return;
    };

    let (snapshot, initial_client_id, panes_to_snapshot) =
        match prepare_attach(state, client_id, &session_name, out_tx) {
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
    for (pane_id, handle, wire_pane_id) in panes_to_snapshot {
        // Subscribe to live PTY output BEFORE requesting the snapshot.
        // Subscribing first means anything the PaneActor broadcasts
        // after this point lands in our receiver; we then ask for a
        // snapshot so the client has a complete starting picture, and
        // any subsequent PaneOutput we forward is "post-snapshot
        // delta" rather than racing against it.
        let mut output_rx = handle.output.subscribe();
        let pump_out_tx = out_tx.clone();
        let pump_wire_pane_id = wire_pane_id.get();
        tokio::task::spawn_local(async move {
            let mut seq: u64 = 0;
            loop {
                match output_rx.recv().await {
                    Ok(bytes) => {
                        seq = seq.wrapping_add(1);
                        if pump_out_tx
                            .send(Outbound::Frame(FrameKind::PaneOutput {
                                pane_id: pump_wire_pane_id,
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
                            pane_id = pump_wire_pane_id,
                            dropped = n,
                            "PaneOutput pump lagged; consider larger broadcast capacity",
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
            warn!(?pane_id, "pane actor dropped; skipping snapshot");
            continue;
        }
        // Tag each in-flight receiver with its identifiers so the drain
        // loop can warn / build a frame without re-deriving them.
        pending.push(async move { (pane_id, wire_pane_id, reply_rx.await) });
    }

    while let Some((pane_id, wire_pane_id, reply)) = pending.next().await {
        let Ok(snap) = reply else {
            warn!(?pane_id, "pane actor failed to reply with snapshot");
            continue;
        };
        if out_tx
            .send(Outbound::Frame(FrameKind::PaneSnapshot {
                pane_id: wire_pane_id,
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

/// Handle a client's `VIEWPORT_RESIZE` (SPEC §7.1 / §10.5).
///
/// Look up the client's currently-focused pane and update the in-memory
/// `dims` so future `PANE_SNAPSHOT` frames reflect the new size. This is
/// the additive surface for phux-4hp: we deliberately do NOT push a
/// resize into the [`PaneActor`] (or call `Terminal::set_size` /
/// `pty.resize(...)`) because byc.5's PTY pump owns the actor-side
/// `Terminal` / `portable-pty` resize integration. The follow-up there
/// will consume this state change (or, if it prefers a direct channel,
/// can add a new `PaneHandle` channel without touching this code).
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
        let Some(pane_id) = window.active else {
            return;
        };
        if let Some(pane) = s.registry.pane_mut(pane_id) {
            pane.dims = (viewport.cols, viewport.rows);
        }
        // Fan the resize out to the PaneActor so libghostty's
        // `Terminal::set_size` and the PTY `winsize` ioctl get
        // updated. byc.5 added the `resize` channel on `PaneHandle`;
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
        if let Some(handle) = s.panes.get(&pane_id) {
            match handle.resize.try_send((viewport.cols, viewport.rows)) {
                Ok(()) => {}
                Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                    warn!(
                        ?client_id,
                        ?pane_id,
                        cols = viewport.cols,
                        rows = viewport.rows,
                        "VIEWPORT_RESIZE: pane resize mailbox full; dropping (fire-and-forget per SPEC §10.5)",
                    );
                }
                Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                    debug!(
                        ?client_id,
                        ?pane_id,
                        "VIEWPORT_RESIZE: pane actor gone; dropping resize",
                    );
                }
            }
        } else {
            debug!(
                ?client_id,
                ?pane_id,
                "VIEWPORT_RESIZE: no PaneHandle registered for pane; dropping resize",
            );
        }
    });
}

/// Route an `INPUT_*` frame body to the target pane's [`PaneActor`].
///
/// SPEC §9: input frames are fire-and-forget — no `Outbound` reply.
/// On the wire the pane is identified by its `WirePaneId` (`u32`); we
/// resolve it back to a core [`PaneId`] via [`ServerState::pane_from_wire`],
/// then locate the [`PaneHandle`] and `try_send` the encoded
/// [`PaneInput`] onto the actor's input mailbox.
///
/// Validation: we drop with `warn!` (not `debug!`, this is observable
/// misbehavior worth surfacing) on:
///   * Unknown wire pane id (no [`PaneId`] mapping).
///   * Client not attached (the per-client task should not be reading
///     frames from a detached identity, but we re-check defensively).
///   * Client attached but not subscribed to this pane — prevents one
///     client from steering another's pane (SPEC §9 leaves multi-client
///     subscription rules to per-pane policy; for now subscription is
///     the gate).
///   * Pane has no registered [`PaneHandle`] (actor never spawned, or
///     spawned but evicted).
///
/// `try_send` is used because we hold the `with_mut` lock while routing:
/// awaiting inside a `with_mut` would deadlock the single-threaded
/// runtime, and an unbounded queue would let a slow PTY producer push
/// memory through the roof. `Full` is treated as a backpressure event
/// (warn-drop); `Closed` is logged at debug and dropped (actor gone).
fn handle_pane_input(
    state: &SharedState,
    client_id: ClientId,
    wire_pane_id: u32,
    input: PaneInput,
    frame_label: &'static str,
) {
    use phux_protocol::ids::PaneId as WirePaneId;
    state.with_mut(|s| {
        let wire = WirePaneId(wire_pane_id);
        let Some(pane) = s.pane_from_wire(wire) else {
            warn!(
                ?client_id,
                wire_pane_id, frame_label, "input frame for unknown pane; dropping",
            );
            return;
        };
        let Some(attached) = s.attached.get(&client_id) else {
            warn!(
                ?client_id,
                wire_pane_id, frame_label, "input frame from non-attached client; dropping",
            );
            return;
        };
        // Subscription gate: the pane must be one the client is observing.
        // For byc.8's "active pane only" subscription model this is the
        // same as "is the pane in the client's attached session"; a
        // richer SUBSCRIBE story (SPEC §7.4) will refine this without
        // changing the dispatch shape.
        let session = attached.session;
        let is_subscribed = s.subscribers_for_pane(pane).contains(&client_id);
        if !is_subscribed {
            warn!(
                ?client_id,
                wire_pane_id,
                ?session,
                frame_label,
                "client not subscribed to pane; dropping input",
            );
            return;
        }
        let Some(handle): Option<&PaneHandle> = s.pane_handle(pane) else {
            warn!(
                ?client_id,
                wire_pane_id, frame_label, "no PaneHandle for pane; dropping input",
            );
            return;
        };
        match handle.input.try_send(input) {
            Ok(()) => {
                trace!(
                    ?client_id,
                    wire_pane_id, frame_label, "input routed to PaneActor"
                );
            }
            Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                warn!(
                    ?client_id,
                    wire_pane_id,
                    frame_label,
                    "pane input mailbox full; dropping (fire-and-forget per SPEC §9)",
                );
            }
            Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                debug!(
                    ?client_id,
                    wire_pane_id, frame_label, "pane actor gone; dropping input",
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
        use phux_core::ids::PaneId as CorePaneId;

        let state = SharedState::new();
        // Seed a session with a pane, then attach a client. Mirrors what
        // `seed_session_with_actor` does on the real path, minus the
        // PaneActor spawn (we're not exercising the actor here — just
        // the state-side dim update).
        let (sid, _wid, pid): (_, _, CorePaneId) =
            state.with_mut(|s| s.seed_session("test-session"));
        let client_id = state.with_mut(crate::state::ServerState::new_client_id);
        let (tx, _rx) = tokio::sync::mpsc::channel(8);
        state
            .with_mut(|s| s.attach(client_id, "test-session", tx))
            .expect("attach");

        // Sanity: starts at 80x24 (default core::Pane::dims).
        let before = state
            .with(|s| s.registry.pane(pid).map(|p| p.dims))
            .expect("pane exists");
        assert_eq!(before, (80, 24));

        let viewport = ViewportInfo::new(132, 50).with_pixels(Some(1320), Some(750));
        handle_viewport_resize(&state, client_id, &viewport);

        let after = state
            .with(|s| s.registry.pane(pid).map(|p| p.dims))
            .expect("pane exists");
        assert_eq!(after, (132, 50));

        // Sanity: the session linkage didn't get clobbered.
        let attached_session = state.with(|s| s.attached.get(&client_id).map(|c| c.session));
        assert_eq!(attached_session, Some(sid));
    }

    /// `VIEWPORT_RESIZE` fans the new (cols, rows) tuple onto the
    /// `PaneHandle::resize` channel byc.5 added. We inject a hand-
    /// built `PaneHandle` (no real actor) so the test can observe the
    /// receiver side directly — this pins the wire from
    /// `handle_viewport_resize` into the actor without needing to
    /// stand up libghostty or a PTY pair.
    #[test]
    fn viewport_resize_sends_to_pane_actor_resize_channel() {
        use crate::pane_actor::PaneHandle;
        use bytes::Bytes;
        use phux_core::ids::PaneId as CorePaneId;
        use tokio::sync::{broadcast, mpsc};

        let state = SharedState::new();
        let (_sid, _wid, pid): (_, _, CorePaneId) =
            state.with_mut(|s| s.seed_session("test-session"));

        // Build a `PaneHandle` directly. The actor side is not running;
        // we only care that `handle.resize.try_send` lands. The other
        // channels exist purely to satisfy the struct shape.
        let (input_tx, _input_rx) = mpsc::channel(8);
        let (snapshot_tx, _snapshot_rx) = mpsc::channel(8);
        let (output_tx, _output_rx_seed) = broadcast::channel::<Bytes>(8);
        let (resize_tx, mut resize_rx) = mpsc::channel::<(u16, u16)>(8);
        let handle = PaneHandle {
            input: input_tx,
            snapshot: snapshot_tx,
            output: output_tx,
            resize: resize_tx,
            cols: 80,
            rows: 24,
        };
        state.with_mut(|s| {
            // `register_pane_handle` wants a CancellationToken; build
            // a fresh one. We don't keep a clone — no actor is running
            // for this test, so cancellation is moot.
            let token = CancellationToken::new();
            let _ = s.register_pane_handle(pid, handle, token);
        });

        let client_id = state.with_mut(crate::state::ServerState::new_client_id);
        let (tx, _rx) = tokio::sync::mpsc::channel(8);
        state
            .with_mut(|s| s.attach(client_id, "test-session", tx))
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
            "PaneHandle::resize must receive the new viewport dims",
        );
        assert!(
            resize_rx.try_recv().is_err(),
            "exactly one resize tuple should be queued — got more",
        );
    }

    /// Concurrency proof for the ATTACH per-pane snapshot fan-out.
    ///
    /// Builds N hand-crafted `PaneHandle`s (no real `PaneActor`) whose
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
        use phux_core::ids::PaneId as CorePaneId;
        use tokio::sync::{broadcast, mpsc, oneshot};
        use tokio::task::LocalSet;

        use crate::grid::SnapshotBytes;
        use crate::pane_actor::{PaneHandle, SnapshotRequest};

        const N: usize = 4;

        let local = LocalSet::new();
        local
            .run_until(async {
                let state = SharedState::new();
                // Seed one session with one window and N panes.
                let (sid, wid, _first_pane) = state.with_mut(|s| s.seed_session("multi"));
                // `seed_session` made one pane already; we want N total.
                let mut pane_ids: Vec<CorePaneId> = Vec::with_capacity(N);
                state.with_mut(|s| {
                    let session = s.registry.session(sid).cloned().expect("session");
                    let window = s
                        .registry
                        .window(session.windows[0])
                        .cloned()
                        .expect("window");
                    pane_ids.push(window.panes[0]);
                    for _ in 1..N {
                        let pid = s.registry.new_pane(wid).expect("new_pane");
                        pane_ids.push(pid);
                    }
                });

                // Build N PaneHandles; keep the snapshot receivers in the test.
                let mut snapshot_rxs: Vec<mpsc::Receiver<SnapshotRequest>> = Vec::with_capacity(N);
                for &pid in &pane_ids {
                    let (input_tx, _input_rx) = mpsc::channel(8);
                    let (snapshot_tx, snapshot_rx) = mpsc::channel(8);
                    let (output_tx, _output_rx_seed) = broadcast::channel::<Bytes>(8);
                    let (resize_tx, _resize_rx) = mpsc::channel::<(u16, u16)>(8);
                    let handle = PaneHandle {
                        input: input_tx,
                        snapshot: snapshot_tx,
                        output: output_tx,
                        resize: resize_tx,
                        cols: 80,
                        rows: 24,
                    };
                    state.with_mut(|s| {
                        let _ = s.register_pane_handle(pid, handle, CancellationToken::new());
                    });
                    snapshot_rxs.push(snapshot_rx);
                }

                // Outbound channel for the would-be writer task; we read
                // PANE_SNAPSHOT frames out of `out_rx` to verify all N
                // shipped.
                let (out_tx, mut out_rx) =
                    mpsc::channel::<Outbound>(crate::state::DEFAULT_CLIENT_MAILBOX);
                let client_id = state.with_mut(crate::state::ServerState::new_client_id);

                // Spawn `handle_attach` on the LocalSet so the test
                // body can interleave with it.
                let state_for_task = state.clone();
                let attach_task = tokio::task::spawn_local(async move {
                    handle_attach(
                        &state_for_task,
                        client_id,
                        AttachTarget::ByName("multi".to_owned()),
                        ViewportInfo::new(80, 24),
                        false,
                        0,
                        &out_tx,
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

                // Drain N PANE_SNAPSHOT frames out of the writer channel.
                let mut snaps_seen = 0usize;
                for _ in 0..N {
                    let frame = tokio::time::timeout(Duration::from_secs(2), out_rx.recv())
                        .await
                        .expect("pane snapshot frame did not arrive")
                        .expect("out_rx closed before snapshot");
                    if matches!(frame, Outbound::Frame(FrameKind::PaneSnapshot { .. })) {
                        snaps_seen += 1;
                    } else {
                        panic!("expected PaneSnapshot, got {frame:?}");
                    }
                }
                assert_eq!(snaps_seen, N, "expected one PANE_SNAPSHOT per pane");

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
            .with(|s| s.registry.pane(pid).map(|p| p.dims))
            .expect("pane exists");
        handle_viewport_resize(&state, bogus_client, &ViewportInfo::new(200, 60));
        let after = state
            .with(|s| s.registry.pane(pid).map(|p| p.dims))
            .expect("pane exists");
        assert_eq!(before, after, "no mutation expected for unattached client");
    }
}
