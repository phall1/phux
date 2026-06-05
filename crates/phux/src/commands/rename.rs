use std::path::PathBuf;
use std::process::ExitCode;

use phux_protocol::ids::CollectionId;
use phux_protocol::wire::frame::{CommandResult, Command as WireCommand};
use phux_server::runtime::default_socket_path;

use crate::commands::{cli_runtime, report_no_server, request_command};

/// `phux rename SESSION NEW_NAME` — reassign a session's name in one
/// `RENAME_SESSION` round-trip (ADR-0021 §3). The server is authoritative;
/// attached clients reconcile the new name on their next snapshot. Exit
/// codes mirror `phux kill`: 0 on success, 1 on no server, 2 on a
/// server-side refusal (unknown session or a name already in use).
pub(crate) fn run_rename(session: &str, new_name: &str, socket: Option<PathBuf>) -> ExitCode {
    let socket_path = socket.unwrap_or_else(default_socket_path);
    let rt = match cli_runtime() {
        Ok(rt) => rt,
        Err(code) => return code,
    };

    let command = WireCommand::RenameSession {
        collection: CollectionId::new(1),
        name: session.to_owned(),
        new_name: new_name.to_owned(),
    };

    match rt.block_on(request_command(&socket_path, command)) {
        Ok(CommandResult::Ok) => {
            println!("renamed {session:?} to {new_name:?}");
            ExitCode::SUCCESS
        }
        Ok(CommandResult::Error { message, .. }) => {
            eprintln!("phux: rename refused for session {session:?}: {message}");
            ExitCode::from(2)
        }
        Ok(other) => {
            eprintln!("phux: unexpected RENAME_SESSION result: {other:?}");
            ExitCode::from(2)
        }
        Err(err) => report_no_server(&err, &socket_path, "rename"),
    }
}
