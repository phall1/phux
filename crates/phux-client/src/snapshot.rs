//! Structured, side-effect-free screen capture — the floor of the agent
//! surface (ADR-0022 §5, `phux-oki`).
//!
//! Sends the `GET_SCREEN` control command and parses the
//! `phux_core::ScreenState` the server returns. The server walks its *own*
//! `Terminal` grid, so — unlike the attach path — this neither resizes the
//! pane nor disturbs the live session. That is what makes it safe to poll
//! (the `phux wait`/`run` floor) against a pane a human or another agent
//! is actively using.
//!
//! The read shape ([`ScreenState`]) lives in `phux-core` so the server
//! (producer) and this client (consumer) share one definition; we
//! re-export it here for callers that only depend on `phux-client`.

use std::path::Path;

use phux_protocol::ids::TerminalId;
use phux_protocol::wire::frame::{Command, CommandResult, CommandValue, FrameKind};

pub use phux_core::screen::{CursorState, SCHEMA_VERSION, ScreenState};

use crate::attach::AttachError;
use crate::attach::connection::Connection;

/// Read `terminal_id`'s current screen as structured data.
///
/// Opens a fresh connection, issues `GET_SCREEN`, and deserializes the
/// JSON reply. No `HELLO` and no `ATTACH`: the control command stands
/// alone (matching `phux ls`/`kill`), and the read is side-effect-free.
///
/// # Errors
///
/// Returns [`AttachError`] on connect/transport failure, when the server
/// refuses the command (e.g. unknown terminal), or when the reply is not
/// the expected `OK_WITH(JSON(..))` carrying a valid [`ScreenState`].
pub async fn get_screen(
    socket: &Path,
    terminal_id: TerminalId,
) -> Result<ScreenState, AttachError> {
    let mut conn = Connection::connect(socket).await?;
    let result = command(&mut conn, 1, Command::GetScreen { terminal_id }).await?;
    match result {
        CommandResult::OkWith(CommandValue::Json(json)) => serde_json::from_str(&json)
            .map_err(|err| AttachError::Protocol(format!("malformed GET_SCREEN JSON: {err}"))),
        CommandResult::Error { message, .. } => Err(AttachError::Refused(message)),
        other => Err(AttachError::Protocol(format!(
            "unexpected GET_SCREEN result: {other:?}"
        ))),
    }
}

/// Send one command and return the matching `COMMAND_RESULT`, skipping any
/// unrelated frames the server interleaves (SPEC §5).
async fn command(
    conn: &mut Connection,
    request_id: u32,
    command: Command,
) -> Result<CommandResult, AttachError> {
    conn.send(&FrameKind::Command {
        request_id,
        command,
    })
    .await?;
    loop {
        if let FrameKind::CommandResult {
            request_id: got,
            result,
        } = conn.recv().await?
            && got == request_id
        {
            return Ok(result);
        }
    }
}
