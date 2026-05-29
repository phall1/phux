//! The phux tool catalog and `tools/call` dispatch.
//!
//! Each tool is a thin wrapper over the `phux-client` agent surface
//! (`snapshot`, `send_keys`, `run`, `wait`) or a direct control-plane
//! command (`GET_STATE`). MCP is a thin adapter over the same structured
//! surface the CLI uses — not a separate core (ADR-0022 §5).
//!
//! A tool either returns a JSON `Value` (serialized into the MCP
//! `content[0].text` field) or a [`ToolError`] carrying a readable message
//! that becomes a `tools/call` result with `isError: true`. Tool failures
//! never crash the JSON-RPC loop.

use std::time::Duration;

use phux_client::attach::AttachError;
use phux_client::attach::connection::Connection;
use phux_client::run::RunOutcome;
use phux_client::wait::{Condition, DEFAULT_IDLE_DWELL, DEFAULT_POLL_INTERVAL, WaitOutcome};
use phux_protocol::ids::TerminalId;
use phux_protocol::wire::frame::{
    AttachTarget, Command as WireCommand, CommandResult, CommandValue, FrameKind, StateScope,
};
use phux_protocol::wire::info::SessionSnapshot;
use serde_json::{Value, json};

use crate::socket;

/// A tool-level failure: surfaced to the caller as a `tools/call` result
/// with `isError: true`, never as a process crash.
#[derive(Debug)]
pub(crate) struct ToolError(pub(crate) String);

impl ToolError {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl From<AttachError> for ToolError {
    fn from(err: AttachError) -> Self {
        Self(err.to_string())
    }
}

/// The MCP tool catalog: name, description, and JSON-Schema input shape.
///
/// Returned verbatim by `tools/list`. Schemas are minimal but valid JSON
/// Schema (`type: object` with `properties`/`required`).
#[must_use]
pub(crate) fn catalog() -> Value {
    json!([
        {
            "name": "phux_ls",
            "description": "List phux sessions on the running server (names, window counts, attached-client counts).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "socket": { "type": "string", "description": "Override the UDS path. Defaults to PHUX_SOCKET or the daemon default." }
                }
            }
        },
        {
            "name": "phux_snapshot",
            "description": "Capture a session's focused pane as structured screen data (side-effect-free; does not attach or resize).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session": { "type": "string", "description": "Session name. Omit for the focused/last session." },
                    "socket": { "type": "string" }
                }
            }
        },
        {
            "name": "phux_send_keys",
            "description": "Send input to a session's focused pane. Each key is a named key (Enter, Tab, C-c, ...) or a literal string, tmux-style.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session": { "type": "string" },
                    "keys": { "type": "array", "items": { "type": "string" } },
                    "socket": { "type": "string" }
                },
                "required": ["session", "keys"]
            }
        },
        {
            "name": "phux_run",
            "description": "Run a command in a session's focused pane and report its exit code, output, and duration. Assumes a POSIX shell.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session": { "type": "string" },
                    "command": { "type": "string" },
                    "timeout_secs": { "type": "number", "description": "Give up after this many seconds. Default 600; 0 waits indefinitely." },
                    "socket": { "type": "string" }
                },
                "required": ["session", "command"]
            }
        },
        {
            "name": "phux_wait",
            "description": "Poll a session's pane until it contains text (`until`) or settles (`idle_ms`). Returns whether the condition was met.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session": { "type": "string", "description": "Session name. Omit for the focused/last session." },
                    "until": { "type": "string", "description": "Succeed once any visible line contains this substring." },
                    "idle_ms": { "type": "number", "description": "Succeed once the screen holds still this long. Default when `until` is absent." },
                    "timeout_secs": { "type": "number", "description": "Give up after this many seconds. Default: wait forever." },
                    "socket": { "type": "string" }
                }
            }
        }
    ])
}

/// Dispatch a `tools/call` by tool name. Returns the tool's JSON result on
/// success, or a [`ToolError`] (rendered as `isError: true`) on failure.
///
/// # Errors
///
/// Returns [`ToolError`] for an unknown tool, a malformed/missing required
/// argument, or any failure from the underlying agent surface (no server,
/// unknown session, transport error).
pub(crate) async fn dispatch(name: &str, args: &Value) -> Result<Value, ToolError> {
    match name {
        "phux_ls" => phux_ls(args).await,
        "phux_snapshot" => phux_snapshot(args).await,
        "phux_send_keys" => phux_send_keys(args).await,
        "phux_run" => phux_run(args).await,
        "phux_wait" => phux_wait(args).await,
        other => Err(ToolError::new(format!("unknown tool: {other}"))),
    }
}

/// `phux_ls` — list sessions via `GET_STATE`.
async fn phux_ls(args: &Value) -> Result<Value, ToolError> {
    let socket = socket::resolve(str_arg(args, "socket"));
    let snapshot = get_state(&socket).await?;
    let mut sessions: Vec<Value> = snapshot
        .sessions
        .iter()
        .map(|s| {
            json!({
                "name": s.name,
                "window_count": s.window_count,
                "attached_client_count": s.attached_client_count,
            })
        })
        .collect();
    sessions.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
    Ok(json!({ "sessions": sessions }))
}

/// `phux_snapshot` — read a session's focused pane as structured data.
async fn phux_snapshot(args: &Value) -> Result<Value, ToolError> {
    let socket = socket::resolve(str_arg(args, "socket"));
    let session = str_arg(args, "session");
    let snapshot = get_state(&socket).await?;
    let terminal_id = resolve_target(&snapshot, session)?;
    let screen = phux_client::snapshot::get_screen(&socket, terminal_id).await?;
    serde_json::to_value(&screen)
        .map_err(|err| ToolError::new(format!("failed to serialize screen: {err}")))
}

/// `phux_send_keys` — send input to a session's focused pane.
async fn phux_send_keys(args: &Value) -> Result<Value, ToolError> {
    let socket = socket::resolve(str_arg(args, "socket"));
    let session = required_str(args, "session")?;
    let keys = string_array(args, "keys")?;
    if keys.is_empty() {
        return Err(ToolError::new(
            "`keys` must be a non-empty array of strings",
        ));
    }
    let target = AttachTarget::ByName(session.to_owned());
    let pane = phux_client::send_keys::send(&socket, target, &keys).await?;
    Ok(json!({ "sent": true, "pane": pane_value(&pane) }))
}

/// `phux_run` — run a command in a session's focused pane.
async fn phux_run(args: &Value) -> Result<Value, ToolError> {
    let socket = socket::resolve(str_arg(args, "socket"));
    let session = required_str(args, "session")?;
    let command = required_str(args, "command")?;
    // None ⇒ default 600s; 0 ⇒ wait indefinitely; N ⇒ N seconds. Mirrors
    // `phux run`'s `--timeout` semantics.
    let timeout = match num_arg(args, "timeout_secs") {
        None => Some(Duration::from_secs(RUN_DEFAULT_TIMEOUT_SECS)),
        Some(0) => None,
        Some(secs) => Some(Duration::from_secs(secs)),
    };
    let target = AttachTarget::ByName(session.to_owned());
    let nonce = run_nonce();
    match phux_client::run::run(&socket, target, command, &nonce, timeout).await? {
        RunOutcome::Completed(result) => serde_json::to_value(&result)
            .map_err(|err| ToolError::new(format!("failed to serialize run result: {err}"))),
        RunOutcome::TimedOut {
            command,
            duration_ms,
            ..
        } => Ok(json!({
            "outcome": "timed_out",
            "command": command,
            "duration_ms": duration_ms,
        })),
    }
}

/// `phux_wait` — poll a session's pane until a condition holds.
async fn phux_wait(args: &Value) -> Result<Value, ToolError> {
    let socket = socket::resolve(str_arg(args, "socket"));
    let session = str_arg(args, "session");
    // `until` takes precedence; otherwise settle on idle (explicit ms or
    // the default dwell). Mirrors `phux wait`.
    let condition = str_arg(args, "until").map_or_else(
        || {
            let dwell = num_arg(args, "idle_ms").map_or(DEFAULT_IDLE_DWELL, Duration::from_millis);
            Condition::Idle(dwell)
        },
        |needle| Condition::Contains(needle.to_owned()),
    );
    let timeout = num_arg(args, "timeout_secs").map(Duration::from_secs);

    let snapshot = get_state(&socket).await?;
    let terminal_id = resolve_target(&snapshot, session)?;
    let result = phux_client::wait::poll_until(
        &socket,
        terminal_id,
        &condition,
        timeout,
        DEFAULT_POLL_INTERVAL,
    )
    .await?;
    let outcome = match result.outcome {
        WaitOutcome::Met => "met",
        WaitOutcome::TimedOut => "timed_out",
    };
    Ok(json!({ "outcome": outcome, "polls": result.polls }))
}

// -----------------------------------------------------------------------------
// Shared helpers.
// -----------------------------------------------------------------------------

/// Default `phux_run` timeout when `timeout_secs` is unset (seconds).
/// Matches `phux run`'s default so the surfaces agree.
const RUN_DEFAULT_TIMEOUT_SECS: u64 = 600;

/// Open a connection and fetch the server-wide state snapshot via
/// `GET_STATE`.
async fn get_state(socket: &std::path::Path) -> Result<SessionSnapshot, ToolError> {
    let mut conn = Connection::connect(socket).await?;
    conn.send(&FrameKind::Command {
        request_id: 1,
        command: WireCommand::GetState {
            scope: StateScope::Server,
        },
    })
    .await?;
    loop {
        if let FrameKind::CommandResult { request_id, result } = conn.recv().await?
            && request_id == 1
        {
            return match result {
                CommandResult::OkWith(CommandValue::State(snap)) => Ok(snap),
                CommandResult::Error { message, .. } => Err(ToolError::new(message)),
                other => Err(ToolError::new(format!(
                    "unexpected GET_STATE result: {other:?}"
                ))),
            };
        }
    }
}

/// Resolve an optional session name to a single pane against `snapshot`.
///
/// Mirrors `phux`'s `resolve_target`/`pick_target_pane`: a named session
/// resolves to its terminals, defaulting to the focused session when
/// `session` is `None`; among the candidates, prefer the one equal to the
/// server's focused pane, else the first in snapshot order.
fn resolve_target(
    snapshot: &SessionSnapshot,
    session: Option<&str>,
) -> Result<TerminalId, ToolError> {
    let candidates: Vec<TerminalId> = match session {
        // Default: the focused session's panes.
        None => terminals_in_session(snapshot, snapshot.focused_session),
        Some(name) => {
            let Some(sid) = snapshot
                .sessions
                .iter()
                .find(|s| s.name == name)
                .map(|s| s.id)
            else {
                return Err(ToolError::new(format!("no such session: {name}")));
            };
            terminals_in_session(snapshot, sid)
        }
    };
    pick_target_pane(&candidates, &snapshot.focused_pane)
        .ok_or_else(|| ToolError::new("no pane found for target session"))
}

/// All Terminals in `session`, across every window, in snapshot order.
/// Mirrors the CLI selector's `terminals_in_session`.
fn terminals_in_session(
    snapshot: &SessionSnapshot,
    session: phux_protocol::ids::SessionId,
) -> Vec<TerminalId> {
    let window_ids: Vec<_> = snapshot
        .windows
        .iter()
        .filter(|w| w.session_id == session)
        .map(|w| w.id)
        .collect();
    snapshot
        .panes
        .iter()
        .filter(|p| window_ids.contains(&p.window_id))
        .map(|p| p.id.clone())
        .collect()
}

/// Prefer the focused pane among `candidates`, else the first; `None` only
/// when nothing matched. Mirrors `phux`'s `pick_target_pane`.
fn pick_target_pane(candidates: &[TerminalId], focused: &TerminalId) -> Option<TerminalId> {
    candidates
        .iter()
        .find(|id| *id == focused)
        .or_else(|| candidates.first())
        .cloned()
}

/// A JSON rendering of a `TerminalId` for tool output.
fn pane_value(id: &TerminalId) -> Value {
    // TODO(phux-93b): TerminalId has no Serialize impl in phux-protocol
    // (it avoids serde to keep a near-empty publish profile); render it via
    // Debug for now. A stable numeric projection would be nicer.
    json!(format!("{id:?}"))
}

/// A per-call sentinel nonce for `phux_run` (pid + epoch-nanos), matching
/// `phux run`'s `run_nonce`. The pid alone is recycled across processes;
/// mixing in nanoseconds makes a residual sentinel from an earlier run
/// unable to collide with this one.
fn run_nonce() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    format!("{}x{nanos}", std::process::id())
}

/// Read an optional string argument from a tool's params object.
fn str_arg<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(Value::as_str)
}

/// Read a required string argument, erroring with a readable message when
/// it is missing or not a string.
fn required_str<'a>(args: &'a Value, key: &str) -> Result<&'a str, ToolError> {
    str_arg(args, key).ok_or_else(|| ToolError::new(format!("missing required string `{key}`")))
}

/// Read an optional non-negative integer argument (as `u64`). Values that
/// are not non-negative integers are treated as absent.
fn num_arg(args: &Value, key: &str) -> Option<u64> {
    args.get(key).and_then(Value::as_u64)
}

/// Read a required array-of-strings argument.
fn string_array(args: &Value, key: &str) -> Result<Vec<String>, ToolError> {
    let arr = args
        .get(key)
        .and_then(Value::as_array)
        .ok_or_else(|| ToolError::new(format!("missing required array `{key}`")))?;
    arr.iter()
        .map(|v| {
            v.as_str()
                .map(str::to_owned)
                .ok_or_else(|| ToolError::new(format!("`{key}` must contain only strings")))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use phux_protocol::ids::{SessionId, WindowId};
    use phux_protocol::wire::info::{SessionInfo, TerminalInfo, WindowInfo};

    #[test]
    fn catalog_lists_all_five_tools_with_object_schemas() {
        let cat = catalog();
        let arr = cat.as_array().expect("catalog is an array");
        let names: Vec<&str> = arr.iter().filter_map(|t| t["name"].as_str()).collect();
        assert_eq!(
            names,
            vec![
                "phux_ls",
                "phux_snapshot",
                "phux_send_keys",
                "phux_run",
                "phux_wait"
            ]
        );
        for tool in arr {
            assert_eq!(tool["inputSchema"]["type"], json!("object"));
            assert!(tool["description"].is_string());
        }
    }

    #[test]
    fn pick_target_pane_prefers_focused_then_first() {
        let a = TerminalId::local(1);
        let b = TerminalId::local(2);
        assert_eq!(pick_target_pane(&[a.clone(), b.clone()], &b), Some(b));
        assert_eq!(
            pick_target_pane(std::slice::from_ref(&a), &TerminalId::local(9)),
            Some(a)
        );
        assert_eq!(pick_target_pane(&[], &TerminalId::local(1)), None);
    }

    #[test]
    fn resolve_target_defaults_to_focused_session() {
        let work = SessionId::new(1);
        let w0 = WindowId::new(10);
        let focused = TerminalId::local(100);
        let snap = SessionSnapshot::new(work, w0, focused.clone())
            .with_sessions(vec![SessionInfo::new(work, "work")])
            .with_windows(vec![WindowInfo::new(w0, work, "shell")])
            .with_panes(vec![TerminalInfo::new(focused.clone(), w0, 80, 24)]);
        // No session arg → focused session's pane.
        assert_eq!(resolve_target(&snap, None).unwrap(), focused);
        // By name → same pane.
        assert_eq!(resolve_target(&snap, Some("work")).unwrap(), focused);
        // Unknown name → error.
        assert!(resolve_target(&snap, Some("ghost")).is_err());
    }
}
