//! The cell — protocol's atomic unit of pane state.
//!
//! Per ADR-0008, `Color` and `Underline` are direct re-exports of
//! libghostty-vt's `style::StyleColor` and `style::Underline`. `Cell` and
//! `CellFlags` remain phux-defined: the wire shape (cell-as-snapshot-with-
//! grapheme-text) is multiplexer-specific, and `CellFlags` is a compact
//! bitfield over libghostty's per-bool `Style` fields.

use bitflags::bitflags;

pub use libghostty_vt::style::StyleColor as Color;
pub use libghostty_vt::style::Underline;

/// Re-export of libghostty-vt's RGB color value, exposed alongside [`Color`]
/// because `Color::Rgb` wraps it.
pub use libghostty_vt::style::RgbColor;

/// Re-export of libghostty-vt's palette-index type, wrapping `u8`.
pub use libghostty_vt::style::PaletteIndex;

/// One screen cell.
///
/// Composes libghostty's color/underline atoms with a grapheme-cluster text
/// field and a compact rendering-flags bitset. `Default` is implemented
/// manually because libghostty's `StyleColor` and `Underline` don't derive
/// `Default` upstream — we pick the obvious zero values (`None`/`None`).
#[derive(Debug, Clone, PartialEq, Eq)]
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

impl Default for Cell {
    fn default() -> Self {
        Self {
            text: Vec::new(),
            fg: Color::None,
            bg: Color::None,
            underline: Underline::None,
            underline_color: Color::None,
            flags: CellFlags::empty(),
        }
    }
}

impl Cell {
    /// A completely blank cell: empty text, default colors, no flags.
    #[must_use]
    pub fn blank() -> Self {
        Self::default()
    }

    /// True if this is a blank cell.
    #[must_use]
    pub const fn is_blank(&self) -> bool {
        self.text.is_empty()
            && matches!(self.fg, Color::None)
            && matches!(self.bg, Color::None)
            && matches!(self.underline, Underline::None)
            && self.flags.is_empty()
    }
}

bitflags! {
    /// Cell-rendering flags. Compact bitfield over libghostty's per-bool
    /// `Style` fields (bold, italic, faint, blink, inverse, invisible,
    /// strikethrough, overline) plus phux-specific render hints
    /// (`WIDE_LEFT`, `WIDE_RIGHT`, `PROTECTED`, `BLINK_FAST`).
    ///
    /// Wire layout matches SPEC.md §8.2.
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
        let c = Cell {
            text: vec!['a'],
            ..Cell::blank()
        };
        assert!(!c.is_blank());
    }

    #[test]
    fn cell_with_color_is_not_blank() {
        let c = Cell {
            fg: Color::Rgb(RgbColor { r: 1, g: 2, b: 3 }),
            ..Cell::blank()
        };
        assert!(!c.is_blank());
    }
}
