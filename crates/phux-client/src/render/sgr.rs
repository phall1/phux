//! Shared per-cell SGR emission for the ratatui-backed chrome layers.
//!
//! Both the overlay painter ([`super::overlay`]) and the status bar
//! ([`super::chrome::status_bar`]) walk a ratatui [`Buffer`] and emit one
//! cell at a time. This module owns the per-cell SGR delta so the two
//! paths style identically. It lives under `render/` so the ratatui
//! dependency stays within the ADR-0020 boundary.

use std::io::{self, Write};

use ratatui::style::Color;

/// Emit the SGR for one ratatui cell.
///
/// Skips emission entirely when the cell has no styling (default fg/bg,
/// no modifier bits) AND the previous cell also had none, so a long run
/// of unstyled text emits no SGR sequences at all. When *any* styling is
/// present, emits a full reset + the cell's attributes so stale style
/// from a previous cell can't leak. `prev_styled` is threaded by the
/// caller across cells in a row.
pub(super) fn emit_cell_sgr(
    out: &mut impl Write,
    cell: &ratatui::buffer::Cell,
    prev_styled: &mut bool,
) -> io::Result<()> {
    use ratatui::style::Modifier;
    let styled = cell.modifier != Modifier::empty()
        || !matches!(cell.fg, Color::Reset)
        || !matches!(cell.bg, Color::Reset);
    if !styled && !*prev_styled {
        return Ok(());
    }
    out.write_all(b"\x1b[0m")?;
    *prev_styled = styled;
    if !styled {
        return Ok(());
    }
    let mut wrote_any = false;
    let sep = |w: &mut dyn Write, wrote: &mut bool| -> io::Result<()> {
        if *wrote {
            w.write_all(b";")?;
        } else {
            w.write_all(b"\x1b[")?;
            *wrote = true;
        }
        Ok(())
    };
    let m = cell.modifier;
    if m.contains(Modifier::BOLD) {
        sep(out, &mut wrote_any)?;
        out.write_all(b"1")?;
    }
    if m.contains(Modifier::DIM) {
        sep(out, &mut wrote_any)?;
        out.write_all(b"2")?;
    }
    if m.contains(Modifier::ITALIC) {
        sep(out, &mut wrote_any)?;
        out.write_all(b"3")?;
    }
    if m.contains(Modifier::UNDERLINED) {
        sep(out, &mut wrote_any)?;
        out.write_all(b"4")?;
    }
    if m.contains(Modifier::REVERSED) {
        sep(out, &mut wrote_any)?;
        out.write_all(b"7")?;
    }
    if let Some((kind, r, g, b)) = color_rgb(cell.fg, true) {
        sep(out, &mut wrote_any)?;
        write!(out, "{kind};2;{r};{g};{b}")?;
    }
    if let Some((kind, r, g, b)) = color_rgb(cell.bg, false) {
        sep(out, &mut wrote_any)?;
        write!(out, "{kind};2;{r};{g};{b}")?;
    }
    if wrote_any {
        out.write_all(b"m")?;
    }
    Ok(())
}

/// Convert a ratatui [`Color`] into a 24-bit SGR triple, plus the SGR
/// kind prefix (`"38"` foreground / `"48"` background). Returns `None`
/// for `Color::Reset` (no override). Indexed ANSI colors map to a small
/// fixed palette so chrome renders consistently across terminal themes.
const fn color_rgb(color: Color, fg: bool) -> Option<(&'static str, u8, u8, u8)> {
    let kind = if fg { "38" } else { "48" };
    let (r, g, b) = match color {
        Color::Reset => return None,
        Color::Black => (0, 0, 0),
        Color::Red => (170, 0, 0),
        Color::Green => (0, 170, 0),
        Color::Yellow => (170, 85, 0),
        Color::Blue => (0, 0, 170),
        Color::Magenta => (170, 0, 170),
        Color::Cyan => (0, 170, 170),
        Color::Gray => (170, 170, 170),
        Color::DarkGray => (85, 85, 85),
        Color::LightRed => (255, 85, 85),
        Color::LightGreen => (85, 255, 85),
        Color::LightYellow => (255, 255, 85),
        Color::LightBlue => (85, 85, 255),
        Color::LightMagenta => (255, 85, 255),
        Color::LightCyan => (85, 255, 255),
        Color::White => (255, 255, 255),
        Color::Rgb(r, g, b) => (r, g, b),
        Color::Indexed(_) => (200, 200, 200),
    };
    Some((kind, r, g, b))
}
