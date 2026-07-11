use std::path::{Path, PathBuf};
use std::process::ExitCode;

use phux_client::attach::connection::Connection;
use phux_protocol::ids::{GroupId, SatelliteHost, TerminalId};
use phux_protocol::wire::frame::{FrameKind, SpawnError, SpawnResult};
use phux_server::runtime::default_socket_path;

use crate::commands::{cli_runtime, report_no_server};

/// `phux spawn` — create a Terminal without attaching (`SPAWN_TERMINAL`,
/// SPEC L1 §3.1). Does not auto-start a server.
///
/// The pane joins the server's most recently active session (the same
/// focus heuristic `GET_STATE` snapshots use). With `--satellite NAME`
/// a federation hub routes the spawn over its link to that satellite
/// (phux-v45.6) and the returned Terminal is satellite-tagged: the
/// printed id is addressable through the hub by the satellite-capable
/// verbs. On a non-hub server (or for an unknown name) the spawn is
/// refused with the typed `UnsupportedSatelliteRoute`; an unreachable
/// satellite fails fast with `SatelliteUnreachable`.
///
/// Output hygiene matches the other one-shot verbs: with `--json` stdout
/// carries only `{"terminal_id": N, "satellite": "NAME" | null}`;
/// diagnostics go to stderr with a nonzero exit.
pub(crate) fn run_spawn(
    satellite: Option<String>,
    cwd: Option<String>,
    json: bool,
    socket: Option<PathBuf>,
    command: Vec<String>,
) -> ExitCode {
    let socket_path = socket.unwrap_or_else(default_socket_path);
    let request_id = 1u32;
    let frame = FrameKind::SpawnTerminal {
        request_id,
        // v0.1 servers expose the single default group (SPEC §3.1).
        group: GroupId::new(1),
        command: if command.is_empty() {
            None
        } else {
            Some(command)
        },
        cwd,
        env: None,
        term: None,
        satellite: satellite.map(SatelliteHost::new),
    };
    match dispatch_spawn(&socket_path, &frame, request_id, "spawn") {
        Ok(SpawnResult::Ok(terminal_id)) => print_spawned(&terminal_id, json),
        Ok(SpawnResult::Err(err)) => {
            report_spawn_error(&err);
            ExitCode::FAILURE
        }
        Ok(other) => {
            eprintln!("phux: unexpected SPAWN_TERMINAL result: {other:?}");
            ExitCode::FAILURE
        }
        Err(code) => code,
    }
}

/// Send a `SPAWN_TERMINAL` frame and return the matching `TERMINAL_SPAWNED`
/// result. Shared by `phux spawn` and `phux launch` (phux-ark7) so both
/// ride the identical wire path — the server injects `PHUX_TERMINAL_ID`
/// into the spawned pane regardless of which verb requested it.
///
/// On a connect/transport failure this prints the `no server` diagnostic
/// (attributed to `verb`) and returns the failure [`ExitCode`] in `Err`, so
/// callers only handle the `SpawnResult` variants.
pub(crate) fn dispatch_spawn(
    socket_path: &Path,
    frame: &FrameKind,
    request_id: u32,
    verb: &str,
) -> Result<SpawnResult, ExitCode> {
    let rt = cli_runtime()?;
    let result = rt.block_on(async {
        let mut conn = Connection::connect(socket_path).await?;
        conn.send(frame).await?;
        loop {
            if let FrameKind::TerminalSpawned {
                request_id: got,
                result,
            } = conn.recv().await?
                && got == request_id
            {
                return Ok(result);
            }
        }
    });
    result.map_err(|err| report_no_server(&err, socket_path, verb))
}

/// Print the freshly spawned Terminal id — human line or the stable JSON
/// document (`terminal_id` is the satellite-local id when `satellite` is
/// non-null; address it through the hub as `satellite`+`terminal_id`).
fn print_spawned(terminal_id: &TerminalId, json: bool) -> ExitCode {
    let (id, host) = match terminal_id {
        TerminalId::Local { id } => (*id, None),
        TerminalId::Satellite { host, id } => (*id, Some(host.as_str())),
    };
    if json {
        let payload = serde_json::json!({ "terminal_id": id, "satellite": host });
        match serde_json::to_string_pretty(&payload) {
            Ok(s) => {
                println!("{s}");
                ExitCode::SUCCESS
            }
            Err(err) => {
                eprintln!("phux: failed to serialize spawn result as JSON: {err}");
                ExitCode::FAILURE
            }
        }
    } else {
        match host {
            Some(host) => println!("spawned terminal {id} on satellite {host}"),
            None => println!("spawned terminal {id}"),
        }
        ExitCode::SUCCESS
    }
}

/// Map the typed `SpawnError` to an actionable stderr diagnostic.
pub(crate) fn report_spawn_error(err: &SpawnError) {
    match err {
        SpawnError::GroupNotFound => {
            eprintln!("phux: spawn failed: server rejected the default group");
        }
        SpawnError::SpawnFailed(reason) => eprintln!("phux: spawn failed: {reason}"),
        SpawnError::UnsupportedSatelliteRoute => {
            eprintln!(
                "phux: spawn failed: no route to that satellite \
                 (is the server running with --hub, and the name in `phux satellite list`?)"
            );
        }
        SpawnError::SatelliteUnreachable(reason) => {
            eprintln!("phux: spawn failed: satellite unreachable: {reason}");
        }
        other => eprintln!("phux: spawn failed: {other:?}"),
    }
}
