//! Translate a tmux-style key-spec into wire input frames (phux-21o,
//! ADR-0022).
//!
//! Each CLI argument is either a **named key** (`Enter`, `Tab`, `Escape`,
//! `Up`, `C-c`, `M-x`, …) or a **literal string** sent character by
//! character — matching `tmux send-keys`, so it's legible to a model that
//! already knows tmux. We translate each arg to its byte sequence and feed
//! the *same* [`StdinParser`] the
//! interactive client uses for real keystrokes, so the resulting
//! `KeyEvent`s are identical to typing — no hand-rolled char→key table.

use std::path::Path;

use phux_protocol::PROTOCOL_VERSION;
use phux_protocol::TerminalId;
use phux_protocol::caps::{ClientCapabilities, Layer, LayerSet, detect_color_support};
use phux_protocol::wire::frame::{AttachTarget, FrameKind, ViewportInfo};

use crate::attach::AttachError;
use crate::attach::connection::Connection;
use crate::attach::input::StdinParser;

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

/// Translate all key-spec args into wire input frames addressed to `target`.
///
/// Concatenates the args' bytes through one [`StdinParser`] (so an `Up`
/// arg yields the arrow `KeyEvent`, a bare `Escape` is flushed at the
/// end, etc.).
#[must_use]
pub fn frames_for(args: &[String], target: &TerminalId) -> Vec<FrameKind> {
    let mut parser = StdinParser::default();
    let mut frames = Vec::new();
    for arg in args {
        for ev in parser.feed(&spec_to_bytes(arg)) {
            frames.push(ev.into_frame(target.clone()));
        }
    }
    for ev in parser.flush() {
        frames.push(ev.into_frame(target.clone()));
    }
    frames
}

/// Attach to `target`, send `keys` to its focused pane, then detach.
///
/// Input must come from an attached, subscribed client — the server drops
/// input frames from unattached connections (`handle_terminal_input`'s
/// attach + subscription gate). So this attaches like the interactive
/// client; ATTACH carries a viewport, so it may transiently resize the
/// pane (same bootstrap caveat as `snapshot`; the side-effect-free path is
/// a control-plane input route, tracked under the agent epic).
///
/// The server reads frames in order, so the `InputKey` frames are
/// dispatched to the pane actor before the trailing `DETACH` — no drain
/// race.
pub async fn send(socket: &Path, target: AttachTarget, keys: &[String]) -> Result<(), AttachError> {
    let mut conn = Connection::connect(socket).await?;
    conn.send(&FrameKind::Hello {
        client_name: format!("phux-send-keys/{}", env!("CARGO_PKG_VERSION")),
        protocol_major: PROTOCOL_VERSION.major,
        protocol_minor: PROTOCOL_VERSION.minor,
        protocol_patch: PROTOCOL_VERSION.patch,
        client_caps: ClientCapabilities::new()
            .with_color_support(detect_color_support())
            .with_layers(LayerSet::with(&[Layer::L3])),
    })
    .await?;
    conn.send(&FrameKind::Attach {
        target,
        viewport: ViewportInfo::new(80, 24),
        request_scrollback: false,
        scrollback_limit_lines: 0,
    })
    .await?;
    let focused = loop {
        match conn.recv().await? {
            FrameKind::Attached { snapshot, .. } => break snapshot.focused_pane,
            FrameKind::Error { message, .. } => return Err(AttachError::Refused(message)),
            _ => {}
        }
    };
    for frame in frames_for(keys, &focused) {
        conn.send(&frame).await?;
    }
    conn.send(&FrameKind::Detach).await?;
    Ok(())
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
    fn frames_emit_one_inputkey_per_char() {
        let t = TerminalId::local(1);
        let frames = frames_for(&["ab".to_owned(), "Enter".to_owned()], &t);
        // 'a', 'b', and Enter — three key events.
        assert_eq!(frames.len(), 3, "got {frames:?}");
        assert!(matches!(frames[0], FrameKind::InputKey { .. }));
    }
}
