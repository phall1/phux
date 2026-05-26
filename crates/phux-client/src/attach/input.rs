//! Stdin VT-byte parsing for the attach loop.
//!
//! The full surface area of terminal input — CSI / SS3 / DCS sequences,
//! mouse reports, focus reports, bracketed paste, the SGR-Pixels mouse
//! protocol — is large. For phux-9gw.3 we ship a deliberately narrow
//! parser: ASCII printable characters, the C0 control range we care about,
//! and the detach chord. Everything else gets dropped on the floor for v0
//! with a tracing log so the gap is visible. The full parser lands in
//! phux-19e.
//!
//! # Detach chord
//!
//! `Ctrl-b d` — tmux-style. The chord is recognised across two consecutive
//! [`feed`] calls; the parser keeps a tiny piece of state ([`StdinParser`])
//! to track whether the prefix byte has been seen. A configurable detach
//! key binding is tracked under phux-631.

use phux_protocol::input::focus::FocusEvent;
use phux_protocol::input::key::{KeyAction, KeyEvent, ModSet, PhysicalKey};
use phux_protocol::input::mouse::MouseEvent;
use phux_protocol::input::paste::PasteEvent;
use phux_protocol::wire::frame::FrameKind;

/// Human-readable description of the v0 detach chord. Surfaced through
/// [`super::DETACH_CHORD_DESCRIPTION`] for help text.
pub const DETACH_CHORD_DESCRIPTION: &str = "Ctrl-b d";

/// The prefix byte of the detach chord. `Ctrl-b` is `0x02`.
const DETACH_PREFIX: u8 = 0x02;

/// The completion byte of the detach chord. ASCII lowercase `d`.
const DETACH_FINISH: u8 = b'd';

/// One client-to-server input event ready to be wrapped in a [`FrameKind`].
///
/// Mouse / focus / paste variants are present so the enum reads true to
/// the SPEC §9 input surface — but the v0 parser only ever yields
/// [`InputEvent::Key`] from real input bytes, plus
/// [`InputEvent::DetachRequested`] for the hardcoded chord. The richer
/// variants come online with phux-19e.
#[derive(Debug, Clone, PartialEq)]
pub enum InputEvent {
    /// A structured key event. Encodes to `INPUT_KEY` per SPEC §9.1.
    Key(KeyEvent),
    /// A structured mouse event. Encodes to `INPUT_MOUSE` per SPEC §9.2.
    Mouse(MouseEvent),
    /// A focus state change on the host window. Encodes to `INPUT_FOCUS`
    /// per SPEC §9.3.
    Focus(FocusEvent),
    /// A paste payload. Encodes to `INPUT_PASTE` per SPEC §9.4.
    Paste(PasteEvent),
    /// The user pressed the detach chord. The driver translates this to
    /// a [`FrameKind::Detach`] and waits for `DETACHED`.
    DetachRequested,
}

impl InputEvent {
    /// Wrap this event in the appropriate [`FrameKind`] addressed to
    /// `pane_id`. [`InputEvent::DetachRequested`] returns `None` — the
    /// driver issues a [`FrameKind::Detach`] directly, since `DETACH`
    /// is session-level and pane-id-agnostic.
    #[must_use]
    pub fn into_frame(self, pane_id: u32) -> Option<FrameKind> {
        match self {
            Self::Key(event) => Some(FrameKind::InputKey { pane_id, event }),
            Self::Mouse(event) => Some(FrameKind::InputMouse { pane_id, event }),
            Self::Focus(event) => Some(FrameKind::InputFocus { pane_id, event }),
            Self::Paste(event) => Some(FrameKind::InputPaste { pane_id, event }),
            Self::DetachRequested => None,
        }
    }
}

/// Stateful parser for stdin bytes.
///
/// The parser holds **one byte** of state — whether the previous call
/// ended on the detach-chord prefix. Real VT input parsing (CSI parameter
/// accumulation, SS3, DCS) requires a larger state machine and lands in
/// phux-19e.
#[derive(Debug, Default)]
pub struct StdinParser {
    /// True iff the last byte we saw was [`DETACH_PREFIX`] and we're
    /// waiting on the chord's completion byte.
    waiting_for_chord_finish: bool,
}

impl StdinParser {
    /// New parser in the empty (no-pending-chord) state.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            waiting_for_chord_finish: false,
        }
    }

    /// Feed `bytes` and return any complete events.
    ///
    /// The parser does not allocate when no events come out; for `n`
    /// printable bytes the returned `Vec` is heap-allocated once with
    /// capacity hint.
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<InputEvent> {
        let mut events = Vec::with_capacity(bytes.len());
        for &b in bytes {
            // Detach chord handling has priority over the normal path so
            // the prefix byte is not also delivered upstream as a Ctrl-b.
            if self.waiting_for_chord_finish {
                self.waiting_for_chord_finish = false;
                if b == DETACH_FINISH {
                    events.push(InputEvent::DetachRequested);
                    continue;
                }
                // Chord aborted. Per tmux semantics, we drop the prefix
                // and process the new byte normally so `Ctrl-b x` doesn't
                // smuggle Ctrl-b upstream.
            }
            if b == DETACH_PREFIX {
                self.waiting_for_chord_finish = true;
                continue;
            }
            if let Some(ev) = byte_to_key_event(b) {
                events.push(InputEvent::Key(ev));
            } else {
                // Unrecognised byte (likely the start of an escape
                // sequence we don't handle yet). Tracked under phux-19e.
                tracing::trace!(byte = b, "dropping unrecognised stdin byte");
            }
        }
        events
    }
}

/// Translate one stdin byte to a [`KeyEvent`].
///
/// Returns `None` for bytes we deliberately ignore (the ESC prefix, since
/// we don't parse the rest of the sequence in v0) and for byte values
/// outside the v0 supported set.
fn byte_to_key_event(b: u8) -> Option<KeyEvent> {
    match b {
        // Printable ASCII — emit a key event with the raw text. PhysicalKey
        // is set to `Unidentified` because we don't actually know which
        // physical key produced the byte (`a` from layout vs `a` from a
        // remap look identical at this layer), and the server's key
        // encoder uses `text` when present per SPEC §9.1.
        0x20..=0x7E => Some(KeyEvent {
            action: KeyAction::Press,
            key: PhysicalKey::Unidentified,
            mods: ModSet::empty(),
            consumed_mods: ModSet::empty(),
            composing: false,
            text: Some(char::from(b).to_string()),
            unshifted_codepoint: Some(u32::from(b)),
        }),
        // CR — Enter. Many terminals send CR not LF for Enter even in raw
        // mode (depends on ICRNL); accept both.
        0x0D | 0x0A => Some(KeyEvent {
            action: KeyAction::Press,
            key: PhysicalKey::Enter,
            mods: ModSet::empty(),
            consumed_mods: ModSet::empty(),
            composing: false,
            text: None,
            unshifted_codepoint: None,
        }),
        // BS / DEL — Backspace. POSIX TTYs default `erase` to DEL (0x7F).
        0x08 | 0x7F => Some(KeyEvent {
            action: KeyAction::Press,
            key: PhysicalKey::Backspace,
            mods: ModSet::empty(),
            consumed_mods: ModSet::empty(),
            composing: false,
            text: None,
            unshifted_codepoint: None,
        }),
        // HT — Tab.
        0x09 => Some(KeyEvent {
            action: KeyAction::Press,
            key: PhysicalKey::Tab,
            mods: ModSet::empty(),
            consumed_mods: ModSet::empty(),
            composing: false,
            text: None,
            unshifted_codepoint: None,
        }),
        // Other C0 control bytes — encode as Ctrl-modified key with the
        // corresponding letter. e.g. 0x03 = Ctrl-C, 0x04 = Ctrl-D. Skip
        // 0x02 (caught upstream as the detach prefix) and the bytes that
        // already have dedicated mappings above.
        //
        // ESC (0x1B) is in this range but is filtered out — for v0 we
        // drop a bare ESC. The full ESC-sequence parser (arrow keys,
        // function keys, alt-prefixed chords) is phux-19e.
        0x01..=0x1A if b != 0x08 && b != 0x09 && b != 0x0A && b != 0x0D => {
            let letter = b'A' + (b - 1);
            Some(KeyEvent {
                action: KeyAction::Press,
                key: ascii_letter_to_key(letter)?,
                mods: ModSet::CTRL,
                consumed_mods: ModSet::CTRL,
                composing: false,
                text: None,
                unshifted_codepoint: Some(u32::from(letter.to_ascii_lowercase())),
            })
        }
        // Everything else — ESC (0x1B), DEL (already handled above), high
        // bytes (UTF-8 continuation, mouse reports, etc.) — is dropped.
        // Handled centrally in phux-19e.
        _ => None,
    }
}

/// Map ASCII uppercase letter bytes to libghostty's `PhysicalKey` variants.
///
/// Returns `None` for non-letters — the caller is responsible for never
/// passing such a byte. Kept as a function rather than a static table so
/// the unused-match-arms lint stays quiet under `#![warn(missing_docs)]`.
const fn ascii_letter_to_key(b: u8) -> Option<PhysicalKey> {
    Some(match b {
        b'A' => PhysicalKey::A,
        b'B' => PhysicalKey::B,
        b'C' => PhysicalKey::C,
        b'D' => PhysicalKey::D,
        b'E' => PhysicalKey::E,
        b'F' => PhysicalKey::F,
        b'G' => PhysicalKey::G,
        b'H' => PhysicalKey::H,
        b'I' => PhysicalKey::I,
        b'J' => PhysicalKey::J,
        b'K' => PhysicalKey::K,
        b'L' => PhysicalKey::L,
        b'M' => PhysicalKey::M,
        b'N' => PhysicalKey::N,
        b'O' => PhysicalKey::O,
        b'P' => PhysicalKey::P,
        b'Q' => PhysicalKey::Q,
        b'R' => PhysicalKey::R,
        b'S' => PhysicalKey::S,
        b'T' => PhysicalKey::T,
        b'U' => PhysicalKey::U,
        b'V' => PhysicalKey::V,
        b'W' => PhysicalKey::W,
        b'X' => PhysicalKey::X,
        b'Y' => PhysicalKey::Y,
        b'Z' => PhysicalKey::Z,
        _ => return None,
    })
}

#[cfg(test)]
#[allow(clippy::expect_used, reason = "tests")]
mod tests {
    use super::*;

    #[test]
    fn printable_byte_becomes_key_event_with_text() {
        let mut p = StdinParser::new();
        let evs = p.feed(b"a");
        assert_eq!(evs.len(), 1);
        match &evs[0] {
            InputEvent::Key(k) => {
                assert_eq!(k.text.as_deref(), Some("a"));
                assert_eq!(k.action, KeyAction::Press);
            }
            other => panic!("expected key event, got {other:?}"),
        }
    }

    #[test]
    fn enter_byte_becomes_enter_key() {
        let mut p = StdinParser::new();
        let evs = p.feed(b"\r");
        assert_eq!(evs.len(), 1);
        match &evs[0] {
            InputEvent::Key(k) => assert_eq!(k.key, PhysicalKey::Enter),
            other => panic!("expected enter key, got {other:?}"),
        }
    }

    #[test]
    fn ctrl_c_byte_becomes_ctrl_modified_c() {
        let mut p = StdinParser::new();
        let evs = p.feed(&[0x03]);
        assert_eq!(evs.len(), 1);
        match &evs[0] {
            InputEvent::Key(k) => {
                assert_eq!(k.key, PhysicalKey::C);
                assert!(k.mods.contains(ModSet::CTRL));
            }
            other => panic!("expected Ctrl-C, got {other:?}"),
        }
    }

    #[test]
    fn detach_chord_emits_detach_requested() {
        let mut p = StdinParser::new();
        let evs = p.feed(&[DETACH_PREFIX, b'd']);
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0], InputEvent::DetachRequested);
    }

    #[test]
    fn detach_chord_across_two_feeds() {
        let mut p = StdinParser::new();
        let first = p.feed(&[DETACH_PREFIX]);
        assert!(first.is_empty(), "prefix alone should not emit");
        let second = p.feed(b"d");
        assert_eq!(second, vec![InputEvent::DetachRequested]);
    }

    #[test]
    fn ctrl_b_followed_by_non_d_drops_prefix() {
        let mut p = StdinParser::new();
        // `Ctrl-b a` aborts the chord; the prefix is dropped and `a` is
        // delivered as a normal key.
        let evs = p.feed(&[DETACH_PREFIX, b'a']);
        assert_eq!(evs.len(), 1);
        match &evs[0] {
            InputEvent::Key(k) => assert_eq!(k.text.as_deref(), Some("a")),
            other => panic!("expected key 'a', got {other:?}"),
        }
    }

    #[test]
    fn esc_byte_alone_is_dropped() {
        let mut p = StdinParser::new();
        let evs = p.feed(&[0x1B]);
        assert!(evs.is_empty(), "ESC alone is dropped in v0");
    }

    #[test]
    fn into_frame_carries_pane_id() {
        let key = KeyEvent {
            action: KeyAction::Press,
            key: PhysicalKey::A,
            mods: ModSet::empty(),
            consumed_mods: ModSet::empty(),
            composing: false,
            text: Some("a".to_owned()),
            unshifted_codepoint: Some(u32::from('a')),
        };
        let frame = InputEvent::Key(key).into_frame(42).expect("frame");
        match frame {
            FrameKind::InputKey { pane_id, .. } => assert_eq!(pane_id, 42),
            other => panic!("expected InputKey, got {other:?}"),
        }
    }

    #[test]
    fn detach_requested_has_no_frame() {
        assert!(InputEvent::DetachRequested.into_frame(1).is_none());
    }
}
