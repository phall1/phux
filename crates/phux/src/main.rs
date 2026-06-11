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

use std::process::ExitCode;

use clap::Parser;
use commands::Command;

mod commands;
mod selector;

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
        grammar works across kill/snapshot/send-keys/run/wait.",
    after_long_help = "ENVIRONMENT\n  \
        PHUX_SOCKET        UDS path for the CLI verbs and the server. A `--socket`\n  \
        \x20                 flag overrides it; default is\n  \
        \x20                 $XDG_RUNTIME_DIR/phux/phux.sock (or /tmp/phux-$USER/...).\n  \
        PHUX_WS_ADDR       Also accept WebSocket clients on HOST:PORT. Equivalent to\n  \
        \x20                 `phux server --listen`, which overrides it.\n  \
        PHUX_WS_SECURE     Force TLS + token auth on a loopback --listen address\n  \
        \x20                 (exercise the remote path locally).\n  \
        PHUX_WS_TLS_CERT   Operator-supplied server cert/key (PEM), instead of the\n  \
        PHUX_WS_TLS_KEY    auto-provisioned self-signed pair used off-loopback.\n  \
        PHUX_WS_TOKENS     Pairing-token store the server reads and `phux pair` writes.\n  \
        PHUX_LOG           Write logs to this file (server tees; client writes here).\n  \
        PHUX_LOG_FORMAT    text (default) or json — log line format.\n  \
        RUST_LOG           tracing level filter, e.g. phux=debug.\n\n\
        Run `phux server --listen 127.0.0.1:8787` to expose a port; see\n  \
        `phux help server` and docs/operations.md for the remote/TLS details."
)]
struct Cli {
    /// Subcommand. Defaults to attaching to the last session if omitted.
    #[command(subcommand)]
    command: Option<Command>,
}

/// Print the one-line build banner to stderr.
///
/// Reserved for the two long-running, human-watched entry points: a
/// foreground `phux server` and the naked-`phux` auto-spawn. It is
/// deliberately NOT printed before a one-shot control verb (`ls`,
/// `snapshot`, `send-keys`, `run`, `wait`, `new`, `kill`, `config`) so
/// those leave stderr clean for scripts and agents, and never before a
/// `--json` path. `phux --version` reports the version on stdout.
pub(crate) fn print_banner() {
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
        Some(Command::Attach { session, socket }) => commands::attach::run_attach(session, socket),
        Some(Command::Server {
            session,
            socket,
            listen,
            daemonize,
            seed_command,
            resume,
        }) => commands::server::run_server(
            &session,
            socket,
            listen,
            daemonize,
            seed_command.as_deref(),
            resume,
        ),
        Some(Command::Ls { json, socket }) => commands::ls::run_ls(json, socket),
        Some(Command::New {
            name,
            session,
            cwd,
            socket,
            json,
            command,
        }) => commands::new::run_new(name, session, cwd, socket, json, command),
        Some(Command::Kill { target, socket }) => commands::kill::run_kill(&target, socket),
        Some(Command::Rename {
            session,
            new_name,
            socket,
        }) => commands::rename::run_rename(&session, &new_name, socket),
        Some(Command::Snapshot {
            session,
            json,
            scrollback,
            cells,
            rendered,
            cols,
            rows,
            socket,
        }) => commands::snapshot::run_snapshot(
            session.as_deref(),
            json,
            scrollback,
            cells,
            &commands::snapshot::RenderedOpts {
                rendered,
                cols,
                rows,
            },
            socket,
        ),
        Some(Command::SendKeys {
            target,
            keys,
            socket,
        }) => commands::send_keys::run_send_keys(&target, &keys, socket),
        Some(Command::Wait {
            session,
            until,
            idle,
            timeout,
            json,
            socket,
        }) => commands::wait::run_wait(session.as_deref(), until, idle, timeout, json, socket),
        Some(Command::Watch {
            session,
            json,
            socket,
        }) => commands::watch::run_watch(session.as_deref(), json, socket),
        Some(Command::Run {
            target,
            command,
            timeout,
            json,
            socket,
        }) => commands::run::run_run(&target, &command, timeout, json, socket),
        Some(Command::Config { action }) => commands::config::run_config(&action),
        Some(Command::Tag { socket, action }) => commands::tag::run_tag(&action, socket),
        Some(Command::Pair { tokens, cert }) => commands::pair::run_pair(tokens, cert),
        None => commands::attach::run_naked(),
    }
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::Cli;
    use crate::commands::Command;

    /// `phux new <NAME>` must read the bare positional as the SESSION NAME,
    /// not as a command to spawn (the phux-new-foo bug: `phux new foo` tried
    /// to exec `foo` in an auto-named "0" session). The seed command is only
    /// taken after `--`.
    #[test]
    fn new_positional_is_session_name_command_requires_dash_dash() {
        let cli = Cli::try_parse_from(["phux", "new", "foo"]).expect("`phux new foo` must parse");
        let Some(Command::New {
            name,
            session,
            command,
            ..
        }) = cli.command
        else {
            panic!("expected New");
        };
        assert_eq!(
            name.as_deref(),
            Some("foo"),
            "positional is the session name"
        );
        assert_eq!(session, None, "-s not given");
        assert!(
            command.is_empty(),
            "no command without `--`; got {command:?}"
        );

        // Name + an explicit `-- CMD …`.
        let cli = Cli::try_parse_from(["phux", "new", "work", "--", "htop", "-d", "1"])
            .expect("`phux new work -- htop -d 1` must parse");
        let Some(Command::New { name, command, .. }) = cli.command else {
            panic!("expected New");
        };
        assert_eq!(name.as_deref(), Some("work"));
        assert_eq!(command, vec!["htop", "-d", "1"]);

        // No name, command-only via `--` ⇒ auto-named session running CMD.
        let cli =
            Cli::try_parse_from(["phux", "new", "--", "htop"]).expect("`phux new -- htop` parses");
        let Some(Command::New { name, command, .. }) = cli.command else {
            panic!("expected New");
        };
        assert_eq!(name, None, "no positional ⇒ auto-name");
        assert_eq!(command, vec!["htop"]);

        // `-s` still works and stays distinct from a positional.
        let cli = Cli::try_parse_from(["phux", "new", "-s", "flagged"])
            .expect("`phux new -s flagged` parses");
        let Some(Command::New { name, session, .. }) = cli.command else {
            panic!("expected New");
        };
        assert_eq!(name, None);
        assert_eq!(session.as_deref(), Some("flagged"));
    }
}
