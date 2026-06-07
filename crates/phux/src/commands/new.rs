use std::path::{Path, PathBuf};
use std::process::ExitCode;

use phux_client::attach::connection::Connection;
use phux_client::predict::PredictiveConfig;
use phux_config::loader as config_loader;
use phux_protocol::wire::frame::{
    AttachTarget, Command as WireCommand, CommandResult, CommandValue, FrameKind,
    SESSION_CREATE_KEY, SESSION_CREATE_RESULT_KEY, Scope, StateScope,
};
use phux_server::runtime::default_socket_path;

use crate::commands::{
    DEFAULT_SESSION_NAME, attach::resolved_default_session_name, attach::run_attach_once,
    cli_runtime, command_on, print_attach_error, report_no_server, request_command,
    server::maybe_auto_spawn_server,
};

/// `phux new` — create a *new* session and attach to it.
///
/// The name comes from the positional `NAME` or the `-s` flag (the same
/// field, two spellings; a genuine conflict is an error). "New" is enforced
/// client-side against a `GET_STATE` snapshot: a name that already exists is
/// an error (like tmux's duplicate-session refusal), and an omitted name
/// falls back to the configured `session-name-template` (e.g. "default"),
/// disambiguated with a numeric suffix if taken. The create+attach itself
/// rides `CreateIfMissing` (ADR-0021 defers a dedicated create-session
/// command).
pub(crate) fn run_new(
    name: Option<String>,
    session: Option<String>,
    cwd: Option<PathBuf>,
    socket: Option<PathBuf>,
    json: bool,
    command: Vec<String>,
) -> ExitCode {
    // The session name can come from the positional NAME or the `-s` flag;
    // they are the same field with two spellings. Reject a genuine conflict
    // rather than silently picking one.
    let requested = match (name, session) {
        (Some(positional), Some(flag)) if positional != flag => {
            eprintln!(
                "phux: conflicting session names: '{positional}' (positional) vs '{flag}' (-s) — pass just one"
            );
            return ExitCode::FAILURE;
        }
        (Some(positional), _) => Some(positional),
        (None, flag) => flag,
    };

    let socket_path = socket.unwrap_or_else(default_socket_path);
    let rt = match cli_runtime() {
        Ok(rt) => rt,
        Err(code) => return code,
    };

    if json {
        return run_new_json(&rt, &socket_path, requested, cwd, command);
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

    let name = match requested {
        Some(requested) => {
            if existing.contains(&requested) {
                eprintln!(
                    "phux: session '{requested}' already exists (use `phux attach {requested}` to join it)"
                );
                return ExitCode::FAILURE;
            }
            requested
        }
        // No name given: start from the configured session-name-template
        // (e.g. "default"), the same base every auto-create path uses, and
        // disambiguate with a numeric suffix instead of emitting a bare "0".
        None => unique_session_name(&existing, &resolved_default_session_name()),
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
/// seed pane's id as JSON.
///
/// Since the v0.3.0 "Option B" re-tier (ADR-0019 / ADR-0027) dissolved the
/// L2 collection tier and removed the `CREATE_SESSION` verb, create-without-
/// attach is expressed as an L3 `SET_METADATA` write of the conventional
/// [`SESSION_CREATE_KEY`] (`Scope::Global`, value = JSON `{name, command?,
/// cwd?}`). The server seeds the session + pane atomically; the client then
/// reads the seed-pane id back from [`SESSION_CREATE_RESULT_KEY`] via
/// `GET_METADATA` (`SET_METADATA` carries no reply frame).
///
/// `--json` requires an explicit `-s NAME` (auto-naming is reserved for the
/// attaching path). A name already in use is reported as an error
/// (checked client-side against the pre-write snapshot) — create-only,
/// never create-or-attach.
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
    // requested name so the create write below does not collide with the
    // seed) and keeps the server alive; the real session is then created
    // without attaching.
    if !socket_path.exists()
        && let Err(err) = maybe_auto_spawn_server(socket_path, DEFAULT_SESSION_NAME, None)
    {
        eprintln!("phux: auto-spawn skipped ({err}). Start a server manually with `phux server`.");
    }

    let cwd = cwd.map(|p| p.to_string_lossy().into_owned());
    let command = if command.is_empty() {
        None
    } else {
        Some(command)
    };

    match rt.block_on(create_session_via_metadata(
        socket_path,
        &name,
        command,
        cwd,
    )) {
        Ok(terminal_id) => {
            let payload = serde_json::json!({ "session": name, "terminal_id": terminal_id });
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
        Err(code) => code,
    }
}

/// Create a named session without attaching via the conventional
/// `SESSION_CREATE_KEY` write, then read the seed-pane id back from
/// `SESSION_CREATE_RESULT_KEY`. Returns the seed pane's local id on success,
/// or the failure `ExitCode` (already reported to stderr) otherwise. Shared
/// by `phux new --json`; mirrors the MCP `phux_new` path.
async fn create_session_via_metadata(
    socket_path: &Path,
    name: &str,
    command: Option<Vec<String>>,
    cwd: Option<String>,
) -> Result<u64, ExitCode> {
    let create_bytes = serde_json::to_vec(&serde_json::json!({
        "name": name,
        "command": command,
        "cwd": cwd,
    }))
    .map_err(|err| {
        eprintln!("phux: failed to serialize create request: {err}");
        ExitCode::FAILURE
    })?;

    let mut conn = Connection::connect(socket_path)
        .await
        .map_err(|err| report_no_server(&err, socket_path, "new"))?;

    // Reject a duplicate name before writing (the server also refuses it, but
    // silently — SET_METADATA has no reply frame).
    let pre = match command_on(
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
            return Err(ExitCode::FAILURE);
        }
        Err(err) => return Err(report_no_server(&err, socket_path, "new")),
    };
    if pre.sessions.iter().any(|s| s.name == name) {
        eprintln!("phux: session '{name}' already exists");
        return Err(ExitCode::FAILURE);
    }

    // Request the create, then read the published result. Frames are ordered
    // on the single connection, so the GET's reply observes the SET's effect.
    conn.send(&FrameKind::SetMetadata {
        request_id: 1,
        scope: Scope::Global,
        key: SESSION_CREATE_KEY.to_owned(),
        value: create_bytes,
    })
    .await
    .map_err(|err| report_no_server(&err, socket_path, "new"))?;
    conn.send(&FrameKind::GetMetadata {
        request_id: 2,
        scope: Scope::Global,
        key: SESSION_CREATE_RESULT_KEY.to_owned(),
    })
    .await
    .map_err(|err| report_no_server(&err, socket_path, "new"))?;

    let result_value = loop {
        match conn.recv().await {
            Ok(FrameKind::MetadataValue {
                request_id: 2,
                value,
            }) => break value,
            Ok(_) => {}
            Err(err) => return Err(report_no_server(&err, socket_path, "new")),
        }
    };
    let not_registered = || {
        eprintln!("phux: create-session failed: server did not register session '{name}'");
        ExitCode::FAILURE
    };
    let bytes = result_value.ok_or_else(not_registered)?;
    serde_json::from_slice::<serde_json::Value>(&bytes)
        .ok()
        .filter(|v| v.get("name").and_then(serde_json::Value::as_str) == Some(name))
        .and_then(|v| v.get("terminal_id").and_then(serde_json::Value::as_u64))
        .ok_or_else(not_registered)
}

/// `base` if it is free, otherwise `base-2`, `base-3`, … — the first
/// available name. Lets `phux new` (no name given) reuse the configured
/// session-name-template as its base and still guarantee a distinct
/// session each time, instead of emitting bare numeric names ("0", "1").
pub(crate) fn unique_session_name(existing: &[String], base: &str) -> String {
    if !existing.iter().any(|e| e == base) {
        return base.to_owned();
    }
    let mut n: u32 = 2;
    loop {
        let candidate = format!("{base}-{n}");
        if !existing.iter().any(|e| e == &candidate) {
            return candidate;
        }
        n = n.saturating_add(1);
    }
}

#[cfg(test)]
mod tests {
    use super::unique_session_name;

    #[test]
    fn unique_session_name_uses_the_base_then_numeric_suffixes() {
        // Free base ⇒ the base verbatim (no "-2" churn, no bare "0").
        assert_eq!(unique_session_name(&[], "default"), "default");
        assert_eq!(
            unique_session_name(&["other".to_owned()], "default"),
            "default",
        );
        // Base taken ⇒ first free `base-N`, starting at 2.
        assert_eq!(
            unique_session_name(&["default".to_owned()], "default"),
            "default-2",
        );
        assert_eq!(
            unique_session_name(&["default".to_owned(), "default-2".to_owned()], "default",),
            "default-3",
        );
        // A non-default template base works the same way.
        assert_eq!(unique_session_name(&["phux".to_owned()], "phux"), "phux-2");
    }
}
