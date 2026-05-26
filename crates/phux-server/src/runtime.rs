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
//! * Accept connections and spawn a per-client task that reads length-prefixed
//!   frames (`SPEC.md` §5) and, for now, echoes `PING` with `PONG`
//!   (`SPEC.md` §7.5). The full message catalog (`ATTACH`, `DETACH`,
//!   `INPUT_KEY`, ...) lands in `phux-byc.4`.
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
use phux_protocol::wire::frame::{FrameKind, MAX_FRAME_LEN, TYPE_PONG};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};
use tokio::runtime::Builder;
use tracing::{debug, error, info, warn};

use crate::state::SharedState;

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
    pub async fn run_async<F>(self, shutdown: F) -> Result<(), ServerError>
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let socket_path = self.cfg.socket_path.clone();
        prepare_socket_dir(&socket_path)?;
        handle_existing_socket(&socket_path).await?;

        // Build and pre-seed shared state. The state is the merge point
        // for multi-client input and the routing table for diffs (see
        // `state.rs`). Cloning the `SharedState` is cheap (`Arc::clone`).
        let state = SharedState::new();
        if let Some(name) = self.cfg.pre_seeded_session.as_deref() {
            state.with_mut(|s| {
                let (_sid, _wid, _pid) = s.seed_session(name);
            });
            debug!(session = name, "pre-seeded session in registry");
        }

        let listener = UnixListener::bind(&socket_path).map_err(ServerError::Bind)?;
        info!(path = %socket_path.display(), "phux-server listening on UDS");

        let result = accept_loop(&listener, state, shutdown).await;

        // Always try to unlink the socket on the way out; ignore NotFound.
        if let Err(err) = std::fs::remove_file(&socket_path)
            && err.kind() != io::ErrorKind::NotFound
        {
            warn!(path = %socket_path.display(), error = %err, "failed to unlink socket");
        }

        result
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
                        tokio::spawn(async move {
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

/// Per-client task. Reads frames in a loop; for each `PING` echoes a `PONG`;
/// logs and drops anything else for now.
///
/// The `ATTACH` / `DETACH` / `INPUT_*` routing branches are still stubbed —
/// see `phux-byc.8` for the full ATTACH handler, which will use
/// `SnapshotSynthesizer` (`grid.rs`) to build the `vt_replay_bytes` for
/// `PANE_SNAPSHOT` per SPEC §13 / ADR-0013. The gap is intentional; this
/// commit only lands the wire shape.
async fn handle_client(
    stream: UnixStream,
    _state: SharedState,
    client_id: crate::state::ClientId,
) -> io::Result<()> {
    debug!(?client_id, "client task started");
    let (mut reader, mut writer) = stream.into_split();
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
                debug!(nonce, "PING -> PONG");
                let mut out = BytesMut::new();
                encode_pong(nonce, &mut out);
                if let Err(err) = writer.write_all(&out).await {
                    debug!(error = %err, "client write error on PONG");
                    return Ok(());
                }
            }
            other => {
                debug!(kind = ?other, "unhandled message type (ATTACH/INPUT_* etc. land in byc.8)");
            }
        }
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
}
