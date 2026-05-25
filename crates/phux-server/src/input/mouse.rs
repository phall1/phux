//! Mouse event translation: wire → libghostty-vt.
//!
//! Mirrors libghostty-vt's `mouse::Event` field-for-field per ADR-0006. The
//! wire form carries [`MouseButton::Unknown`] in place of libghostty's
//! "no button" / `None` button — this conversion remaps the sentinel.

use libghostty_vt::{
    Error, Terminal,
    mouse::{
        Action as LgMouseAction, Button as LgMouseButton, Encoder as LgMouseEncoder,
        Event as LgMouseEvent, Position as LgMousePosition,
    },
};
use phux_protocol::input::mouse::{MouseAction, MouseButton, MouseEvent};

use super::key::modset_to_libghostty;

/// Map wire [`MouseAction`] to libghostty's [`LgMouseAction`].
/// Discriminants match (ADR-0006).
#[must_use]
pub const fn mouse_action_to_libghostty(action: MouseAction) -> LgMouseAction {
    match action {
        MouseAction::Press => LgMouseAction::Press,
        MouseAction::Release => LgMouseAction::Release,
        MouseAction::Motion => LgMouseAction::Motion,
    }
}

/// Map wire [`MouseButton`] to libghostty's `Option<Button>`.
///
/// [`MouseButton::Unknown`] is the wire's "no button" sentinel (used for
/// naked motion) and maps to `None`. Every other variant maps verbatim;
/// discriminants are pinned by the
/// [`mouse_button_discriminants_match_libghostty`](self::tests::mouse_button_discriminants_match_libghostty)
/// test.
#[must_use]
pub const fn mouse_button_to_libghostty(button: MouseButton) -> Option<LgMouseButton> {
    match button {
        MouseButton::Unknown => None,
        MouseButton::Left => Some(LgMouseButton::Left),
        MouseButton::Right => Some(LgMouseButton::Right),
        MouseButton::Middle => Some(LgMouseButton::Middle),
        MouseButton::Four => Some(LgMouseButton::Four),
        MouseButton::Five => Some(LgMouseButton::Five),
        MouseButton::Six => Some(LgMouseButton::Six),
        MouseButton::Seven => Some(LgMouseButton::Seven),
        MouseButton::Eight => Some(LgMouseButton::Eight),
        MouseButton::Nine => Some(LgMouseButton::Nine),
        MouseButton::Ten => Some(LgMouseButton::Ten),
        MouseButton::Eleven => Some(LgMouseButton::Eleven),
    }
}

/// Fallible conversion: wire [`MouseEvent`] → libghostty [`LgMouseEvent`].
///
/// Orphan-rules prevent a trait impl (both types are foreign). Fallibility
/// comes only from the FFI allocator; the field copy itself is total. Note
/// that wire positions are `f64` per `SPEC.md` §9.2; libghostty's
/// `MousePosition` is `f32` — values are downcast here. (This is the
/// behavior libghostty's encoders expect; surface-space pixel precision
/// at `f32` is ample for terminal cell geometry.)
#[allow(
    clippy::cast_possible_truncation,
    reason = "f64 → f32 downcast is by design — libghostty's surface coords are f32"
)]
pub fn mouse_event_to_libghostty(ev: &MouseEvent) -> Result<LgMouseEvent<'static>, Error> {
    let mut out = LgMouseEvent::new()?;
    out.set_action(mouse_action_to_libghostty(ev.action))
        .set_button(mouse_button_to_libghostty(ev.button))
        .set_mods(modset_to_libghostty(ev.mods))
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
pub struct PerPaneMouseEncoder {
    encoder: LgMouseEncoder<'static>,
    buf: Vec<u8>,
}

impl PerPaneMouseEncoder {
    /// Construct a new per-pane mouse encoder.
    pub fn new() -> Result<Self, Error> {
        Ok(Self {
            encoder: LgMouseEncoder::new()?,
            buf: Vec::with_capacity(32),
        })
    }

    /// Encode a wire mouse event into PTY bytes.
    ///
    /// Refreshes tracking-mode and format from `terminal` before each call so
    /// the encoded sequence matches whatever the inner program currently has
    /// enabled (SGR, SGR-Pixels, urxvt, X10, UTF-8, or off).
    ///
    /// Note: callers MUST separately configure `EncoderSize` (cell geometry)
    /// via the encoder's `set_size` if the inner program may use SGR-Pixels —
    /// that data is not in `Terminal` state. See `SPEC.md` §9.2.2.
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
        assert_eq!(lg.action(), LgMouseAction::Press);
        assert_eq!(lg.button(), Some(LgMouseButton::Left));
        assert_eq!(lg.mods(), libghostty_vt::key::Mods::SHIFT);
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

    #[test]
    fn mouse_button_discriminants_match_libghostty() {
        const PAIRS: &[(MouseButton, LgMouseButton)] = &[
            (MouseButton::Unknown, LgMouseButton::Unknown),
            (MouseButton::Left, LgMouseButton::Left),
            (MouseButton::Right, LgMouseButton::Right),
            (MouseButton::Middle, LgMouseButton::Middle),
            (MouseButton::Four, LgMouseButton::Four),
            (MouseButton::Five, LgMouseButton::Five),
            (MouseButton::Six, LgMouseButton::Six),
            (MouseButton::Seven, LgMouseButton::Seven),
            (MouseButton::Eight, LgMouseButton::Eight),
            (MouseButton::Nine, LgMouseButton::Nine),
            (MouseButton::Ten, LgMouseButton::Ten),
            (MouseButton::Eleven, LgMouseButton::Eleven),
        ];
        for &(wire, lg) in PAIRS {
            assert_eq!(
                wire as u32, lg as u32,
                "MouseButton::{wire:?} discriminant drifts vs libghostty {lg:?}",
            );
        }
    }

    #[test]
    fn mouse_action_discriminants_match_libghostty() {
        const PAIRS: &[(MouseAction, LgMouseAction)] = &[
            (MouseAction::Press, LgMouseAction::Press),
            (MouseAction::Release, LgMouseAction::Release),
            (MouseAction::Motion, LgMouseAction::Motion),
        ];
        for &(wire, lg) in PAIRS {
            assert_eq!(
                wire as u32, lg as u32,
                "MouseAction::{wire:?} discriminant drifts vs libghostty {lg:?}",
            );
        }
    }
}
