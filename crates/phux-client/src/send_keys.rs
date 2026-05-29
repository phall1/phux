//! Translate a tmux-style key-spec into routed input events (phux-21o,
//! ADR-0022, phux-3j3).
//!
//! Each CLI argument is either a **named key** (`Enter`, `Tab`, `Escape`,
//! `Up`, `C-c`, `M-x`, …) or a **literal string** sent character by
//! character — matching `tmux send-keys`, so it's legible to a model that
//! already knows tmux. We translate each arg to its byte sequence and feed
//! the *same* [`StdinParser`] the
//! interactive client uses for real keystrokes, so the resulting
//! `KeyEvent`s are identical to typing — no hand-rolled char→key table.
//!
//! The built events ride the side-effect-free `ROUTE_INPUT` control
//! command (L1.md §5.1) rather than an `ATTACH` + `INPUT_KEY` stream. So,
//! like `GET_SCREEN`, this neither subscribes the caller nor resizes the
//! pane to the caller's viewport — the live session keeps its dimensions
//! (phux-3j3).

use std::path::Path;

use phux_protocol::TerminalId;
use phux_protocol::input::InputEvent as WireInputEvent;
use phux_protocol::wire::frame::{AttachTarget, Command, CommandResult, CommandValue, StateScope};
use phux_protocol::wire::info::SessionSnapshot;

use crate::attach::AttachError;
use crate::attach::connection::Connection;
use crate::attach::input::{InputEvent, StdinParser};
use crate::snapshot::command;

/// Translate one key-spec argument to the bytes a terminal would receive.
///
/// Named keys map to their control/escape bytes; `C-<x>` is a control
/// byte, `M-<x>` is ESC-prefixed; anything else is literal UTF-8 (sent
/// char by char by the parser downstream).
#[must_use]
pub fn spec_to_bytes(arg: &str) -> Vec<u8> {
    match arg.to_ascii_lowercase().as_str() {
        "enter" | "return" => return b"\r".to_vec(),
        "tab" => return b"\t".to_vec(),
        "escape" | "esc" => return vec![0x1b],
        "space" => return b" ".to_vec(),
        "bspace" | "backspace" => return vec![0x7f],
        "up" => return b"\x1b[A".to_vec(),
        "down" => return b"\x1b[B".to_vec(),
        "right" => return b"\x1b[C".to_vec(),
        "left" => return b"\x1b[D".to_vec(),
        "home" => return b"\x1b[H".to_vec(),
        "end" => return b"\x1b[F".to_vec(),
        _ => {}
    }
    // `C-x` → control byte (C-a = 0x01 … C-z = 0x1a; C-[ = ESC, etc.).
    if let Some(rest) = strip_prefix_ci(arg, "c-")
        && rest.chars().count() == 1
        && let Some(c) = rest.chars().next()
        && c.is_ascii()
    {
        return vec![(c.to_ascii_uppercase() as u8) & 0x1f];
    }
    // `M-x` → ESC-prefixed (Alt).
    if let Some(rest) = strip_prefix_ci(arg, "m-") {
        let mut v = vec![0x1b];
        v.extend_from_slice(rest.as_bytes());
        return v;
    }
    arg.as_bytes().to_vec()
}

/// Case-insensitive prefix strip (so `C-` and `c-` both work).
fn strip_prefix_ci<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    if s.len() >= prefix.len() && s[..prefix.len()].eq_ignore_ascii_case(prefix) {
        Some(&s[prefix.len()..])
    } else {
        None
    }
}

/// Translate all key-spec args into structured [`InputEvent`]s.
///
/// Concatenates the args' bytes through one [`StdinParser`] (so an `Up`
/// arg yields the arrow `KeyEvent`, a bare `Escape` is flushed at the
/// end, etc.). The events are then routed by id via `ROUTE_INPUT`; they
/// are not addressed to a Terminal here.
#[must_use]
pub fn events_for(args: &[String]) -> Vec<InputEvent> {
    let mut parser = StdinParser::default();
    let mut events = Vec::new();
    for arg in args {
        events.extend(parser.feed(&spec_to_bytes(arg)));
    }
    events.extend(parser.flush());
    events
}

/// Lower a client-side [`InputEvent`] to the protocol's wire input union.
///
/// The two enums carry the same four libghostty-backed atoms; this is a
/// field-for-field rewrap, kept here so `ROUTE_INPUT` need not depend on
/// the client's parser type.
fn to_wire_event(event: InputEvent) -> WireInputEvent {
    match event {
        InputEvent::Key(e) => WireInputEvent::Key(e),
        InputEvent::Mouse(e) => WireInputEvent::Mouse(e),
        InputEvent::Focus(e) => WireInputEvent::Focus(e),
        InputEvent::Paste(e) => WireInputEvent::Paste(e),
    }
}

/// Resolve `target` to the Terminal id of its focused pane, against a
/// `GET_STATE` snapshot. Mirrors the server's own focus rule: a session's
/// `active_window`, then that window's `active_pane`. For
/// [`AttachTarget::Last`] (no name) we defer to the snapshot's
/// server-wide `focused_pane`.
fn resolve_focused_pane(snapshot: &SessionSnapshot, target: &AttachTarget) -> Option<TerminalId> {
    let session = match target {
        AttachTarget::Last => return Some(snapshot.focused_pane.clone()),
        AttachTarget::ByName(name) | AttachTarget::CreateIfMissing { name, .. } => {
            snapshot.sessions.iter().find(|s| &s.name == name)?
        }
        AttachTarget::ById(id) => snapshot.sessions.iter().find(|s| s.id == *id)?,
        // `AttachTarget` is `#[non_exhaustive]`; a future target a newer
        // build introduces is not resolvable here.
        _ => return None,
    };
    let active_window = session.active_window?;
    snapshot
        .windows
        .iter()
        .find(|w| w.id == active_window)
        .and_then(|w| w.active_pane.clone())
}

/// Send `keys` to the focused pane of `target` via the side-effect-free
/// `ROUTE_INPUT` route, returning the resolved pane id.
///
/// No `ATTACH`: the target's focused pane is resolved client-side from a
/// `GET_STATE` snapshot (the same way `phux snapshot`/`kill` resolve
/// selectors), then each built [`InputEvent`] is delivered with a
/// `ROUTE_INPUT` command. Because nothing attaches and no viewport is
/// advertised, the live pane keeps its dimensions — unlike the old
/// attach-then-`INPUT_KEY` path, which transiently resized the pane to the
/// caller's `80x24` viewport (phux-3j3).
///
/// Each `ROUTE_INPUT` is acked by a `COMMAND_RESULT` the server emits in
/// order, so the events land on the pane actor's input mailbox in send
/// order — no drain race.
///
/// Returns the [`TerminalId`] of the focused pane the keys were sent to,
/// so callers (e.g. `phux run`) can read back the *same* pane.
pub async fn send(
    socket: &Path,
    target: AttachTarget,
    keys: &[String],
) -> Result<TerminalId, AttachError> {
    let mut conn = Connection::connect(socket).await?;
    // Resolve the focused pane without attaching (side-effect-free).
    let snapshot = match command(
        &mut conn,
        1,
        Command::GetState {
            scope: StateScope::Server,
        },
    )
    .await?
    {
        CommandResult::OkWith(CommandValue::State(snap)) => snap,
        CommandResult::Error { message, .. } => return Err(AttachError::Refused(message)),
        other => {
            return Err(AttachError::Protocol(format!(
                "unexpected GET_STATE result: {other:?}"
            )));
        }
    };
    let pane = resolve_focused_pane(&snapshot, &target).ok_or_else(|| {
        AttachError::Refused("no such session, or it has no focused pane".to_owned())
    })?;

    for (i, event) in events_for(keys).into_iter().enumerate() {
        // request_id 1 was GET_STATE; route-input acks start at 2.
        let request_id = u32::try_from(i).unwrap_or(u32::MAX - 1).saturating_add(2);
        match command(
            &mut conn,
            request_id,
            Command::RouteInput {
                terminal_id: pane.clone(),
                event: to_wire_event(event),
            },
        )
        .await?
        {
            CommandResult::Ok => {}
            CommandResult::Error { message, .. } => return Err(AttachError::Refused(message)),
            other => {
                return Err(AttachError::Protocol(format!(
                    "unexpected ROUTE_INPUT result: {other:?}"
                )));
            }
        }
    }
    Ok(pane)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn named_keys_map_to_control_bytes() {
        assert_eq!(spec_to_bytes("Enter"), b"\r");
        assert_eq!(spec_to_bytes("tab"), b"\t");
        assert_eq!(spec_to_bytes("Escape"), vec![0x1b]);
        assert_eq!(spec_to_bytes("Up"), b"\x1b[A");
    }

    #[test]
    fn control_and_alt_combos() {
        assert_eq!(spec_to_bytes("C-c"), vec![0x03]);
        assert_eq!(spec_to_bytes("c-a"), vec![0x01]);
        assert_eq!(spec_to_bytes("M-x"), vec![0x1b, b'x']);
    }

    #[test]
    fn literal_text_passes_through() {
        assert_eq!(spec_to_bytes("echo hi"), b"echo hi");
    }

    #[test]
    fn events_emit_one_key_per_char() {
        let events = events_for(&["ab".to_owned(), "Enter".to_owned()]);
        // 'a', 'b', and Enter — three key events.
        assert_eq!(events.len(), 3, "got {events:?}");
        assert!(matches!(events[0], InputEvent::Key(_)));
    }
}
