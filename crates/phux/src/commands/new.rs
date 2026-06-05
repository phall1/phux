use std::path::{Path, PathBuf};
use std::process::ExitCode;

use phux_client::predict::PredictiveConfig;
use phux_config::loader as config_loader;
use phux_protocol::ids::CollectionId;
use phux_protocol::wire::frame::{
    AttachTarget, Command as WireCommand, CommandResult, CommandValue, StateScope,
};
use phux_server::runtime::default_socket_path;

use crate::commands::{
    DEFAULT_SESSION_NAME, attach::run_attach_once, cli_runtime, print_attach_error,
    report_no_server, request_command, server::maybe_auto_spawn_server,
};

/// `phux new` — create a *new* session and attach to it.
///
/// "New" is enforced client-side against a `GET_STATE` snapshot: an
/// explicit `-s NAME` that already exists is an error (like tmux's
/// duplicate-session refusal), and an omitted name is auto-assigned the
/// smallest free numeric name. The create+attach itself rides
/// `CreateIfMissing` (ADR-0021 defers a dedicated create-session command).
pub(crate) fn run_new(
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
pub(crate) fn run_new_json(
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
        collection: CollectionId::new(1),
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

/// Smallest non-negative integer (as a string) not already a session
/// name. Matches tmux's default numeric session naming and guarantees
/// `phux new` (no `-s`) produces a distinct session each time.
pub(crate) fn unique_session_name(existing: &[String]) -> String {
    let mut n: u32 = 0;
    loop {
        let candidate = n.to_string();
        if !existing.contains(&candidate) {
            return candidate;
        }
        n = n.saturating_add(1);
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
