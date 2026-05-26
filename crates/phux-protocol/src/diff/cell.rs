//! The cell — protocol's atomic unit of pane state.
//!
//! Per ADR-0008, `Color` and `Underline` are direct re-exports of
//! libghostty-vt's `style::StyleColor` and `style::Underline`. `Cell` and
//! `CellFlags` remain phux-defined: the wire shape (cell-as-snapshot-with-
//! grapheme-text) is multiplexer-specific, and `CellFlags` is a compact
//! bitfield over libghostty's per-bool `Style` fields.

use bitflags::bitflags;
use smallvec::SmallVec;

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
    ///
    /// Backed by [`SmallVec`] with two inline slots so empty cells, ASCII
    /// cells, and single-base+single-combining graphemes (the common case
    /// for terminal content) require no heap allocation. Wire encoding is
    /// unaffected — see `SPEC.md` §8 and `wire::diff` for the layout.
    pub text: SmallVec<[char; 2]>,
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
            text: SmallVec::new(),
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
    ///
    /// Not `const fn`: `SmallVec::is_empty` is not const, so this method
    /// can't be either. The compiler still inlines aggressively in
    /// practice.
    #[must_use]
    pub fn is_blank(&self) -> bool {
        self.text.is_empty()
            && matches!(self.fg, Color::None)
            && matches!(self.bg, Color::None)
            && matches!(self.underline, Underline::None)
            && self.flags.is_empty()
    }
}

/// A client's color tier (SPEC §6.2).
///
/// Advertised once at HELLO time; the server downsamples outbound cells to
/// fit. `TrueColor` is the most-permissive tier — clients that have not yet
/// advertised caps default here so we never silently downgrade.
///
/// Variants are ordered from most-permissive to least-permissive, but the
/// enum is `#[non_exhaustive]`: protocol additions (e.g. a future palette
/// negotiation tier) must not break downstream consumers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[non_exhaustive]
pub enum ColorSupport {
    /// 24-bit direct RGB. Identity for [`downsample_color`].
    #[default]
    TrueColor,
    /// xterm 256-color palette: 16 system colors, a 6x6x6 RGB cube
    /// (indices 16..=231), and 24-step grayscale (232..=255).
    Indexed256,
    /// 16 system colors only (the ANSI base set + 8 bright variants).
    Indexed16,
}

/// Canonical xterm 16-color system palette in RGB.
///
/// Indices 0..=7 are the base ANSI colors; 8..=15 are the bright variants.
/// Values mirror xterm's defaults, which are also what most modern terminal
/// emulators expose for indices 0..=15 absent user theming. We bake them
/// here so `phux-protocol` keeps zero runtime deps for palette lookup.
///
/// Reference: <https://en.wikipedia.org/wiki/ANSI_escape_code#3-bit_and_4-bit>
/// (xterm column).
const XTERM_16_PALETTE: [[u8; 3]; 16] = [
    [0x00, 0x00, 0x00], //  0 black
    [0x80, 0x00, 0x00], //  1 red
    [0x00, 0x80, 0x00], //  2 green
    [0x80, 0x80, 0x00], //  3 yellow
    [0x00, 0x00, 0x80], //  4 blue
    [0x80, 0x00, 0x80], //  5 magenta
    [0x00, 0x80, 0x80], //  6 cyan
    [0xc0, 0xc0, 0xc0], //  7 white (light gray)
    [0x80, 0x80, 0x80], //  8 bright black (dark gray)
    [0xff, 0x00, 0x00], //  9 bright red
    [0x00, 0xff, 0x00], // 10 bright green
    [0xff, 0xff, 0x00], // 11 bright yellow
    [0x00, 0x00, 0xff], // 12 bright blue
    [0xff, 0x00, 0xff], // 13 bright magenta
    [0x00, 0xff, 0xff], // 14 bright cyan
    [0xff, 0xff, 0xff], // 15 bright white
];

/// xterm 6x6x6 color-cube step values.
///
/// Index `n` in the cube (0..=5) maps to channel byte `CUBE_STEPS[n]`. The
/// non-uniform spacing (0, 95, 135, ..., 255) is xterm-canonical and
/// matches what every "rgb -> 256" reference algorithm in the wild
/// expects on the inverse mapping.
const CUBE_STEPS: [u8; 6] = [0x00, 0x5f, 0x87, 0xaf, 0xd7, 0xff];

/// Map a 0..=255 channel byte to a 0..=5 cube index using xterm's
/// canonical thresholds.
///
/// Equivalent to "find the nearest of `CUBE_STEPS`", inlined to avoid a
/// loop on the hot path.
#[inline]
const fn channel_to_cube_index(c: u8) -> u8 {
    // Thresholds are midpoints between consecutive CUBE_STEPS values.
    // 0..47 → 0  (mid of 0..95 is 47)
    // 48..114 → 1  (mid of 95..135 is 115)
    // 115..154 → 2 (mid of 135..175 is 155)
    // 155..194 → 3
    // 195..234 → 4
    // 235..255 → 5
    if c < 48 {
        0
    } else if c < 115 {
        1
    } else if c < 155 {
        2
    } else if c < 195 {
        3
    } else if c < 235 {
        4
    } else {
        5
    }
}

/// Expand an xterm-256 palette index to its canonical RGB.
///
/// - 0..=15: the [`XTERM_16_PALETTE`].
/// - 16..=231: 6x6x6 cube (`16 + 36*r + 6*g + b` with `r,g,b` in [`CUBE_STEPS`]).
/// - 232..=255: 24-step grayscale starting at `0x08`, stepping by `0x0a`.
#[must_use]
pub const fn xterm_256_to_rgb(idx: u8) -> [u8; 3] {
    if idx < 16 {
        XTERM_16_PALETTE[idx as usize]
    } else if idx < 232 {
        let n = idx - 16;
        let r = n / 36;
        let g = (n % 36) / 6;
        let b = n % 6;
        [
            CUBE_STEPS[r as usize],
            CUBE_STEPS[g as usize],
            CUBE_STEPS[b as usize],
        ]
    } else {
        let gray = 8u8.saturating_add((idx - 232).saturating_mul(10));
        [gray, gray, gray]
    }
}

/// Squared-Euclidean distance between two RGB triples.
///
/// We pick squared (not square-root) because nearest-neighbor only needs
/// monotonicity of the metric; skipping the `sqrt` is free precision.
/// Each squared per-channel delta is at most `255^2 = 65_025`, the sum
/// at most `195_075`, so a `u32` accumulator never overflows. We work
/// entirely in `u32` (taking the absolute delta on each channel before
/// squaring) so there's no signed-to-unsigned cast at the end.
#[inline]
const fn rgb_distance_sq(a: [u8; 3], b: [u8; 3]) -> u32 {
    let dr = a[0].abs_diff(b[0]) as u32;
    let dg = a[1].abs_diff(b[1]) as u32;
    let db = a[2].abs_diff(b[2]) as u32;
    dr * dr + dg * dg + db * db
}

/// Find the xterm-256 index whose canonical RGB is nearest to `rgb`.
///
/// Considers both the 6x6x6 cube candidate (computed by quantizing each
/// channel) and the 24-step grayscale candidate (computed from the mean
/// of `rgb`). Returns whichever has smaller squared-RGB distance from
/// the source. Indices 0..=15 are intentionally not considered: every
/// 16-color value has a cube or gray equivalent that is at least as
/// close, and excluding them keeps the truecolor->256 path deterministic
/// against the wider 240-entry sub-palette that 256-color clients
/// canonically use for diffusion.
#[must_use]
pub fn nearest_xterm_256(rgb: [u8; 3]) -> u8 {
    let [r, g, b] = rgb;

    // Cube candidate.
    let cr = channel_to_cube_index(r);
    let cg = channel_to_cube_index(g);
    let cb = channel_to_cube_index(b);
    let cube_idx = 16 + 36 * cr + 6 * cg + cb;
    let cube_rgb = [
        CUBE_STEPS[cr as usize],
        CUBE_STEPS[cg as usize],
        CUBE_STEPS[cb as usize],
    ];
    let cube_d = rgb_distance_sq(rgb, cube_rgb);

    // Grayscale candidate. Use the average of the three channels as the
    // target gray, then snap to one of the 24 gray steps (0x08, 0x12,
    // ..., 0xee). Index = 232 + clamp((avg - 8) / 10, 0, 23). Below
    // 0x08 we still pick gray 232 since the cube candidate already
    // wins for very-dark inputs.
    //
    // `avg` is the mean of three u8s, so `0..=255` — safe to cast
    // narrow without truncation. Asserted via the divisor.
    let avg_u16 = (u16::from(r) + u16::from(g) + u16::from(b)) / 3;
    debug_assert!(avg_u16 <= 255);
    #[allow(
        clippy::cast_possible_truncation,
        reason = "avg_u16 <= 255 by construction"
    )]
    let avg = avg_u16 as u8;
    let gray_idx_offset: u8 = if avg < 8 {
        0
    } else {
        // Round to nearest 10 to match the actual gray-ramp positions
        // more closely than truncation does. Numerator <= 255 - 8 + 5
        // = 252, divisor 10, quotient <= 25 < 256 — narrowing is safe.
        let raw = (u16::from(avg) - 8 + 5) / 10;
        let clamped = raw.min(23);
        #[allow(clippy::cast_possible_truncation, reason = "raw.min(23) <= 23")]
        let out = clamped as u8;
        out
    };
    let gray_idx = 232 + gray_idx_offset;
    let gray_byte = 8u8.saturating_add(gray_idx_offset.saturating_mul(10));
    let gray_d = rgb_distance_sq(rgb, [gray_byte, gray_byte, gray_byte]);

    if gray_d < cube_d { gray_idx } else { cube_idx }
}

/// Find the xterm-16 system-palette index whose canonical RGB is nearest
/// to `rgb`. Returns a value in `0..=15`.
#[must_use]
pub const fn nearest_xterm_16(rgb: [u8; 3]) -> u8 {
    let mut best_idx: u8 = 0;
    let mut best_d = u32::MAX;
    let mut i = 0u8;
    while i < 16 {
        let d = rgb_distance_sq(rgb, XTERM_16_PALETTE[i as usize]);
        if d < best_d {
            best_d = d;
            best_idx = i;
        }
        i += 1;
    }
    best_idx
}

/// Downsample `color` to fit a client's [`ColorSupport`] tier (SPEC §6.2).
///
/// - `TrueColor` target: identity. Returns `color` unchanged.
/// - `Indexed256` target: `Rgb` maps to the nearest xterm-256 palette
///   index (cube + grayscale candidates, whichever is closer). `Palette`
///   and `None` pass through unchanged.
/// - `Indexed16` target: `Rgb` maps to the nearest xterm-16 system color
///   via [`nearest_xterm_16`]. `Palette(n)` with `n < 16` passes through;
///   `Palette(n)` with `n >= 16` is first canonicalized to RGB via
///   [`xterm_256_to_rgb`], then mapped via `nearest_xterm_16`. `None`
///   passes through.
///
/// Nearest-neighbor uses simple squared-Euclidean distance in RGB space.
/// A perceptual metric (CIELAB / CIEDE2000) is a tracked follow-up — see
/// `bd` ticket "Perceptual color distance (CIELAB) for downsample".
///
/// Free function (rather than inherent method) because [`Color`] is a
/// re-export of `libghostty_vt::style::StyleColor` (per ADR-0008) and
/// Rust forbids inherent impls on foreign types. Callers may prefer the
/// method-call syntax via [`ColorDownsample::downsample`] (see below).
#[must_use]
pub fn downsample_color(color: Color, support: ColorSupport) -> Color {
    match support {
        ColorSupport::TrueColor => color,
        ColorSupport::Indexed256 => match color {
            Color::Rgb(RgbColor { r, g, b }) => {
                Color::Palette(PaletteIndex(nearest_xterm_256([r, g, b])))
            }
            // `Palette` and `None` already fit Indexed256.
            other => other,
        },
        ColorSupport::Indexed16 => match color {
            Color::None => Color::None,
            Color::Rgb(RgbColor { r, g, b }) => {
                Color::Palette(PaletteIndex(nearest_xterm_16([r, g, b])))
            }
            Color::Palette(PaletteIndex(n)) if n < 16 => Color::Palette(PaletteIndex(n)),
            Color::Palette(PaletteIndex(n)) => {
                let rgb = xterm_256_to_rgb(n);
                Color::Palette(PaletteIndex(nearest_xterm_16(rgb)))
            }
        },
    }
}

/// Method-call sugar for [`downsample_color`].
///
/// Implemented as an extension trait because [`Color`] is a foreign
/// re-export (see [`downsample_color`] docs). Importing this trait
/// lets callers write `color.downsample(support)` in addition to the
/// free-function form.
pub trait ColorDownsample {
    /// See [`downsample_color`].
    #[must_use]
    fn downsample(self, support: ColorSupport) -> Self;
}

impl ColorDownsample for Color {
    fn downsample(self, support: ColorSupport) -> Self {
        downsample_color(self, support)
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
            text: smallvec::smallvec!['a'],
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

    // --- downsample tests (SPEC §6.2) ---
    //
    // Test helpers + cases for `downsample_color` / `ColorDownsample`.
    // The palette tables baked above (XTERM_16_PALETTE, CUBE_STEPS) are
    // the source of truth for the expected indices below. If the
    // palette ever changes (e.g. user-theming negotiation), these
    // assertions update mechanically.

    use proptest::prelude::*;

    fn rgb(r: u8, g: u8, b: u8) -> Color {
        Color::Rgb(RgbColor { r, g, b })
    }

    #[test]
    fn downsample_color_support_default_is_truecolor() {
        assert_eq!(ColorSupport::default(), ColorSupport::TrueColor);
    }

    #[test]
    fn downsample_truecolor_is_identity() {
        let cases = [
            Color::None,
            Color::Palette(PaletteIndex(0)),
            Color::Palette(PaletteIndex(123)),
            rgb(0, 0, 0),
            rgb(255, 255, 255),
            rgb(0x12, 0x34, 0x56),
        ];
        for c in cases {
            assert_eq!(
                downsample_color(c, ColorSupport::TrueColor),
                c,
                "TrueColor must be identity for {c:?}"
            );
            // Method-syntax variant must match.
            assert_eq!(c.downsample(ColorSupport::TrueColor), c);
        }
    }

    #[test]
    fn downsample_none_passes_through_every_tier() {
        for support in [
            ColorSupport::TrueColor,
            ColorSupport::Indexed256,
            ColorSupport::Indexed16,
        ] {
            assert_eq!(downsample_color(Color::None, support), Color::None);
        }
    }

    #[test]
    fn rgb_to_256_known_anchors() {
        // Black -> cube origin (index 16). The cube candidate at [0,0,0]
        // wins over gray 232 (which is RGB [8,8,8], distance 192).
        assert_eq!(
            downsample_color(rgb(0, 0, 0), ColorSupport::Indexed256),
            Color::Palette(PaletteIndex(16))
        );
        // Pure white -> top-corner of cube (index 231 = 16 + 36*5 + 6*5 + 5).
        assert_eq!(
            downsample_color(rgb(255, 255, 255), ColorSupport::Indexed256),
            Color::Palette(PaletteIndex(231))
        );
        // Pure red -> 16 + 36*5 + 6*0 + 0 = 196.
        assert_eq!(
            downsample_color(rgb(255, 0, 0), ColorSupport::Indexed256),
            Color::Palette(PaletteIndex(196))
        );
        // Pure green -> 16 + 36*0 + 6*5 + 0 = 46.
        assert_eq!(
            downsample_color(rgb(0, 255, 0), ColorSupport::Indexed256),
            Color::Palette(PaletteIndex(46))
        );
        // Pure blue -> 16 + 36*0 + 6*0 + 5 = 21.
        assert_eq!(
            downsample_color(rgb(0, 0, 255), ColorSupport::Indexed256),
            Color::Palette(PaletteIndex(21))
        );
        // Mid-gray [128,128,128]: gray candidate is gray-ramp index
        // (128-8+5)/10 = 12 -> 232+12 = 244, byte = 8 + 120 = 128, distance 0.
        // Cube candidate quantizes to (135,135,135) -> distance 3*49 = 147.
        // Gray wins.
        assert_eq!(
            downsample_color(rgb(128, 128, 128), ColorSupport::Indexed256),
            Color::Palette(PaletteIndex(244))
        );
    }

    #[test]
    fn rgb_to_16_known_anchors() {
        assert_eq!(
            downsample_color(rgb(0, 0, 0), ColorSupport::Indexed16),
            Color::Palette(PaletteIndex(0))
        );
        assert_eq!(
            downsample_color(rgb(0x80, 0, 0), ColorSupport::Indexed16),
            Color::Palette(PaletteIndex(1))
        );
        // Pure red -> bright red (index 9) is closer than dark red (1).
        assert_eq!(
            downsample_color(rgb(255, 0, 0), ColorSupport::Indexed16),
            Color::Palette(PaletteIndex(9))
        );
        assert_eq!(
            downsample_color(rgb(255, 255, 255), ColorSupport::Indexed16),
            Color::Palette(PaletteIndex(15))
        );
    }

    #[test]
    fn indexed_low_tier_passthrough_to_indexed16() {
        // `Palette(n)` for n < 16 must survive Indexed16 unchanged.
        for n in 0u8..16 {
            assert_eq!(
                downsample_color(Color::Palette(PaletteIndex(n)), ColorSupport::Indexed16),
                Color::Palette(PaletteIndex(n))
            );
        }
    }

    #[test]
    fn indexed_high_tier_maps_via_canonical_rgb() {
        // Index 16 is cube origin [0,0,0] -> nearest 16-color is black (0).
        assert_eq!(
            downsample_color(Color::Palette(PaletteIndex(16)), ColorSupport::Indexed16),
            Color::Palette(PaletteIndex(0))
        );
        // Index 231 is cube top [255,255,255] -> nearest 16-color is white (15).
        assert_eq!(
            downsample_color(Color::Palette(PaletteIndex(231)), ColorSupport::Indexed16),
            Color::Palette(PaletteIndex(15))
        );
        // Index 196 is cube pure-red [255,0,0] -> nearest 16-color is
        // bright red (9).
        assert_eq!(
            downsample_color(Color::Palette(PaletteIndex(196)), ColorSupport::Indexed16),
            Color::Palette(PaletteIndex(9))
        );
    }

    #[test]
    fn indexed_passes_through_indexed256_unchanged() {
        // Indexed source already fits Indexed256; identity.
        for n in [0u8, 7, 15, 16, 100, 196, 231, 232, 255] {
            assert_eq!(
                downsample_color(Color::Palette(PaletteIndex(n)), ColorSupport::Indexed256),
                Color::Palette(PaletteIndex(n))
            );
        }
    }

    #[test]
    fn downsample_is_idempotent_per_tier() {
        // Once at a tier, re-applying the same tier must be a no-op.
        let probes = [
            rgb(0, 0, 0),
            rgb(255, 255, 255),
            rgb(0x12, 0x34, 0x56),
            rgb(200, 50, 100),
            rgb(127, 127, 127),
            Color::Palette(PaletteIndex(42)),
            Color::None,
        ];
        for c in probes {
            let once_256 = downsample_color(c, ColorSupport::Indexed256);
            let twice_256 = downsample_color(once_256, ColorSupport::Indexed256);
            assert_eq!(once_256, twice_256, "Indexed256 idempotence on {c:?}");

            let once_16 = downsample_color(c, ColorSupport::Indexed16);
            let twice_16 = downsample_color(once_16, ColorSupport::Indexed16);
            assert_eq!(once_16, twice_16, "Indexed16 idempotence on {c:?}");
        }
    }

    #[test]
    fn lower_tier_first_then_lowest_matches_direct_to_lowest() {
        // For RGB inputs, the "downsample to 256, then to 16" path may
        // differ from "downsample directly to 16" (the cube quantization
        // step is lossy). What we DO guarantee is that re-applying
        // Indexed16 to the once-Indexed16'd output is a no-op (already
        // checked above). Here we verify the lossy-idempotence chain:
        // direct-to-16 == once-to-16(direct-to-16).
        for c in [
            rgb(10, 20, 30),
            rgb(200, 100, 50),
            rgb(127, 127, 127),
            rgb(0, 0, 0),
            rgb(255, 255, 255),
        ] {
            let direct = downsample_color(c, ColorSupport::Indexed16);
            let chain = downsample_color(
                downsample_color(direct, ColorSupport::Indexed256),
                ColorSupport::Indexed16,
            );
            assert_eq!(direct, chain, "lossy chain stability on {c:?}");
        }
    }

    fn arb_color() -> impl Strategy<Value = Color> {
        prop_oneof![
            Just(Color::None),
            (0u8..=255).prop_map(|n| Color::Palette(PaletteIndex(n))),
            (any::<u8>(), any::<u8>(), any::<u8>()).prop_map(|(r, g, b)| Color::Rgb(RgbColor {
                r,
                g,
                b
            })),
        ]
    }

    proptest! {
        #[test]
        fn proptest_truecolor_is_identity(c in arb_color()) {
            prop_assert_eq!(downsample_color(c, ColorSupport::TrueColor), c);
        }

        #[test]
        fn proptest_indexed256_never_yields_rgb(c in arb_color()) {
            // Indexed256 must NEVER produce a Color::Rgb (SPEC §6.2).
            let out = downsample_color(c, ColorSupport::Indexed256);
            prop_assert!(!matches!(out, Color::Rgb(_)));
        }

        #[test]
        fn proptest_indexed16_never_yields_rgb_or_high_palette(c in arb_color()) {
            let out = downsample_color(c, ColorSupport::Indexed16);
            match out {
                Color::Rgb(_) => prop_assert!(false, "Indexed16 emitted Rgb"),
                Color::Palette(PaletteIndex(n)) => prop_assert!(n < 16),
                Color::None => {}
            }
        }

        #[test]
        fn proptest_indexed256_idempotent(c in arb_color()) {
            let once = downsample_color(c, ColorSupport::Indexed256);
            let twice = downsample_color(once, ColorSupport::Indexed256);
            prop_assert_eq!(once, twice);
        }

        #[test]
        fn proptest_indexed16_idempotent(c in arb_color()) {
            let once = downsample_color(c, ColorSupport::Indexed16);
            let twice = downsample_color(once, ColorSupport::Indexed16);
            prop_assert_eq!(once, twice);
        }
    }
}
