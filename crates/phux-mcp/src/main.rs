//! `phux-mcp` — a minimal Model Context Protocol stdio adapter over the
//! phux agent surface (phux-93b, ADR-0022 §5 "MCP as a thin adapter").
//!
//! Speaks JSON-RPC 2.0 over the MCP stdio transport: newline-delimited
//! JSON, one message per line on stdin/stdout. The JSON-RPC is hand-rolled
//! over `serde_json` (no framework dep); every tool is a thin wrapper over
//! `phux-client`'s agent surface (`snapshot`, `send_keys`, `run`, `wait`)
//! or a direct `GET_STATE` control command — the same structured surface
//! the CLI uses, never a separate core.
//!
//! Methods:
//! - `initialize` → capabilities + serverInfo.
//! - `notifications/initialized` → no reply (notification).
//! - `tools/list` → the tool catalog.
//! - `tools/call` → dispatch by tool name; tool failures become a result
//!   with `isError: true`, never a process crash.
//!
//! Robustness: malformed JSON or an unknown method yields a JSON-RPC error
//! response; the loop continues until stdin EOF.

#![forbid(unsafe_code)]
// The MCP transport speaks on stdout; writing responses there is the whole
// point of this binary (the workspace lints deny stdout/stderr by default).
#![allow(
    clippy::print_stdout,
    reason = "stdout is the MCP transport for JSON-RPC responses"
)]
#![allow(
    clippy::print_stderr,
    reason = "stderr is the adapter's out-of-band diagnostic channel"
)]
#![allow(
    clippy::redundant_pub_crate,
    reason = "bin-internal modules expose items via `pub`; `pub(crate)` would trip unreachable_pub in a binary with no external API (matches crates/phux/src/main.rs)"
)]

mod jsonrpc;
mod socket;
mod tools;

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, BufReader};

use jsonrpc::{INTERNAL_ERROR, INVALID_REQUEST, METHOD_NOT_FOUND, PARSE_ERROR, Request};

/// The MCP protocol version this adapter implements.
///
/// TODO(phux-93b): pinned to the 2024-11-05 revision the task specifies.
/// Newer MCP revisions are additive; bump when we adopt one.
const MCP_PROTOCOL_VERSION: &str = "2024-11-05";

fn main() -> std::process::ExitCode {
    // Current-thread runtime: the phux client surface is async and its
    // client-side libghostty Terminal is !Send (ADR-0003).
    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(err) => {
            eprintln!("phux-mcp: failed to build runtime: {err}");
            return std::process::ExitCode::FAILURE;
        }
    };
    match rt.block_on(serve()) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("phux-mcp: fatal: {err}");
            std::process::ExitCode::FAILURE
        }
    }
}

/// Read newline-delimited JSON-RPC messages from stdin until EOF, handling
/// each and writing any response to stdout.
///
/// # Errors
///
/// Returns an [`std::io::Error`] only on an stdin read failure (not EOF) —
/// per-message parse/dispatch failures are turned into JSON-RPC error
/// responses, not propagated.
async fn serve() -> std::io::Result<()> {
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    while let Some(line) = lines.next_line().await? {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some(response) = handle_line(trimmed).await {
            emit(&response);
        }
    }
    Ok(())
}

/// Handle one line of input, returning the JSON-RPC response to emit, or
/// `None` for a notification (which gets no reply).
async fn handle_line(line: &str) -> Option<Value> {
    // Parse the envelope. A malformed line is a JSON-RPC parse error with a
    // null id (we cannot recover the request id from unparseable JSON).
    let request: Request = match serde_json::from_str(line) {
        Ok(req) => req,
        Err(err) => {
            return Some(jsonrpc::error(
                Value::Null,
                PARSE_ERROR,
                format!("parse error: {err}"),
            ));
        }
    };
    handle_request(request).await
}

/// Dispatch a parsed [`Request`] to its method handler.
async fn handle_request(request: Request) -> Option<Value> {
    let is_notification = request.is_notification();
    // For a request, echo the id; for a notification there is no reply, but
    // we still carry a placeholder so the error path is uniform.
    let id = request.id.clone().unwrap_or(Value::Null);

    match request.method.as_str() {
        "initialize" => Some(jsonrpc::success(id, initialize_result())),
        "notifications/initialized" => None,
        "tools/list" => Some(jsonrpc::success(id, json!({ "tools": tools::catalog() }))),
        "tools/call" => Some(handle_tools_call(id, request.params.as_ref()).await),
        // `ping` is a common MCP keepalive; reply with an empty result.
        "ping" => Some(jsonrpc::success(id, json!({}))),
        other => {
            if is_notification {
                // Unknown notification: ignore silently (no reply for
                // notifications, per JSON-RPC).
                None
            } else {
                Some(jsonrpc::error(
                    id,
                    METHOD_NOT_FOUND,
                    format!("method not found: {other}"),
                ))
            }
        }
    }
}

/// The `initialize` result: protocol version, advertised capabilities, and
/// server identity. The client's params are tolerated and ignored.
fn initialize_result() -> Value {
    json!({
        "protocolVersion": MCP_PROTOCOL_VERSION,
        "capabilities": { "tools": {} },
        "serverInfo": {
            "name": "phux",
            "version": env!("CARGO_PKG_VERSION"),
        },
    })
}

/// Handle `tools/call`: extract `name`/`arguments`, dispatch, and wrap the
/// outcome in the MCP `content`/`isError` envelope. A tool failure is a
/// *successful* JSON-RPC response carrying `isError: true` — not a
/// JSON-RPC error and never a crash.
async fn handle_tools_call(id: Value, params: Option<&Value>) -> Value {
    let Some(params) = params else {
        return jsonrpc::error(id, INVALID_REQUEST, "tools/call requires params");
    };
    let Some(name) = params.get("name").and_then(Value::as_str) else {
        return jsonrpc::error(id, INVALID_REQUEST, "tools/call requires a string `name`");
    };
    // `arguments` is optional; default to an empty object so tools that take
    // no required args (e.g. phux_ls) work without it.
    let args = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));

    match tools::dispatch(name, &args).await {
        Ok(value) => jsonrpc::success(id, tool_content(&value, false)),
        Err(tools::ToolError(message)) => {
            jsonrpc::success(id, tool_content(&Value::String(message), true))
        }
    }
}

/// Build the MCP `tools/call` result envelope: a single text content block
/// carrying the value as pretty JSON (or the raw message), plus `isError`.
fn tool_content(value: &Value, is_error: bool) -> Value {
    // A bare error string is shown verbatim; structured results are
    // pretty-printed JSON so a model reads them cleanly.
    let text = match value {
        Value::String(s) => s.clone(),
        other => serde_json::to_string_pretty(other)
            .unwrap_or_else(|err| format!("<failed to serialize result: {err}>")),
    };
    json!({
        "content": [ { "type": "text", "text": text } ],
        "isError": is_error,
    })
}

/// Write one JSON-RPC response to stdout as a single newline-terminated
/// line (the MCP stdio framing).
fn emit(response: &Value) {
    match serde_json::to_string(response) {
        Ok(line) => println!("{line}"),
        Err(err) => {
            // Last-resort: a response we built ourselves should always
            // serialize; if it somehow doesn't, emit a minimal internal
            // error rather than dropping the turn silently.
            let fallback = jsonrpc::error(
                Value::Null,
                INTERNAL_ERROR,
                format!("failed to serialize response: {err}"),
            );
            if let Ok(line) = serde_json::to_string(&fallback) {
                println!("{line}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn initialize_returns_protocol_and_server_info() {
        let req: Request = serde_json::from_str(
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{}}}"#,
        )
        .unwrap();
        let resp = handle_request(req).await.expect("initialize replies");
        assert_eq!(resp["jsonrpc"], json!("2.0"));
        assert_eq!(resp["id"], json!(1));
        assert_eq!(
            resp["result"]["protocolVersion"],
            json!(MCP_PROTOCOL_VERSION)
        );
        assert_eq!(resp["result"]["serverInfo"]["name"], json!("phux"));
        assert_eq!(
            resp["result"]["serverInfo"]["version"],
            json!(env!("CARGO_PKG_VERSION"))
        );
        assert!(resp["result"]["capabilities"]["tools"].is_object());
    }

    #[tokio::test]
    async fn initialized_notification_gets_no_reply() {
        let req: Request =
            serde_json::from_str(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#)
                .unwrap();
        assert!(handle_request(req).await.is_none());
    }

    #[tokio::test]
    async fn tools_list_is_well_formed() {
        let req: Request =
            serde_json::from_str(r#"{"jsonrpc":"2.0","id":7,"method":"tools/list"}"#).unwrap();
        let resp = handle_request(req).await.expect("tools/list replies");
        let tools = resp["result"]["tools"].as_array().expect("tools array");
        assert_eq!(tools.len(), 5);
        assert!(tools.iter().any(|t| t["name"] == json!("phux_ls")));
    }

    #[tokio::test]
    async fn unknown_method_is_a_jsonrpc_error() {
        let req: Request =
            serde_json::from_str(r#"{"jsonrpc":"2.0","id":3,"method":"frobnicate"}"#).unwrap();
        let resp = handle_request(req).await.expect("error reply");
        assert_eq!(resp["error"]["code"], json!(METHOD_NOT_FOUND));
        assert_eq!(resp["id"], json!(3));
    }

    #[tokio::test]
    async fn malformed_json_yields_parse_error_with_null_id() {
        let resp = handle_line("{ this is not json")
            .await
            .expect("parse error reply");
        assert_eq!(resp["error"]["code"], json!(PARSE_ERROR));
        assert_eq!(resp["id"], Value::Null);
    }

    #[tokio::test]
    async fn tools_call_without_params_is_invalid_request() {
        let req: Request =
            serde_json::from_str(r#"{"jsonrpc":"2.0","id":5,"method":"tools/call"}"#).unwrap();
        let resp = handle_request(req).await.expect("error reply");
        assert_eq!(resp["error"]["code"], json!(INVALID_REQUEST));
    }
}
