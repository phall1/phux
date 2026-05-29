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
#![allow(
    clippy::print_stdout,
    reason = "binary entry point; `phux ls` writes its listing to stdout"
)]
#![allow(
    clippy::redundant_pub_crate,
    reason = "bin-internal submodules expose items to the crate root via pub(crate); plain `pub` would trip unreachable_pub in a binary with no external API"
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
use phux_client::attach::connection::Connection;
use phux_client::attach::{self, AttachError};
use phux_client::predict::PredictiveConfig;
use phux_config::loader as config_loader;
use phux_protocol::wire::frame::{
    AttachTarget, Command as WireCommand, CommandResult, CommandValue, FrameKind, StateScope,
};
use phux_protocol::wire::info::SessionSnapshot;
use phux_server::runtime::default_socket_path;
use phux_server::{ServerConfig, ServerRuntime};

mod selector;

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

    /// List sessions on the running server.
    ///
    /// Queries the server via the `GET_STATE` control command (ADR-0021)
    /// and prints one line per session. Does not start a server: with no
    /// server running it reports as much and exits non-zero (like
    /// `tmux ls`).
    Ls {
        /// Override the UDS path.
        #[arg(long)]
        socket: Option<PathBuf>,
    },

    /// Create a new session and attach to it.
    ///
    /// v0.1 maps to "create the named session if it does not exist, then
    /// attach" (the server's `CreateIfMissing` path). Auto-starts a
    /// server if none is running.
    New {
        /// Session name. Defaults to the standard session name.
        #[arg(short = 's', long = "session")]
        session: Option<String>,

        /// Working directory for the seed pane.
        #[arg(short = 'c', long = "cwd")]
        cwd: Option<PathBuf>,

        /// Override the UDS path.
        #[arg(long)]
        socket: Option<PathBuf>,

        /// Command (and arguments) to run in the seed pane instead of the
        /// default shell. Everything after `--` is taken verbatim.
        #[arg(trailing_var_arg = true)]
        command: Vec<String>,
    },

    /// Kill a session, window, or pane.
    ///
    /// `TARGET` uses the selector grammar (`docs/consumers/tui.md` §3):
    /// `name`, `name:N`, `name:N.M`, `name:tag`, `@N`, `.`. The selector
    /// is resolved client-side against a server-state snapshot to a set of
    /// Terminals; the server is then asked to kill each (ADR-0021).
    Kill {
        /// What to kill (selector).
        target: String,

        /// Override the UDS path.
        #[arg(long)]
        socket: Option<PathBuf>,
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
        Some(Command::Ls { socket }) => run_ls(socket),
        Some(Command::New {
            session,
            cwd,
            socket,
            command,
        }) => run_new(session, cwd, socket, command),
        Some(Command::Kill { target, socket }) => run_kill(&target, socket),
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
fn run_naked() -> ExitCode {
    let socket_path = default_socket_path();

    // phux-4li.1: name the auto-created default session from
    // `defaults.session-name-template` (e.g. `phux-${cwd-basename}`)
    // instead of the bare `DEFAULT_SESSION_NAME`. The same resolved name
    // feeds the auto-spawn seed AND the CreateIfMissing fallback so both
    // paths agree on which session to attach to.
    let default_name = resolved_default_session_name();

    if !socket_path.exists()
        && let Err(err) = maybe_auto_spawn_server(&socket_path, &default_name)
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

    match rt.block_on(attach_default_with_fallback(
        &socket_path,
        &default_name,
        predict_cfg,
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
fn resolved_default_session_name() -> String {
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
async fn attach_default_with_fallback(
    socket_path: &Path,
    default_name: &str,
    predict_cfg: PredictiveConfig,
) -> Result<(), AttachError> {
    match run_attach_once(socket_path, AttachTarget::Last, predict_cfg).await {
        Ok(()) => Ok(()),
        Err(AttachError::Refused(message)) => {
            eprintln!(
                "phux: no prior-attach session (server said: {message}); creating `{default_name}`"
            );
            run_attach_once(
                socket_path,
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

/// Block on the tokio current-thread runtime, drive the attach loop,
/// translate the result into a process exit code.
///
/// If the socket isn't there (or refuses connections), this also
/// attempts a best-effort auto-spawn of `phux server` before
/// connecting — see [`maybe_auto_spawn_server`].
fn run_attach(session: Option<String>, socket: Option<PathBuf>) -> ExitCode {
    let socket_path = socket.unwrap_or_else(default_socket_path);
    // Resolve the session name to pass through to auto-spawn before we
    // move `session` into the AttachTarget. With no explicit name this
    // path behaves like naked `phux`, so it resolves the same
    // `session-name-template` (phux-4li.1) rather than the bare
    // DEFAULT_SESSION_NAME — keeping the auto-spawn seed and the
    // create-and-attach fallback on one agreed name.
    let default_name = resolved_default_session_name();
    let session_for_spawn = session.clone().unwrap_or_else(|| default_name.clone());
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

    // No explicit name → behave like naked `phux`: try `Last`, then
    // create-and-attach the default session. This is robust to an empty
    // server (e.g. one whose auto-spawned seed pane exited before we
    // connected). An explicit name attaches to that session only.
    let result = match target {
        AttachTarget::Last => rt.block_on(attach_default_with_fallback(
            &socket_path,
            &default_name,
            predict_cfg,
        )),
        other => rt.block_on(run_attach_once(&socket_path, other, predict_cfg)),
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

// -----------------------------------------------------------------------------
// Control-plane CLI verbs — `ls` / `new` / `kill` (phux-k61, ADR-0021).
//
// `ls` and `kill` ride the generic COMMAND envelope: connect, send
// COMMAND, read COMMAND_RESULT. Selectors for `kill` are resolved
// client-side against a GET_STATE snapshot (the server never sees a
// session/window selector). `new` reuses the ATTACH CreateIfMissing path.
// -----------------------------------------------------------------------------

/// Build a current-thread tokio runtime, or print why and return the
/// failure exit code.
fn cli_runtime() -> Result<tokio::runtime::Runtime, ExitCode> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|err| {
            eprintln!("failed to build runtime: {err}");
            ExitCode::FAILURE
        })
}

/// Send one command over `conn` and return the matching `COMMAND_RESULT`,
/// skipping any unrelated frames the server interleaves (SPEC §5).
async fn command_on(
    conn: &mut Connection,
    request_id: u32,
    command: WireCommand,
) -> Result<CommandResult, AttachError> {
    conn.send(&FrameKind::Command {
        request_id,
        command,
    })
    .await?;
    loop {
        match conn.recv().await? {
            FrameKind::CommandResult {
                request_id: got,
                result,
            } if got == request_id => return Ok(result),
            _ => {}
        }
    }
}

/// One-shot: open a fresh connection, send `command`, return its result.
async fn request_command(
    socket_path: &Path,
    command: WireCommand,
) -> Result<CommandResult, AttachError> {
    let mut conn = Connection::connect(socket_path).await?;
    command_on(&mut conn, 1, command).await
}

/// Print a "no server" diagnostic for a connect-time error, or a generic
/// one otherwise. Returns the failure exit code for the caller to bubble.
fn report_no_server(err: &AttachError, socket_path: &Path, verb: &str) -> ExitCode {
    match err {
        AttachError::Io(io_err)
            if matches!(
                io_err.kind(),
                std::io::ErrorKind::ConnectionRefused | std::io::ErrorKind::NotFound,
            ) =>
        {
            eprintln!("phux: no server running at {}", socket_path.display());
        }
        AttachError::Disconnected => {
            eprintln!("phux: server closed the connection during {verb}");
        }
        other => eprintln!("phux: {verb} failed: {other}"),
    }
    ExitCode::FAILURE
}

/// `phux ls` — list sessions via `GET_STATE`. Does not auto-start a
/// server.
fn run_ls(socket: Option<PathBuf>) -> ExitCode {
    let socket_path = socket.unwrap_or_else(default_socket_path);
    let rt = match cli_runtime() {
        Ok(rt) => rt,
        Err(code) => return code,
    };
    let result = rt.block_on(request_command(
        &socket_path,
        WireCommand::GetState {
            scope: StateScope::Server,
        },
    ));
    match result {
        Ok(CommandResult::OkWith(CommandValue::State(snapshot))) => {
            print_sessions(&snapshot);
            ExitCode::SUCCESS
        }
        Ok(other) => {
            eprintln!("phux: unexpected GET_STATE result: {other:?}");
            ExitCode::FAILURE
        }
        Err(err) => report_no_server(&err, &socket_path, "ls"),
    }
}

/// Render the session list, one line per session (tmux-`ls`-ish).
fn print_sessions(snapshot: &SessionSnapshot) {
    let mut sessions: Vec<_> = snapshot.sessions.iter().collect();
    sessions.sort_by(|a, b| a.name.cmp(&b.name));
    for s in sessions {
        let windows = if s.window_count == 1 {
            "window"
        } else {
            "windows"
        };
        let attached = if s.attached_client_count > 0 {
            " (attached)"
        } else {
            ""
        };
        println!("{}: {} {windows}{attached}", s.name, s.window_count);
    }
}

/// `phux new` — create a *new* session and attach to it.
///
/// "New" is enforced client-side against a `GET_STATE` snapshot: an
/// explicit `-s NAME` that already exists is an error (like tmux's
/// duplicate-session refusal), and an omitted name is auto-assigned the
/// smallest free numeric name. The create+attach itself rides
/// `CreateIfMissing` (ADR-0021 defers a dedicated create-session command).
fn run_new(
    session: Option<String>,
    cwd: Option<PathBuf>,
    socket: Option<PathBuf>,
    command: Vec<String>,
) -> ExitCode {
    let socket_path = socket.unwrap_or_else(default_socket_path);
    let rt = match cli_runtime() {
        Ok(rt) => rt,
        Err(code) => return code,
    };

    // If a server is up, snapshot its session names so we can enforce
    // "new" (reject a duplicate -s, auto-name an omitted one). No server
    // yet → no existing names; the auto-spawn below seeds the chosen name.
    let existing = if socket_path.exists() {
        match rt.block_on(request_command(
            &socket_path,
            WireCommand::GetState {
                scope: StateScope::Server,
            },
        )) {
            Ok(CommandResult::OkWith(CommandValue::State(snap))) => {
                snap.sessions.iter().map(|s| s.name.clone()).collect()
            }
            _ => Vec::new(),
        }
    } else {
        Vec::new()
    };

    let name = match session {
        Some(requested) => {
            if existing.contains(&requested) {
                eprintln!(
                    "phux: session '{requested}' already exists (use `phux attach {requested}` to join it)"
                );
                return ExitCode::FAILURE;
            }
            requested
        }
        None => unique_session_name(&existing),
    };

    if !socket_path.exists()
        && let Err(err) = maybe_auto_spawn_server(&socket_path, &name)
    {
        eprintln!("phux: auto-spawn skipped ({err}). Start a server manually with `phux server`.");
    }

    let target = AttachTarget::CreateIfMissing {
        name: name.clone(),
        command: if command.is_empty() {
            None
        } else {
            Some(command)
        },
        cwd: cwd.map(|p| p.to_string_lossy().into_owned()),
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
    match rt.block_on(run_attach_once(&socket_path, target, predict_cfg)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            print_attach_error(&err, &socket_path, &name);
            ExitCode::FAILURE
        }
    }
}

/// Smallest non-negative integer (as a string) not already a session
/// name. Matches tmux's default numeric session naming and guarantees
/// `phux new` (no `-s`) produces a distinct session each time.
fn unique_session_name(existing: &[String]) -> String {
    let mut n: u32 = 0;
    loop {
        let candidate = n.to_string();
        if !existing.contains(&candidate) {
            return candidate;
        }
        n = n.saturating_add(1);
    }
}

/// `phux kill TARGET` — resolve the selector client-side, then ask the
/// server to kill each resolved Terminal. Exit codes: 0 on success,
/// 1 on a selector miss / no server, 2 on a server-side refusal.
fn run_kill(target: &str, socket: Option<PathBuf>) -> ExitCode {
    let selector = match selector::parse(target) {
        Ok(sel) => sel,
        Err(err) => {
            eprintln!("phux: invalid target '{target}': {err}");
            return ExitCode::FAILURE;
        }
    };
    let socket_path = socket.unwrap_or_else(default_socket_path);
    let rt = match cli_runtime() {
        Ok(rt) => rt,
        Err(code) => return code,
    };

    rt.block_on(async move {
        let mut conn = match Connection::connect(&socket_path).await {
            Ok(conn) => conn,
            Err(err) => return report_no_server(&err, &socket_path, "kill"),
        };

        // Resolve the selector against a fresh snapshot.
        let snapshot = match command_on(
            &mut conn,
            0,
            WireCommand::GetState {
                scope: StateScope::Server,
            },
        )
        .await
        {
            Ok(CommandResult::OkWith(CommandValue::State(snap))) => snap,
            Ok(other) => {
                eprintln!("phux: unexpected GET_STATE result: {other:?}");
                return ExitCode::FAILURE;
            }
            Err(err) => return report_no_server(&err, &socket_path, "kill"),
        };

        let terminals = selector::resolve(&selector, &snapshot);
        if terminals.is_empty() {
            eprintln!("phux: no such target: {target}");
            return ExitCode::FAILURE;
        }

        let mut refused = false;
        for (i, terminal_id) in terminals.into_iter().enumerate() {
            let request_id = u32::try_from(i).unwrap_or(u32::MAX).saturating_add(1);
            match command_on(
                &mut conn,
                request_id,
                WireCommand::KillTerminal {
                    terminal_id: terminal_id.clone(),
                },
            )
            .await
            {
                Ok(CommandResult::Ok) => {}
                Ok(CommandResult::Error { message, .. }) => {
                    eprintln!("phux: kill refused for {terminal_id:?}: {message}");
                    refused = true;
                }
                Ok(other) => {
                    eprintln!("phux: unexpected kill result for {terminal_id:?}: {other:?}");
                    refused = true;
                }
                // A clean disconnect means the server self-exited after its
                // last session was reaped (phux-60s): the remaining target
                // Terminals are already gone, so this is success, not failure.
                Err(AttachError::Disconnected) => break,
                Err(err) => {
                    eprintln!("phux: kill failed for {terminal_id:?}: {err}");
                    refused = true;
                }
            }
        }

        if refused {
            ExitCode::from(2)
        } else {
            ExitCode::SUCCESS
        }
    })
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

#[cfg(test)]
mod tests {
    use super::unique_session_name;

    #[test]
    fn unique_session_name_starts_at_zero_and_skips_taken() {
        assert_eq!(unique_session_name(&[]), "0");
        assert_eq!(unique_session_name(&["0".to_owned()]), "1");
        assert_eq!(
            unique_session_name(&["0".to_owned(), "1".to_owned(), "3".to_owned()]),
            "2",
        );
        // Non-numeric names (e.g. the auto-spawn "default") don't block
        // the numeric sequence.
        assert_eq!(unique_session_name(&["default".to_owned()]), "0");
    }
}
