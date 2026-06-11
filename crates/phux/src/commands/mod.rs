use std::path::Path;
use std::process::ExitCode;

use clap::Subcommand;
use phux_client::attach::AttachError;
use phux_client::attach::connection::Connection;
use phux_protocol::wire::frame::{
    Command as WireCommand, CommandResult, CommandValue, FrameKind, StateScope,
};

pub(crate) mod attach;
pub(crate) mod config;
pub(crate) mod kill;
pub(crate) mod ls;
pub(crate) mod new;
pub(crate) mod pair;
pub(crate) mod rename;
pub(crate) mod run;
pub(crate) mod send_keys;
pub(crate) mod server;
pub(crate) mod snapshot;
pub(crate) mod tag;
pub(crate) mod wait;
pub(crate) mod watch;

/// Default name the `phux server` subcommand pre-seeds, and the name
/// the `phux attach` auto-spawn path requests when the user doesn't
/// provide one. Keeping both halves on a single constant means
/// "`phux` with no arguments after a fresh boot" Just Works.
pub(crate) const DEFAULT_SESSION_NAME: &str = "default";

#[derive(Debug, Subcommand)]
pub(crate) enum Command {
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
        socket: Option<std::path::PathBuf>,
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

        /// Override the UDS path. Defaults to `$PHUX_SOCKET`, else
        /// `$XDG_RUNTIME_DIR/phux/phux.sock` (or `/tmp/phux-$USER/phux.sock`
        /// if `XDG_RUNTIME_DIR` isn't set).
        #[arg(long)]
        socket: Option<std::path::PathBuf>,

        /// Also accept WebSocket clients on this `HOST:PORT` (the UDS stays
        /// on). Loopback (e.g. `127.0.0.1:8787`) is plaintext for local
        /// browser dev; any routable address (e.g. `0.0.0.0:8787`)
        /// auto-provisions TLS and requires a `phux pair` token (ADR-0031).
        /// Overrides `$PHUX_WS_ADDR`.
        #[arg(long, value_name = "HOST:PORT")]
        listen: Option<std::net::SocketAddr>,

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
        socket: Option<std::path::PathBuf>,
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
        /// Session name. `phux new work` creates a session named "work".
        /// Omitted ⇒ the `session-name-template` (e.g. "default"),
        /// disambiguated with a numeric suffix if that name is taken.
        #[arg(value_name = "NAME")]
        name: Option<String>,

        /// Session name in flag form — equivalent to the positional NAME,
        /// and the form required by `--json`. An error if it conflicts
        /// with NAME.
        #[arg(short = 's', long = "session")]
        session: Option<String>,

        /// Working directory for the seed pane.
        #[arg(short = 'c', long = "cwd")]
        cwd: Option<std::path::PathBuf>,

        /// Override the UDS path.
        #[arg(long)]
        socket: Option<std::path::PathBuf>,

        /// Create without attaching and print the seed pane's id as JSON
        /// (the `CREATE_SESSION` command). Requires `-s NAME`.
        #[arg(long)]
        json: bool,

        /// Command (and arguments) to run in the seed pane instead of the
        /// default shell. Must follow `--`: `phux new work -- htop`.
        #[arg(last = true)]
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
        socket: Option<std::path::PathBuf>,
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
        socket: Option<std::path::PathBuf>,
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

        /// Emit the CLIENT's composited multi-pane view — the assembled
        /// frame (layout tiling + dividers + status bar) as the human's glass
        /// shows it — as dense structured cells (`phux-l5xa`). Unlike the
        /// default side-effect-free read this ATTACHES (drives the headless
        /// client render path). Mutually exclusive with `--cells` /
        /// `--scrollback`; sizes the composite via `--cols` / `--rows`.
        #[arg(long, conflicts_with_all = ["cells", "scrollback"])]
        rendered: bool,

        /// Composited viewport width for `--rendered` (no TTY to measure).
        #[arg(long, value_name = "COLS", default_value_t = 80)]
        cols: u16,

        /// Composited viewport height for `--rendered`.
        #[arg(long, value_name = "ROWS", default_value_t = 24)]
        rows: u16,

        /// Override the UDS path.
        #[arg(long)]
        socket: Option<std::path::PathBuf>,
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
        socket: Option<std::path::PathBuf>,
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
        socket: Option<std::path::PathBuf>,
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
        socket: Option<std::path::PathBuf>,
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
        socket: Option<std::path::PathBuf>,
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

    /// Read and write a Terminal's L3 tags (phux-f8wi, ADR-0027).
    ///
    /// Tags are freeform strings stored as L3 metadata (`phux.tags/v1`),
    /// the server stores them opaquely. Once a Terminal is tagged, the
    /// `#tag` selector addresses every Terminal carrying that tag — e.g.
    /// `phux kill #build`, `phux snapshot #web`.
    Tag {
        /// Override the UDS path. Global so it may precede or follow the
        /// action (`phux tag --socket … add` and `phux tag add … --socket`
        /// both parse).
        #[arg(long, global = true)]
        socket: Option<std::path::PathBuf>,

        #[command(subcommand)]
        action: TagAction,
    },

    /// Mint a pairing token for a remote consumer (ADR-0031).
    ///
    /// Remote consumers (e.g. the native mobile app) attach over `wss://`
    /// without an SSH tunnel: TLS encrypts the link and an opaque bearer
    /// token authenticates the device. This mints one token into the store
    /// the server reads (`PHUX_WS_TOKENS`) and prints it once alongside the
    /// server certificate's SHA-256 fingerprint. Pair both into the device:
    /// the token is the credential, and verifying the fingerprint on first
    /// connect defeats a man-in-the-middle. Revoke a device by deleting its
    /// line from the token file.
    ///
    /// This never contacts a running server — it only writes the token file.
    Pair {
        /// Token store to append to. Defaults to `PHUX_WS_TOKENS`.
        #[arg(long, value_name = "PATH")]
        tokens: Option<std::path::PathBuf>,

        /// Server certificate PEM, used to print the pairing fingerprint.
        /// Defaults to `PHUX_WS_TLS_CERT`.
        #[arg(long, value_name = "PATH")]
        cert: Option<std::path::PathBuf>,
    },
}

/// `phux tag <action>` — list and edit a Terminal's L3 tags.
#[derive(Debug, Subcommand)]
pub(crate) enum TagAction {
    /// List the tags on each Terminal a selector resolves to.
    Ls {
        /// Target selector (session, `session:window`, `@id`, `.`, `#tag`).
        target: String,
    },

    /// Add one or more tags to each Terminal a selector resolves to.
    Add {
        /// Target selector.
        target: String,
        /// Tags to add (the leading `#` is optional).
        #[arg(required = true)]
        tags: Vec<String>,
    },

    /// Remove one or more tags from each Terminal a selector resolves to.
    Rm {
        /// Target selector.
        target: String,
        /// Tags to remove (the leading `#` is optional).
        #[arg(required = true)]
        tags: Vec<String>,
    },
}

/// `phux config <action>` — local config inspection and scaffolding.
#[derive(Debug, Subcommand)]
pub(crate) enum ConfigAction {
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

/// Build a current-thread tokio runtime, or print why and return the
/// failure exit code.
pub(crate) fn cli_runtime() -> Result<tokio::runtime::Runtime, ExitCode> {
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
pub(crate) async fn command_on(
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
pub(crate) async fn request_command(
    socket_path: &Path,
    command: WireCommand,
) -> Result<CommandResult, AttachError> {
    let mut conn = Connection::connect(socket_path).await?;
    command_on(&mut conn, 1, command).await
}

/// Print a "no server" diagnostic for a connect-time error, or a generic
/// one otherwise. Returns the failure exit code for the caller to bubble.
pub(crate) fn report_no_server(err: &AttachError, socket_path: &Path, verb: &str) -> ExitCode {
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

/// Parse an optional target string into a [`crate::selector::Selector`],
/// defaulting to the focused/last session when absent. On a parse error,
/// prints a diagnostic and returns the failure exit code for the caller to
/// bubble.
pub(crate) fn parse_selector(session: Option<&str>) -> Result<crate::selector::Selector, ExitCode> {
    session.map_or(Ok(crate::selector::Selector::Last), |target| {
        crate::selector::parse(target).map_err(|err| {
            eprintln!("phux: invalid target '{target}': {err}");
            ExitCode::FAILURE
        })
    })
}

/// Resolve `selector` to a single pane against a fresh `GET_STATE`
/// snapshot. Prefers the focused pane when the selector spans several
/// (e.g. a whole session); otherwise the first in snapshot order. Prints
/// diagnostics and returns the failure exit code on no-server / miss.
pub(crate) async fn resolve_target(
    socket_path: &Path,
    selector: &crate::selector::Selector,
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
    let candidates = resolve_targets(socket_path, selector, &snapshot).await;
    crate::selector::pick_target_pane(&candidates, &snapshot.focused_pane).ok_or_else(|| {
        eprintln!("phux: no such target");
        ExitCode::FAILURE
    })
}

/// Resolve `selector` to its `TerminalId`s, fetching L3 tag metadata first
/// only when the selector is `#tag` (`phux-f8wi`). Non-tag selectors resolve
/// purely against `snapshot`, so the common path pays no extra round-trip.
///
/// A tag fetch that fails (no server mid-flight, a malformed value) degrades
/// to an empty tag index, so a `#tag` selector then resolves to nothing —
/// the caller reports it as a selector miss, never a hang.
pub(crate) async fn resolve_targets(
    socket_path: &Path,
    selector: &crate::selector::Selector,
    snapshot: &phux_protocol::wire::info::SessionSnapshot,
) -> Vec<phux_protocol::ids::TerminalId> {
    if !matches!(selector, crate::selector::Selector::Tag(_)) {
        return crate::selector::resolve(selector, snapshot);
    }
    let tags = match Connection::connect(socket_path).await {
        Ok(mut conn) => tag::fetch_tag_index(&mut conn, snapshot).await,
        Err(_) => crate::selector::TagIndex::new(),
    };
    crate::selector::resolve_with_tags(selector, snapshot, &tags)
}

/// Print an `AttachError` as a one-line, actionable message on stderr.
///
/// `phux-roz` (5): the previous output was `attach failed: connection
/// refused` — accurate but useless. The new shape names the socket and
/// suggests the exact `phux server --session …` invocation, so the
/// user can copy-paste their way out of the failure mode.
pub(crate) fn print_attach_error(err: &AttachError, socket_path: &Path, session: &str) {
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

#[cfg(test)]
mod tests {
    use crate::commands::parse_selector;
    use crate::selector::{Selector, WindowRef};

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
