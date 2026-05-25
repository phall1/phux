//! The cell — protocol's atomic unit of pane state.
//!
//! A cell is one screen position: a grapheme cluster plus fully-resolved
//! rendering attributes (`SPEC.md` §8.2). Server-side resolution means
//! `Color::Default` is the only "follow the terminal default" value; all
//! palette indices and truecolor values arrive on the wire as themselves.

use bitflags::bitflags;

/// One screen cell.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct Cell {
    /// Grapheme cluster occupying this cell. May be empty for a blank cell.
    /// First element is the base codepoint; remaining elements are combining
    /// codepoints in source order.
    pub text: Vec<char>,
    /// Foreground color.
    pub fg: Color,
    /// Background color.
    pub bg: Color,
    /// Underline kind.
    pub underline: Underline,
    /// Underline color. Often the same as `fg`; tracked separately for
    /// terminals that set it independently.
    pub underline_color: Color,
    /// Rendering flag bitset.
    pub flags: CellFlags,
}

impl Cell {
    /// A completely blank cell: empty text, default colors, no flags.
    /// Identical to `Cell::default()`; named for clarity at call sites.
    #[must_use]
    pub fn blank() -> Self {
        Self::default()
    }

    /// True if this is a blank cell.
    #[must_use]
    pub fn is_blank(&self) -> bool {
        self.text.is_empty()
            && self.fg == Color::Default
            && self.bg == Color::Default
            && self.underline == Underline::None
            && self.flags.is_empty()
    }
}

/// A fully-resolved cell color.
///
/// `SPEC.md` §8.2 says: servers MUST NOT emit `Rgb` to clients without
/// `TrueColor` capability. Downsampling happens server-side.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Color {
    /// The terminal's foreground/background default — resolved by the client
    /// against its theme. The only "indirection" allowed on the wire.
    #[default]
    Default,
    /// Palette index in the 0..=255 ANSI palette.
    Indexed(u8),
    /// 24-bit truecolor.
    Rgb(u8, u8, u8),
}

/// Underline kind. Mirrors `SPEC.md` §8.2.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Underline {
    /// No underline.
    #[default]
    None = 0,
    /// `CSI 4 m`.
    Single = 1,
    /// `CSI 21 m`.
    Double = 2,
    /// Curly / squiggly underline.
    Curly = 3,
    /// Dotted underline.
    Dotted = 4,
    /// Dashed underline.
    Dashed = 5,
}

bitflags! {
    /// Cell-rendering flags. Wire layout matches `SPEC.md` §8.2.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
    pub struct CellFlags: u16 {
        /// Bold.
        const BOLD          = 0x0001;
        /// Faint / dim.
        const FAINT         = 0x0002;
        /// Italic.
        const ITALIC        = 0x0004;
        /// Slow blink.
        const BLINK_SLOW    = 0x0008;
        /// Fast blink.
        const BLINK_FAST    = 0x0010;
        /// Reverse video.
        const REVERSE       = 0x0020;
        /// Invisible (text not rendered).
        const INVISIBLE     = 0x0040;
        /// Strikethrough.
        const STRIKETHROUGH = 0x0080;
        /// Overlined.
        const OVERLINED     = 0x0100;
        /// First half of a wide character.
        const WIDE_LEFT     = 0x0200;
        /// Second half of a wide character.
        const WIDE_RIGHT    = 0x0400;
        /// DEC protected.
        const PROTECTED     = 0x0800;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blank_cell_is_blank() {
        assert!(Cell::blank().is_blank());
    }

    #[test]
    fn cell_with_text_is_not_blank() {
        let c = Cell { text: vec!['a'], ..Cell::blank() };
        assert!(!c.is_blank());
    }

    #[test]
    fn cell_with_color_is_not_blank() {
        let c = Cell { fg: Color::Rgb(1, 2, 3), ..Cell::blank() };
        assert!(!c.is_blank());
    }
}
