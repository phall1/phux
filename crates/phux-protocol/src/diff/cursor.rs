//! Cursor state and pane-wide modes (`SPEC.md` §8.5).
//!
//! Per SPEC §8.1, cursor state and pane modes ride along with every
//! `PANE_DIFF` as **struct fields**, not as ops in the diff op stream.
//! SPEC §8.5: "pulling them out into separate messages would increase wire
//! chatter for no benefit."
//!
//! Both types are small, `Copy`, and re-exported from the crate root.

/// Cursor shape (`SPEC.md` §8.5).
///
/// `Block` matches `DECSCUSR 1,2`; `Bar` matches `DECSCUSR 5,6`;
/// `Underline` matches `DECSCUSR 3,4`. `BlockHollow` is a phux extension for
/// rendering the focused-vs-unfocused distinction common in terminal UIs.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[non_exhaustive]
pub enum CursorShape {
    /// `DECSCUSR 1, 2` — block.
    #[default]
    Block = 0,
    /// `DECSCUSR 5, 6` — bar.
    Bar = 1,
    /// `DECSCUSR 3, 4` — underline.
    Underline = 2,
    /// Hollow block (rendered when the pane has no focus).
    BlockHollow = 3,
}

/// Cursor state carried with every `PANE_DIFF` (`SPEC.md` §8.1, §8.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CursorState {
    /// Zero-based row index.
    pub row: u16,
    /// Zero-based column index.
    pub col: u16,
    /// Whether the cursor is visible.
    pub visible: bool,
    /// Cursor shape.
    pub shape: CursorShape,
    /// Whether the cursor is blinking.
    pub blink: bool,
}

impl Default for CursorState {
    fn default() -> Self {
        Self {
            row: 0,
            col: 0,
            visible: true,
            shape: CursorShape::Block,
            blink: true,
        }
    }
}

/// Pane-wide modes bitset (`SPEC.md` §8.5).
///
/// Stored as a `u16` to match SPEC's layout: single-bit flags occupy
/// `0x0001..=0x0008` and `0x1000..=0x2000`; the `MOUSE_PROTOCOL` /
/// `MOUSE_ENCODING` fields are 4-bit packed values at `0x00F0` and `0x0F00`.
///
/// `MouseProtocol` / `MouseEncoding` enums are not yet modeled — see
/// `bd show phux-bxa`. For now mouse protocol/encoding are addressed as raw
/// `u8` via [`PaneModes::mouse_protocol`] / [`PaneModes::with_mouse_protocol`]
/// and the matching encoding accessors.
///
/// Reserved bits (`0x4000`, `0x8000`) round-trip on the wire so future
/// protocol minor versions can attach meaning additively per SPEC §16.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct PaneModes(pub u16);

impl PaneModes {
    /// All flags clear.
    pub const EMPTY: Self = Self(0);

    /// Alternate screen buffer active (`xterm` `?1049`).
    pub const ALTSCREEN_ACTIVE: u16 = 0x0001;
    /// Bracketed paste mode (`xterm` `?2004`).
    pub const BRACKETED_PASTE: u16 = 0x0002;
    /// Application cursor keys (`DECCKM`).
    pub const APP_CURSOR_KEYS: u16 = 0x0004;
    /// Application keypad (`DECKPAM`).
    pub const APP_KEYPAD: u16 = 0x0008;
    /// Focus reporting (`xterm` `?1004`).
    pub const FOCUS_REPORTING: u16 = 0x1000;
    /// Origin mode (`DECOM`).
    pub const ORIGIN_MODE: u16 = 0x2000;

    /// Mask covering the 4-bit `MouseProtocol` packed field.
    pub const MOUSE_PROTOCOL_MASK: u16 = 0x00F0;
    /// Bit-shift to read/write the `MouseProtocol` field.
    pub const MOUSE_PROTOCOL_SHIFT: u32 = 4;
    /// Mask covering the 4-bit `MouseEncoding` packed field.
    pub const MOUSE_ENCODING_MASK: u16 = 0x0F00;
    /// Bit-shift to read/write the `MouseEncoding` field.
    pub const MOUSE_ENCODING_SHIFT: u32 = 8;

    /// Construct from a raw `u16`. Unknown bits are preserved on the wire so
    /// minor-version protocol additions remain backward compatible per
    /// SPEC §16 ("tolerate unknown trailing fields").
    #[must_use]
    pub const fn from_bits(bits: u16) -> Self {
        Self(bits)
    }

    /// Raw bit representation.
    #[must_use]
    pub const fn bits(self) -> u16 {
        self.0
    }

    /// True iff every bit in `flag` is set in `self`.
    #[must_use]
    pub const fn contains(self, flag: u16) -> bool {
        (self.0 & flag) == flag && flag != 0
    }

    /// Return a copy of `self` with the bits in `flag` set.
    #[must_use]
    pub const fn insert(mut self, flag: u16) -> Self {
        self.0 |= flag;
        self
    }

    /// Return a copy of `self` with the bits in `flag` cleared.
    #[must_use]
    pub const fn remove(mut self, flag: u16) -> Self {
        self.0 &= !flag;
        self
    }

    /// Decode the 4-bit `MouseProtocol` field as a `u8`. The concrete enum
    /// is tracked under `bd show phux-bxa`; until then callers see the raw
    /// nibble (`0..=15`).
    #[must_use]
    pub const fn mouse_protocol(self) -> u8 {
        ((self.0 & Self::MOUSE_PROTOCOL_MASK) >> Self::MOUSE_PROTOCOL_SHIFT) as u8
    }

    /// Decode the 4-bit `MouseEncoding` field as a `u8`. See
    /// [`Self::mouse_protocol`] for the enum follow-up.
    #[must_use]
    pub const fn mouse_encoding(self) -> u8 {
        ((self.0 & Self::MOUSE_ENCODING_MASK) >> Self::MOUSE_ENCODING_SHIFT) as u8
    }

    /// Return a copy of `self` with the `MouseProtocol` field set to the
    /// low 4 bits of `protocol`.
    #[must_use]
    pub const fn with_mouse_protocol(mut self, protocol: u8) -> Self {
        let shifted = ((protocol as u16) << Self::MOUSE_PROTOCOL_SHIFT) & Self::MOUSE_PROTOCOL_MASK;
        self.0 = (self.0 & !Self::MOUSE_PROTOCOL_MASK) | shifted;
        self
    }

    /// Return a copy of `self` with the `MouseEncoding` field set to the
    /// low 4 bits of `encoding`.
    #[must_use]
    pub const fn with_mouse_encoding(mut self, encoding: u8) -> Self {
        let shifted = ((encoding as u16) << Self::MOUSE_ENCODING_SHIFT) & Self::MOUSE_ENCODING_MASK;
        self.0 = (self.0 & !Self::MOUSE_ENCODING_MASK) | shifted;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_is_all_zero() {
        assert_eq!(PaneModes::EMPTY.bits(), 0);
        assert_eq!(PaneModes::default(), PaneModes::EMPTY);
    }

    #[test]
    fn insert_and_contains() {
        let m = PaneModes::EMPTY
            .insert(PaneModes::ALTSCREEN_ACTIVE)
            .insert(PaneModes::BRACKETED_PASTE);
        assert!(m.contains(PaneModes::ALTSCREEN_ACTIVE));
        assert!(m.contains(PaneModes::BRACKETED_PASTE));
        assert!(!m.contains(PaneModes::APP_CURSOR_KEYS));
    }

    #[test]
    fn remove_clears_bit() {
        let m = PaneModes::EMPTY
            .insert(PaneModes::ALTSCREEN_ACTIVE)
            .remove(PaneModes::ALTSCREEN_ACTIVE);
        assert!(!m.contains(PaneModes::ALTSCREEN_ACTIVE));
        assert_eq!(m.bits(), 0);
    }

    #[test]
    fn mouse_protocol_roundtrips_low_nibble() {
        let m = PaneModes::EMPTY.with_mouse_protocol(0b1010);
        assert_eq!(m.mouse_protocol(), 0b1010);
        // High bits of the input get masked off.
        let m = PaneModes::EMPTY.with_mouse_protocol(0xF7);
        assert_eq!(m.mouse_protocol(), 0x7);
    }

    #[test]
    fn mouse_encoding_roundtrips_low_nibble() {
        let m = PaneModes::EMPTY.with_mouse_encoding(0b1100);
        assert_eq!(m.mouse_encoding(), 0b1100);
    }

    #[test]
    fn protocol_and_encoding_dont_collide() {
        let m = PaneModes::EMPTY
            .with_mouse_protocol(0xA)
            .with_mouse_encoding(0x5)
            .insert(PaneModes::ALTSCREEN_ACTIVE);
        assert_eq!(m.mouse_protocol(), 0xA);
        assert_eq!(m.mouse_encoding(), 0x5);
        assert!(m.contains(PaneModes::ALTSCREEN_ACTIVE));
    }

    #[test]
    fn unknown_bits_preserved() {
        // Bit 0x4000 isn't named today; we still want round-trip preservation.
        let m = PaneModes::from_bits(0x4000);
        assert_eq!(m.bits(), 0x4000);
    }
}
