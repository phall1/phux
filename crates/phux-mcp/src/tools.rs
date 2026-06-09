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
use phux_client::selector::{self, Selector};
use phux_client::wait::{Condition, DEFAULT_IDLE_DWELL, DEFAULT_POLL_INTERVAL, WaitOutcome};
use phux_protocol::ids::TerminalId;
use phux_protocol::wire::frame::{
    Command as WireCommand, CommandResult, CommandValue, FrameKind, StateScope,
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

/// Shared `inputSchema` description for the `target` selector argument.
///
/// The four targeted tools accept the CLI's full `TARGET` grammar
/// (`docs/consumers/tui.md` §3), resolved client-side against a `GET_STATE`
/// snapshot exactly as the CLI resolves it (ADR-0021).
const TARGET_DESC: &str = "Target selector: session, session:window, \
    session:window.pane, @paneid, or `.`/`=` for the focused session. Omit \
    for the focused/last session.";

/// The MCP tool catalog: name, description, and JSON-Schema input shape.
///
/// Returned verbatim by `tools/list`. Schemas are minimal but valid JSON
/// Schema (`type: object` with `properties`/`required`).
#[must_use]
#[allow(
    clippy::too_many_lines,
    reason = "one flat JSON literal — the tool catalog; splitting it hurts readability"
)]
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
            "description": "Capture a pane as structured screen data (side-effect-free; does not attach or resize).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "target": { "type": "string", "description": TARGET_DESC },
                    "scrollback": { "type": "number", "description": "Include scrollback history. 0 = all retained history; N = the most-recent N rows. Omit for the viewport only." },
                    "cells": { "type": "boolean", "description": "When true, include per-cell OSC-133 marks and styles. Default false." },
                    "socket": { "type": "string" }
                }
            }
        },
        {
            "name": "phux_send_keys",
            "description": "Send input to a pane. Each key is a named key (Enter, Tab, C-c, ...) or a literal string, tmux-style.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "target": { "type": "string", "description": TARGET_DESC },
                    "keys": { "type": "array", "items": { "type": "string" } },
                    "socket": { "type": "string" }
                },
                "required": ["target", "keys"]
            }
        },
        {
            "name": "phux_run",
            "description": "Run a command in a pane and report its exit code, output, and duration. Assumes a POSIX shell.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "target": { "type": "string", "description": TARGET_DESC },
                    "command": { "type": "string" },
                    "timeout_secs": { "type": "number", "description": "Give up after this many seconds. Default 600; 0 waits indefinitely." },
                    "socket": { "type": "string" }
                },
                "required": ["target", "command"]
            }
        },
        {
            "name": "phux_wait",
            "description": "Poll a pane until it contains text (`until`) or settles (`idle_ms`). Returns whether the condition was met.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "target": { "type": "string", "description": TARGET_DESC },
                    "until": { "type": "string", "description": "Succeed once any visible line contains this substring." },
                    "idle_ms": { "type": "number", "description": "Succeed once the screen holds still this long. Default when `until` is absent." },
                    "timeout_secs": { "type": "number", "description": "Give up after this many seconds. Default: wait forever." },
                    "socket": { "type": "string" }
                }
            }
        },
        {
            "name": "phux_new",
            "description": "Create a named session on the running server without attaching, returning its name and seed pane id. The server must already be running (this does not auto-spawn one).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "Name for the new session. Required; a name already in use is rejected." },
                    "command": { "type": "array", "items": { "type": "string" }, "description": "Initial command (argv) for the seed pane. Omit or pass an empty array to use the server's default shell." },
                    "cwd": { "type": "string", "description": "Working directory for the seed pane." },
                    "socket": { "type": "string" }
                },
                "required": ["name"]
            }
        },
        {
            "name": "phux_kill",
            "description": "Kill the Terminal(s) a selector resolves to (a whole session, a window, a pane, or `#tag`). Atomic for a group via KILL_TERMINALS.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "target": { "type": "string", "description": TARGET_DESC },
                    "socket": { "type": "string" }
                },
                "required": ["target"]
            }
        },
        {
            "name": "phux_watch",
            "description": "Collect server-pushed events (command_started/finished, title_changed, bell, pane_spawned/closed, dirty, idle) for a pane or server-wide. Bounded one-shot: returns after max_events or timeout_secs.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "target": { "type": "string", "description": "Pane selector to watch. Omit to watch server-wide events." },
                    "max_events": { "type": "number", "description": "Return after collecting this many events. Omit for no count cap." },
                    "timeout_secs": { "type": "number", "description": "Return after this many seconds regardless of count. Strongly recommended — without it the call blocks until the server exits." },
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
        "phux_new" => phux_new(args).await,
        "phux_kill" => phux_kill(args).await,
        "phux_watch" => phux_watch(args).await,
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

/// `phux_snapshot` — read a pane as structured data.
///
/// `scrollback` is tri-state, matching `phux snapshot --scrollback`:
/// absent ⇒ viewport only; `0` ⇒ all retained history; `N` ⇒ the
/// most-recent `N` rows. `cells` adds per-cell OSC-133 marks + styles.
async fn phux_snapshot(args: &Value) -> Result<Value, ToolError> {
    let socket = socket::resolve(str_arg(args, "socket"));
    let selector = parse_target(args)?;
    let scrollback = u32_arg(args, "scrollback");
    let cells = bool_arg(args, "cells").unwrap_or(false);
    let snapshot = get_state(&socket).await?;
    let terminal_id = resolve_one(&selector, &snapshot)?;
    let screen =
        phux_client::snapshot::get_screen_scrollback(&socket, terminal_id, scrollback, cells)
            .await?;
    serde_json::to_value(&screen)
        .map_err(|err| ToolError::new(format!("failed to serialize screen: {err}")))
}

/// `phux_send_keys` — send input to the pane named by the selector.
async fn phux_send_keys(args: &Value) -> Result<Value, ToolError> {
    let socket = socket::resolve(str_arg(args, "socket"));
    let selector = required_target(args)?;
    let keys = string_array(args, "keys")?;
    if keys.is_empty() {
        return Err(ToolError::new(
            "`keys` must be a non-empty array of strings",
        ));
    }
    let snapshot = get_state(&socket).await?;
    let pane = resolve_one(&selector, &snapshot)?;
    // `send_to` returns `()`; echo the pane we resolved ourselves.
    phux_client::send_keys::send_to(&socket, pane.clone(), &keys).await?;
    Ok(json!({ "sent": true, "pane": pane_value(&pane) }))
}

/// `phux_run` — run a command in the pane named by the selector.
async fn phux_run(args: &Value) -> Result<Value, ToolError> {
    let socket = socket::resolve(str_arg(args, "socket"));
    let selector = required_target(args)?;
    let command = required_str(args, "command")?;
    // None ⇒ default 600s; 0 ⇒ wait indefinitely; N ⇒ N seconds. Mirrors
    // `phux run`'s `--timeout` semantics.
    let timeout = match num_arg(args, "timeout_secs") {
        None => Some(Duration::from_secs(RUN_DEFAULT_TIMEOUT_SECS)),
        Some(0) => None,
        Some(secs) => Some(Duration::from_secs(secs)),
    };
    let snapshot = get_state(&socket).await?;
    let pane = resolve_one(&selector, &snapshot)?;
    let nonce = run_nonce();
    match phux_client::run::run_in(&socket, pane, command, &nonce, timeout).await? {
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

/// `phux_wait` — poll the pane named by the selector until a condition holds.
async fn phux_wait(args: &Value) -> Result<Value, ToolError> {
    let socket = socket::resolve(str_arg(args, "socket"));
    let selector = parse_target(args)?;
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
    let terminal_id = resolve_one(&selector, &snapshot)?;
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

/// `phux_new` — create a named session without attaching.
///
/// Mirrors `phux new --json`: `name` is required (the create-only path never
/// auto-names), `command`/`cwd` are optional. The server must already be
/// running; unlike the CLI this never auto-spawns one. Returns
/// `{session, terminal_id}`, where `terminal_id` is the seed pane's local id.
///
/// Since the v0.3.0 "Option B" re-tier (ADR-0019 / ADR-0027) removed the
/// `CREATE_SESSION` verb, this writes the conventional `SESSION_CREATE_KEY`
/// L3 metadata key and reads the seed-pane id back from
/// `SESSION_CREATE_RESULT_KEY` (`SET_METADATA` has no reply frame).
async fn phux_new(args: &Value) -> Result<Value, ToolError> {
    let socket = socket::resolve(str_arg(args, "socket"));
    let name = required_str(args, "name")?.to_owned();
    let cwd = str_arg(args, "cwd").map(str::to_owned);
    // Absent or empty `command` ⇒ None (server default shell), mirroring the
    // CLI's `if command.is_empty() { None }`.
    let command = string_array_opt(args, "command")?.filter(|c| !c.is_empty());

    create_session(&socket, &name, command, cwd).await
}

/// `phux_kill` — tear down the Terminal(s) a selector resolves to.
///
/// Resolves the selector client-side to its full id set (a whole session, a
/// window, a pane, or `@id`) and sends one atomic `KILL_TERMINALS { ids }`,
/// the same op `phux kill` uses. A clean server disconnect after the kill
/// (the server self-exits once its last session is reaped) is success, not a
/// failure.
async fn phux_kill(args: &Value) -> Result<Value, ToolError> {
    let socket = socket::resolve(str_arg(args, "socket"));
    let selector = required_target(args)?;
    let snapshot = get_state(&socket).await?;
    let ids = selector::resolve(&selector, &snapshot);
    if ids.is_empty() {
        return Err(ToolError::new("no such target"));
    }
    let count = ids.len();

    let mut conn = Connection::connect(&socket).await?;
    conn.send(&FrameKind::Command {
        request_id: 1,
        command: WireCommand::KillTerminals { ids },
    })
    .await?;
    loop {
        match conn.recv().await {
            Ok(FrameKind::CommandResult {
                request_id: 1,
                result,
            }) => {
                return match result {
                    CommandResult::Ok => Ok(json!({ "killed": count })),
                    CommandResult::Error { message, .. } => Err(ToolError::new(message)),
                    other => Err(ToolError::new(format!("unexpected kill result: {other:?}"))),
                };
            }
            Ok(_) => {}
            // Server closed after reaping its last session: the kill landed.
            Err(AttachError::Disconnected) => return Ok(json!({ "killed": count })),
            Err(err) => return Err(err.into()),
        }
    }
}

/// `phux_watch` — collect server-pushed agent events, bounded.
///
/// The streaming `phux watch` doesn't fit a request/response tool call, so
/// this returns a finite batch: it stops at `max_events`, after
/// `timeout_secs`, or when the server closes, whichever comes first, and
/// returns the collected events as structured JSON. Omitting both bounds
/// blocks until the server exits — callers SHOULD pass `timeout_secs`.
async fn phux_watch(args: &Value) -> Result<Value, ToolError> {
    let socket = socket::resolve(str_arg(args, "socket"));
    // `target` is optional: absent ⇒ server-wide subscription.
    let terminal = match str_arg(args, "target") {
        None => None,
        Some(raw) => {
            let selector = selector::parse(raw)
                .map_err(|err| ToolError::new(format!("invalid target '{raw}': {err}")))?;
            let snapshot = get_state(&socket).await?;
            Some(resolve_one(&selector, &snapshot)?)
        }
    };
    let max_events = num_arg(args, "max_events").and_then(|n| usize::try_from(n).ok());
    let timeout = num_arg(args, "timeout_secs").map(Duration::from_secs);

    let events = phux_client::watch::collect_events(&socket, terminal, max_events, timeout).await?;
    let rendered: Vec<Value> = events.iter().map(agent_event_json).collect();
    Ok(json!({ "events": rendered, "count": rendered.len() }))
}

/// Project one [`phux_client::watch::WatchEvent`] to the stable JSON shape the
/// CLI's `phux watch --json` emits (a `event` name plus the payload field).
fn agent_event_json(ev: &phux_client::watch::WatchEvent) -> Value {
    use phux_protocol::wire::frame::AgentEvent;
    let (kind, mut obj) = match &ev.event {
        AgentEvent::CommandStarted => ("command_started", json!({})),
        AgentEvent::CommandFinished { exit_code } => {
            ("command_finished", json!({ "exit_code": exit_code }))
        }
        AgentEvent::TitleChanged { title } => ("title_changed", json!({ "title": title })),
        AgentEvent::Bell => ("bell", json!({})),
        AgentEvent::PaneSpawned => ("pane_spawned", json!({})),
        AgentEvent::PaneClosed { exit_status } => {
            ("pane_closed", json!({ "exit_status": exit_status }))
        }
        AgentEvent::Dirty => ("dirty", json!({})),
        AgentEvent::Idle => ("idle", json!({})),
        AgentEvent::Unknown { tag, .. } => ("unknown", json!({ "tag": tag })),
        // `AgentEvent` is `#[non_exhaustive]`: a future minor may add a kind
        // this build predates. Surface it generically rather than failing.
        _ => ("unknown", json!({})),
    };
    if let Value::Object(map) = &mut obj {
        map.insert("event".to_owned(), Value::from(kind));
        if let Some(t) = &ev.terminal {
            map.insert("terminal".to_owned(), Value::from(format!("{t:?}")));
        }
    }
    obj
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

/// Open a connection and issue a `CREATE_SESSION` command, returning its
/// [`CommandResult`]. Self-contained over the low-level [`Connection`] — the
/// same wire-call pattern as [`get_state`], reaching the daemon without any
/// `phux-client` agent API.
/// Create a named session via the conventional `SESSION_CREATE_KEY` L3
/// write, then read the seed-pane id back from `SESSION_CREATE_RESULT_KEY`.
/// Returns `{session, terminal_id}` on success. A duplicate name (checked
/// against a pre-write `GET_STATE`) or a server-side seed failure (absent
/// result key) is a [`ToolError`].
async fn create_session(
    socket: &std::path::Path,
    name: &str,
    command: Option<Vec<String>>,
    cwd: Option<String>,
) -> Result<Value, ToolError> {
    use phux_protocol::wire::frame::{SESSION_CREATE_KEY, SESSION_CREATE_RESULT_KEY, Scope};

    let mut conn = Connection::connect(socket).await?;

    // Reject a duplicate name before writing (the server refuses it too, but
    // silently — SET_METADATA has no reply frame).
    let snap = get_state_on(&mut conn).await?;
    if snap.sessions.iter().any(|s| s.name == name) {
        return Err(ToolError::new(format!("session {name:?} already exists")));
    }

    let create_value = json!({ "name": name, "command": command, "cwd": cwd });
    let create_bytes = serde_json::to_vec(&create_value)
        .map_err(|err| ToolError::new(format!("failed to serialize create request: {err}")))?;
    conn.send(&FrameKind::SetMetadata {
        request_id: 1,
        scope: Scope::Global,
        key: SESSION_CREATE_KEY.to_owned(),
        value: create_bytes,
    })
    .await?;

    conn.send(&FrameKind::GetMetadata {
        request_id: 2,
        scope: Scope::Global,
        key: SESSION_CREATE_RESULT_KEY.to_owned(),
    })
    .await?;
    let value = loop {
        if let FrameKind::MetadataValue {
            request_id: 2,
            value,
        } = conn.recv().await?
        {
            break value;
        }
    };
    let bytes =
        value.ok_or_else(|| ToolError::new(format!("server did not register session {name:?}")))?;
    let terminal_id = serde_json::from_slice::<Value>(&bytes)
        .ok()
        .filter(|v| v.get("name").and_then(Value::as_str) == Some(name))
        .and_then(|v| v.get("terminal_id").and_then(Value::as_u64))
        .ok_or_else(|| ToolError::new(format!("server did not register session {name:?}")))?;
    Ok(json!({ "session": name, "terminal_id": terminal_id }))
}

/// Send `GET_STATE` over an existing connection and return the snapshot.
async fn get_state_on(conn: &mut Connection) -> Result<SessionSnapshot, ToolError> {
    conn.send(&FrameKind::Command {
        request_id: 100,
        command: WireCommand::GetState {
            scope: StateScope::Server,
        },
    })
    .await?;
    loop {
        if let FrameKind::CommandResult {
            request_id: 100,
            result,
        } = conn.recv().await?
        {
            return match result {
                CommandResult::OkWith(CommandValue::State(snap)) => Ok(snap),
                other => Err(ToolError::new(format!(
                    "unexpected GET_STATE result: {other:?}"
                ))),
            };
        }
    }
}

/// Parse the optional `target` argument into a [`Selector`], defaulting to
/// the focused/last session when absent. Mirrors the CLI's `parse_selector`
/// front door (phux-n95).
///
/// # Errors
///
/// Returns [`ToolError`] when an explicit `target` is present but malformed
/// (e.g. `@nope`, `work:1.x`).
fn parse_target(args: &Value) -> Result<Selector, ToolError> {
    str_arg(args, "target").map_or(Ok(Selector::Last), |raw| {
        selector::parse(raw).map_err(|err| ToolError::new(format!("invalid target '{raw}': {err}")))
    })
}

/// Parse a required `target` argument into a [`Selector`]. Used by tools
/// where the target is not optional (`send_keys`, `run`).
///
/// # Errors
///
/// Returns [`ToolError`] when `target` is missing/not a string, or present
/// but malformed.
fn required_target(args: &Value) -> Result<Selector, ToolError> {
    let raw = required_str(args, "target")?;
    selector::parse(raw).map_err(|err| ToolError::new(format!("invalid target '{raw}': {err}")))
}

/// Resolve `selector` against `snapshot` to a single pane, exactly as the
/// CLI does (ADR-0021): resolve to the candidate terminals, then narrow via
/// [`selector::pick_target_pane`] (prefer the focused pane, else the first).
///
/// # Errors
///
/// Returns [`ToolError`] when the selector matches no pane.
fn resolve_one(selector: &Selector, snapshot: &SessionSnapshot) -> Result<TerminalId, ToolError> {
    let candidates = selector::resolve(selector, snapshot);
    selector::pick_target_pane(&candidates, &snapshot.focused_pane)
        .ok_or_else(|| ToolError::new("no such target"))
}

/// A JSON rendering of a `TerminalId` for tool output.
fn pane_value(id: &TerminalId) -> Value {
    // TODO(phux-93b): TerminalId has no Serialize impl in phux-protocol
    // (it avoids serde to keep a near-empty publish profile); render it via
    // Debug for now. A stable numeric projection would be nicer.
    json!(format!("{id:?}"))
}

/// A per-call sentinel nonce for `phux_run`, matching `phux run`'s
/// `run_nonce`: pid (concurrent processes) + epoch-nanos (residual sentinels
/// from earlier processes) + a process-global monotonic counter so two calls
/// in one clock tick can't collide. The counter matters most here: an MCP
/// host can fire `phux_run` calls back-to-back within a single `SystemTime`
/// tick, which pid+nanos alone would not distinguish.
fn run_nonce() -> String {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    format!("{}x{nanos}x{seq}", std::process::id())
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

/// Read an optional non-negative integer argument as `u32`. A value that is
/// not a non-negative integer, or that overflows `u32`, is treated as
/// absent. Used for `scrollback`, whose `None`/`Some(0)`/`Some(n)` triad is
/// load-bearing (viewport / all history / last-n rows).
fn u32_arg(args: &Value, key: &str) -> Option<u32> {
    num_arg(args, key).and_then(|n| u32::try_from(n).ok())
}

/// Read an optional boolean argument. Non-boolean values are treated as
/// absent.
fn bool_arg(args: &Value, key: &str) -> Option<bool> {
    args.get(key).and_then(Value::as_bool)
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

/// Read an optional array-of-strings argument. Absent ⇒ `None`; present must
/// be an array whose every element is a string (a non-string element errors).
/// An empty array yields `Some(vec![])` — callers that treat empty as absent
/// filter it themselves.
fn string_array_opt(args: &Value, key: &str) -> Result<Option<Vec<String>>, ToolError> {
    let Some(value) = args.get(key) else {
        return Ok(None);
    };
    let arr = value
        .as_array()
        .ok_or_else(|| ToolError::new(format!("`{key}` must be an array of strings")))?;
    arr.iter()
        .map(|v| {
            v.as_str()
                .map(str::to_owned)
                .ok_or_else(|| ToolError::new(format!("`{key}` must contain only strings")))
        })
        .collect::<Result<Vec<String>, _>>()
        .map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;
    use phux_protocol::ids::{SessionId, WindowId};
    use phux_protocol::wire::info::{SessionInfo, TerminalInfo, WindowInfo};

    #[test]
    fn catalog_lists_all_tools_with_object_schemas() {
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
                "phux_wait",
                "phux_new",
                "phux_kill",
                "phux_watch",
            ]
        );
        for tool in arr {
            assert_eq!(tool["inputSchema"]["type"], json!("object"));
            assert!(tool["description"].is_string());
        }
    }

    /// `phux_new` exposes a required `name` and optional `cwd`/`command`/
    /// `socket` props, with `command` typed as a string array.
    #[test]
    fn catalog_phux_new_requires_name_and_object_schema() {
        let cat = catalog();
        let arr = cat.as_array().expect("catalog is an array");
        let new = arr
            .iter()
            .find(|t| t["name"] == json!("phux_new"))
            .expect("phux_new present");

        assert_eq!(new["inputSchema"]["type"], json!("object"));
        assert_eq!(new["inputSchema"]["required"], json!(["name"]));

        let props = &new["inputSchema"]["properties"];
        assert_eq!(props["name"]["type"], json!("string"));
        assert_eq!(props["cwd"]["type"], json!("string"));
        assert_eq!(props["socket"]["type"], json!("string"));
        assert_eq!(props["command"]["type"], json!("array"));
        assert_eq!(props["command"]["items"]["type"], json!("string"));
    }

    /// `string_array_opt` maps absent ⇒ None, empty ⇒ Some([]), strings ⇒
    /// Some(vec), and errors on a non-array or a non-string element.
    #[test]
    fn string_array_opt_handles_absent_empty_and_strings() {
        assert_eq!(string_array_opt(&json!({}), "command").unwrap(), None);
        assert_eq!(
            string_array_opt(&json!({ "command": [] }), "command").unwrap(),
            Some(vec![]),
        );
        assert_eq!(
            string_array_opt(&json!({ "command": ["ssh", "host"] }), "command").unwrap(),
            Some(vec!["ssh".to_owned(), "host".to_owned()]),
        );
        // Non-array and non-string element both error.
        assert!(string_array_opt(&json!({ "command": "ssh host" }), "command").is_err());
        assert!(string_array_opt(&json!({ "command": ["ssh", 7] }), "command").is_err());
    }

    /// The grown snapshot surface: `scrollback` + `cells` params, plus the
    /// unified `target` selector (no more `session`-name-only `session`
    /// param) on all four targeted tools.
    #[test]
    fn catalog_exposes_scrollback_cells_and_target_selector() {
        let cat = catalog();
        let arr = cat.as_array().expect("catalog is an array");
        let tool = |name: &str| {
            arr.iter()
                .find(|t| t["name"] == json!(name))
                .unwrap_or_else(|| panic!("missing tool {name}"))
                .clone()
        };

        let snap = tool("phux_snapshot");
        let props = &snap["inputSchema"]["properties"];
        assert_eq!(props["scrollback"]["type"], json!("number"));
        assert_eq!(props["cells"]["type"], json!("boolean"));
        // snapshot's selector is optional (no `required`).
        assert!(snap["inputSchema"].get("required").is_none());

        // Every targeted tool documents the unified `target` selector and
        // dropped the old `session`-only param.
        for name in ["phux_snapshot", "phux_send_keys", "phux_run", "phux_wait"] {
            let t = tool(name);
            let p = &t["inputSchema"]["properties"];
            assert!(p["target"].is_object(), "{name} missing target");
            assert!(p.get("session").is_none(), "{name} still has session");
        }

        // send_keys/run now require `target` (not `session`).
        assert_eq!(
            tool("phux_send_keys")["inputSchema"]["required"],
            json!(["target", "keys"]),
        );
        assert_eq!(
            tool("phux_run")["inputSchema"]["required"],
            json!(["target", "command"]),
        );

        // phux-yhyi: kill requires a target; watch's target is optional.
        assert_eq!(
            tool("phux_kill")["inputSchema"]["required"],
            json!(["target"]),
        );
        assert!(tool("phux_watch")["inputSchema"].get("required").is_none());
    }

    /// `agent_event_json` projects each event kind to the same stable shape
    /// the CLI's `phux watch --json` emits (`event` name + payload field).
    #[test]
    fn agent_event_json_projects_kind_and_payload() {
        use phux_client::watch::WatchEvent;
        use phux_protocol::wire::frame::AgentEvent;

        let ev = WatchEvent {
            terminal: None,
            event: AgentEvent::CommandFinished {
                exit_code: Some(42),
            },
        };
        let v = agent_event_json(&ev);
        assert_eq!(v["event"], json!("command_finished"));
        assert_eq!(v["exit_code"], json!(42));

        let bell = WatchEvent {
            terminal: None,
            event: AgentEvent::Bell,
        };
        assert_eq!(agent_event_json(&bell)["event"], json!("bell"));

        let titled = WatchEvent {
            terminal: None,
            event: AgentEvent::TitleChanged {
                title: "vim".to_owned(),
            },
        };
        let tv = agent_event_json(&titled);
        assert_eq!(tv["event"], json!("title_changed"));
        assert_eq!(tv["title"], json!("vim"));
    }

    /// `scrollback`/`cells` arg plumbing: the tri-state scrollback and the
    /// optional bool map as documented.
    #[test]
    fn scrollback_and_cells_args_parse() {
        // Absent → None (viewport only); 0 → Some(0) (all history); N → N.
        assert_eq!(u32_arg(&json!({}), "scrollback"), None);
        assert_eq!(u32_arg(&json!({ "scrollback": 0 }), "scrollback"), Some(0));
        assert_eq!(
            u32_arg(&json!({ "scrollback": 25 }), "scrollback"),
            Some(25)
        );
        // Negative / overflowing values are treated as absent.
        assert_eq!(u32_arg(&json!({ "scrollback": -3 }), "scrollback"), None);
        assert_eq!(
            u32_arg(
                &json!({ "scrollback": u64::from(u32::MAX) + 1 }),
                "scrollback"
            ),
            None,
        );

        assert_eq!(bool_arg(&json!({}), "cells"), None);
        assert_eq!(bool_arg(&json!({ "cells": true }), "cells"), Some(true));
        assert_eq!(bool_arg(&json!({ "cells": false }), "cells"), Some(false));
        assert_eq!(bool_arg(&json!({ "cells": "yes" }), "cells"), None);
    }

    /// `parse_target` is the optional-selector front door (snapshot/wait):
    /// absent ⇒ `Last`, every grammar form parses, malformed ⇒ error.
    #[test]
    fn parse_target_defaults_and_accepts_grammar() {
        assert_eq!(parse_target(&json!({})).unwrap(), Selector::Last);
        assert_eq!(
            parse_target(&json!({ "target": "." })).unwrap(),
            Selector::Current,
        );
        assert_eq!(
            parse_target(&json!({ "target": "work:1.2" })).unwrap(),
            Selector::Pane("work".to_owned(), selector::WindowRef::Index(1), 2),
        );
        assert_eq!(
            parse_target(&json!({ "target": "@100" })).unwrap(),
            Selector::TerminalId(100),
        );
        // Malformed → error (no server round trip).
        assert!(parse_target(&json!({ "target": "@nope" })).is_err());
    }

    /// `required_target` (the `send_keys`/`run` front door) rejects a
    /// missing target and a malformed one alike.
    #[test]
    fn required_target_demands_a_selector() {
        assert!(required_target(&json!({})).is_err());
        assert_eq!(
            required_target(&json!({ "target": "work" })).unwrap(),
            Selector::Session("work".to_owned()),
        );
        assert!(required_target(&json!({ "target": "work:1.x" })).is_err());
    }

    /// `resolve_one` maps each selector form to the expected pane against a
    /// multi-session/window/pane snapshot, exactly as the CLI does, and
    /// errors on a miss.
    #[test]
    fn resolve_one_maps_every_selector_form() {
        let snap = fixture();
        let one = |target: &str| resolve_one(&selector::parse(target).unwrap(), &snap);

        // Bare session → focused-or-first pane of the session.
        assert_eq!(one("work").unwrap(), TerminalId::local(100));
        // Window by index and by tag → the window's first pane.
        assert_eq!(one("work:1").unwrap(), TerminalId::local(101));
        assert_eq!(one("work:editor").unwrap(), TerminalId::local(101));
        // Exact pane.
        assert_eq!(one("work:1.1").unwrap(), TerminalId::local(102));
        // Opaque terminal id.
        assert_eq!(one("@200").unwrap(), TerminalId::local(200));
        // `.`/`=` → the focused session's focused pane.
        assert_eq!(
            resolve_one(&Selector::Current, &snap).unwrap(),
            TerminalId::local(100),
        );
        assert_eq!(
            resolve_one(&Selector::Last, &snap).unwrap(),
            TerminalId::local(100),
        );
        // Misses error.
        assert!(one("ghost").is_err());
        assert!(one("@999").is_err());
    }

    /// Build a snapshot: session "work" (id 1, focused, pane 100 focused)
    /// with two windows, plus a second session "play" (pane 200).
    fn fixture() -> SessionSnapshot {
        let work = SessionId::new(1);
        let play = SessionId::new(2);
        let w0 = WindowId::new(10);
        let w1 = WindowId::new(11);
        let p0 = WindowId::new(20);
        let sessions = vec![
            SessionInfo::new(work, "work"),
            SessionInfo::new(play, "play"),
        ];
        let windows = vec![
            WindowInfo::new(w0, work, "shell").with_index(0),
            WindowInfo::new(w1, work, "editor").with_index(1),
            WindowInfo::new(p0, play, "shell").with_index(0),
        ];
        let panes = vec![
            TerminalInfo::new(TerminalId::local(100), w0, 80, 24),
            TerminalInfo::new(TerminalId::local(101), w1, 80, 24),
            TerminalInfo::new(TerminalId::local(102), w1, 80, 24),
            TerminalInfo::new(TerminalId::local(200), p0, 80, 24),
        ];
        SessionSnapshot::new(work, w0, TerminalId::local(100))
            .with_sessions(sessions)
            .with_windows(windows)
            .with_panes(panes)
    }
}
