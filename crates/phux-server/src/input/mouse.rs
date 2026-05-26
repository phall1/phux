//! Wire `MouseEvent` → libghostty allocator-bound `mouse::Event` + per-pane encoder.
//!
//! Per ADR-0008, `MouseAction` and `MouseButton` are re-exports of
//! libghostty's `mouse::{Action, Button}`. Composition is the only work
//! here; no enum conversions.
//!
//! The wire form treats [`MouseButton::Unknown`] as the "no button"
//! sentinel for naked motion. libghostty's encoder takes `Option<Button>`,
//! so this module wraps each call site with `option_for_encoder`.

use libghostty_vt::{
    Error, Terminal,
    mouse::{Encoder as LgMouseEncoder, Event as LgMouseEvent, Position as LgMousePosition},
};
use phux_protocol::input::mouse::{MouseButton, MouseEvent};

/// Treat [`MouseButton::Unknown`] as the wire sentinel for "no button" and
/// hand libghostty `None` for it. Every other variant flows through
/// verbatim (they're the same type).
#[must_use]
pub const fn option_for_encoder(button: MouseButton) -> Option<MouseButton> {
    match button {
        MouseButton::Unknown => None,
        other => Some(other),
    }
}

/// Build a libghostty `mouse::Event` from our wire `MouseEvent`.
///
/// Fallibility comes only from libghostty's FFI allocator. Wire positions
/// are `f64`; libghostty's `MousePosition` is `f32` — downcast here. (That
/// surface-pixel precision is ample for terminal cell geometry.)
#[allow(
    clippy::cast_possible_truncation,
    reason = "f64 → f32 downcast is by design — libghostty's surface coords are f32"
)]
pub fn mouse_event_to_libghostty(ev: &MouseEvent) -> Result<LgMouseEvent<'static>, Error> {
    let mut out = LgMouseEvent::new()?;
    out.set_action(ev.action)
        .set_button(option_for_encoder(ev.button))
        .set_mods(ev.mods)
        .set_position(LgMousePosition {
            x: ev.x as f32,
            y: ev.y as f32,
        });
    Ok(out)
}

/// Per-pane mouse encoder.
///
/// Wraps one `libghostty_vt::mouse::Encoder` plus a reusable byte buffer.
/// Per-pane: tracking mode, output format, and motion-deduplication state
/// reflect a single pane's terminal modes.
#[derive(Debug)]
pub struct PerTerminalMouseEncoder {
    encoder: LgMouseEncoder<'static>,
    buf: Vec<u8>,
}

impl PerTerminalMouseEncoder {
    /// Construct a new per-pane mouse encoder.
    pub fn new() -> Result<Self, Error> {
        Ok(Self {
            encoder: LgMouseEncoder::new()?,
            buf: Vec::with_capacity(32),
        })
    }

    /// Encode a wire mouse event into PTY bytes.
    ///
    /// Refreshes tracking-mode and format from `terminal` before each call
    /// so the encoded sequence matches whatever the inner program currently
    /// has enabled.
    ///
    /// Note: callers MUST separately configure `EncoderSize` (cell geometry)
    /// via the encoder's `set_size` if the inner program may use SGR-Pixels.
    pub fn encode(
        &mut self,
        event: &MouseEvent,
        terminal: &Terminal<'_, '_>,
    ) -> Result<&[u8], Error> {
        let lg_event = mouse_event_to_libghostty(event)?;
        self.encoder.set_options_from_terminal(terminal);
        self.buf.clear();
        self.encoder.encode_to_vec(&lg_event, &mut self.buf)?;
        Ok(&self.buf)
    }

    /// Access the underlying libghostty encoder for size / dedup tuning.
    pub const fn inner_mut(&mut self) -> &mut LgMouseEncoder<'static> {
        &mut self.encoder
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, reason = "tests")]
mod tests {
    use super::*;
    use phux_protocol::input::key::ModSet;
    use phux_protocol::input::mouse::MouseAction;

    #[test]
    fn mouse_event_to_libghostty_round_trips_fields() {
        let ev = MouseEvent {
            action: MouseAction::Press,
            button: MouseButton::Left,
            mods: ModSet::SHIFT,
            x: 12.5,
            y: 34.25,
        };
        let lg = mouse_event_to_libghostty(&ev).expect("convert");
        assert_eq!(lg.action(), MouseAction::Press);
        assert_eq!(lg.button(), Some(MouseButton::Left));
        assert_eq!(lg.mods(), ModSet::SHIFT);
        let pos = lg.position();
        assert!((pos.x - 12.5_f32).abs() < f32::EPSILON);
        assert!((pos.y - 34.25_f32).abs() < f32::EPSILON);
    }

    #[test]
    fn unknown_button_maps_to_none() {
        let ev = MouseEvent {
            action: MouseAction::Motion,
            button: MouseButton::Unknown,
            mods: ModSet::empty(),
            x: 0.0,
            y: 0.0,
        };
        let lg = mouse_event_to_libghostty(&ev).expect("convert");
        assert_eq!(lg.button(), None);
    }
}
