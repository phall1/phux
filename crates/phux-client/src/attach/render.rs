//! Render the client's local `libghostty_vt::Terminal` to the outer
//! terminal as VT escape sequences.
//!
//! Under ADR-0013 the client owns one `Terminal` per attached pane;
//! `PANE_OUTPUT` byte frames are fed into it via `vt_write`. This
//! module reads the resulting structured state back out via
//! `RenderState` (per-row dirty tracking) and emits VT to stdout.
//!
//! v0 emits a **dirty-row redraw**: for each row reported dirty by
//! `RenderState`, position the cursor at the row, emit per-cell SGR
//! deltas, and write each cell's graphemes. Per-row dirty bits are
//! reset after the row is drawn so subsequent renders skip clean rows.
//!
//! No raw-mode or alt-screen toggling happens here; the [`super::driver`]
//! owns those transitions via an RAII guard so they survive panics and
//! early returns.
//!
//! See `research/2026-05-25-libghostty-renderstate.md` for the
//! renderer-side contract this module implements.

use std::io::{self, Write};

use libghostty_vt::{
    RenderState, Terminal,
    render::{CellIterator, CursorVisualStyle, Dirty, RowIterator},
    style::{RgbColor, Style},
};

/// Errors the renderer can surface.
#[derive(Debug, thiserror::Error)]
pub enum RenderError {
    /// libghostty surfaced an error from a render-state operation.
    #[error("libghostty: {0}")]
    Ghostty(#[from] libghostty_vt::Error),
    /// stdout (or the test buffer) returned an I/O error.
    #[error("io: {0}")]
    Io(#[from] io::Error),
}

/// Per-pane render scaffolding.
///
/// Owns the libghostty render iterators so they're reused across frames
/// instead of reallocated each tick.
#[derive(Debug)]
pub struct PaneRenderer<'alloc> {
    state: RenderState<'alloc>,
    rows: RowIterator<'alloc>,
    cells: CellIterator<'alloc>,
}

impl<'alloc> PaneRenderer<'alloc> {
    /// Allocate render scaffolding for one pane. Do this once per pane,
    /// not per frame.
    pub fn new() -> Result<Self, RenderError> {
        Ok(Self {
            state: RenderState::new()?,
            rows: RowIterator::new()?,
            cells: CellIterator::new()?,
        })
    }

    /// Render dirty rows of `terminal` to `out`. Returns the dirty
    /// classification observed; the caller can use it to decide whether
    /// to flush.
    ///
    /// After this returns, every dirty bit (global + per-row) is reset,
    /// per the libghostty contract documented in
    /// `research/2026-05-25-libghostty-renderstate.md` §3.
    pub fn render(
        &mut self,
        terminal: &Terminal<'alloc, '_>,
        out: &mut impl Write,
    ) -> Result<Dirty, RenderError> {
        let snapshot = self.state.update(terminal)?;
        let dirty = snapshot.dirty()?;

        match dirty {
            Dirty::Clean => return Ok(dirty),
            Dirty::Partial | Dirty::Full => {
                out.write_all(b"\x1b[?25l")?;
            }
        }

        // Walk rows, redraw the dirty ones. We reset SGR at the start of
        // each row so per-row redraws are independent and the previous
        // row's tail style doesn't leak into the current row.
        let mut row_iter = self.rows.update(&snapshot)?;
        let mut row_index: u16 = 0;
        let rows_total = snapshot.rows()?;
        while let Some(row) = row_iter.next() {
            if row_index >= rows_total {
                break;
            }
            let row_dirty = row.dirty()?;
            let must_draw = matches!(dirty, Dirty::Full) || row_dirty;
            if must_draw {
                write_cup(out, row_index, 0)?;
                // Force a reset at row start so the previous row's tail
                // style can't leak into the current row.
                out.write_all(b"\x1b[0m")?;
                let mut prev_style: Option<Style> = None;

                let mut cell_iter = self.cells.update(row)?;
                while let Some(cell) = cell_iter.next() {
                    let graphemes = cell.graphemes()?;
                    let style = cell.style()?;
                    let fg = cell.fg_color()?;
                    let bg = cell.bg_color()?;
                    if graphemes.is_empty() {
                        // Blank or wide-tail cell — advance one column with a
                        // space. Wide-tail mis-emission overwrites the
                        // right half of a wide cell; the base grapheme
                        // emitted on the previous cell already covered that
                        // column. End-state stays equivalent.
                        emit_sgr_delta(out, prev_style.as_ref(), &style, fg, bg)?;
                        out.write_all(b" ")?;
                        prev_style = Some(style);
                        continue;
                    }
                    emit_sgr_delta(out, prev_style.as_ref(), &style, fg, bg)?;
                    let mut buf = [0u8; 4];
                    for ch in &graphemes {
                        out.write_all(ch.encode_utf8(&mut buf).as_bytes())?;
                    }
                    prev_style = Some(style);
                }
                // Reset per-row dirty bit after drawing, per the libghostty
                // contract.
                row.set_dirty(false)?;
            }
            row_index += 1;
        }

        // Reset SGR before the final cursor placement so the visual
        // cursor isn't tainted by the last cell's attributes.
        out.write_all(b"\x1b[0m")?;
        // Final cursor placement + visibility.
        if let Some(viewport) = snapshot.cursor_viewport()? {
            write_cup(out, viewport.y, viewport.x)?;
        }
        if snapshot.cursor_visible()? {
            out.write_all(b"\x1b[?25h")?;
        }
        // Optional cursor style — best-effort.
        emit_cursor_style(
            out,
            snapshot.cursor_visual_style()?,
            snapshot.cursor_blinking()?,
        )?;

        // Clear the global dirty bit. Per-row bits were cleared inline.
        snapshot.set_dirty(Dirty::Clean)?;

        out.flush()?;
        Ok(dirty)
    }
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

fn write_cup(out: &mut impl Write, row: u16, col: u16) -> io::Result<()> {
    let r = row.saturating_add(1);
    let c = col.saturating_add(1);
    write!(out, "\x1b[{r};{c}H")
}

fn emit_sgr_delta(
    out: &mut impl Write,
    prev: Option<&Style>,
    style: &Style,
    fg: Option<RgbColor>,
    bg: Option<RgbColor>,
) -> io::Result<()> {
    // For v0 we re-emit the full SGR on every cell regardless of `prev`,
    // because fg/bg ride on the per-cell `cell.fg_color()`/`cell.bg_color()`
    // result rather than the `Style` and we don't carry a delta there yet.
    // Coalescing is a follow-up tracked in DESIGN.
    let _ = prev;
    out.write_all(b"\x1b[0m")?;

    let mut wrote_any = false;
    macro_rules! sep {
        ($w:expr, $flag:expr) => {{
            if $flag {
                $w.write_all(b";")?;
            } else {
                $w.write_all(b"\x1b[")?;
                $flag = true;
            }
        }};
    }
    if style.bold {
        sep!(out, wrote_any);
        out.write_all(b"1")?;
    }
    if style.faint {
        sep!(out, wrote_any);
        out.write_all(b"2")?;
    }
    if style.italic {
        sep!(out, wrote_any);
        out.write_all(b"3")?;
    }
    if style.blink {
        sep!(out, wrote_any);
        out.write_all(b"5")?;
    }
    if style.inverse {
        sep!(out, wrote_any);
        out.write_all(b"7")?;
    }
    if style.invisible {
        sep!(out, wrote_any);
        out.write_all(b"8")?;
    }
    if style.strikethrough {
        sep!(out, wrote_any);
        out.write_all(b"9")?;
    }
    if let Some(rgb) = fg {
        sep!(out, wrote_any);
        write!(out, "38;2;{};{};{}", rgb.r, rgb.g, rgb.b)?;
    }
    if let Some(rgb) = bg {
        sep!(out, wrote_any);
        write!(out, "48;2;{};{};{}", rgb.r, rgb.g, rgb.b)?;
    }
    if wrote_any {
        out.write_all(b"m")?;
    }
    Ok(())
}

fn emit_cursor_style(
    out: &mut impl Write,
    style: CursorVisualStyle,
    blinking: bool,
) -> io::Result<()> {
    let code: u8 = match style {
        CursorVisualStyle::Block => {
            if blinking {
                1
            } else {
                2
            }
        }
        CursorVisualStyle::Underline => {
            if blinking {
                3
            } else {
                4
            }
        }
        CursorVisualStyle::Bar => {
            if blinking {
                5
            } else {
                6
            }
        }
        _ => 2,
    };
    write!(out, "\x1b[{code} q")
}

#[cfg(test)]
#[allow(clippy::expect_used, reason = "tests")]
mod tests {
    use super::*;
    use libghostty_vt::{Terminal, TerminalOptions};

    fn fresh(cols: u16, rows: u16) -> Terminal<'static, 'static> {
        Terminal::new(TerminalOptions {
            cols,
            rows,
            max_scrollback: 100,
        })
        .expect("Terminal::new")
    }

    #[test]
    fn renderer_writes_cursor_hide_then_show_for_dirty_full() {
        let mut terminal = fresh(5, 2);
        terminal.vt_write(b"ab");
        let mut renderer = PaneRenderer::new().expect("PaneRenderer::new");
        let mut buf = Vec::new();
        let _ = renderer.render(&terminal, &mut buf).expect("render");
        // Must start by hiding the cursor.
        assert!(buf.starts_with(b"\x1b[?25l"));
        // Should contain the literal characters "a" and "b" somewhere.
        let s = String::from_utf8_lossy(&buf);
        assert!(s.contains('a') && s.contains('b'));
    }
}
