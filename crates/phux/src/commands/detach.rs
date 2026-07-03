use std::path::PathBuf;
use std::process::ExitCode;

use phux_client::attach::AttachError;
use phux_client::attach::connection::Connection;
use phux_protocol::wire::frame::{Command as WireCommand, CommandResult, CommandValue};
use phux_server::runtime::default_socket_path;

use crate::commands::{cli_runtime, command_on, report_no_server};

/// `phux detach [SESSION]` — force-detach clients from *outside* the attach UI.
///
/// With `SESSION`, detaches every client attached to that session; with no
/// argument, detaches every attached client on the server. Each target
/// client's TUI receives a `DETACHED` frame and exits cleanly — the CLI
/// analogue of the `C-a d` keybinding, usable for scripting or reclaiming a
/// session that's attached (or wedged) elsewhere. Distinct from
/// `FrameKind::Detach`, which only detaches the sending connection.
///
/// Exit codes: 0 on success (including "nobody was attached"), 1 on no server,
/// 2 on a server-side refusal.
pub(crate) fn run_detach(session: Option<String>, socket: Option<PathBuf>) -> ExitCode {
    let socket_path = socket.unwrap_or_else(default_socket_path);
    let rt = match cli_runtime() {
        Ok(rt) => rt,
        Err(code) => return code,
    };

    rt.block_on(async move {
        let mut conn = match Connection::connect(&socket_path).await {
            Ok(conn) => conn,
            Err(err) => return report_no_server(&err, &socket_path, "detach"),
        };

        match command_on(
            &mut conn,
            1,
            WireCommand::DetachClients {
                session: session.clone(),
            },
        )
        .await
        {
            Ok(CommandResult::OkWith(CommandValue::Json(count))) => {
                let n = count.trim().parse::<u64>().unwrap_or(0);
                match session.as_deref() {
                    Some(name) => println!("phux: detached {n} client(s) from session {name:?}"),
                    None => println!("phux: detached {n} client(s)"),
                }
                ExitCode::SUCCESS
            }
            // A clean disconnect after the op is still success (the server may
            // self-exit once its last consumer leaves).
            Ok(CommandResult::Ok) | Err(AttachError::Disconnected) => ExitCode::SUCCESS,
            Ok(CommandResult::Error { message, .. }) => {
                eprintln!("phux: detach refused: {message}");
                ExitCode::from(2)
            }
            Ok(other) => {
                eprintln!("phux: unexpected detach result: {other:?}");
                ExitCode::from(2)
            }
            Err(err) => report_no_server(&err, &socket_path, "detach"),
        }
    })
}
