//! phux binary entry point.
//!
//! Single executable, multiple subcommands. By convention:
//!   phux           → attach to (or auto-spawn) the user's server
//!   phux server    → run a server in the foreground (for `--stdio`, supervisord, etc.)
//!   phux attach    → attach to a session by name (phux-9gw.3)
//!   phux new       → create a new session
//!   phux ls        → list sessions
//!   phux kill      → kill sessions / windows / panes
//!
//! Subcommands are unstable until v0.1. The full CLI shape lives in
//! docs/consumers/tui.md §4; subcommands not listed here are not yet wired.

#![forbid(unsafe_code)]
#![allow(
    clippy::print_stderr,
    reason = "binary entry point; stderr is the report"
)]

// Opt-in dhat heap profiling. Swaps the global allocator for
// `dhat::Alloc` and the `Profiler::new_heap()` guard installed in
// `main()` writes `dhat-heap.json` to CWD on clean shutdown. View with
// https://nnethercote.github.io/dh_view/dh_view.html. The instrumented
// allocator is significantly slower than the system allocator — debug
// / profiling use only.
#[cfg(feature = "dhat-heap")]
#[global_allocator]
static ALLOC: dhat::Alloc = dhat::Alloc;

use std::path::{Path, PathBuf};
use std::process::{ExitCode, Stdio};
use std::time::{Duration, Instant};

use clap::{Parser, Subcommand};
use phux_client::attach::{self, AttachError};
use phux_client::predict::PredictiveConfig;
use phux_config::loader as config_loader;
use phux_protocol::wire::frame::AttachTarget;
use phux_server::runtime::default_socket_path;
use phux_server::{ServerConfig, ServerRuntime};

/// Default name the `phux server` subcommand pre-seeds, and the name
/// the `phux attach` auto-spawn path requests when the user doesn't
/// provide one. Keeping both halves on a single constant means
/// "`phux` with no arguments after a fresh boot" Just Works.
const DEFAULT_SESSION_NAME: &str = "default";

/// How long the auto-spawn path waits for the freshly-launched server
/// to bind its socket before giving up. The server's bind is sub-ms on
/// a healthy system; 2s tolerates a slow-CI host without making a
/// failed spawn feel like a hang.
const AUTO_SPAWN_SOCKET_TIMEOUT: Duration = Duration::from_secs(2);

/// Poll cadence while waiting for the auto-spawned server's socket to
/// appear. 25ms is well under user-perceptible delay and small enough
/// that the typical happy path resolves in a single poll.
const AUTO_SPAWN_POLL_INTERVAL: Duration = Duration::from_millis(25);

/// phux — terminal multiplexer built on libghostty-vt.
#[derive(Debug, Parser)]
#[command(version, about, long_about = None)]
struct Cli {
    /// Subcommand. Defaults to attaching to the last session if omitted.
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Attach to a session by name.
    Attach {
        /// Session name (matches the name used at creation time).
        ///
        /// Omit to attach to the most-recently-focused session.
        session: Option<String>,

        /// Override the UDS path. Defaults to `$XDG_RUNTIME_DIR/phux/phux.sock`
        /// (or `/tmp/phux-$USER/phux.sock` if `XDG_RUNTIME_DIR` isn't set).
        #[arg(long)]
        socket: Option<PathBuf>,
    },

    /// Run a phux server in the foreground.
    ///
    /// Binds a Unix domain socket, pre-seeds a session whose initial
    /// pane spawns the user's `$SHELL` inside a real PTY, and serves
    /// `ATTACH` requests until Ctrl-C.
    Server {
        /// Name of the pre-seeded session. Matches what
        /// `phux attach <name>` will request.
        #[arg(long, default_value = DEFAULT_SESSION_NAME)]
        session: String,

        /// Override the UDS path. Defaults to `$XDG_RUNTIME_DIR/phux/phux.sock`
        /// (or `/tmp/phux-$USER/phux.sock` if `XDG_RUNTIME_DIR` isn't set).
        #[arg(long)]
        socket: Option<PathBuf>,

        /// Detach from the controlling terminal via `setsid(2)` before
        /// binding. Set by the auto-spawn path so the server outlives
        /// the launching client's terminal; a foreground `phux server`
        /// run by hand leaves this off so Ctrl-C still works.
        #[arg(long, hide = true)]
        daemonize: bool,
    },
}

fn main() -> ExitCode {
    // Heap profiler must outlive everything else in `main` — its Drop
    // is what flushes `dhat-heap.json`. Bind to `_dhat` (NOT `_`, which
    // would drop immediately) so the guard lives until `main` returns.
    #[cfg(feature = "dhat-heap")]
    let _dhat = dhat::Profiler::new_heap();

    eprintln!(
        "phux {} (pre-alpha; see docs/spec/)",
        env!("CARGO_PKG_VERSION")
    );

    // Install the process-global tracing subscriber once, before any
    // runtime spins up. Without this, every `tracing::{info,debug,...}`
    // call site is a no-op. An init failure is non-fatal: we want the
    // binary to keep working even if a future test harness or library
    // has already installed its own subscriber.
    if let Err(err) = phux_server::telemetry::init() {
        eprintln!("phux: tracing init failed (continuing): {err}");
    }

    let cli = Cli::parse();
    match cli.command {
        Some(Command::Attach { session, socket }) => run_attach(session, socket),
        Some(Command::Server {
            session,
            socket,
            daemonize,
        }) => run_server(&session, socket, daemonize),
        None => run_naked(),
    }
}

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
/// 3. If `Last` is refused with no prior-attach memory (which is the
///    case on a freshly spawned server, or one whose only prior client
///    detached and the slot was never repopulated), fall back to
///    `ATTACH { target: ByName(DEFAULT_SESSION_NAME) }`. The auto-spawn
///    path pre-seeds that name, so this is always wired up immediately
///    after step 1.
///
/// We avoid introducing a new `ListSessions` / `CreateIfMissing` wire
/// frame here: the existing `AttachTarget::Last` + `AttachTarget::ByName`
/// pair covers the "server is alive with sessions" and "server is fresh
/// with default" cases without expanding the protocol. The "server is
/// alive but all sessions were killed" edge case still surfaces a clean
/// error — see follow-up phux-k61.2 (server-side `CreateIfMissing`).
fn run_naked() -> ExitCode {
    let socket_path = default_socket_path();

    if !socket_path.exists()
        && let Err(err) = maybe_auto_spawn_server(&socket_path, DEFAULT_SESSION_NAME)
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

    // Step 1: try Last. The server resolves this to the most-recently-
    // focused session and falls back to SessionNotFound when prior
    // activity memory is empty.
    let last_result = rt.block_on(run_attach_once(
        &socket_path,
        AttachTarget::Last,
        predict_cfg,
    ));

    match last_result {
        Ok(()) => ExitCode::SUCCESS,
        Err(AttachError::Refused(message)) => {
            // The server told us no — most commonly because there is no
            // prior-attach memory yet. Step 2: ask for the default
            // session by name. If the server was auto-spawned we just
            // pre-seeded it; if the user spun up the server manually
            // they likely used the default `--session default` and the
            // name is still right.
            eprintln!(
                "phux: no prior-attach session (server said: {message}); trying `{DEFAULT_SESSION_NAME}`"
            );
            let by_name_result = rt.block_on(run_attach_once(
                &socket_path,
                AttachTarget::ByName(DEFAULT_SESSION_NAME.to_owned()),
                predict_cfg,
            ));
            match by_name_result {
                Ok(()) => ExitCode::SUCCESS,
                Err(err) => {
                    print_attach_error(&err, &socket_path, DEFAULT_SESSION_NAME);
                    ExitCode::FAILURE
                }
            }
        }
        Err(err) => {
            print_attach_error(&err, &socket_path, DEFAULT_SESSION_NAME);
            ExitCode::FAILURE
        }
    }
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
async fn run_attach_once(
    socket_path: &Path,
    target: AttachTarget,
    predict_cfg: PredictiveConfig,
) -> Result<(), AttachError> {
    if predict_cfg.enabled {
        attach::run_with_predict(socket_path, target, predict_cfg).await
    } else {
        attach::run(socket_path, target).await
    }
}

/// Block on the tokio current-thread runtime, drive the attach loop,
/// translate the result into a process exit code.
///
/// If the socket isn't there (or refuses connections), this also
/// attempts a best-effort auto-spawn of `phux server` before
/// connecting — see [`maybe_auto_spawn_server`].
fn run_attach(session: Option<String>, socket: Option<PathBuf>) -> ExitCode {
    let socket_path = socket.unwrap_or_else(default_socket_path);
    // Resolve the session name to pass through to auto-spawn before we
    // move `session` into the AttachTarget. Falling back to
    // DEFAULT_SESSION_NAME matches what bare `phux` (no subcommand,
    // no name) will eventually request.
    let session_for_spawn = session
        .as_deref()
        .unwrap_or(DEFAULT_SESSION_NAME)
        .to_owned();
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
        match maybe_auto_spawn_server(&socket_path, &session_for_spawn) {
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

    let result = if predict_cfg.enabled {
        rt.block_on(attach::run_with_predict(&socket_path, target, predict_cfg))
    } else {
        rt.block_on(attach::run(&socket_path, target))
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

/// Print an `AttachError` as a one-line, actionable message on stderr.
///
/// `phux-roz` (5): the previous output was `attach failed: connection
/// refused` — accurate but useless. The new shape names the socket and
/// suggests the exact `phux server --session …` invocation, so the
/// user can copy-paste their way out of the failure mode.
fn print_attach_error(err: &AttachError, socket_path: &Path, session: &str) {
    match err {
        AttachError::Io(io_err)
            if matches!(
                io_err.kind(),
                std::io::ErrorKind::ConnectionRefused | std::io::ErrorKind::NotFound,
            ) =>
        {
            eprintln!(
                "phux: no server at {}. Start one with: phux server --session {session}",
                socket_path.display(),
            );
        }
        AttachError::Refused(message) => {
            eprintln!("phux: server refused attach: {message}");
        }
        AttachError::NotATty => {
            eprintln!("phux: attach requires an interactive terminal (stdin is not a TTY).",);
        }
        other => {
            eprintln!("phux: attach failed: {other}");
        }
    }
}

/// Build a current-thread tokio runtime and drive `ServerRuntime`
/// until Ctrl-C.
///
/// The runtime pre-seeds a session named `session` whose initial pane
/// is backed by a real PTY running the user's `$SHELL` (falling back
/// to `/bin/sh`). On Ctrl-C, `run_async` returns `Ok(())` and the
/// process exits 0.
fn run_server(session: &str, socket: Option<PathBuf>, daemonize: bool) -> ExitCode {
    let socket_path = socket.unwrap_or_else(default_socket_path);

    // Auto-spawn path: detach from the launching client's controlling
    // terminal so closing that terminal (SIGHUP to its session) can't
    // take the server — and the sessions it holds — down with it. The
    // client already nulled our stdio, so as a non-leader process
    // `setsid` gives us a fresh session with no controlling terminal;
    // we never open a tty afterward, so a session-leader double-fork
    // isn't needed. An `EPERM` (already a group leader) is harmless.
    if daemonize {
        let _ = rustix::process::setsid();
    }

    let cfg = ServerConfig {
        socket_path: socket_path.clone(),
        pre_seeded_session: Some(session.to_owned()),
        seed_with_pty: true,
        seed_command: None,
    };

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

    eprintln!(
        "phux server listening on {} (session={session}; Ctrl-C to stop)",
        socket_path.display()
    );

    let server = ServerRuntime::new(cfg);
    let result = rt.block_on(async move {
        server
            .run_async(async {
                // tokio::signal::ctrl_c() resolves on SIGINT *or*
                // closure of the process's stdin equivalent on some
                // platforms; either way, treat it as "user wants out".
                let _ = tokio::signal::ctrl_c().await;
            })
            .await
    });

    match result {
        Ok(()) => {
            eprintln!("phux server: shutting down cleanly");
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("phux server failed: {err}");
            ExitCode::FAILURE
        }
    }
}

/// Fork-exec the current binary as `phux server` (with the same
/// `--socket` override), then poll for the socket to appear.
///
/// Detachment strategy: the child is launched with `--daemonize`, so it
/// calls `setsid(2)` before binding and lands in its own session with no
/// controlling terminal — closing the launching terminal can't SIGHUP it.
/// stdin/stdout are nulled; stderr is redirected to `phux-server.log`
/// beside the socket so a startup crash is debuggable. The server never
/// opens a tty afterward, so a session-leader double-fork isn't needed.
///
/// Returns `Ok` if the socket showed up within the timeout.
fn maybe_auto_spawn_server(socket_path: &Path, session: &str) -> std::io::Result<()> {
    let current_exe = std::env::current_exe()?;

    eprintln!(
        "phux: starting server at {} (auto-spawn, session={session})",
        socket_path.display()
    );

    // Redirect the daemon's stderr to a log file next to the socket so a
    // crash-on-startup is debuggable (nulled stdio leaves no trace).
    // Best-effort: fall back to /dev/null if the file can't be opened.
    let log = socket_path
        .parent()
        .map(|dir| dir.join("phux-server.log"))
        .and_then(|p| {
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(p)
                .ok()
        });

    let mut cmd = std::process::Command::new(current_exe);
    cmd.arg("server")
        .arg("--socket")
        .arg(socket_path)
        .arg("--session")
        .arg(session)
        .arg("--daemonize")
        .stdin(Stdio::null())
        .stdout(Stdio::null());
    match log {
        Some(file) => {
            cmd.stderr(file);
        }
        None => {
            cmd.stderr(Stdio::null());
        }
    }

    // Spawn — we deliberately don't keep the `Child` around; the
    // server is its own lifecycle now. The OS reaps it when it exits.
    let _child = cmd.spawn()?;

    // Poll for the socket. The server's bind is fast (sub-ms on a
    // healthy system); the timeout exists to avoid hanging if the
    // child crashed at startup.
    let deadline = Instant::now() + AUTO_SPAWN_SOCKET_TIMEOUT;
    loop {
        if socket_path.exists() {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                format!(
                    "auto-spawned server did not bind {} within {:?}",
                    socket_path.display(),
                    AUTO_SPAWN_SOCKET_TIMEOUT
                ),
            ));
        }
        std::thread::sleep(AUTO_SPAWN_POLL_INTERVAL);
    }
}
