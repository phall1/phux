//! Wire codec for [`DiffOp`] and its sub-types.
//!
//! See `SPEC.md` §8.3 for the operation set and Appendix A for the encoding
//! primitives. All multi-byte integers are big-endian. The layout chosen here
//! deliberately uses fixed-width tags so on-wire diffs render cleanly under
//! `insta` snapshots.
//!
//! # Layout summary
//!
//! ```text
//! DiffOp[]      := u32 count | DiffOp{count}
//! DiffOp        := u8 tag | body
//!     0x01 CellRun     := u16 row | u16 col | u32 cell_count | Cell{cell_count}
//!     0x02 Clear       := u16 row | u16 col | u16 count
//!
//! Cell          := Text | Color fg | Color bg | Underline | Color uline | CellFlags
//! Text          := u8 grapheme_count | u32 codepoint{grapheme_count}
//! Color         := u8 tag (0 None | 1 Palette u8 | 2 Rgb u8 u8 u8)
//! Underline     := u8 discriminant
//! CellFlags     := u16 bitset (matches bitflags repr)
//! CursorShape   := u8 discriminant (only on PANE_DIFF cursor field, not in ops)
//! ```
//!
//! Tags `0x03` (`CursorMove`) and `0x04` (`CursorStyle`) used to live here.
//! They were removed when the protocol was conformed to SPEC §8.1/§8.5:
//! cursor state and pane modes are now struct fields on `PANE_DIFF`, not
//! ops. See `bd show phux-429` for the rationale.

use crate::diff::{
    Cell, CellFlags, Color, CursorShape, CursorState, DiffOp, PaletteIndex, PaneModes, RgbColor,
    Underline,
};
use smallvec::SmallVec;

use super::decode::Decoder;
use super::encode::Encoder;
use super::error::DecodeError;

/// `DiffOp` discriminants, occupying their own 1-byte tag space (independent
/// of the frame-type space in `SPEC.md` §7). `0x03` and `0x04` were retired
/// in phux-429 when cursor state moved to `PANE_DIFF` struct fields per
/// SPEC §8.1/§8.5; they are reserved (decoder rejects them) until the
/// `DiffOp` enum expands per SPEC §8.3 (`Repeat`, `EraseLine`, etc.).
pub(crate) const OP_CELL_RUN: u8 = 0x01;
pub(crate) const OP_CLEAR: u8 = 0x02;

/// `Color` tag discriminants. Per ADR-0008, `Color` is libghostty-vt's
/// `StyleColor`; the tag values are phux-stable bytes on the wire (not
/// dependent on libghostty's repr).
pub(crate) const COLOR_NONE: u8 = 0x00;
pub(crate) const COLOR_PALETTE: u8 = 0x01;
pub(crate) const COLOR_RGB: u8 = 0x02;

/// Encode a slice of [`DiffOp`] with a leading `u32` big-endian count.
pub fn encode_diff_ops(ops: &[DiffOp], enc: &mut Encoder<'_>) {
    // `u32::try_from` for length safety; the protocol caps frames at 16 MiB so
    // any plausible op count fits in `u32`.
    debug_assert!(
        u32::try_from(ops.len()).is_ok(),
        "DiffOp slice length exceeds u32",
    );
    let count = u32::try_from(ops.len()).unwrap_or(u32::MAX);
    enc.write_u32_be(count);
    for op in ops {
        encode_diff_op(op, enc);
    }
}

/// Decode a `u32`-prefixed sequence of [`DiffOp`].
pub fn decode_diff_ops(dec: &mut Decoder<'_>) -> Result<Vec<DiffOp>, DecodeError> {
    let count = dec.read_u32_be()?;
    let count_usize = usize::try_from(count).map_err(|_| DecodeError::LengthOverflow)?;
    // Don't preallocate from untrusted length; the decoder is bounds-checked
    // anyway, but a malicious header could request a huge allocation.
    let mut ops = Vec::new();
    for _ in 0..count_usize {
        ops.push(decode_diff_op(dec)?);
    }
    Ok(ops)
}

fn encode_diff_op(op: &DiffOp, enc: &mut Encoder<'_>) {
    match op {
        DiffOp::CellRun { row, col, cells } => {
            enc.write_u8(OP_CELL_RUN);
            enc.write_u16_be(*row);
            enc.write_u16_be(*col);
            debug_assert!(
                u32::try_from(cells.len()).is_ok(),
                "CellRun cells length exceeds u32",
            );
            let n = u32::try_from(cells.len()).unwrap_or(u32::MAX);
            enc.write_u32_be(n);
            for cell in cells {
                encode_cell(cell, enc);
            }
        }
        DiffOp::Clear { row, col, count } => {
            enc.write_u8(OP_CLEAR);
            enc.write_u16_be(*row);
            enc.write_u16_be(*col);
            enc.write_u16_be(*count);
        }
    }
}

fn decode_diff_op(dec: &mut Decoder<'_>) -> Result<DiffOp, DecodeError> {
    let tag = dec.read_u8()?;
    match tag {
        OP_CELL_RUN => {
            let row = dec.read_u16_be()?;
            let col = dec.read_u16_be()?;
            let n = dec.read_u32_be()?;
            let n_usize = usize::try_from(n).map_err(|_| DecodeError::LengthOverflow)?;
            let mut cells = Vec::new();
            for _ in 0..n_usize {
                cells.push(decode_cell(dec)?);
            }
            Ok(DiffOp::CellRun { row, col, cells })
        }
        OP_CLEAR => {
            let row = dec.read_u16_be()?;
            let col = dec.read_u16_be()?;
            let count = dec.read_u16_be()?;
            Ok(DiffOp::Clear { row, col, count })
        }
        other => Err(DecodeError::UnknownFrameKind {
            tag: u16::from(other),
        }),
    }
}

fn encode_cell(cell: &Cell, enc: &mut Encoder<'_>) {
    encode_text(&cell.text, enc);
    encode_color(cell.fg, enc);
    encode_color(cell.bg, enc);
    encode_underline(cell.underline, enc);
    encode_color(cell.underline_color, enc);
    enc.write_u16_be(cell.flags.bits());
}

fn decode_cell(dec: &mut Decoder<'_>) -> Result<Cell, DecodeError> {
    let text = decode_text(dec)?;
    let fg = decode_color(dec)?;
    let bg = decode_color(dec)?;
    let underline = decode_underline(dec)?;
    let underline_color = decode_color(dec)?;
    let flags_bits = dec.read_u16_be()?;
    let flags = CellFlags::from_bits_truncate(flags_bits);
    Ok(Cell {
        text,
        fg,
        bg,
        underline,
        underline_color,
        flags,
    })
}

fn encode_text(text: &[char], enc: &mut Encoder<'_>) {
    // Per spec: grapheme count fits in a u8 (no single cell carries more than
    // ~255 combining marks). Saturate on overflow; the decoder will round-trip
    // whatever count was actually serialised.
    debug_assert!(u8::try_from(text.len()).is_ok(), "grapheme run exceeds u8");
    let n = u8::try_from(text.len()).unwrap_or(u8::MAX);
    enc.write_u8(n);
    for ch in text.iter().take(usize::from(n)) {
        enc.write_u32_be(*ch as u32);
    }
}

fn decode_text(dec: &mut Decoder<'_>) -> Result<SmallVec<[char; 2]>, DecodeError> {
    let n = dec.read_u8()?;
    // `SmallVec::with_capacity` stays inline when `n <= 2` (the common
    // case) and otherwise spills to the heap exactly like `Vec`. The wire
    // bytes the encoder produces are identical either way.
    let mut out = SmallVec::with_capacity(usize::from(n));
    for _ in 0..n {
        let cp = dec.read_u32_be()?;
        let ch = char::from_u32(cp).ok_or(DecodeError::InvalidUtf8)?;
        out.push(ch);
    }
    Ok(out)
}

fn encode_color(color: Color, enc: &mut Encoder<'_>) {
    match color {
        Color::None => enc.write_u8(COLOR_NONE),
        Color::Palette(idx) => {
            enc.write_u8(COLOR_PALETTE);
            enc.write_u8(idx.0);
        }
        Color::Rgb(rgb) => {
            enc.write_u8(COLOR_RGB);
            enc.write_u8(rgb.r);
            enc.write_u8(rgb.g);
            enc.write_u8(rgb.b);
        }
    }
}

fn decode_color(dec: &mut Decoder<'_>) -> Result<Color, DecodeError> {
    let tag = dec.read_u8()?;
    match tag {
        COLOR_NONE => Ok(Color::None),
        COLOR_PALETTE => Ok(Color::Palette(PaletteIndex(dec.read_u8()?))),
        COLOR_RGB => {
            let r = dec.read_u8()?;
            let g = dec.read_u8()?;
            let b = dec.read_u8()?;
            Ok(Color::Rgb(RgbColor { r, g, b }))
        }
        other => Err(DecodeError::UnknownFrameKind {
            tag: u16::from(other),
        }),
    }
}

fn encode_underline(u: Underline, enc: &mut Encoder<'_>) {
    enc.write_u8(u as u8);
}

fn decode_underline(dec: &mut Decoder<'_>) -> Result<Underline, DecodeError> {
    let tag = dec.read_u8()?;
    match tag {
        0 => Ok(Underline::None),
        1 => Ok(Underline::Single),
        2 => Ok(Underline::Double),
        3 => Ok(Underline::Curly),
        4 => Ok(Underline::Dotted),
        5 => Ok(Underline::Dashed),
        other => Err(DecodeError::UnknownFrameKind {
            tag: u16::from(other),
        }),
    }
}

fn encode_cursor_shape(shape: CursorShape, enc: &mut Encoder<'_>) {
    enc.write_u8(shape as u8);
}

fn decode_cursor_shape(dec: &mut Decoder<'_>) -> Result<CursorShape, DecodeError> {
    let tag = dec.read_u8()?;
    match tag {
        0 => Ok(CursorShape::Block),
        1 => Ok(CursorShape::Bar),
        2 => Ok(CursorShape::Underline),
        3 => Ok(CursorShape::BlockHollow),
        other => Err(DecodeError::UnknownEnumValue {
            field: "CursorShape",
            value: u32::from(other),
        }),
    }
}

fn decode_bool(dec: &mut Decoder<'_>) -> Result<bool, DecodeError> {
    Ok(dec.read_u8()? != 0)
}

/// Encode a [`CursorState`] (SPEC §8.5) for inclusion in a `PANE_DIFF` body.
///
/// Layout (all big-endian): `u16 row | u16 col | u8 visible | u8 shape | u8 blink`.
/// `CursorState` is `Copy` and 8 bytes; pass by value to match clippy's
/// `trivially_copy_pass_by_ref` heuristic.
pub fn encode_cursor_state(state: CursorState, enc: &mut Encoder<'_>) {
    enc.write_u16_be(state.row);
    enc.write_u16_be(state.col);
    enc.write_u8(u8::from(state.visible));
    encode_cursor_shape(state.shape, enc);
    enc.write_u8(u8::from(state.blink));
}

/// Decode a [`CursorState`] written by [`encode_cursor_state`].
pub fn decode_cursor_state(dec: &mut Decoder<'_>) -> Result<CursorState, DecodeError> {
    let row = dec.read_u16_be()?;
    let col = dec.read_u16_be()?;
    let visible = decode_bool(dec)?;
    let shape = decode_cursor_shape(dec)?;
    let blink = decode_bool(dec)?;
    Ok(CursorState {
        row,
        col,
        visible,
        shape,
        blink,
    })
}

/// Encode a [`PaneModes`] bitset (SPEC §8.5) as a raw `u16` big-endian.
///
/// Unknown bits round-trip unchanged so additive minor-version protocol
/// changes remain backward compatible per SPEC §16.
pub fn encode_pane_modes(modes: PaneModes, enc: &mut Encoder<'_>) {
    enc.write_u16_be(modes.bits());
}

/// Decode a [`PaneModes`] bitset written by [`encode_pane_modes`].
pub fn decode_pane_modes(dec: &mut Decoder<'_>) -> Result<PaneModes, DecodeError> {
    Ok(PaneModes::from_bits(dec.read_u16_be()?))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use bytes::BytesMut;
    use proptest::prelude::*;

    fn arb_color() -> impl Strategy<Value = Color> {
        prop_oneof![
            Just(Color::None),
            any::<u8>().prop_map(|n| Color::Palette(PaletteIndex(n))),
            (any::<u8>(), any::<u8>(), any::<u8>()).prop_map(|(r, g, b)| Color::Rgb(RgbColor {
                r,
                g,
                b
            })),
        ]
    }

    fn arb_underline() -> impl Strategy<Value = Underline> {
        prop_oneof![
            Just(Underline::None),
            Just(Underline::Single),
            Just(Underline::Double),
            Just(Underline::Curly),
            Just(Underline::Dotted),
            Just(Underline::Dashed),
        ]
    }

    fn arb_cursor_shape() -> impl Strategy<Value = CursorShape> {
        prop_oneof![
            Just(CursorShape::Block),
            Just(CursorShape::Bar),
            Just(CursorShape::Underline),
            Just(CursorShape::BlockHollow),
        ]
    }

    fn arb_flags() -> impl Strategy<Value = CellFlags> {
        // Restrict to bits actually defined in the bitflags so
        // `from_bits_truncate` round-trips exactly.
        any::<u16>().prop_map(|bits| CellFlags::from_bits_truncate(bits & CellFlags::all().bits()))
    }

    fn arb_text() -> impl Strategy<Value = SmallVec<[char; 2]>> {
        // `char::arbitrary` covers the full Unicode scalar range, exercising
        // `char::from_u32` on the decode side. Bound length to keep tests fast.
        // Range straddles the inline capacity (2) so both inline and spilled
        // representations are exercised.
        proptest::collection::vec(any::<char>(), 0..4)
            .prop_map(|v| v.into_iter().collect::<SmallVec<[char; 2]>>())
    }

    fn arb_cell() -> impl Strategy<Value = Cell> {
        (
            arb_text(),
            arb_color(),
            arb_color(),
            arb_underline(),
            arb_color(),
            arb_flags(),
        )
            .prop_map(|(text, fg, bg, underline, underline_color, flags)| Cell {
                text,
                fg,
                bg,
                underline,
                underline_color,
                flags,
            })
    }

    fn arb_diff_op() -> impl Strategy<Value = DiffOp> {
        prop_oneof![
            (
                any::<u16>(),
                any::<u16>(),
                proptest::collection::vec(arb_cell(), 0..4),
            )
                .prop_map(|(row, col, cells)| DiffOp::CellRun { row, col, cells }),
            (any::<u16>(), any::<u16>(), any::<u16>())
                .prop_map(|(row, col, count)| DiffOp::Clear { row, col, count }),
        ]
    }

    fn arb_cursor_state() -> impl Strategy<Value = CursorState> {
        (
            any::<u16>(),
            any::<u16>(),
            any::<bool>(),
            arb_cursor_shape(),
            any::<bool>(),
        )
            .prop_map(|(row, col, visible, shape, blink)| CursorState {
                row,
                col,
                visible,
                shape,
                blink,
            })
    }

    fn arb_pane_modes() -> impl Strategy<Value = PaneModes> {
        any::<u16>().prop_map(PaneModes::from_bits)
    }

    proptest! {
        #[test]
        fn ops_roundtrip(ops in proptest::collection::vec(arb_diff_op(), 0..20)) {
            let mut buf = BytesMut::new();
            {
                let mut enc = Encoder::new(&mut buf);
                encode_diff_ops(&ops, &mut enc);
            }
            let mut dec = Decoder::new(&buf);
            let decoded = decode_diff_ops(&mut dec).unwrap();
            prop_assert_eq!(ops, decoded);
        }

        #[test]
        fn cell_roundtrip(cell in arb_cell()) {
            let mut buf = BytesMut::new();
            {
                let mut enc = Encoder::new(&mut buf);
                encode_cell(&cell, &mut enc);
            }
            let mut dec = Decoder::new(&buf);
            let decoded = decode_cell(&mut dec).unwrap();
            prop_assert_eq!(cell, decoded);
        }

        #[test]
        fn cursor_state_roundtrip(state in arb_cursor_state()) {
            let mut buf = BytesMut::new();
            {
                let mut enc = Encoder::new(&mut buf);
                encode_cursor_state(state, &mut enc);
            }
            let mut dec = Decoder::new(&buf);
            let decoded = decode_cursor_state(&mut dec).unwrap();
            prop_assert_eq!(state, decoded);
        }

        #[test]
        fn pane_modes_roundtrip(modes in arb_pane_modes()) {
            let mut buf = BytesMut::new();
            {
                let mut enc = Encoder::new(&mut buf);
                encode_pane_modes(modes, &mut enc);
            }
            let mut dec = Decoder::new(&buf);
            let decoded = decode_pane_modes(&mut dec).unwrap();
            prop_assert_eq!(modes, decoded);
        }
    }

    #[test]
    fn cursor_state_invalid_shape_rejected() {
        // Hand-build a CursorState with shape tag 0xFF.
        let bytes = [0u8, 0, 0, 0, 0, 0xFF, 0]; // row=0 col=0 visible=0 shape=0xFF blink=0
        let mut dec = Decoder::new(&bytes);
        let err = decode_cursor_state(&mut dec).unwrap_err();
        assert!(matches!(
            err,
            crate::wire::error::DecodeError::UnknownEnumValue {
                field: "CursorShape",
                value: 0xFF
            }
        ));
    }

    #[test]
    fn pane_modes_unknown_bits_preserved() {
        // Reserved bit (0x4000) round-trips per SPEC §16 ("tolerate unknown
        // trailing fields"). The decoder doesn't reject reserved bits;
        // unknown future flags travel through untouched.
        let modes = PaneModes::from_bits(0x4000);
        let mut buf = BytesMut::new();
        {
            let mut enc = Encoder::new(&mut buf);
            encode_pane_modes(modes, &mut enc);
        }
        let mut dec = Decoder::new(&buf);
        let decoded = decode_pane_modes(&mut dec).unwrap();
        assert_eq!(decoded.bits(), 0x4000);
    }

    #[test]
    fn empty_ops_roundtrip() {
        let ops: Vec<DiffOp> = vec![];
        let mut buf = BytesMut::new();
        {
            let mut enc = Encoder::new(&mut buf);
            encode_diff_ops(&ops, &mut enc);
        }
        // Just a u32 zero count.
        assert_eq!(&buf[..], &[0, 0, 0, 0]);
        let mut dec = Decoder::new(&buf);
        let decoded = decode_diff_ops(&mut dec).unwrap();
        assert!(decoded.is_empty());
    }
}
