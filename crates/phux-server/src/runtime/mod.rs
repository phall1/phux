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
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use phux_protocol::wire::frame::{ErrorCode, FrameKind};
use tokio::net::UnixListener;
use tokio::runtime::Builder;
use tokio::task::LocalSet;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, trace, warn};

use crate::state::{Outbound, SharedState};

pub mod attach;
pub mod client;
pub mod commands;

pub(crate) use attach::*;
pub(crate) use client::*;
pub use commands::*;

/// Timeout for the "is the socket still live?" liveness probe used when an
/// existing socket file is encountered during bind.
pub(crate) const STALE_PROBE_TIMEOUT: Duration = Duration::from_millis(50);

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
    /// How a Terminal viewed by clients of differing sizes resolves its one
    /// authoritative PTY geometry (`defaults.window-size`, phux-nk07).
    /// Threaded into shared state so `handle_viewport_resize` applies the
    /// policy across every subscriber's viewport. The binary populates this
    /// from `phux_config`'s `defaults.window-size`; [`Self::with_default_socket`]
    /// uses the schema default ([`phux_config::WindowSize::Smallest`]).
    pub window_size: phux_config::WindowSize,
    /// Optional policy extension bundle. When `None`, the server uses the
    /// default permissive policy (allow everything, audit nothing).
    pub policy_bundle: Option<crate::policy::PolicyBundle>,
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
            window_size: phux_config::WindowSize::default(),
            policy_bundle: None,
        }
    }
}

/// Resolve the default Unix-domain-socket path.
///
/// Precedence (matches the MCP adapter's `resolve`, so the daemon, the CLI
/// verbs, and the MCP bridge all agree on one socket):
/// 1. `$PHUX_SOCKET` if set — an explicit `--socket` flag still overrides it
///    at the call sites that take one;
/// 2. `$XDG_RUNTIME_DIR/phux/phux.sock` if `XDG_RUNTIME_DIR` is set;
/// 3. `/tmp/phux-$UID/phux.sock` otherwise.
#[must_use]
pub fn default_socket_path() -> PathBuf {
    if let Some(path) = std::env::var_os("PHUX_SOCKET") {
        return PathBuf::from(path);
    }
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
    /// Optional WebSocket listen address (in addition to the always-on UDS).
    /// `None` falls back to the `PHUX_WS_ADDR` environment variable. The
    /// `phux server --listen <ADDR>` flag populates this; binding off-loopback
    /// auto-engages TLS + token auth (see [`build_ws_listener`]).
    ws_addr: Option<SocketAddr>,
}

impl ServerRuntime {
    /// Create a runtime ready to be `run`. Does not perform I/O.
    #[must_use]
    pub const fn new(cfg: ServerConfig) -> Self {
        Self { cfg, ws_addr: None }
    }

    /// Also accept WebSocket connections on `addr` (the UDS stays on).
    ///
    /// Overrides the `PHUX_WS_ADDR` environment variable. A loopback address
    /// is plaintext + unauthenticated (the local browser-dev path); any
    /// routable address auto-provisions TLS and requires a paired bearer
    /// token (ADR-0031).
    #[must_use]
    pub const fn listen_ws(mut self, addr: SocketAddr) -> Self {
        self.ws_addr = Some(addr);
        self
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
        // Mirror `defaults.window-size` into shared state so
        // `handle_viewport_resize` resolves a shared Terminal's geometry from
        // the configured multi-client policy (phux-nk07).
        let window_size = self.cfg.window_size;
        state.with_mut(|s| s.set_window_size(window_size));
        // WebSocket listen address: the `--listen` flag (via `listen_ws`)
        // wins; otherwise fall back to `PHUX_WS_ADDR` inside the accept setup.
        let ws_addr_override = self.ws_addr;
        // Wire policy bundle from config into shared state.
        if let Some(bundle) = self.cfg.policy_bundle.clone() {
            state.with_mut(|s| s.set_policy_bundle(bundle));
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
                // Opt-in via `phux server --listen <ADDR>` or the `PHUX_WS_ADDR`
                // environment variable (e.g. "127.0.0.1:8787"); UDS is always
                // on. The flag wins when both are set.
                let ws_addr = ws_addr_override.or_else(|| {
                    std::env::var("PHUX_WS_ADDR").ok().and_then(|raw| {
                        match raw.parse::<SocketAddr>() {
                            Ok(addr) => Some(addr),
                            Err(err) => {
                                warn!(addr = %raw, error = %err, "invalid PHUX_WS_ADDR; WebSocket transport disabled");
                                None
                            }
                        }
                    })
                });
                let ws_listener = match ws_addr {
                    Some(addr) => build_ws_listener(addr).await,
                    None => None,
                };
                match ws_listener {
                    Some(ws) => {
                        // Both loops run until the root token cancels;
                        // whichever returns first ends the server (on
                        // shutdown both observe the cancellation).
                        tokio::select! {
                            r = accept_loop(&listener, state.clone(), root_token.clone()) => r,
                            r = accept_loop(&ws, state, root_token) => r,
                        }
                    }
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
/// Build the optional WebSocket listener for `PHUX_WS_ADDR`, applying the
/// ADR-0031 remote-consumer policy. Returns `None` (WebSocket disabled, UDS
/// only) on any setup failure rather than failing the whole server.
///
/// **The bind address is the toggle, so there is no remote-mode setup friction:**
///
/// * **Loopback address → plaintext, unauthenticated.** The local browser-client
///   dev path; zero config.
/// * **Routable address → TLS + bearer-token auth, auto-provisioned.** Binding to
///   anything off-loopback is treated as exposing the server, so phux generates
///   and persists a self-signed certificate (if none is configured) and reads
///   the default token store — no openssl, no manual PEM. The operator just runs
///   `phux pair` to mint a device token. Plaintext never reaches a routable
///   address (ADR-0031 no-plaintext-remote invariant).
///
/// `PHUX_WS_SECURE=1` forces the secure path on a loopback address (for testing
/// the remote path locally). `PHUX_WS_TLS_CERT` + `PHUX_WS_TLS_KEY` override the
/// auto-generated certificate with an operator-supplied one; `PHUX_WS_TOKENS`
/// overrides the default token-store path.
async fn build_ws_listener(addr: SocketAddr) -> Option<crate::transport::WsListener> {
    let force_secure = std::env::var_os("PHUX_WS_SECURE").is_some_and(|v| !v.is_empty());
    let secure = !addr.ip().is_loopback() || force_secure;

    if !secure {
        return match crate::transport::WsListener::bind(addr).await {
            Ok(ws) => {
                let bound = ws.local_addr().map(|a| a.to_string()).unwrap_or_default();
                info!(addr = %bound, "WebSocket listening (plaintext, loopback)");
                Some(ws)
            }
            Err(err) => {
                warn!(addr = %addr, error = %err, "failed to bind WebSocket; UDS only");
                None
            }
        };
    }

    // Secure path. Operator-supplied cert overrides the auto-generated one;
    // otherwise provision a persisted self-signed cert at the default paths.
    let cert_env = std::env::var_os("PHUX_WS_TLS_CERT").map(PathBuf::from);
    let key_env = std::env::var_os("PHUX_WS_TLS_KEY").map(PathBuf::from);
    let operator_cert = cert_env.is_some() || key_env.is_some();
    let cert_path = cert_env.unwrap_or_else(crate::transport::tls::default_cert_path);
    let key_path = key_env.unwrap_or_else(crate::transport::tls::default_key_path);
    if !operator_cert
        && let Err(err) = crate::transport::tls::ensure_self_signed(&cert_path, &key_path)
    {
        error!(error = %err, "failed to provision self-signed certificate; WebSocket disabled");
        return None;
    }
    let acceptor = match crate::transport::tls::acceptor_from_pem(&cert_path, &key_path) {
        Ok(acceptor) => acceptor,
        Err(err) => {
            error!(error = %err, "TLS setup failed; WebSocket disabled");
            return None;
        }
    };

    let tokens_path = std::env::var_os("PHUX_WS_TOKENS")
        .map_or_else(crate::auth::default_token_store_path, PathBuf::from);
    let store = match crate::auth::TokenStore::load(&tokens_path) {
        Ok(store) => store,
        Err(err) => {
            error!(error = %err, path = %tokens_path.display(), "failed to load token store; WebSocket disabled");
            return None;
        }
    };
    if store.is_empty() {
        warn!(
            path = %tokens_path.display(),
            "no pairing tokens; every remote consumer is rejected until `phux pair`"
        );
    }
    let token_count = store.len();

    match crate::transport::WsListener::bind_secure(addr, acceptor, std::sync::Arc::new(store))
        .await
    {
        Ok(ws) => {
            let bound = ws.local_addr().map(|a| a.to_string()).unwrap_or_default();
            info!(addr = %bound, tokens = token_count, "WebSocket listening with TLS + token auth");
            Some(ws)
        }
        Err(err) => {
            warn!(addr = %addr, error = %err, "failed to bind secure WebSocket; UDS only");
            None
        }
    }
}

/// Queue an `ERROR` frame on `out_tx`. Used by attach failure paths.
pub(crate) async fn send_error(
    out_tx: &tokio::sync::mpsc::Sender<Outbound>,
    code: ErrorCode,
    message: &str,
) {
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
    // Used only by the tests below — scoped here rather than at module level
    // so the lib's import set stays clean under `-D warnings`.
    use crate::state::ClientId;
    use crate::terminal_actor::ResizeRequest;
    use phux_protocol::caps::ClientCapabilities;
    use phux_protocol::wire::frame::{AttachTarget, ViewportInfo};
    use tokio::task::JoinSet;

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
                            bytes,
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
        let (output_tx, _output_rx_seed) =
            broadcast::channel::<crate::terminal_actor::PaneOutput>(8);
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
            upgrade: mpsc::channel::<crate::terminal_actor::UpgradeHandleRequest>(8).0,
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
        assert_eq!(
            observed.cell_px, None,
            "a pixel-less viewport report must not invent a cell size",
        );
        assert!(
            resize_rx.try_recv().is_err(),
            "exactly one resize request should be queued — got more",
        );

        // A pixel-bearing report resolves the per-cell size and rides the
        // same request: 1320x750 px over 132x50 cells -> 10x15 px cells.
        let viewport = ViewportInfo::new(132, 50).with_pixels(Some(1320), Some(750));
        handle_viewport_resize(&state, client_id, &viewport);
        let observed = resize_rx
            .try_recv()
            .expect("second resize request must be queued on the channel");
        assert_eq!(
            observed.cell_px,
            Some((10, 15)),
            "the resolved cell pixel size must reach the actor",
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
                    let (output_tx, _output_rx_seed) =
                        broadcast::channel::<crate::terminal_actor::PaneOutput>(8);
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
                        upgrade: mpsc::channel::<crate::terminal_actor::UpgradeHandleRequest>(8).0,
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
                        scrollback: Vec::new(),
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
                let (output_tx, _output_rx_seed) =
                    broadcast::channel::<crate::terminal_actor::PaneOutput>(8);
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
                    upgrade: mpsc::channel::<crate::terminal_actor::UpgradeHandleRequest>(8).0,
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
                        scrollback: Vec::new(),
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
