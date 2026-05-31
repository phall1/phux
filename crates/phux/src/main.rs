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
use phux_core::session_list::{SessionJson, SessionListJson};
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

/// phux — a libghostty-backed terminal multiplexer and control plane.
#[derive(Debug, Parser)]
#[command(
    version,
    about = "A terminal multiplexer you can drive by hand or script.",
    long_about = "phux — a libghostty-backed terminal multiplexer and control plane.\n\n\
        Run `phux` with no arguments to attach to your session (auto-starting a\n\
        server if needed). The control verbs below read and drive panes without a\n\
        TTY, and most accept `--json` for clean, scriptable output.\n\n\
        ATTACH / SERVE\n  \
          attach     Attach to a session (interactive)\n  \
          server     Run a server in the foreground\n\n\
        INSPECT\n  \
          ls         List sessions\n  \
          snapshot   Capture a pane's screen as JSON or a boxed view\n\n\
        DRIVE\n  \
          new        Create a session\n  \
          kill       Kill a session, window, or pane\n  \
          rename     Rename a session\n  \
          send-keys  Send keys to a pane\n  \
          run        Run a command in a pane and capture its exit code\n  \
          wait       Block until a pane meets a condition\n\n\
        CONFIG\n  \
          config     Inspect and scaffold the config file\n\n\
        TARGET is the selector grammar: a session name, `name:window`,\n\
        `name:window.pane`, `@id`, `.` (focused), or `=` (last-focused). The same\n\
        grammar works across kill/snapshot/send-keys/run/wait."
)]
struct Cli {
    /// Subcommand. Defaults to attaching to the last session if omitted.
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Attach to a session (interactive).
    ///
    /// With no name, attaches to the most-recently-focused session,
    /// auto-spawning a server if none is running. Requires a TTY.
    #[command(visible_alias = "a")]
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

        /// Run this command (via `$SHELL -c`) as the pre-seeded session's
        /// initial program instead of a bare shell. The naked-`phux`
        /// auto-spawn path passes `defaults.spawn-on-attach` here
        /// (phux-07y); `phux new` deliberately does not, so an
        /// explicitly-created session still gets a shell.
        #[arg(long, hide = true)]
        seed_command: Option<String>,
    },

    /// List sessions on the running server.
    ///
    /// Queries the server via the `GET_STATE` control command (ADR-0021)
    /// and prints one line per session. Does not start a server: with no
    /// server running it reports as much and exits non-zero (like
    /// `tmux ls`). Pass `--json` for the stable, versioned machine shape
    /// (ADR-0022) instead of the human text.
    #[command(visible_alias = "list")]
    Ls {
        /// Emit the session list as stable, versioned JSON
        /// (`SessionListJson`, ADR-0022) instead of human text.
        #[arg(long)]
        json: bool,

        /// Override the UDS path.
        #[arg(long)]
        socket: Option<PathBuf>,
    },

    /// Create a new session and attach to it.
    ///
    /// v0.1 maps to "create the named session if it does not exist, then
    /// attach" (the server's `CreateIfMissing` path). Auto-starts a
    /// server if none is running.
    ///
    /// With `--json`, creates the session *without* attaching and prints
    /// the seed pane's id as JSON instead — the `CREATE_SESSION` control
    /// command (ADR-0021 §3, `phux-fdh`). This neither attaches nor
    /// resizes, and the create is atomic server-side (no `GET_STATE`→attach
    /// race). `--json` requires an explicit `-s NAME`, and a name already
    /// in use is an error (create-only, never create-or-attach).
    New {
        /// Session name. Defaults to the standard session name. Required
        /// with `--json`.
        #[arg(short = 's', long = "session")]
        session: Option<String>,

        /// Working directory for the seed pane.
        #[arg(short = 'c', long = "cwd")]
        cwd: Option<PathBuf>,

        /// Override the UDS path.
        #[arg(long)]
        socket: Option<PathBuf>,

        /// Create without attaching and print the seed pane's id as JSON
        /// (the `CREATE_SESSION` command). Requires `-s NAME`.
        #[arg(long)]
        json: bool,

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

    /// Rename a session.
    ///
    /// Reassigns `SESSION`'s human-readable name to `NEW_NAME` in one
    /// `RENAME_SESSION` round-trip (ADR-0021). The server is authoritative;
    /// attached clients pick up the new name on their next snapshot. An
    /// unknown `SESSION` or a `NEW_NAME` already in use is an error.
    Rename {
        /// Current session name.
        session: String,

        /// New session name.
        new_name: String,

        /// Override the UDS path.
        #[arg(long)]
        socket: Option<PathBuf>,
    },

    /// Capture a pane's screen as JSON or a boxed text view.
    ///
    /// The agent "floor": read what's on screen as JSON (`--json`) or a
    /// boxed text view, without a TTY or tmux. Issues the side-effect-free
    /// `GET_SCREEN` command (ADR-0022) — the server walks its own grid, so
    /// this neither attaches nor resizes the pane, and is safe to poll
    /// against a pane another client is using.
    ///
    /// TARGET is a selector (see the top-level help); omit it for the
    /// most-recently-focused session.
    #[command(about = "Capture a pane's screen as JSON or a boxed text view")]
    Snapshot {
        /// Target selector. Omit for the most-recently-focused session.
        #[arg(value_name = "TARGET")]
        session: Option<String>,

        /// Emit JSON (stable schema) instead of the human boxed view.
        #[arg(long)]
        json: bool,

        /// Include scrollback history above the viewport (`phux-o1v`).
        /// Bare `--scrollback` requests all retained history; `--scrollback
        /// N` requests the most-recent N rows. History appears in the JSON
        /// `scrollback` field; the boxed view shows it above the viewport.
        #[arg(long, value_name = "N", num_args = 0..=1, default_missing_value = "0")]
        scrollback: Option<u32>,

        /// Include per-cell OSC-133 semantic marks + styles (`phux-8yl`).
        /// Populates the JSON `cells` array (sparse: only cells with a
        /// non-default style or a semantic mark). No effect on the boxed
        /// view, which is plain text.
        #[arg(long)]
        cells: bool,

        /// Override the UDS path.
        #[arg(long)]
        socket: Option<PathBuf>,
    },

    /// Send keys to a pane.
    ///
    /// tmux-shaped: each KEY is a named key (`Enter`, `Tab`, `Escape`,
    /// `Up`, `C-c`, `M-x`, …) or a literal string sent character by
    /// character. TARGET is a selector (see the top-level help); it
    /// resolves client-side to one pane and the events route to it by id,
    /// so the live pane is neither attached nor resized (ADR-0022).
    ///
    /// Flags (`--socket`) MUST precede TARGET: KEYS is a trailing var-arg,
    /// so anything after TARGET is taken as a key to send.
    ///
    ///   phux send-keys demo "echo hi" Enter
    ///   phux send-keys work:1.0 C-c
    #[command(name = "send-keys", about = "Send keys to a pane")]
    SendKeys {
        /// Target selector: session, session:window, session:window.pane,
        /// @id, `.` (focused), or `=` (last-focused).
        target: String,

        /// Keys to send: named keys and/or literal strings, in order.
        #[arg(trailing_var_arg = true, required = true)]
        keys: Vec<String>,

        /// Override the UDS path.
        #[arg(long)]
        socket: Option<PathBuf>,
    },

    /// Block until a pane meets a condition.
    ///
    /// Polls the side-effect-free screen read (ADR-0022 §4) — the poll
    /// floor of the event surface: always works, no shell integration.
    /// Exits 0 when met, 124 on `--timeout`. TARGET is a selector (see the
    /// top-level help); omit it for the most-recently-focused session.
    ///
    /// Flags (`--until`, `--idle`, `--timeout`, `--json`, `--socket`) MUST
    /// precede TARGET if you give one.
    ///
    ///   phux wait build --until "BUILD SUCCESSFUL"
    ///   phux wait repl --idle 750
    #[command(about = "Block until a pane meets a condition")]
    Wait {
        /// Target selector. Omit for the most-recently-focused session.
        #[arg(value_name = "TARGET")]
        session: Option<String>,

        /// Succeed once any visible line contains this substring. NOTE: this
        /// matches ANY visible row, including the shell's echo of a command
        /// you just typed — match on text that appears only in OUTPUT.
        #[arg(long, value_name = "TEXT")]
        until: Option<String>,

        /// Succeed once the screen holds still for this many milliseconds
        /// (the pane has settled). Default when no `--until` is given.
        #[arg(long, value_name = "MS")]
        idle: Option<u64>,

        /// Give up after this many seconds (exit 124). Default: wait forever.
        #[arg(long, value_name = "SECS")]
        timeout: Option<u64>,

        /// Emit the final screen as JSON instead of staying silent.
        #[arg(long)]
        json: bool,

        /// Override the UDS path.
        #[arg(long)]
        socket: Option<PathBuf>,
    },

    /// Stream a pane's live events (the push half of the agent surface).
    ///
    /// Subscribes to the server's `EVENT` stream (SPEC §7.5, ADR-0022
    /// 'events') and prints one event per line until EOF or Ctrl-C. The
    /// subscription neither attaches nor resizes the pane — safe to watch
    /// a pane a human or another agent is actively using. This is the
    /// latency-cutting accelerator of `phux wait`'s poll floor: events
    /// (bell, title change, output dirty/idle, pane spawn/close) arrive as
    /// they happen rather than on a poll tick.
    ///
    /// TARGET is a selector (see the top-level help); omit it for the
    /// most-recently-focused session. With `--json`, each line is a JSON
    /// object (stdout stays pure JSON); otherwise each line is a compact
    /// human form.
    ///
    ///   phux watch build
    ///   phux watch --json work:1.0
    #[command(about = "Stream a pane's live events (bell, title, dirty/idle, lifecycle)")]
    Watch {
        /// Target selector. Omit for the most-recently-focused session.
        #[arg(value_name = "TARGET")]
        session: Option<String>,

        /// Emit one JSON object per line instead of the human form. stdout
        /// stays pure JSON (diagnostics go to stderr).
        #[arg(long)]
        json: bool,

        /// Override the UDS path.
        #[arg(long)]
        socket: Option<PathBuf>,
    },

    /// Run a command in a pane and capture its exit code.
    ///
    /// Reports the command's exit code, output, and duration (ADR-0022
    /// §3). Brackets the command with sentinels to capture `$?`, so it
    /// assumes a POSIX shell (sh/bash/zsh). The process exit code mirrors
    /// the command's (125 if `phux` gives up on `--timeout`), so
    /// `phux run … && next` composes like a shell. TARGET is a selector
    /// (see the top-level help), resolved client-side to one pane; the
    /// command routes to it by id (no attach, no resize).
    ///
    /// Flags (`--timeout`, `--json`, `--socket`) MUST precede TARGET, or
    /// they are swallowed into the trailing command.
    ///
    ///   phux run build "cargo test"
    ///   phux run --timeout 30 work:1.0 "cargo test"
    #[command(about = "Run a command in a pane and capture its exit code")]
    Run {
        /// Target selector: session, session:window, session:window.pane,
        /// @id, `.` (focused), or `=` (last-focused).
        target: String,

        /// The command line: all trailing args, joined with spaces.
        #[arg(trailing_var_arg = true, required = true)]
        command: Vec<String>,

        /// Give up after this many seconds (exit 125). Default: 600s.
        /// Pass 0 to wait indefinitely.
        #[arg(long, value_name = "SECS")]
        timeout: Option<u64>,

        /// Emit the result as JSON instead of the human view.
        #[arg(long)]
        json: bool,

        /// Override the UDS path.
        #[arg(long)]
        socket: Option<PathBuf>,
    },

    /// Inspect and scaffold the phux config file (phux-ijp).
    ///
    /// phux is config-driven (ADR-0023): defaults ship in the binary and
    /// your `config.toml` is a sparse overlay merged on top. These
    /// subcommands never touch a running server.
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },
}

/// `phux config <action>` — local config inspection and scaffolding.
#[derive(Debug, Subcommand)]
enum ConfigAction {
    /// Write a commented starter config to the canonical path.
    ///
    /// The file is the shipped defaults, fully commented out: inert until
    /// you uncomment a line, so the binary's defaults stay authoritative.
    /// Refuses to overwrite an existing config unless `--force`.
    Init {
        /// Overwrite an existing config file instead of refusing.
        #[arg(long)]
        force: bool,
    },

    /// Print the resolved config path. Pure path math — prints the path
    /// whether or not the file exists.
    Path,

    /// Print the effective config (shipped defaults + your overrides) as
    /// TOML. With `--default`, print the shipped defaults verbatim
    /// instead, ignoring any user config.
    Show {
        /// Show the shipped defaults verbatim, not the merged result.
        #[arg(long)]
        default: bool,
    },
}

/// Print the one-line build banner to stderr.
///
/// Reserved for the two long-running, human-watched entry points: a
/// foreground `phux server` and the naked-`phux` auto-spawn. It is
/// deliberately NOT printed before a one-shot control verb (`ls`,
/// `snapshot`, `send-keys`, `run`, `wait`, `new`, `kill`, `config`) so
/// those leave stderr clean for scripts and agents, and never before a
/// `--json` path. `phux --version` reports the version on stdout.
fn print_banner() {
    eprintln!(
        "phux {} (pre-alpha; see docs/spec/)",
        env!("CARGO_PKG_VERSION")
    );
}

/// Whether this invocation will enter the interactive TUI (raw mode +
/// alt screen) and therefore MUST keep logs off stderr.
///
/// The alt-screen-entering paths are: `phux attach`, naked `phux` (attach
/// fallback), and `phux new` *without* `--json` (which attaches after
/// creating). `phux new --json` creates without attaching, so it stays on
/// the stderr path like every other one-shot verb.
fn is_interactive_client(cli: &Cli) -> bool {
    match &cli.command {
        Some(Command::Attach { .. }) | None => true,
        Some(Command::New { json, .. }) => !json,
        _ => false,
    }
}

fn main() -> ExitCode {
    // Heap profiler must outlive everything else in `main` — its Drop
    // is what flushes `dhat-heap.json`. Bind to `_dhat` (NOT `_`, which
    // would drop immediately) so the guard lives until `main` returns.
    #[cfg(feature = "dhat-heap")]
    let _dhat = dhat::Profiler::new_heap();

    let cli = Cli::parse();

    // Install the process-global tracing subscriber once, before any
    // runtime spins up. Without this, every `tracing::{info,debug,...}`
    // call site is a no-op.
    //
    // The choice of sink depends on whether this invocation will enter
    // the TUI (raw mode + alt screen). An interactive client owns the
    // alt screen, so it MUST log to a file only — a stray stderr line
    // corrupts the display. Every other command (foreground server,
    // one-shot control verbs, `--json` paths) keeps the historical
    // stderr layer (plus an optional `PHUX_LOG` file tee).
    //
    // The returned `WorkerGuard` (when a file sink is involved) keeps
    // the non-blocking writer's background thread alive; bind it for the
    // lifetime of `main` so logs flush on exit. An init failure is
    // non-fatal: the binary should keep working even if a future test
    // harness or library already installed its own subscriber.
    let _log_guard: Option<phux_server::telemetry::WorkerGuard> = if is_interactive_client(&cli) {
        // The client uses a synchronous file writer (no guard) so its trace
        // survives the `process::exit` detach path; see `init_client`.
        if let Err(err) = phux_server::telemetry::init_client() {
            // The client never logs to stderr, but a one-line init failure
            // on the cooked terminal (before alt screen) is acceptable and
            // beats a silent no-op subscriber.
            eprintln!("phux: client tracing init failed (continuing): {err}");
        }
        None
    } else {
        match phux_server::telemetry::init() {
            Ok(guard) => guard,
            Err(err) => {
                eprintln!("phux: tracing init failed (continuing): {err}");
                None
            }
        }
    };

    match cli.command {
        Some(Command::Attach { session, socket }) => run_attach(session, socket),
        Some(Command::Server {
            session,
            socket,
            daemonize,
            seed_command,
        }) => run_server(&session, socket, daemonize, seed_command.as_deref()),
        Some(Command::Ls { json, socket }) => run_ls(json, socket),
        Some(Command::New {
            session,
            cwd,
            socket,
            json,
            command,
        }) => run_new(session, cwd, socket, json, command),
        Some(Command::Kill { target, socket }) => run_kill(&target, socket),
        Some(Command::Rename {
            session,
            new_name,
            socket,
        }) => run_rename(&session, &new_name, socket),
        Some(Command::Snapshot {
            session,
            json,
            scrollback,
            cells,
            socket,
        }) => run_snapshot(session.as_deref(), json, scrollback, cells, socket),
        Some(Command::SendKeys {
            target,
            keys,
            socket,
        }) => run_send_keys(&target, &keys, socket),
        Some(Command::Wait {
            session,
            until,
            idle,
            timeout,
            json,
            socket,
        }) => run_wait(session.as_deref(), until, idle, timeout, json, socket),
        Some(Command::Watch {
            session,
            json,
            socket,
        }) => run_watch(session.as_deref(), json, socket),
        Some(Command::Run {
            target,
            command,
            timeout,
            json,
            socket,
        }) => run_run(&target, &command, timeout, json, socket),
        Some(Command::Config { action }) => run_config(&action),
        None => run_naked(),
    }
}

/// `phux config <action>` (phux-ijp). Entirely client-local: inspects
/// and scaffolds the on-disk config without contacting a server.
fn run_config(action: &ConfigAction) -> ExitCode {
    match action {
        ConfigAction::Path => {
            println!("{}", config_loader::config_path().display());
            ExitCode::SUCCESS
        }
        ConfigAction::Init { force } => {
            let path = config_loader::config_path();
            match phux_config::scaffold::write_reference_config(&path, *force) {
                Ok(phux_config::scaffold::ScaffoldOutcome::Wrote(p)) => {
                    println!("wrote {}", p.display());
                    ExitCode::SUCCESS
                }
                Ok(phux_config::scaffold::ScaffoldOutcome::Skipped(p)) => {
                    eprintln!(
                        "phux: {} already exists; refusing to overwrite (use --force)",
                        p.display()
                    );
                    ExitCode::FAILURE
                }
                Err(err) => {
                    eprintln!("phux: could not write config: {err}");
                    ExitCode::FAILURE
                }
            }
        }
        ConfigAction::Show { default } => {
            // `--default` echoes the embedded defaults verbatim, comments
            // and all — the annotated source of truth. Plain `show`
            // renders the effective merged document (defaults + the user's
            // overrides) as canonical TOML.
            if *default {
                print!("{}", phux_config::DEFAULT_CONFIG_TOML);
                return ExitCode::SUCCESS;
            }
            let path = config_loader::config_path();
            let user_input = match std::fs::read_to_string(&path) {
                Ok(s) => s,
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => String::new(),
                Err(err) => {
                    eprintln!("phux: could not read {}: {err}", path.display());
                    return ExitCode::FAILURE;
                }
            };
            let merged = match phux_config::merged_config_table(&user_input, &path) {
                Ok(table) => table,
                Err(err) => {
                    eprintln!("phux: {err}");
                    return ExitCode::FAILURE;
                }
            };
            match toml::to_string(&merged) {
                Ok(rendered) => {
                    print!("{rendered}");
                    ExitCode::SUCCESS
                }
                Err(err) => {
                    eprintln!("phux: could not render config: {err}");
                    ExitCode::FAILURE
                }
            }
        }
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

/// Read `defaults.spawn-on-attach` from the on-disk config (phux-07y).
///
/// The naked-`phux` / `phux attach`-no-name auto-spawn passes this to the
/// server as the pre-seeded session's initial program. `None` (unset key
/// or unreadable config) ⇒ the seed pane runs the user's `$SHELL`.
fn configured_spawn_on_attach() -> Option<String> {
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
/// server. With `json`, emits the stable [`SessionListJson`] contract
/// (ADR-0022); otherwise the human text from [`print_sessions`].
fn run_ls(json: bool, socket: Option<PathBuf>) -> ExitCode {
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
            if json {
                print_sessions_json(&snapshot)
            } else {
                print_sessions(&snapshot);
                ExitCode::SUCCESS
            }
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

/// Emit the session list as the stable [`SessionListJson`] contract.
///
/// Sessions are name-sorted to match [`print_sessions`], keeping the two
/// views consistent and the JSON stable across runs.
fn print_sessions_json(snapshot: &SessionSnapshot) -> ExitCode {
    let mut sessions: Vec<_> = snapshot.sessions.iter().collect();
    sessions.sort_by(|a, b| a.name.cmp(&b.name));
    let entries = sessions
        .into_iter()
        .map(|s| SessionJson {
            name: s.name.clone(),
            windows: s.window_count,
            attached: s.attached_client_count > 0,
        })
        .collect();
    let list = SessionListJson::new(entries);
    match serde_json::to_string_pretty(&list) {
        Ok(s) => {
            println!("{s}");
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("phux: failed to serialize session list as JSON: {err}");
            ExitCode::FAILURE
        }
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
    json: bool,
    command: Vec<String>,
) -> ExitCode {
    let socket_path = socket.unwrap_or_else(default_socket_path);
    let rt = match cli_runtime() {
        Ok(rt) => rt,
        Err(code) => return code,
    };

    if json {
        return run_new_json(&rt, &socket_path, session, cwd, command);
    }

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
        // phux-07y: `phux new` never seeds with spawn-on-attach — an
        // explicitly-created session gets a plain shell (or the `-- CMD`
        // the user gave, applied per-session via CreateIfMissing).
        && let Err(err) = maybe_auto_spawn_server(&socket_path, &name, None)
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

/// `phux new --json` — create a session *without* attaching and print its
/// seed pane's id as JSON (ADR-0021 §3, `phux-fdh`).
///
/// Issues the `CREATE_SESSION` control command rather than
/// `ATTACH { CreateIfMissing }`: no attach, no subscription, no resize, and
/// no client-side `GET_STATE`→attach race — the server allocates the session
/// and its seed pane atomically, so two concurrent `phux new --json -s X`
/// callers cannot both create `X` (the loser gets an error).
///
/// `--json` requires an explicit `-s NAME` (auto-naming is reserved for the
/// attaching path, where the client already snapshots state). A name already
/// in use is the server's error, surfaced verbatim — create-only, never
/// create-or-attach.
fn run_new_json(
    rt: &tokio::runtime::Runtime,
    socket_path: &Path,
    session: Option<String>,
    cwd: Option<PathBuf>,
    command: Vec<String>,
) -> ExitCode {
    let Some(name) = session else {
        eprintln!("phux: `phux new --json` requires an explicit -s NAME");
        return ExitCode::FAILURE;
    };

    // A server must be running to host the new session. Auto-spawn seeds a
    // throwaway session under DEFAULT_SESSION_NAME (kept distinct from the
    // requested name so the subsequent CREATE_SESSION does not collide with
    // the seed) and keeps the server alive; the real session is then created
    // without attaching. If the requested name equals the seed name the
    // server rejects the duplicate cleanly — no silent reuse.
    if !socket_path.exists()
        && let Err(err) = maybe_auto_spawn_server(socket_path, DEFAULT_SESSION_NAME, None)
    {
        eprintln!("phux: auto-spawn skipped ({err}). Start a server manually with `phux server`.");
    }

    let target = WireCommand::CreateSession {
        collection: phux_protocol::ids::CollectionId::new(1),
        name: name.clone(),
        command: if command.is_empty() {
            None
        } else {
            Some(command)
        },
        cwd: cwd.map(|p| p.to_string_lossy().into_owned()),
    };

    match rt.block_on(request_command(socket_path, target)) {
        Ok(CommandResult::OkWith(CommandValue::TerminalId(id))) => {
            let payload = serde_json::json!({
                "session": name,
                "terminal_id": id.local_id(),
            });
            match serde_json::to_string_pretty(&payload) {
                Ok(s) => {
                    println!("{s}");
                    ExitCode::SUCCESS
                }
                Err(err) => {
                    eprintln!("phux: failed to serialize create result as JSON: {err}");
                    ExitCode::FAILURE
                }
            }
        }
        Ok(CommandResult::Error { code, message }) => {
            eprintln!("phux: create-session refused ({code:?}): {message}");
            ExitCode::FAILURE
        }
        Ok(other) => {
            eprintln!("phux: unexpected CREATE_SESSION result: {other:?}");
            ExitCode::FAILURE
        }
        Err(err) => report_no_server(&err, socket_path, "new"),
    }
}

/// `phux rename SESSION NEW_NAME` — reassign a session's name in one
/// `RENAME_SESSION` round-trip (ADR-0021 §3). The server is authoritative;
/// attached clients reconcile the new name on their next snapshot. Exit
/// codes mirror `phux kill`: 0 on success, 1 on no server, 2 on a
/// server-side refusal (unknown session or a name already in use).
fn run_rename(session: &str, new_name: &str, socket: Option<PathBuf>) -> ExitCode {
    let socket_path = socket.unwrap_or_else(default_socket_path);
    let rt = match cli_runtime() {
        Ok(rt) => rt,
        Err(code) => return code,
    };

    let command = WireCommand::RenameSession {
        collection: phux_protocol::ids::CollectionId::new(1),
        name: session.to_owned(),
        new_name: new_name.to_owned(),
    };

    match rt.block_on(request_command(&socket_path, command)) {
        Ok(CommandResult::Ok) => {
            println!("renamed {session:?} to {new_name:?}");
            ExitCode::SUCCESS
        }
        Ok(CommandResult::Error { message, .. }) => {
            eprintln!("phux: rename refused for session {session:?}: {message}");
            ExitCode::from(2)
        }
        Ok(other) => {
            eprintln!("phux: unexpected RENAME_SESSION result: {other:?}");
            ExitCode::from(2)
        }
        Err(err) => report_no_server(&err, &socket_path, "rename"),
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
/// server to tear it down. A whole-session target (`.`, `=`, or a bare
/// `name`) rides a single `KILL_COLLECTION` round-trip (phux-h9s); a
/// window / pane / `@id` target falls back to one `KILL_TERMINAL` per
/// resolved Terminal. Exit codes: 0 on success, 1 on a selector miss /
/// no server, 2 on a server-side refusal.
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

        // A whole-session target tears down in one round-trip via
        // KILL_COLLECTION (the teardown counterpart to CREATE_SESSION;
        // phux-h9s, ADR-0021 §3). Window / pane / @id selectors address a
        // strict subset and stay on the per-KILL_TERMINAL path below.
        if let Some(session_name) = selector::whole_session_name(&selector, &snapshot) {
            return match command_on(
                &mut conn,
                1,
                WireCommand::KillCollection {
                    collection: phux_protocol::ids::CollectionId::new(1),
                    name: session_name.clone(),
                },
            )
            .await
            {
                // `Ok` is the ack; a clean disconnect means the server
                // self-exited after its last session was reaped (phux-60s),
                // so the session is already gone — both are success.
                Ok(CommandResult::Ok) | Err(AttachError::Disconnected) => ExitCode::SUCCESS,
                Ok(CommandResult::Error { message, .. }) => {
                    eprintln!("phux: kill refused for session {session_name:?}: {message}");
                    ExitCode::from(2)
                }
                Ok(other) => {
                    eprintln!(
                        "phux: unexpected kill result for session {session_name:?}: {other:?}"
                    );
                    ExitCode::from(2)
                }
                Err(err) => report_no_server(&err, &socket_path, "kill"),
            };
        }

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

/// Parse an optional target string into a [`selector::Selector`],
/// defaulting to the focused/last session when absent. On a parse error,
/// prints a diagnostic and returns the failure exit code for the caller to
/// bubble.
fn parse_selector(session: Option<&str>) -> Result<selector::Selector, ExitCode> {
    session.map_or(Ok(selector::Selector::Last), |target| {
        selector::parse(target).map_err(|err| {
            eprintln!("phux: invalid target '{target}': {err}");
            ExitCode::FAILURE
        })
    })
}

/// Resolve `selector` to a single pane against a fresh `GET_STATE`
/// snapshot. Prefers the focused pane when the selector spans several
/// (e.g. a whole session); otherwise the first in snapshot order. Prints
/// diagnostics and returns the failure exit code on no-server / miss.
async fn resolve_target(
    socket_path: &Path,
    selector: &selector::Selector,
    verb: &str,
) -> Result<phux_protocol::ids::TerminalId, ExitCode> {
    let snapshot = match request_command(
        socket_path,
        WireCommand::GetState {
            scope: StateScope::Server,
        },
    )
    .await
    {
        Ok(CommandResult::OkWith(CommandValue::State(snap))) => snap,
        Ok(other) => {
            eprintln!("phux: unexpected GET_STATE result: {other:?}");
            return Err(ExitCode::FAILURE);
        }
        Err(err) => return Err(report_no_server(&err, socket_path, verb)),
    };
    let candidates = selector::resolve(selector, &snapshot);
    selector::pick_target_pane(&candidates, &snapshot.focused_pane).ok_or_else(|| {
        eprintln!("phux: no such target");
        ExitCode::FAILURE
    })
}

/// `phux snapshot [TARGET]` — read a pane as structured data (ADR-0022).
///
/// Resolves `TARGET` (a selector; default: the focused session) to a pane
/// client-side, then issues the side-effect-free `GET_SCREEN` command —
/// the server walks its own grid, so this neither attaches nor resizes the
/// pane (unlike the old attach-walk path; ADR-0022 §5, `phux-oki`). Emits
/// JSON or a boxed text view, then exits.
fn run_snapshot(
    session: Option<&str>,
    json: bool,
    scrollback: Option<u32>,
    cells: bool,
    socket: Option<PathBuf>,
) -> ExitCode {
    let selector = match parse_selector(session) {
        Ok(sel) => sel,
        Err(code) => return code,
    };
    let socket_path = socket.unwrap_or_else(default_socket_path);
    let rt = match cli_runtime() {
        Ok(rt) => rt,
        Err(code) => return code,
    };

    rt.block_on(async move {
        let terminal_id = match resolve_target(&socket_path, &selector, "snapshot").await {
            Ok(id) => id,
            Err(code) => return code,
        };

        // Read the screen — side-effect-free, safe to poll. `scrollback`
        // maps straight onto the wire request: None/Some(0=all)/Some(n);
        // `cells` requests the per-cell semantic/style projection.
        let screen = match phux_client::snapshot::get_screen_scrollback(
            &socket_path,
            terminal_id,
            scrollback,
            cells,
        )
        .await
        {
            Ok(screen) => screen,
            Err(err @ AttachError::Io(_)) => {
                return report_no_server(&err, &socket_path, "snapshot");
            }
            Err(err) => {
                eprintln!("phux: snapshot failed: {err}");
                return ExitCode::FAILURE;
            }
        };

        if json {
            match serde_json::to_string_pretty(&screen) {
                Ok(s) => {
                    println!("{s}");
                    ExitCode::SUCCESS
                }
                Err(err) => {
                    eprintln!("phux: failed to serialize snapshot: {err}");
                    ExitCode::FAILURE
                }
            }
        } else {
            print_screen_box(&screen);
            ExitCode::SUCCESS
        }
    })
}

/// `phux wait [TARGET]` — poll until a pane meets a condition (ADR-0022 §4).
///
/// `--until TEXT` waits for a visible line to contain `TEXT`; `--idle MS`
/// waits for the screen to settle; with neither, defaults to idle. Exits 0
/// when met, 124 on `--timeout`. The poll floor of the event surface: it
/// reads via the side-effect-free `GET_SCREEN`, so it never disturbs the
/// pane.
fn run_wait(
    session: Option<&str>,
    until: Option<String>,
    idle: Option<u64>,
    timeout: Option<u64>,
    json: bool,
    socket: Option<PathBuf>,
) -> ExitCode {
    use phux_client::wait::{Condition, DEFAULT_IDLE_DWELL, DEFAULT_POLL_INTERVAL, WaitOutcome};

    let selector = match parse_selector(session) {
        Ok(sel) => sel,
        Err(code) => return code,
    };
    // `--until` takes precedence; otherwise settle on idle (explicit ms or
    // the default dwell).
    let condition = until.map_or_else(
        || Condition::Idle(idle.map_or(DEFAULT_IDLE_DWELL, Duration::from_millis)),
        Condition::Contains,
    );
    let timeout = timeout.map(Duration::from_secs);
    let socket_path = socket.unwrap_or_else(default_socket_path);
    let rt = match cli_runtime() {
        Ok(rt) => rt,
        Err(code) => return code,
    };

    rt.block_on(async move {
        let terminal_id = match resolve_target(&socket_path, &selector, "wait").await {
            Ok(id) => id,
            Err(code) => return code,
        };
        let result = match phux_client::wait::poll_until(
            &socket_path,
            terminal_id,
            &condition,
            timeout,
            DEFAULT_POLL_INTERVAL,
        )
        .await
        {
            Ok(result) => result,
            Err(err @ AttachError::Io(_)) => return report_no_server(&err, &socket_path, "wait"),
            Err(err) => {
                eprintln!("phux: wait failed: {err}");
                return ExitCode::FAILURE;
            }
        };
        if json && let Ok(s) = serde_json::to_string_pretty(&result.screen) {
            println!("{s}");
        }
        match result.outcome {
            WaitOutcome::Met => ExitCode::SUCCESS,
            WaitOutcome::TimedOut => {
                eprintln!("phux: wait timed out after {} polls", result.polls);
                ExitCode::from(124)
            }
        }
    })
}

/// `phux watch [TARGET]` — stream a pane's live events (SPEC §7.5,
/// ADR-0022 'events', `phux-y2t`).
///
/// Resolves `TARGET` (a selector; default: the focused session) to a pane
/// client-side, subscribes to the server's `EVENT` stream scoped to that
/// pane, and prints one event per line until EOF (server gone) or Ctrl-C.
/// The subscription neither attaches nor resizes the pane.
///
/// `--json` emits one JSON object per line and keeps stdout pure (the
/// resolved-target diagnostics and connect errors go to stderr); the
/// human form is a compact one-liner.
fn run_watch(session: Option<&str>, json: bool, socket: Option<PathBuf>) -> ExitCode {
    let selector = match parse_selector(session) {
        Ok(sel) => sel,
        Err(code) => return code,
    };
    let socket_path = socket.unwrap_or_else(default_socket_path);
    let rt = match cli_runtime() {
        Ok(rt) => rt,
        Err(code) => return code,
    };

    rt.block_on(async move {
        let terminal_id = match resolve_target(&socket_path, &selector, "watch").await {
            Ok(id) => id,
            Err(code) => return code,
        };

        // Stream until EOF or Ctrl-C. `tokio::select!` races the event
        // stream against the interrupt so Ctrl-C exits cleanly (exit 0 —
        // the user asked to stop, not a failure).
        let stream = phux_client::watch::watch_events(&socket_path, Some(terminal_id), |ev| {
            print_watch_event(&ev, json);
            true
        });
        tokio::pin!(stream);
        tokio::select! {
            result = &mut stream => match result {
                Ok(()) => ExitCode::SUCCESS,
                Err(err @ AttachError::Io(_)) => report_no_server(&err, &socket_path, "watch"),
                Err(err) => {
                    eprintln!("phux: watch failed: {err}");
                    ExitCode::FAILURE
                }
            },
            _ = tokio::signal::ctrl_c() => ExitCode::SUCCESS,
        }
    })
}

/// Render one [`phux_client::watch::WatchEvent`] to stdout — one line, as
/// JSON (`--json`) or a compact human form. Keeps stdout pure JSON under
/// `--json` (no human framing). A serialization failure is reported to
/// stderr and the line skipped rather than aborting the stream.
fn print_watch_event(ev: &phux_client::watch::WatchEvent, json: bool) {
    use phux_protocol::wire::frame::AgentEvent;

    // A stable, scriptable name for each event kind (matches the spec
    // taxonomy in §7.5.1).
    let kind = match &ev.event {
        AgentEvent::CommandStarted => "command_started",
        AgentEvent::CommandFinished { .. } => "command_finished",
        AgentEvent::TitleChanged { .. } => "title_changed",
        AgentEvent::Bell => "bell",
        AgentEvent::PaneSpawned => "pane_spawned",
        AgentEvent::PaneClosed { .. } => "pane_closed",
        AgentEvent::Dirty => "dirty",
        AgentEvent::Idle => "idle",
        AgentEvent::Unknown { .. } => "unknown",
        // `AgentEvent` is `#[non_exhaustive]`: a future server may push a
        // kind this client predates. Render it generically rather than
        // failing the stream.
        _ => "unknown",
    };
    let terminal = ev.terminal.as_ref().map(format_wire_terminal_id);

    if json {
        let mut obj = serde_json::Map::new();
        obj.insert("event".to_owned(), serde_json::Value::from(kind));
        if let Some(t) = &terminal {
            obj.insert("terminal".to_owned(), serde_json::Value::from(t.clone()));
        }
        match &ev.event {
            AgentEvent::TitleChanged { title } => {
                obj.insert("title".to_owned(), serde_json::Value::from(title.clone()));
            }
            AgentEvent::CommandFinished { exit_code } => {
                obj.insert(
                    "exit_code".to_owned(),
                    exit_code.map_or(serde_json::Value::Null, serde_json::Value::from),
                );
            }
            AgentEvent::PaneClosed { exit_status } => {
                obj.insert(
                    "exit_status".to_owned(),
                    exit_status.map_or(serde_json::Value::Null, serde_json::Value::from),
                );
            }
            AgentEvent::Unknown { tag, .. } => {
                obj.insert("tag".to_owned(), serde_json::Value::from(*tag));
            }
            _ => {}
        }
        match serde_json::to_string(&serde_json::Value::Object(obj)) {
            Ok(s) => println!("{s}"),
            Err(err) => eprintln!("phux: failed to serialize event: {err}"),
        }
    } else {
        let scope = terminal.as_deref().unwrap_or("server");
        let detail = match &ev.event {
            AgentEvent::TitleChanged { title } => format!(" {title:?}"),
            AgentEvent::CommandFinished { exit_code } => {
                exit_code.map_or_else(String::new, |c| format!(" exit={c}"))
            }
            AgentEvent::PaneClosed { exit_status } => {
                exit_status.map_or_else(String::new, |c| format!(" exit={c}"))
            }
            AgentEvent::Unknown { tag, .. } => format!(" tag={tag}"),
            _ => String::new(),
        };
        println!("{scope}\t{kind}{detail}");
    }
}

/// Render a wire [`phux_protocol::ids::TerminalId`] as the `@id` selector
/// form the rest of the CLI uses (e.g. `@3`). Satellite ids carry their
/// host (`host/@id`) so a federated event is still legible.
fn format_wire_terminal_id(id: &phux_protocol::ids::TerminalId) -> String {
    match id {
        phux_protocol::ids::TerminalId::Local { id } => format!("@{id}"),
        phux_protocol::ids::TerminalId::Satellite { host, id } => {
            format!("{}/@{id}", host.as_str())
        }
    }
}

/// `phux run TARGET CMD...` — run a command in a pane and report its exit
/// code, output, and duration (ADR-0022 §3). The process exits with the
/// command's own code, so `phux run … && next` composes like a shell.
///
/// `TARGET` is the full selector grammar (`docs/consumers/tui.md` §3):
/// `session`, `session:window`, `session:window.pane`, `@id`, `.`, `=`. It
/// is resolved client-side to a single pane (the selected one — the focused
/// pane when the selector spans several), then the command runs in exactly
/// that pane (phux-n95).
fn run_run(
    target: &str,
    command: &[String],
    timeout: Option<u64>,
    json: bool,
    socket: Option<PathBuf>,
) -> ExitCode {
    use phux_client::run::RunOutcome;

    let selector = match parse_selector(Some(target)) {
        Ok(sel) => sel,
        Err(code) => return code,
    };
    let cmd = command.join(" ");
    // `run` polls until the command's sentinel appears; an interactive or
    // never-returning command would otherwise hang forever. Default to a
    // generous cap; `--timeout 0` opts back into waiting indefinitely.
    let timeout = match timeout {
        None => Some(Duration::from_secs(RUN_DEFAULT_TIMEOUT_SECS)),
        Some(0) => None,
        Some(secs) => Some(Duration::from_secs(secs)),
    };
    let nonce = run_nonce();
    let socket_path = socket.unwrap_or_else(default_socket_path);
    let rt = match cli_runtime() {
        Ok(rt) => rt,
        Err(code) => return code,
    };

    rt.block_on(async move {
        let pane = match resolve_target(&socket_path, &selector, "run").await {
            Ok(id) => id,
            Err(code) => return code,
        };
        match phux_client::run::run_in(&socket_path, pane, &cmd, &nonce, timeout).await {
            Ok(RunOutcome::Completed(result)) => {
                if json {
                    match serde_json::to_string_pretty(&result) {
                        Ok(s) => println!("{s}"),
                        Err(err) => {
                            eprintln!("phux: failed to serialize run result: {err}");
                            return ExitCode::FAILURE;
                        }
                    }
                } else {
                    print_run_result(&result);
                }
                // Mirror the command's exit code (clamped to the 0..=255
                // process-exit range; negative/large codes saturate to 255).
                ExitCode::from(u8::try_from(result.exit_code).unwrap_or(255))
            }
            Ok(RunOutcome::TimedOut {
                command,
                duration_ms,
                ..
            }) => {
                eprintln!("phux: '{command}' did not finish within {duration_ms}ms");
                // 125, not 124: `run` mirrors the child's code into 0..=255,
                // and 124 is a code real commands (notably GNU `timeout`)
                // produce. 125 is the wrapper-failure convention (env/timeout),
                // so a caller can distinguish "phux gave up" from the child.
                ExitCode::from(RUN_TIMEOUT_EXIT_CODE)
            }
            Err(err @ AttachError::Io(_)) => report_no_server(&err, &socket_path, "run"),
            Err(AttachError::Refused(msg)) => {
                eprintln!("phux: cannot run in '{target}': {msg} (try `phux ls`)");
                ExitCode::FAILURE
            }
            Err(err) => {
                eprintln!("phux: run failed: {err}");
                ExitCode::FAILURE
            }
        }
    })
}

/// Default `run` timeout when `--timeout` is unset. Bounds the poll so an
/// interactive or never-returning command does not hang forever; users opt
/// back into unbounded waits with `--timeout 0`.
const RUN_DEFAULT_TIMEOUT_SECS: u64 = 600;

/// Exit code `run` returns when it gives up waiting for the sentinel.
/// Distinct from a mirrored child code (the wrapper-failure convention).
const RUN_TIMEOUT_EXIT_CODE: u8 = 125;

/// A per-invocation sentinel nonce. The pid is recycled across process
/// lifetimes, so it alone is not unique over time; mixing in the process
/// start time (nanoseconds since the epoch) makes a residual sentinel from
/// an earlier `run` unable to collide with this one.
fn run_nonce() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    format!("{}x{nanos}", std::process::id())
}

/// Human-readable rendering of a `run` result.
fn print_run_result(result: &phux_client::run::RunResult) {
    if !result.output.is_empty() {
        println!("{}", result.output);
    }
    let trunc = if result.truncated {
        " (output truncated; needs scrollback)"
    } else {
        ""
    };
    println!(
        "exit={} ({}ms){trunc}",
        result.exit_code, result.duration_ms
    );
}

/// `phux send-keys TARGET KEYS...` — send input to a pane via the
/// side-effect-free `ROUTE_INPUT` route.
///
/// `TARGET` is the full selector grammar (`docs/consumers/tui.md` §3):
/// `session`, `session:window`, `session:window.pane`, `@id`, `.`, `=`. It
/// is resolved client-side to a single pane (the selected one — the focused
/// pane when the selector spans several), then the events route to exactly
/// that pane by id, so this neither attaches nor resizes the live pane
/// (phux-n95; see [`phux_client::send_keys::send_to`]).
fn run_send_keys(target: &str, keys: &[String], socket: Option<PathBuf>) -> ExitCode {
    let selector = match parse_selector(Some(target)) {
        Ok(sel) => sel,
        Err(code) => return code,
    };
    let socket_path = socket.unwrap_or_else(default_socket_path);
    let rt = match cli_runtime() {
        Ok(rt) => rt,
        Err(code) => return code,
    };
    rt.block_on(async move {
        let pane = match resolve_target(&socket_path, &selector, "send-keys").await {
            Ok(id) => id,
            Err(code) => return code,
        };
        match phux_client::send_keys::send_to(&socket_path, pane, keys).await {
            Ok(()) => ExitCode::SUCCESS,
            Err(err @ AttachError::Io(_)) => report_no_server(&err, &socket_path, "send-keys"),
            Err(AttachError::Refused(msg)) => {
                eprintln!("phux: cannot send to '{target}': {msg} (try `phux ls`)");
                ExitCode::FAILURE
            }
            Err(err) => {
                eprintln!("phux: send-keys failed: {err}");
                ExitCode::FAILURE
            }
        }
    })
}

/// Human-readable boxed rendering of a captured screen (no tmux, no TTY).
///
/// Scrollback history, when present (`--scrollback`), is printed above the
/// viewport, dimmed and separated by a `╌` rule so it reads as "older
/// content above the live screen" (`phux-o1v`).
fn print_screen_box(screen: &phux_client::snapshot::ScreenState) {
    let bar = "─".repeat(usize::from(screen.cols));
    let pad_line = |line: &str| {
        let pad = usize::from(screen.cols).saturating_sub(line.chars().count());
        " ".repeat(pad)
    };
    if screen.scrollback.is_empty() {
        println!("┌{bar}┐");
    } else {
        let rule = "╌".repeat(usize::from(screen.cols));
        println!("┌{rule}┐");
        for line in &screen.scrollback {
            println!("┊{line}{}┊", pad_line(line));
        }
        println!("├{bar}┤");
    }
    for line in &screen.lines {
        println!("│{line}{}│", pad_line(line));
    }
    println!("└{bar}┘");
    let cursor = screen
        .cursor
        .as_ref()
        .map_or_else(|| "none".to_owned(), |c| format!("{},{}", c.x, c.y));
    println!(
        "pane={} {}x{} cursor={cursor}",
        screen.pane, screen.cols, screen.rows
    );
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
fn run_server(
    session: &str,
    socket: Option<PathBuf>,
    daemonize: bool,
    seed_command: Option<&str>,
) -> ExitCode {
    let socket_path = socket.unwrap_or_else(default_socket_path);

    // Banner only for a hand-started foreground server (a human watching
    // a long-running process). The `--daemonize` child of the auto-spawn
    // path nulls its stdio and logs to a file, so a banner there is noise.
    if !daemonize {
        print_banner();
    }

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

    // phux-07y: `--seed-command` runs that command (via `$SHELL -c`) as
    // the pre-seeded session's initial program instead of a bare shell.
    // The naked-`phux` auto-spawn path passes `defaults.spawn-on-attach`
    // here; `phux new`'s auto-spawn and a hand-started `phux server`
    // pass nothing, so an explicitly-created session still gets a shell.
    let seed_command = seed_command.map(phux_server::terminal_actor::shell_command);

    // `defaults.history-limit` bounds each pane's retained scrollback.
    // A failed/absent config falls back to the schema default rather
    // than aborting startup, mirroring the other config reads here.
    let history_limit = config_loader::load().map_or_else(
        |_| phux_config::DefaultsCfg::default().history_limit,
        |cfg| cfg.defaults.history_limit,
    );

    // `defaults.cwd-inheritance` selects how `SPAWN_TERMINAL` resolves a
    // new pane's working directory. Same fallback-on-error policy as the
    // other config reads here.
    let cwd_inheritance = config_loader::load().map_or_else(
        |_| phux_config::DefaultsCfg::default().cwd_inheritance,
        |cfg| cfg.defaults.cwd_inheritance,
    );

    // `defaults.term` is the `TERM` advertised to every server-spawned
    // pane (a per-spawn `SPAWN_TERMINAL.env` entry for `TERM` overrides
    // it). Same fallback-on-error policy as the other config reads here.
    let term = config_loader::load().map_or_else(
        |_| phux_config::DefaultsCfg::default().term,
        |cfg| cfg.defaults.term,
    );

    let cfg = ServerConfig {
        socket_path: socket_path.clone(),
        pre_seeded_session: Some(session.to_owned()),
        seed_with_pty: true,
        seed_command,
        history_limit,
        cwd_inheritance,
        term,
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
fn maybe_auto_spawn_server(
    socket_path: &Path,
    session: &str,
    seed_command: Option<&str>,
) -> std::io::Result<()> {
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
    // phux-07y: forward the pre-seed command (naked `phux` passes
    // `defaults.spawn-on-attach`; other callers pass `None`).
    if let Some(seed) = seed_command {
        cmd.arg("--seed-command").arg(seed);
    }
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
    use super::selector::{Selector, WindowRef};
    use super::{parse_selector, run_nonce, unique_session_name};

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

    #[test]
    fn run_nonce_is_unique_across_invocations() {
        // The pid is stable within a process; the time component must still
        // make two nonces differ (defends the stale-sentinel fix).
        assert_ne!(run_nonce(), run_nonce());
    }

    /// The full `TARGET` grammar now feeds run/send-keys/snapshot/wait/kill
    /// alike (phux-n95). `parse_selector` is the shared CLI front door:
    /// `None` defaults to the focused/last session, and every documented
    /// form parses to its [`Selector`] variant.
    #[test]
    fn parse_selector_accepts_every_grammar_form() {
        // Absent target defaults to the last/focused session.
        assert_eq!(parse_selector(None).unwrap(), Selector::Last);
        assert_eq!(parse_selector(Some(".")).unwrap(), Selector::Current);
        assert_eq!(parse_selector(Some("=")).unwrap(), Selector::Last);
        assert_eq!(
            parse_selector(Some("work")).unwrap(),
            Selector::Session("work".to_owned()),
        );
        assert_eq!(
            parse_selector(Some("work:1")).unwrap(),
            Selector::Window("work".to_owned(), WindowRef::Index(1)),
        );
        assert_eq!(
            parse_selector(Some("work:editor")).unwrap(),
            Selector::Window("work".to_owned(), WindowRef::Tag("editor".to_owned())),
        );
        assert_eq!(
            parse_selector(Some("work:1.2")).unwrap(),
            Selector::Pane("work".to_owned(), WindowRef::Index(1), 2),
        );
        assert_eq!(
            parse_selector(Some("work:editor.0")).unwrap(),
            Selector::Pane("work".to_owned(), WindowRef::Tag("editor".to_owned()), 0),
        );
        assert_eq!(
            parse_selector(Some("@42")).unwrap(),
            Selector::TerminalId(42),
        );
    }

    /// Malformed targets fail at parse time with the CLI failure code,
    /// before any server round trip (so run/send-keys reject bad syntax up
    /// front rather than resolving it). A nonexistent-but-well-formed target
    /// parses fine here; it fails later as a resolution miss.
    #[test]
    fn parse_selector_rejects_malformed_targets() {
        // Explicit empty string is a parse error (distinct from `None`).
        assert!(parse_selector(Some("")).is_err());
        // `@N` with a non-numeric id.
        assert!(parse_selector(Some("@nope")).is_err());
        // Pane index after the `.` must be numeric.
        assert!(parse_selector(Some("work:1.x")).is_err());
        // A well-formed but unknown session is NOT a parse error — it
        // resolves to nothing later.
        assert_eq!(
            parse_selector(Some("ghost")).unwrap(),
            Selector::Session("ghost".to_owned()),
        );
    }
}
