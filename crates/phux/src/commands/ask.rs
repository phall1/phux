use std::path::PathBuf;
use std::process::ExitCode;

use phux_client::ask::AskedPayload;
use phux_client::attach::AttachError;
use phux_protocol::TerminalId;
use phux_server::runtime::default_socket_path;

use crate::commands::{cli_runtime, parse_selector, report_no_server, resolve_target};

pub(crate) fn run_ask(
    target: &str,
    id: String,
    suggestions: Vec<String>,
    elapsed_seconds: Option<u64>,
    json: bool,
    question: String,
    socket: Option<PathBuf>,
) -> ExitCode {
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
        let pane = match resolve_target(&socket_path, &selector, "ask").await {
            Ok(id) => id,
            Err(code) => return code,
        };
        let payload = AskedPayload {
            id,
            question,
            suggestions,
            elapsed_seconds,
        };
        match phux_client::ask::report(&socket_path, pane.clone(), payload.clone()).await {
            Ok(()) => print_success(&pane, &payload, json),
            Err(err @ AttachError::Io(_)) => report_no_server(&err, &socket_path, "ask"),
            Err(AttachError::Refused(msg)) => {
                eprintln!("phux: cannot report ask for '{target}': {msg}");
                ExitCode::FAILURE
            }
            Err(err) => {
                eprintln!("phux: ask failed: {err}");
                ExitCode::FAILURE
            }
        }
    })
}

fn print_success(pane: &TerminalId, payload: &AskedPayload, json: bool) -> ExitCode {
    if json {
        let value = serde_json::json!({
            "event": "asked",
            "terminal": format!("@{}", pane.local_id().unwrap_or(0)),
            "id": payload.id,
            "question": payload.question,
            "suggestions": payload.suggestions,
            "elapsed_seconds": payload.elapsed_seconds,
        });
        match serde_json::to_string_pretty(&value) {
            Ok(line) => println!("{line}"),
            Err(err) => {
                eprintln!("phux: failed to serialize ask: {err}");
                return ExitCode::FAILURE;
            }
        }
    } else {
        println!(
            "reported ask {} to @{}",
            payload.id,
            pane.local_id().unwrap_or(0)
        );
    }
    ExitCode::SUCCESS
}
