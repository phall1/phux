use std::path::PathBuf;
use std::process::ExitCode;

use phux_client::attach::AttachError;
use phux_client::snapshot::ScreenState;
use phux_server::runtime::default_socket_path;

use crate::commands::{cli_runtime, parse_selector, report_no_server, resolve_target};

/// `phux snapshot [TARGET]` — read a pane as structured data (ADR-0022).
///
/// Resolves `TARGET` (a selector; default: the focused session) to a pane
/// client-side, then issues the side-effect-free `GET_SCREEN` command —
/// the server walks its own grid, so this neither attaches nor resizes the
/// pane (unlike the old attach-walk path; ADR-0022 §5, `phux-oki`). Emits
/// JSON or a boxed text view, then exits.
pub(crate) fn run_snapshot(
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

/// Human-readable boxed rendering of a captured screen (no tmux, no TTY).
///
/// Scrollback history, when present (`--scrollback`), is printed above the
/// viewport, dimmed and separated by a `╌` rule so it reads as "older
/// content above the live screen" (`phux-o1v`).
pub(crate) fn print_screen_box(screen: &ScreenState) {
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
