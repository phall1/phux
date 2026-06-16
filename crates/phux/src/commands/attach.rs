use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use phux_client::attach::connection::Connection;
use phux_client::attach::{self, AttachError, CertTrust, Dial, QuicDial};
use phux_client::predict::PredictiveConfig;
use phux_config::loader as config_loader;
use phux_protocol::wire::frame::AttachTarget;
use phux_server::runtime::default_socket_path;

use crate::commands::{DEFAULT_SESSION_NAME, print_attach_error, server::maybe_auto_spawn_server};
use crate::print_banner;

/// Naked `phux` invocation (phux-k61.1).
///
/// Per docs/consumers/tui.md §1, `phux` with no arguments is the common case: attach
/// to the user's server, lazily spawning it if it isn't running.
///
/// Resolution cascade:
///
/// 1. If the socket is missing, fork-exec ourselves as `phux server`
///    (which pre-seeds the [`DEFAULT_SESSION_NAME`] session) and wait
///    for the socket to bind. Reuses [`maybe_auto_spawn_server`].
/// 2. Attempt `ATTACH { target: Last }`. On a server with prior session
///    activity this resolves to the most-recently-focused session,
///    matching docs/consumers/tui.md §1's "attach to default session" intent.
/// 3. If `Last` is refused with no prior-attach memory (a freshly spawned
///    server, or one whose sessions were all reaped), fall back to
///    `ATTACH { target: CreateIfMissing(DEFAULT_SESSION_NAME) }`, which
///    attaches to the default session or creates it first. This is what
///    makes the auto-spawn path robust: if the server's seed pane exited
///    before we connected (the server stays alive but empty, phux-60s
///    "serve before self-exit"), this step repopulates it instead of
///    surfacing a dead-end "no session" error.
///
/// The shared cascade lives in [`attach_default_with_fallback`].
pub(crate) fn run_naked() -> ExitCode {
    // The naked invocation is a human launching their session and
    // watching it come up (possibly auto-spawning a server). One line of
    // build identity is welcome here; one-shot verbs stay silent.
    print_banner();

    let socket_path = default_socket_path();

    // phux-4li.1: name the auto-created default session from
    // `defaults.session-name-template` (e.g. `phux-${cwd-basename}`)
    // instead of the bare `DEFAULT_SESSION_NAME`. The same resolved name
    // feeds the auto-spawn seed AND the CreateIfMissing fallback so both
    // paths agree on which session to attach to.
    let default_name = resolved_default_session_name();

    if !socket_path.exists()
        && let Err(err) = maybe_auto_spawn_server(
            &socket_path,
            &default_name,
            configured_spawn_on_attach().as_deref(),
        )
    {
        eprintln!("phux: auto-spawn skipped ({err}). Start a server manually with `phux server`.");
    }

    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(err) => {
            eprintln!("failed to build runtime: {err}");
            return ExitCode::FAILURE;
        }
    };

    let predict_cfg = match config_loader::load() {
        Ok(cfg) => PredictiveConfig {
            enabled: cfg.experimental.predictive_echo,
        },
        Err(err) => {
            eprintln!("phux: config load failed ({err}); using defaults");
            PredictiveConfig::disabled()
        }
    };

    match rt.block_on(attach_with_reconnect(
        &Dial::uds(&socket_path),
        AttachTarget::Last,
        predict_cfg,
        Some(&default_name),
    )) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            print_attach_error(&err, &socket_path, &default_name);
            ExitCode::FAILURE
        }
    }
}

/// Resolve the name for an auto-created default session from
/// `defaults.session-name-template`, substituting `${cwd-basename}`
/// against the client's current working directory (phux-4li.1).
///
/// Falls back to [`DEFAULT_SESSION_NAME`] when the config can't be
/// loaded, the cwd can't be read, or the template renders empty (e.g. a
/// `${cwd-basename}`-only template invoked from `/`).
pub(crate) fn resolved_default_session_name() -> String {
    let template = config_loader::load().map_or_else(
        |_| DEFAULT_SESSION_NAME.to_owned(),
        |cfg| cfg.defaults.session_name_template,
    );
    let cwd = std::env::current_dir().unwrap_or_default();
    let name = phux_config::render_session_name_template(&template, &cwd);
    if name.is_empty() {
        DEFAULT_SESSION_NAME.to_owned()
    } else {
        name
    }
}

/// Read `defaults.spawn-on-attach` from the on-disk config (phux-07y).
///
/// The naked-`phux` / `phux attach`-no-name auto-spawn passes this to the
/// server as the pre-seeded session's initial program. `None` (unset key
/// or unreadable config) ⇒ the seed pane runs the user's `$SHELL`.
pub(crate) fn configured_spawn_on_attach() -> Option<String> {
    config_loader::load().ok()?.defaults.spawn_on_attach
}

/// Drive one attach attempt against `socket_path` with `target`, picking
/// the predict-enabled entry point iff the user opted in. Pulled out
/// because [`run_naked`] needs to call attach twice (once for `Last`,
/// once for `ByName` fallback) and the predict/no-predict split would
/// otherwise duplicate four lines twice.
#[allow(
    clippy::future_not_send,
    reason = "client-side libghostty Terminal is !Send; ADR-0003 binds us to current-thread"
)]
pub(crate) async fn run_attach_once(
    dial: &Dial,
    target: AttachTarget,
    predict_cfg: PredictiveConfig,
) -> Result<(), AttachError> {
    // `run_with_predict_dial` with `predict.enabled = false` is identical to the
    // non-predictive path, so one call covers both transports and both modes.
    attach::run_with_predict_dial(dial, target, predict_cfg).await
}

/// Attach to the user's default session with the naked-`phux` fallback
/// cascade: try `Last`; if the server has no prior-attach memory, fall
/// back to `CreateIfMissing(default)`.
///
/// The `CreateIfMissing` step is what makes the cascade robust to an
/// *empty* server — e.g. one whose auto-spawned seed pane exited before
/// any client attached. The server stays alive (phux-60s only self-exits
/// after it has served a client), and this step creates a fresh default
/// session and attaches, rather than dead-ending on "session not found".
#[allow(
    clippy::future_not_send,
    reason = "client-side libghostty Terminal is !Send; ADR-0003 binds us to current-thread"
)]
pub(crate) async fn attach_default_with_fallback(
    dial: &Dial,
    default_name: &str,
    predict_cfg: PredictiveConfig,
) -> Result<(), AttachError> {
    match run_attach_once(dial, AttachTarget::Last, predict_cfg).await {
        Ok(()) => Ok(()),
        Err(AttachError::Refused(message)) => {
            eprintln!(
                "phux: no prior-attach session (server said: {message}); creating `{default_name}`"
            );
            run_attach_once(
                dial,
                AttachTarget::CreateIfMissing {
                    name: default_name.to_owned(),
                    command: None,
                    cwd: None,
                },
                predict_cfg,
            )
            .await
        }
        Err(err) => Err(err),
    }
}

/// How long a vanished server is given to come back before the client gives up
/// and exits. A graceful upgrade (ADR-0032) re-execs in well under a second;
/// the generous window only matters for a server that crashed and won't return.
const RECONNECT_DEADLINE: Duration = Duration::from_secs(10);
/// Poll cadence while waiting for the re-exec'd server to start accepting.
const RECONNECT_POLL: Duration = Duration::from_millis(100);

/// Drive an attach, transparently reconnecting if the server *vanishes*
/// mid-session — the graceful-upgrade blink (ADR-0032): the re-exec'd server
/// keeps the socket bound, so the client re-attaches and the `ATTACH`
/// handshake resyncs the screen via `TERMINAL_SNAPSHOT`.
///
/// A clean detach returns `Ok`. An [`AttachError::Disconnected`] (server closed
/// without `DETACHED`) triggers a bounded reconnect: if the socket starts
/// accepting again within [`RECONNECT_DEADLINE`] we re-attach; if the socket
/// file is gone (a clean shutdown unlinks it) or never accepts again, the
/// disconnect is surfaced. `default_name = Some` drives the naked-`phux`
/// `Last` + `CreateIfMissing` cascade each attempt; `None` re-attaches `target`
/// directly.
#[allow(
    clippy::future_not_send,
    reason = "client-side libghostty Terminal is !Send; ADR-0003 binds us to current-thread"
)]
async fn attach_with_reconnect(
    dial: &Dial,
    target: AttachTarget,
    predict_cfg: PredictiveConfig,
    default_name: Option<&str>,
) -> Result<(), AttachError> {
    loop {
        let result = match default_name {
            Some(name) => attach_default_with_fallback(dial, name, predict_cfg).await,
            None => run_attach_once(dial, target.clone(), predict_cfg).await,
        };
        match result {
            Ok(()) => return Ok(()),
            Err(AttachError::Disconnected) => {
                if wait_until_connectable(dial, RECONNECT_DEADLINE).await {
                    eprintln!("phux: server restarted; re-attaching…");
                } else {
                    return Err(AttachError::Disconnected);
                }
            }
            Err(other) => return Err(other),
        }
    }
}

/// Wait until the server accepts again on `dial`, or give up.
///
/// Returns `true` as soon as a fresh connection succeeds (the re-exec'd server
/// is up), and `false` once `deadline` elapses while connections keep failing
/// (e.g. a crashed server). For UDS it short-circuits to `false` if the socket
/// file is gone — a clean shutdown unlinks it, so there is nothing to reconnect
/// to; a graceful upgrade never removes the socket, so it falls into the
/// retry-until-connectable path. For QUIC it probes by completing a real dial
/// (TLS + stream open + preamble) and dropping it, the transport analogue of the
/// UDS connect-and-drop probe.
async fn wait_until_connectable(dial: &Dial, deadline: Duration) -> bool {
    let end = Instant::now() + deadline;
    loop {
        let connectable = match dial {
            Dial::Uds(path) => {
                if !path.exists() {
                    return false;
                }
                tokio::net::UnixStream::connect(path).await.is_ok()
            }
            Dial::Quic(quic) => match Connection::connect_quic(quic).await {
                // Close the probe cleanly so the server reaps it now; otherwise
                // each 100ms probe during a restart would leave a phantom
                // connection alive until the idle timeout.
                Ok(conn) => {
                    conn.shutdown().await;
                    true
                }
                Err(_) => false,
            },
        };
        if connectable {
            return true;
        }
        if Instant::now() >= end {
            return false;
        }
        tokio::time::sleep(RECONNECT_POLL).await;
    }
}

/// Block on the tokio current-thread runtime, drive the attach loop,
/// translate the result into a process exit code.
///
/// If the socket isn't there (or refuses connections), this also
/// attempts a best-effort auto-spawn of `phux server` before
/// connecting — see [`maybe_auto_spawn_server`].
pub(crate) fn run_attach(session: Option<String>, socket: Option<PathBuf>) -> ExitCode {
    let socket_path = socket.unwrap_or_else(default_socket_path);
    // Resolve the session name to pass through to auto-spawn before we
    // move `session` into the AttachTarget. With no explicit name this
    // path behaves like naked `phux`, so it resolves the same
    // `session-name-template` (phux-4li.1) rather than the bare
    // DEFAULT_SESSION_NAME — keeping the auto-spawn seed and the
    // create-and-attach fallback on one agreed name.
    let default_name = resolved_default_session_name();
    let session_for_spawn = session.clone().unwrap_or_else(|| default_name.clone());
    // phux-07y: only the no-name (naked-`phux`-equivalent) case seeds with
    // `defaults.spawn-on-attach`. An explicit `phux attach NAME` is like
    // `phux new`: its auto-spawned seed pane gets a plain shell.
    let seed_command = if session.is_none() {
        configured_spawn_on_attach()
    } else {
        None
    };
    let target = session.map_or(AttachTarget::Last, AttachTarget::ByName);

    // Best-effort: if no socket exists, fork-exec ourselves into a
    // detached server. Failures here are non-fatal — the subsequent
    // `attach::run` call will surface the connect error.
    //
    // `phux-roz` (4): the spawned server is pre-seeded with the same
    // session name the user is trying to attach to, so the subsequent
    // `ATTACH` doesn't refuse with "session not found" against a
    // surprise `default` session.
    if !socket_path.exists() {
        match maybe_auto_spawn_server(&socket_path, &session_for_spawn, seed_command.as_deref()) {
            Ok(()) => {}
            Err(err) => {
                eprintln!(
                    "phux: auto-spawn skipped ({err}). Start a server manually with `phux server`."
                );
            }
        }
    }

    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(err) => {
            eprintln!("failed to build runtime: {err}");
            return ExitCode::FAILURE;
        }
    };

    // Load user config to discover experimental opt-ins. Failures here
    // are non-fatal — we log and fall back to defaults so a syntax
    // error in config.toml doesn't lock the user out of their server.
    let predict_cfg = match config_loader::load() {
        Ok(cfg) => PredictiveConfig {
            enabled: cfg.experimental.predictive_echo,
        },
        Err(err) => {
            eprintln!("phux: config load failed ({err}); using defaults");
            PredictiveConfig::disabled()
        }
    };

    // No explicit name → behave like naked `phux`: try `Last`, then
    // create-and-attach the default session. This is robust to an empty
    // server (e.g. one whose auto-spawned seed pane exited before we
    // connected). An explicit name attaches to that session only.
    let dial = Dial::uds(&socket_path);
    let result = match target {
        AttachTarget::Last => rt.block_on(attach_with_reconnect(
            &dial,
            AttachTarget::Last,
            predict_cfg,
            Some(&default_name),
        )),
        other => rt.block_on(attach_with_reconnect(&dial, other, predict_cfg, None)),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            // `phux-roz` (5): produce actionable text per variant. The
            // guard (if any) has already dropped, so this lands on the
            // cooked terminal.
            print_attach_error(&err, &socket_path, &session_for_spawn);
            ExitCode::FAILURE
        }
    }
}

/// Attach over QUIC (`phux-y8v6`, ADR-0007) to a `phux server --quic`
/// listener at `addr`.
///
/// Unlike the UDS path there is no auto-spawn — the server lives on another
/// host (or another address) and the user points at it explicitly. TLS trust is
/// resolved up front:
///
/// * an explicit `--cert-fingerprint` pins the server's leaf certificate (the
///   value `phux pair` prints), the trust anchor for any routable host;
/// * a **loopback** `addr` with no fingerprint falls back to skip-verify (local
///   dev — TLS still runs, but there is no untrusted network path to MITM);
/// * a **routable** `addr` with no fingerprint is refused, rather than silently
///   trusting whatever certificate answers.
///
/// With no session name this runs the same `Last` → `CreateIfMissing` cascade
/// the naked path does; an explicit name attaches to that session only.
pub(crate) fn run_attach_quic(
    session: Option<String>,
    addr: std::net::SocketAddr,
    token: Option<String>,
    cert_fingerprint: Option<String>,
    server_name: Option<String>,
) -> ExitCode {
    print_banner();

    let trust = match cert_fingerprint {
        Some(fingerprint) => CertTrust::Pinned(fingerprint),
        None if addr.ip().is_loopback() => CertTrust::SkipVerify,
        None => {
            eprintln!(
                "phux: refusing to dial non-loopback QUIC server {addr} without --cert-fingerprint."
            );
            eprintln!(
                "      Run `phux pair` on the server host to print its certificate fingerprint,"
            );
            eprintln!("      then pass it: phux attach --quic {addr} --cert-fingerprint <FP>");
            return ExitCode::FAILURE;
        }
    };

    let token = match token {
        Some(token) => match attach::quic::parse_token_hex(&token) {
            Ok(bytes) => Some(bytes),
            Err(err) => {
                eprintln!("phux: {err}");
                return ExitCode::FAILURE;
            }
        },
        None => None,
    };

    let dial = Dial::Quic(QuicDial {
        addr,
        server_name: server_name.unwrap_or_else(|| "localhost".to_owned()),
        token,
        trust,
    });

    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(err) => {
            eprintln!("failed to build runtime: {err}");
            return ExitCode::FAILURE;
        }
    };

    let predict_cfg = match config_loader::load() {
        Ok(cfg) => PredictiveConfig {
            enabled: cfg.experimental.predictive_echo,
        },
        Err(err) => {
            eprintln!("phux: config load failed ({err}); using defaults");
            PredictiveConfig::disabled()
        }
    };

    let default_name = resolved_default_session_name();
    let (target, default) = session.map_or_else(
        || (AttachTarget::Last, Some(default_name.as_str())),
        |name| (AttachTarget::ByName(name), None),
    );

    match rt.block_on(attach_with_reconnect(&dial, target, predict_cfg, default)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("phux: QUIC attach to {addr} failed: {err}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The reconnect probe returns fast for a missing socket (clean shutdown),
    /// and `true` once a listener is bound (the re-exec'd server is up).
    #[tokio::test]
    async fn reconnect_probe_distinguishes_missing_and_live_sockets() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("probe.sock");

        // No socket file: nothing to reconnect to — returns without waiting.
        let start = Instant::now();
        assert!(!wait_until_connectable(&Dial::uds(&path), Duration::from_secs(5)).await);
        assert!(
            start.elapsed() < Duration::from_secs(1),
            "a missing socket should fail fast, not burn the deadline"
        );

        // A bound listener: connectable.
        let _listener = tokio::net::UnixListener::bind(&path).expect("bind");
        assert!(wait_until_connectable(&Dial::uds(&path), Duration::from_secs(2)).await);
    }
}
