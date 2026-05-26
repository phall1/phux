//! Render a [`DiffMirror`] grid to a byte stream as VT escape sequences.
//!
//! v0 emits a **full-frame redraw** on every call: cursor home, then a
//! row-major walk that writes one SGR sequence per cell plus the cell's
//! grapheme text, then a final cursor-position write to mirror
//! `state.cursor`. This is intentionally simple — the renderer is the
//! easiest place to optimise later (dirty-rect tracking, run coalescing,
//! double-buffered diff) once the loop is end-to-end working.
//!
//! No raw-mode or alt-screen toggling happens here; the [`super::driver`]
//! owns those transitions via an RAII guard so they survive panics and
//! early returns.

use std::io::{self, Write};

use phux_protocol::diff::{Cell, CellFlags, Color, CursorState, PaletteIndex, RgbColor, Underline};

use crate::DiffMirror;

/// Render `mirror` to `out` as a sequence of VT escapes.
///
/// The output starts by hiding the cursor (to suppress the flicker that
/// would otherwise occur on each cell write), resets SGR state, jumps to
/// the home position, writes the grid, and finally positions and reveals
/// the cursor according to [`DiffMirror::cursor`].
///
/// # Errors
///
/// Propagates any `io::Error` from `out`.
pub fn render_frame(mirror: &DiffMirror, out: &mut impl Write) -> io::Result<()> {
    // Hide cursor + reset SGR + home.
    out.write_all(b"\x1b[?25l")?;
    out.write_all(b"\x1b[0m")?;
    out.write_all(b"\x1b[H")?;

    let mut current = SgrState::default();
    for (row_idx, row) in mirror.grid.cells.iter().enumerate() {
        // Re-position at the start of each row to handle short rows or
        // empty grids cleanly. Cursor is 1-based on the wire.
        write_cursor_position(out, u16::try_from(row_idx).unwrap_or(u16::MAX), 0)?;
        for cell in row {
            apply_sgr_delta(out, &mut current, cell)?;
            write_cell_text(out, cell)?;
        }
    }

    // Restore SGR to default before the final cursor placement so the
    // cursor's visual style isn't tainted by the last cell's attributes.
    out.write_all(b"\x1b[0m")?;
    write_cursor_position(out, mirror.cursor.row, mirror.cursor.col)?;
    if mirror.cursor.visible {
        out.write_all(b"\x1b[?25h")?;
    }
    out.flush()
}

/// 1-based CUP (`CSI row;col H`). `row`/`col` are zero-based here.
fn write_cursor_position(out: &mut impl Write, row: u16, col: u16) -> io::Result<()> {
    let r = row.saturating_add(1);
    let c = col.saturating_add(1);
    write!(out, "\x1b[{r};{c}H")
}

/// Materialise a cell's grapheme cluster, defaulting to a space if empty so
/// the cursor advances by one column and the row width stays correct.
fn write_cell_text(out: &mut impl Write, cell: &Cell) -> io::Result<()> {
    if cell.text.is_empty() {
        out.write_all(b" ")
    } else {
        let mut buf = [0u8; 4];
        for ch in &cell.text {
            out.write_all(ch.encode_utf8(&mut buf).as_bytes())?;
        }
        Ok(())
    }
}

/// Tracks the SGR state we've emitted so far so we only resend deltas.
///
/// All fields are intentionally "the protocol's view" — i.e. the same
/// types `Cell` exposes — so equality is exact and we don't reify a parser
/// over our own escape sequences. The `Default` impl matches the SGR-reset
/// state (no colors, no underline, no flags).
#[derive(Debug, Clone)]
struct SgrState {
    fg: Color,
    bg: Color,
    underline: Underline,
    underline_color: Color,
    flags: CellFlags,
}

impl Default for SgrState {
    fn default() -> Self {
        // libghostty's `Color` (== `StyleColor`) and `Underline` don't
        // derive `Default` upstream, but their `None` variant is the
        // obvious zero. Mirrors the `Default` impl on `Cell`.
        Self {
            fg: Color::None,
            bg: Color::None,
            underline: Underline::None,
            underline_color: Color::None,
            flags: CellFlags::empty(),
        }
    }
}

impl SgrState {
    /// Did we ever write any SGR for this state? `Default::default()` is
    /// "all `None`/empty", which corresponds to a default SGR-reset state.
    const fn is_default(&self) -> bool {
        matches!(self.fg, Color::None)
            && matches!(self.bg, Color::None)
            && matches!(self.underline, Underline::None)
            && matches!(self.underline_color, Color::None)
            && self.flags.is_empty()
    }
}

fn apply_sgr_delta(out: &mut impl Write, current: &mut SgrState, cell: &Cell) -> io::Result<()> {
    let target_default = matches!(cell.fg, Color::None)
        && matches!(cell.bg, Color::None)
        && matches!(cell.underline, Underline::None)
        && matches!(cell.underline_color, Color::None)
        && cell.flags.is_empty();
    if target_default {
        if !current.is_default() {
            out.write_all(b"\x1b[0m")?;
            *current = SgrState::default();
        }
        return Ok(());
    }

    // For v0 simplicity, reset and re-apply the full SGR for any cell whose
    // attributes differ. SGR run-coalescing is the obvious follow-up.
    if !sgr_eq(current, cell) {
        out.write_all(b"\x1b[0m")?;
        write_full_sgr(out, cell)?;
        current.fg = cell.fg;
        current.bg = cell.bg;
        current.underline = cell.underline;
        current.underline_color = cell.underline_color;
        current.flags = cell.flags;
    }
    Ok(())
}

fn sgr_eq(state: &SgrState, cell: &Cell) -> bool {
    state.fg == cell.fg
        && state.bg == cell.bg
        && state.underline == cell.underline
        && state.underline_color == cell.underline_color
        && state.flags == cell.flags
}

/// Write a full SGR sequence representing `cell`'s style. Always begins
/// after a `\x1b[0m` reset, so it starts the parameter list from scratch.
fn write_full_sgr(out: &mut impl Write, cell: &Cell) -> io::Result<()> {
    out.write_all(b"\x1b[")?;
    let mut wrote = false;

    macro_rules! sep {
        () => {
            if wrote {
                out.write_all(b";")?;
            } else {
                wrote = true;
            }
        };
    }

    if cell.flags.contains(CellFlags::BOLD) {
        sep!();
        out.write_all(b"1")?;
    }
    if cell.flags.contains(CellFlags::FAINT) {
        sep!();
        out.write_all(b"2")?;
    }
    if cell.flags.contains(CellFlags::ITALIC) {
        sep!();
        out.write_all(b"3")?;
    }
    match cell.underline {
        Underline::None => {}
        Underline::Double => {
            sep!();
            out.write_all(b"21")?;
        }
        // libghostty-vt's `Underline` is `#[non_exhaustive]`. We map
        // `Single` plus all unmodeled variants (Curly/Dotted/Dashed and
        // any future additions) to plain `SGR 4` — kitty's `4:N`
        // parameter form is the obvious follow-up if those styles
        // matter to a downstream renderer.
        _ => {
            sep!();
            out.write_all(b"4")?;
        }
    }
    if cell.flags.contains(CellFlags::REVERSE) {
        sep!();
        out.write_all(b"7")?;
    }
    if cell.flags.contains(CellFlags::INVISIBLE) {
        sep!();
        out.write_all(b"8")?;
    }
    if cell.flags.contains(CellFlags::STRIKETHROUGH) {
        sep!();
        out.write_all(b"9")?;
    }

    match cell.fg {
        Color::None => {}
        Color::Palette(idx) => {
            sep!();
            write_palette_fg(out, idx)?;
        }
        Color::Rgb(rgb) => {
            sep!();
            write_rgb_fg(out, rgb)?;
        }
    }
    match cell.bg {
        Color::None => {}
        Color::Palette(idx) => {
            sep!();
            write_palette_bg(out, idx)?;
        }
        Color::Rgb(rgb) => {
            sep!();
            write_rgb_bg(out, rgb)?;
        }
    }

    if !wrote {
        // No attributes to write — emit `0` (reset) so we leave a well-
        // formed CSI sequence.
        out.write_all(b"0")?;
    }
    out.write_all(b"m")
}

fn write_palette_fg(out: &mut impl Write, idx: PaletteIndex) -> io::Result<()> {
    write!(out, "38;5;{}", idx.0)
}

fn write_palette_bg(out: &mut impl Write, idx: PaletteIndex) -> io::Result<()> {
    write!(out, "48;5;{}", idx.0)
}

fn write_rgb_fg(out: &mut impl Write, rgb: RgbColor) -> io::Result<()> {
    write!(out, "38;2;{};{};{}", rgb.r, rgb.g, rgb.b)
}

fn write_rgb_bg(out: &mut impl Write, rgb: RgbColor) -> io::Result<()> {
    write!(out, "48;2;{};{};{}", rgb.r, rgb.g, rgb.b)
}

/// Convenience for callers that just want the cursor reset.
///
/// Used by [`super::driver::RawModeGuard`]'s `Drop` to ensure the outer
/// terminal isn't left with our hidden cursor or random SGR state. Kept
/// fallible because the underlying `Write` might be a closed stdout.
pub fn write_reset(out: &mut impl Write) -> io::Result<()> {
    out.write_all(b"\x1b[0m")?;
    out.write_all(b"\x1b[?25h")?;
    out.flush()
}

/// Position the cursor at `cursor`. Public for tests and for future use by
/// the predictive-echo layer (phux-9gw.1) which needs cursor placement
/// without a full redraw.
pub fn position_cursor(out: &mut impl Write, cursor: CursorState) -> io::Result<()> {
    write_cursor_position(out, cursor.row, cursor.col)
}

#[cfg(test)]
#[allow(clippy::expect_used, reason = "tests")]
mod tests {
    use super::*;
    use phux_protocol::{CursorState, DiffOp};
    use smallvec::smallvec;

    fn cell_with_text(s: &str) -> Cell {
        Cell {
            text: s.chars().collect(),
            ..Cell::blank()
        }
    }

    #[test]
    fn render_writes_cursor_home_first() {
        let mirror = DiffMirror::new(1, 4);
        let mut buf = Vec::new();
        render_frame(&mirror, &mut buf).expect("render");
        // Must hide the cursor before the first cell write so the user
        // never sees the cursor walk across the grid.
        assert!(buf.starts_with(b"\x1b[?25l"));
        // Must contain a home CUP `\x1b[H` after the reset.
        assert!(buf.windows(3).any(|w| w == b"\x1b[H"));
    }

    #[test]
    fn render_emits_one_cell_per_grid_position() {
        let mut mirror = DiffMirror::new(1, 3);
        mirror.apply(&[DiffOp::CellRun {
            row: 0,
            col: 0,
            cells: vec![
                cell_with_text("a"),
                cell_with_text("b"),
                cell_with_text("c"),
            ],
        }]);
        let mut buf = Vec::new();
        render_frame(&mirror, &mut buf).expect("render");
        // The output must contain the literal cell text in order.
        let s = String::from_utf8_lossy(&buf);
        let ai = s.find('a').expect("a present");
        let bi = s.find('b').expect("b present");
        let ci = s.find('c').expect("c present");
        assert!(ai < bi);
        assert!(bi < ci);
    }

    #[test]
    fn render_positions_visible_cursor_at_the_end() {
        let mut mirror = DiffMirror::new(5, 5);
        mirror.cursor = CursorState {
            row: 2,
            col: 3,
            visible: true,
            ..CursorState::default()
        };
        let mut buf = Vec::new();
        render_frame(&mirror, &mut buf).expect("render");
        // Final cursor placement uses 1-based CUP.
        let s = String::from_utf8_lossy(&buf);
        assert!(s.contains("\x1b[3;4H"), "missing final CUP in: {s:?}");
        // Cursor should be re-shown at the end since `visible == true`.
        assert!(buf.ends_with(b"\x1b[?25h"));
    }

    #[test]
    fn render_keeps_cursor_hidden_when_invisible() {
        let mut mirror = DiffMirror::new(1, 1);
        mirror.cursor = CursorState {
            visible: false,
            ..CursorState::default()
        };
        let mut buf = Vec::new();
        render_frame(&mirror, &mut buf).expect("render");
        // `?25h` must not appear when the protocol says cursor is hidden.
        let s = String::from_utf8_lossy(&buf);
        assert!(!s.contains("\x1b[?25h"));
    }

    #[test]
    fn smallvec_imported_is_used() {
        // Touch the import so clippy doesn't complain in the test cfg.
        let v: smallvec::SmallVec<[char; 2]> = smallvec!['x'];
        assert_eq!(v[0], 'x');
    }
}
