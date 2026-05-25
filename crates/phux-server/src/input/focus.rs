//! Focus event translation: wire → libghostty-vt with DEC 1004 gating.
//!
//! Per `SPEC.md` §9.3 and ADR-0006: the wire form is a single boolean
//! `gained`; this maps to libghostty's `focus::Event::{Gained, Lost}`. The
//! server SHOULD only encode the report when the pane has DEC mode 1004
//! (focus reporting) enabled — [`PerPaneFocusEncoder::encode`] enforces
//! that.

use libghostty_vt::{Error, Terminal, focus::Event as LgFocusEvent, terminal::Mode};
use phux_protocol::input::focus::FocusEvent;

/// Convert a wire [`FocusEvent`] to libghostty's [`LgFocusEvent`].
///
/// Orphan-rules prevent a trait impl (both types are foreign). The
/// reference-taking signature mirrors the sibling `*_to_libghostty`
/// functions for the larger event types.
#[must_use]
#[allow(
    clippy::trivially_copy_pass_by_ref,
    reason = "API symmetry with the other From-style conversions in this module"
)]
pub const fn focus_event_to_libghostty(ev: &FocusEvent) -> LgFocusEvent {
    if ev.gained {
        LgFocusEvent::Gained
    } else {
        LgFocusEvent::Lost
    }
}

/// Per-pane focus encoder.
///
/// Holds a reusable byte buffer. Focus encoding itself is a stateless
/// libghostty free function (`focus::Event::encode`); the type exists for
/// API symmetry with the other per-pane encoders and to own the byte
/// buffer.
#[derive(Debug, Default)]
pub struct PerPaneFocusEncoder {
    buf: Vec<u8>,
}

impl PerPaneFocusEncoder {
    /// Construct a new per-pane focus encoder.
    #[must_use]
    pub fn new() -> Self {
        Self {
            buf: Vec::with_capacity(8),
        }
    }

    /// Encode a focus event into PTY bytes — but only if the pane has DEC
    /// mode 1004 enabled.
    ///
    /// Returns `Ok(None)` if focus reporting is off (the event is silently
    /// dropped per `SPEC.md` §9.3), `Ok(Some(&[u8]))` with the report
    /// otherwise.
    #[allow(
        clippy::trivially_copy_pass_by_ref,
        reason = "API symmetry with KeyEvent/MouseEvent/PasteEvent encoders"
    )]
    pub fn encode(
        &mut self,
        event: &FocusEvent,
        terminal: &Terminal<'_, '_>,
    ) -> Result<Option<&[u8]>, Error> {
        if !terminal.mode(Mode::FOCUS_EVENT)? {
            return Ok(None);
        }
        let lg_event = focus_event_to_libghostty(event);
        // 8 bytes is plenty for CSI I / CSI O (3 bytes each).
        self.buf.resize(8, 0);
        let written = loop {
            match lg_event.encode(&mut self.buf) {
                Ok(n) => break n,
                Err(Error::OutOfSpace { required }) => {
                    self.buf.resize(required, 0);
                }
                Err(e) => return Err(e),
            }
        };
        self.buf.truncate(written);
        Ok(Some(&self.buf))
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, reason = "tests")]
mod tests {
    use super::*;
    use libghostty_vt::TerminalOptions;

    fn make_terminal() -> Terminal<'static, 'static> {
        Terminal::new(TerminalOptions {
            cols: 80,
            rows: 24,
            max_scrollback: 1000,
        })
        .expect("Terminal::new")
    }

    #[test]
    fn focus_event_conversion_maps_both_variants() {
        assert_eq!(
            focus_event_to_libghostty(&FocusEvent { gained: true }),
            LgFocusEvent::Gained
        );
        assert_eq!(
            focus_event_to_libghostty(&FocusEvent { gained: false }),
            LgFocusEvent::Lost
        );
    }

    #[test]
    fn encode_drops_when_mode_1004_off() {
        let terminal = make_terminal();
        // Default state: mode 1004 should be off.
        let mut enc = PerPaneFocusEncoder::new();
        let out = enc
            .encode(&FocusEvent { gained: true }, &terminal)
            .expect("encode");
        assert!(out.is_none(), "expected drop, got {out:?}");
    }

    #[test]
    fn encode_emits_csi_i_when_mode_1004_on() {
        let mut terminal = make_terminal();
        terminal
            .set_mode(Mode::FOCUS_EVENT, true)
            .expect("enable 1004");
        let mut enc = PerPaneFocusEncoder::new();
        let bytes = enc
            .encode(&FocusEvent { gained: true }, &terminal)
            .expect("encode")
            .expect("encoded payload");
        // CSI I = ESC [ I
        assert_eq!(
            bytes, b"\x1b[I",
            "unexpected focus-gained report: {bytes:?}"
        );
    }

    #[test]
    fn encode_emits_csi_o_for_lost_when_mode_on() {
        let mut terminal = make_terminal();
        terminal
            .set_mode(Mode::FOCUS_EVENT, true)
            .expect("enable 1004");
        let mut enc = PerPaneFocusEncoder::new();
        let bytes = enc
            .encode(&FocusEvent { gained: false }, &terminal)
            .expect("encode")
            .expect("encoded payload");
        assert_eq!(bytes, b"\x1b[O", "unexpected focus-lost report: {bytes:?}");
    }
}
