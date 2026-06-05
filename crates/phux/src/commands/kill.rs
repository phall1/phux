use std::path::PathBuf;
use std::process::ExitCode;

use phux_client::attach::AttachError;
use phux_client::attach::connection::Connection;
use phux_protocol::ids::CollectionId;
use phux_protocol::wire::frame::{Command as WireCommand, CommandResult, CommandValue, StateScope};
use phux_server::runtime::default_socket_path;

use crate::commands::{cli_runtime, command_on, report_no_server};
use crate::selector;

/// `phux kill TARGET` — resolve the selector client-side, then ask the
/// server to tear it down. A whole-session target (`.`, `=`, or a bare
/// `name`) rides a single `KILL_COLLECTION` round-trip (phux-h9s); a
/// window / pane / `@id` target falls back to one `KILL_TERMINAL` per
/// resolved Terminal. Exit codes: 0 on success, 1 on a selector miss /
/// no server, 2 on a server-side refusal.
pub(crate) fn run_kill(target: &str, socket: Option<PathBuf>) -> ExitCode {
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
                    collection: CollectionId::new(1),
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
