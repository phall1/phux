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
use phux_protocol::wire::frame::{
    AttachTarget, ErrorCode, FrameKind, MAX_FRAME_LEN, TYPE_PONG, ViewportInfo,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};
use tokio::runtime::Builder;
use tokio::sync::oneshot;
use tokio::task::LocalSet;
use tracing::{debug, error, info, warn};

use crate::pane_actor::{PaneActor, SnapshotRequest};
use crate::state::{ClientId, DEFAULT_CLIENT_MAILBOX, OutboundFrame, SharedState};

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
}

impl ServerConfig {
    /// Build a config with `socket_path` resolved via [`default_socket_path`]
    /// and no pre-seeded session.
    #[must_use]
    pub fn with_default_socket() -> Self {
        Self {
            socket_path: default_socket_path(),
            pre_seeded_session: None,
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
        let local = LocalSet::new();
        let result = local
            .run_until(async move {
                // Pre-seed inside the LocalSet so we can `spawn_local`
                // the pane actor. Without this, the pre-seed path
                // would have to call `tokio::spawn`, which requires
                // `Send` futures — exactly what `PaneActor` is not.
                if let Some(name) = pre_seeded.as_deref() {
                    if let Err(err) = seed_session_with_actor(&state, name) {
                        warn!(
                            session = name,
                            error = %err,
                            "failed to spawn pane actor for pre-seeded session",
                        );
                    } else {
                        debug!(session = name, "pre-seeded session in registry");
                    }
                }
                accept_loop(&listener, state, shutdown).await
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
) -> Result<phux_core::ids::PaneId, crate::pane_actor::PaneActorError> {
    use phux_core::ids::PaneId;
    let pane: PaneId = state.with_mut(|s| s.seed_session(name).2);
    // Default 80x24 — same as `phux_core::Pane::new`'s default dims.
    // Real resize wiring lands with VIEWPORT_RESIZE (phux-4hp).
    let bundle = PaneActor::new(80, 24)?;
    let crate::pane_actor::PaneActorBundle {
        actor,
        handle,
        shutdown,
    } = bundle;
    tokio::task::spawn_local(actor.run());
    state.with_mut(|s| {
        let _ = s.register_pane_handle(pane, handle, shutdown);
    });
    Ok(pane)
}

/// Seed `(session, window, pane)` and spawn a **PTY-backed**
/// `PaneActor` running `cmd`. Sibling of [`seed_session_with_actor`]
/// for the real server path (`phux-byc.5`).
///
/// Eventual call site: the binary entry point that takes a user
/// session's `$SHELL` (or a configured command) and creates the
/// initial pane. Today the binary doesn't exist; this helper exists
/// so the eventual wire-up is a single function call.
#[allow(
    dead_code,
    reason = "phux-byc.5 lands the actor side; the call site lands with the binary"
)]
pub(crate) fn seed_session_with_pty(
    state: &SharedState,
    name: &str,
    cmd: portable_pty::CommandBuilder,
) -> Result<phux_core::ids::PaneId, crate::pane_actor::PaneActorError> {
    use phux_core::ids::PaneId;
    let pane: PaneId = state.with_mut(|s| s.seed_session(name).2);
    let bundle = PaneActor::new_with_command(cmd, 80, 24)?;
    let crate::pane_actor::PaneActorBundle {
        actor,
        handle,
        shutdown,
    } = bundle;
    tokio::task::spawn_local(actor.run());
    state.with_mut(|s| {
        let _ = s.register_pane_handle(pane, handle, shutdown);
    });
    Ok(pane)
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
/// [`tokio::task::spawn_local`]; the futures we hand it are `!Send`
/// because they call into pane actors that own `!Send` `Terminal`s.
async fn accept_loop<F>(
    listener: &UnixListener,
    state: SharedState,
    shutdown: F,
) -> Result<(), ServerError>
where
    F: Future<Output = ()> + Send + 'static,
{
    tokio::pin!(shutdown);
    loop {
        tokio::select! {
            () = &mut shutdown => {
                info!("shutdown signal received");
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
                        tokio::task::spawn_local(async move {
                            if let Err(err) = handle_client(stream, task_state.clone(), client_id).await {
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
/// Outbound frames are routed through a per-client `mpsc` channel
/// drained by a sibling writer task (also `spawn_local`'d). This gives
/// us one place to back-pressure on slow clients without entangling
/// the read side, and matches the `tx: mpsc::Sender<OutboundFrame>`
/// shape `ServerState::attach` already wants.
///
/// `phux-byc.8`: implements the ATTACH path. Resolves the target,
/// builds a [`SessionSnapshot`](phux_protocol::wire::info::SessionSnapshot)
/// from the registry, requests a snapshot from each pane's
/// [`PaneActor`](crate::pane_actor::PaneActor), and emits
/// `ATTACHED` + `PANE_SNAPSHOT` frames per SPEC §13. On unknown
/// session, emits an `ERROR` frame with `SessionNotFound` (SPEC §14).
async fn handle_client(
    stream: UnixStream,
    state: SharedState,
    client_id: ClientId,
) -> io::Result<()> {
    debug!(?client_id, "client task started");
    let (mut reader, writer) = stream.into_split();

    // Allocate the per-client outbound mailbox + spawn the writer task.
    // Structured frames flow through `out_tx`; pre-encoded raw byte
    // blobs (currently only PONG — see `encode_pong` and the wire-
    // protocol comment on `TYPE_PONG`) flow through `raw_tx`. Both
    // channels feed the same writer task; the writer drains them
    // fairly via `select!`.
    let (out_tx, out_rx) = tokio::sync::mpsc::channel::<OutboundFrame>(DEFAULT_CLIENT_MAILBOX);
    let (raw_tx, raw_rx) = tokio::sync::mpsc::channel::<BytesMut>(DEFAULT_CLIENT_MAILBOX);
    tokio::task::spawn_local(writer_task(writer, out_rx, raw_rx, client_id));

    let mut header = [0u8; LENGTH_PREFIX];
    let mut payload = BytesMut::new();
    let mut framed = BytesMut::new();

    loop {
        // Read the length prefix. EOF cleanly ends the session; a partial read
        // is treated as a malformed frame and also ends the session.
        match reader.read_exact(&mut header).await {
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
                // through the writer's `raw_tx` side channel. Once
                // the protocol crate lifts `Pong` into the enum, this
                // collapses to a structured send through `out_tx`.
                debug!(nonce, "PING -> PONG");
                let mut buf = BytesMut::new();
                encode_pong(nonce, &mut buf);
                let _ = raw_tx.send(buf).await;
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
                let _ = out_tx.send(FrameKind::Detached).await;
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
            other => {
                debug!(kind = ?other, "unhandled message type (INPUT_* / etc.)");
            }
        }
    }
}

/// Writer task: drain the per-client outbound channels and write each
/// frame to the socket. Encodes [`FrameKind`] frames via
/// `FrameKind::encode`; pre-encoded raw byte blobs (PONG today) go
/// straight to the wire.
///
/// Exits when **both** channels close — i.e. the client task drops its
/// senders. Either channel closing alone is not a shutdown signal:
/// `select!` with `.recv()` returns `None` on closure, and we ignore
/// `None` from one branch as long as the other stays open.
async fn writer_task(
    mut writer: tokio::net::unix::OwnedWriteHalf,
    mut frame_rx: tokio::sync::mpsc::Receiver<OutboundFrame>,
    mut raw_rx: tokio::sync::mpsc::Receiver<BytesMut>,
    client_id: ClientId,
) {
    let mut buf = BytesMut::with_capacity(1024);
    let mut frame_open = true;
    let mut raw_open = true;
    while frame_open || raw_open {
        tokio::select! {
            biased;
            maybe_frame = frame_rx.recv(), if frame_open => match maybe_frame {
                Some(frame) => {
                    buf.clear();
                    frame.encode(&mut buf);
                    if let Err(err) = writer.write_all(&buf).await {
                        debug!(?client_id, error = %err, "writer error on frame; client task ending");
                        return;
                    }
                }
                None => frame_open = false,
            },
            maybe_raw = raw_rx.recv(), if raw_open => match maybe_raw {
                Some(bytes) => {
                    if let Err(err) = writer.write_all(&bytes).await {
                        debug!(?client_id, error = %err, "writer error on raw; client task ending");
                        return;
                    }
                }
                None => raw_open = false,
            },
        }
    }
    debug!(?client_id, "writer task exiting (both channels closed)");
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
    out_tx: &tokio::sync::mpsc::Sender<OutboundFrame>,
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
            send_error(
                out_tx,
                ErrorCode::SessionNotFound,
                "AttachTarget::Last is not yet implemented",
            )
            .await;
            None
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
    out_tx: &tokio::sync::mpsc::Sender<OutboundFrame>,
) -> Result<AttachPrepared, crate::state::AttachError> {
    state.with_mut(|s| {
        let sid = s.attach(client_id, session_name, out_tx.clone())?;
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
async fn handle_attach(
    state: &SharedState,
    client_id: ClientId,
    target: AttachTarget,
    _viewport: phux_protocol::wire::frame::ViewportInfo,
    _request_scrollback: bool,
    _scrollback_limit_lines: u32,
    out_tx: &tokio::sync::mpsc::Sender<OutboundFrame>,
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
        .send(FrameKind::Attached {
            snapshot,
            initial_client_id,
        })
        .await
        .is_err()
    {
        return;
    }

    for (pane_id, handle, wire_pane_id) in panes_to_snapshot {
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
        let Ok(snap) = reply_rx.await else {
            warn!(?pane_id, "pane actor failed to reply with snapshot");
            continue;
        };
        if out_tx
            .send(FrameKind::PaneSnapshot {
                pane_id: wire_pane_id,
                cols: snap.cols,
                rows: snap.rows,
                vt_replay_bytes: snap.bytes,
                // Scrollback negotiation per ATTACH viewport metrics
                // lands with the PTY pump; byc.8 always sends None.
                scrollback_bytes: None,
            })
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
    });
}

/// Queue an `ERROR` frame on `out_tx`. Used by attach failure paths.
async fn send_error(
    out_tx: &tokio::sync::mpsc::Sender<OutboundFrame>,
    code: ErrorCode,
    message: &str,
) {
    let _ = out_tx
        .send(FrameKind::Error {
            request_id: None,
            code,
            message: message.to_owned(),
        })
        .await;
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
