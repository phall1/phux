//! Per-client color downsampling helper for the fanout layer.
//!
//! Per SPEC §6.2, the server MUST adapt outbound messages to each
//! client's [`ColorSupport`]. The diff is captured once per pane (in
//! canonical `TrueColor`) and downsampled per client at fanout.
//!
//! `phux-byc.5` will build the fanout layer that calls into this. For
//! now, this module owns the transformation logic and its tests so
//! that landing the fanout layer becomes a one-line plumbing change.
//!
//! ## API shape note
//!
//! [`downsample_op`] and [`downsample_cell`] take `&T` and return an
//! owned `T`. The `TrueColor` fast path therefore unnecessarily clones
//! its input; callers should skip the call entirely when the cap is
//! `TrueColor`. We keep the cloned-return signature for now because
//! (a) it's simpler than `Cow`, (b) the fanout layer doesn't exist yet
//! so there's nothing to benchmark, and (c) the API is stable —
//! switching to `Cow<'_, T>` later is non-breaking for the only
//! caller (byc.5). Follow-up: `bd` ticket "`phux-server::downsample`:
//! switch to Cow if benchmarks justify".
//!
//! ## What's downsampled
//!
//! Every `Color` reachable from a [`DiffOp`] (`Cell::{fg, bg,
//! underline_color}`). [`Cell::flags`], cursor metadata, and structural
//! fields (`row`, `col`, `count`) pass through untouched — only color
//! channels carry tier-sensitive information.

use phux_protocol::diff::{Cell, ColorDownsample, ColorSupport, DiffOp};

/// Downsample every [`Color`](phux_protocol::diff::Color) reachable from
/// `op` to fit `support`.
///
/// Returns a new [`DiffOp`]; the input is borrowed and not mutated.
/// The `TrueColor` arm is identity (`op.clone()`); callers on the hot
/// path should branch on `support == ColorSupport::TrueColor` before
/// calling this and skip the clone.
///
/// See the module doc for the API-shape rationale and follow-up.
#[must_use]
pub fn downsample_op(op: &DiffOp, support: ColorSupport) -> DiffOp {
    if matches!(support, ColorSupport::TrueColor) {
        return op.clone();
    }
    match op {
        DiffOp::CellRun { row, col, cells } => DiffOp::CellRun {
            row: *row,
            col: *col,
            cells: cells.iter().map(|c| downsample_cell(c, support)).collect(),
        },
        // Variants without embedded Color values are tier-agnostic.
        DiffOp::Clear { .. } | DiffOp::CursorMove { .. } | DiffOp::CursorStyle { .. } => op.clone(),
    }
}

/// Downsample every color reachable from `cell` to fit `support`.
///
/// Fast-path identity for [`ColorSupport::TrueColor`]: returns `cell.clone()`.
#[must_use]
pub fn downsample_cell(cell: &Cell, support: ColorSupport) -> Cell {
    if matches!(support, ColorSupport::TrueColor) {
        return cell.clone();
    }
    Cell {
        text: cell.text.clone(),
        fg: cell.fg.downsample(support),
        bg: cell.bg.downsample(support),
        underline: cell.underline,
        underline_color: cell.underline_color.downsample(support),
        flags: cell.flags,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use phux_protocol::diff::{CellFlags, Color, CursorShape, PaletteIndex, RgbColor, Underline};

    fn rgb(r: u8, g: u8, b: u8) -> Color {
        Color::Rgb(RgbColor { r, g, b })
    }

    fn sample_rgb_cell() -> Cell {
        // Build via Default so we don't pull smallvec into phux-server's
        // dependency set just for the test fixture.
        let mut c = Cell::default();
        c.text.push('x');
        c.fg = rgb(255, 0, 0);
        c.bg = rgb(0, 0, 255);
        c.underline = Underline::Single;
        c.underline_color = rgb(128, 128, 128);
        c.flags = CellFlags::BOLD;
        c
    }

    #[test]
    fn downsample_cell_truecolor_is_identity() {
        let c = sample_rgb_cell();
        assert_eq!(downsample_cell(&c, ColorSupport::TrueColor), c);
    }

    #[test]
    fn downsample_cell_indexed256_maps_known_rgbs() {
        let c = sample_rgb_cell();
        let out = downsample_cell(&c, ColorSupport::Indexed256);
        // Pure red -> 196, pure blue -> 21, mid-gray -> 244 (see
        // cell.rs unit tests for the derivation).
        assert_eq!(out.fg, Color::Palette(PaletteIndex(196)));
        assert_eq!(out.bg, Color::Palette(PaletteIndex(21)));
        assert_eq!(out.underline_color, Color::Palette(PaletteIndex(244)));
        // Non-color fields preserved.
        assert_eq!(out.text, c.text);
        assert_eq!(out.underline, Underline::Single);
        assert_eq!(out.flags, CellFlags::BOLD);
    }

    #[test]
    fn downsample_cell_indexed16_maps_known_rgbs() {
        let c = sample_rgb_cell();
        let out = downsample_cell(&c, ColorSupport::Indexed16);
        // Pure red -> bright red (9). Pure blue -> bright blue (12).
        // Mid-gray (128) is closer to xterm-16 bright-black (8 =
        // [128,128,128]) than to anything else.
        assert_eq!(out.fg, Color::Palette(PaletteIndex(9)));
        assert_eq!(out.bg, Color::Palette(PaletteIndex(12)));
        assert_eq!(out.underline_color, Color::Palette(PaletteIndex(8)));
    }

    #[test]
    fn downsample_op_cellrun_indexed256() {
        let op = DiffOp::CellRun {
            row: 3,
            col: 7,
            cells: vec![sample_rgb_cell(), Cell::blank()],
        };
        let out = downsample_op(&op, ColorSupport::Indexed256);
        match out {
            DiffOp::CellRun { row, col, cells } => {
                assert_eq!(row, 3);
                assert_eq!(col, 7);
                assert_eq!(cells.len(), 2);
                assert_eq!(cells[0].fg, Color::Palette(PaletteIndex(196)));
                // Blank cell stays blank.
                assert_eq!(cells[1], Cell::blank());
            }
            other => panic!("expected CellRun, got {other:?}"),
        }
    }

    #[test]
    fn downsample_op_truecolor_is_identity() {
        let op = DiffOp::CellRun {
            row: 1,
            col: 2,
            cells: vec![sample_rgb_cell()],
        };
        assert_eq!(downsample_op(&op, ColorSupport::TrueColor), op);
    }

    #[test]
    fn downsample_op_clear_passes_through() {
        let op = DiffOp::Clear {
            row: 5,
            col: 10,
            count: 20,
        };
        // Clear carries no Color; every tier is identity (modulo clone).
        for support in [
            ColorSupport::TrueColor,
            ColorSupport::Indexed256,
            ColorSupport::Indexed16,
        ] {
            assert_eq!(downsample_op(&op, support), op);
        }
    }

    #[test]
    fn downsample_op_cursor_variants_pass_through() {
        // Cursor ops carry no Color today. (The byc.5-adjacent #2 agent
        // is reshaping these into PaneDiff fields; this test only
        // asserts the current DiffOp::CursorMove / CursorStyle shape
        // is tier-agnostic. When cursor-as-field lands, the cursor
        // variants in DiffOp will be deleted entirely and this test
        // will be deleted with them — that's a pure subtraction.)
        let mv = DiffOp::CursorMove { row: 4, col: 9 };
        let style = DiffOp::CursorStyle {
            visible: true,
            shape: CursorShape::Bar,
            blink: false,
        };
        for support in [
            ColorSupport::TrueColor,
            ColorSupport::Indexed256,
            ColorSupport::Indexed16,
        ] {
            assert_eq!(downsample_op(&mv, support), mv);
            assert_eq!(downsample_op(&style, support), style);
        }
    }
}
