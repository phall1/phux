//! Synthesize a `PANE_SNAPSHOT` `vt_replay_bytes` blob from a
//! `libghostty_vt::Terminal`.
//!
//! Under [ADR-0013] the wire carries VT bytes, not structured grids.
//! When a client attaches, the server owes it a `PANE_SNAPSHOT`
//! (SPEC §8.4) whose body is a self-contained VT byte sequence that —
//! when `vt_write`-en into a fresh `Terminal` of the matching `cols × rows`
//! — reproduces the current grid. This module owns that synthesis.
//!
//! The walk mirrors `research/2026-05-25-libghostty-renderstate.md` §7:
//!
//! 1. Reset (`DECSTR + ED 2 + CUP home`).
//! 2. For each visible row, emit SGR deltas as cell styles change and
//!    write the row's graphemes. Wide-cell tails (empty grapheme on a
//!    `at_wide_tail` cell) are skipped — the base grapheme advanced the
//!    cursor across both cells. Wrapped rows omit the trailing CRLF so
//!    libghostty's parser preserves the soft wrap.
//! 3. Re-establish cursor position (`CUP`).
//! 4. Re-establish cursor visibility (`DECSET 25` / `DECRST 25`) and
//!    visual style (`DECSCUSR`).
//! 5. Re-establish a small set of mode bits queried from the canonical
//!    `Terminal` via [`libghostty_vt::Terminal::mode`].
//!
//! Out-of-band registries (OSC 8 hyperlinks, kitty graphics, etc.) are
//! deferred — they need their own re-emission strategy and don't appear
//! in `RenderState` directly.
//!
//! [ADR-0013]: https://github.com/phall1/phux/blob/main/ADR/0013-libghostty-bytes-on-wire.md

use std::io::Write as _;

use libghostty_vt::{
    RenderState, Terminal,
    render::{CellIterator, CursorVisualStyle, RowIterator},
    screen::CellWide,
    style::{RgbColor, Style},
    terminal::Mode,
};

/// Errors that can occur while synthesising a snapshot.
#[derive(Debug, thiserror::Error)]
pub enum SynthesisError {
    /// Surfaced from libghostty-vt.
    #[error("libghostty: {0}")]
    Ghostty(#[from] libghostty_vt::Error),
    /// A `write!` into the snapshot buffer failed (the buffer is a
    /// `Vec<u8>`, so this is structurally unreachable; we keep the
    /// variant to satisfy the error-propagation contract).
    #[error("snapshot buffer write failed")]
    Buffer,
}

/// Pooled per-pane snapshot scaffolding.
///
/// Owns the libghostty render iterators ([`RenderState`], [`RowIterator`],
/// [`CellIterator`]) so the synthesis path reuses them across attaches
/// instead of reallocating each time. The free [`synthesize`] function is
/// the one-shot wrapper.
#[derive(Debug)]
pub struct SnapshotSynthesizer<'alloc> {
    render_state: RenderState<'alloc>,
    rows: RowIterator<'alloc>,
    cells: CellIterator<'alloc>,
}

impl<'alloc> SnapshotSynthesizer<'alloc> {
    /// Allocate a fresh pool of render iterators. Do this once per pane.
    pub fn new() -> Result<Self, SynthesisError> {
        Ok(Self {
            render_state: RenderState::new()?,
            rows: RowIterator::new()?,
            cells: CellIterator::new()?,
        })
    }

    /// Walk `terminal`'s viewport and emit a VT byte sequence that
    /// reproduces it on a fresh Terminal.
    ///
    /// Returns the synthesised bytes plus the queried `(cols, rows)`
    /// dimensions, since `PANE_SNAPSHOT` carries them alongside the
    /// replay body (SPEC §8.4).
    pub fn synthesize(
        &mut self,
        terminal: &Terminal<'alloc, '_>,
    ) -> Result<SnapshotBytes, SynthesisError> {
        let snapshot = self.render_state.update(terminal)?;
        let cols = snapshot.cols()?;
        let rows_n = snapshot.rows()?;

        let mut out: Vec<u8> = Vec::with_capacity(usize::from(cols) * usize::from(rows_n) * 2);

        // 1. Reset target: DECSTR (soft reset) + ED 2 (clear screen) + CUP home.
        out.extend_from_slice(b"\x1b[!p\x1b[2J\x1b[H");

        // 2. Walk rows + cells, emitting SGR deltas and graphemes.
        let mut prev_style: Option<Style> = None;
        let mut row_iter = self.rows.update(&snapshot)?;
        let mut row_index: u16 = 0;
        while let Some(row) = row_iter.next() {
            if row_index >= rows_n {
                break;
            }
            // Position to the start of the row. CUP is 1-based.
            write_cup(&mut out, row_index, 0);
            let mut cell_iter = self.cells.update(row)?;
            while let Some(cell) = cell_iter.next() {
                // Discriminate wide-cell tails (the right half of a
                // double-width glyph) from genuinely-blank cells. The base
                // grapheme on the wide cell already advanced the cursor
                // across both columns, so the tail must NOT emit a space
                // (which would clobber the right half of the wide glyph).
                // See libghostty's `CellWide`: `SpacerTail` is documented
                // as "do not render".
                let wide = cell.raw_cell()?.wide()?;
                if matches!(wide, CellWide::SpacerTail) {
                    continue;
                }

                let graphemes = cell.graphemes()?;
                if graphemes.is_empty() {
                    // Genuinely blank cell — emit a space so the column
                    // advances. (Wide-tail case was handled above.)
                    out.push(b' ');
                    continue;
                }

                let style = cell.style()?;
                let fg = cell.fg_color()?;
                let bg = cell.bg_color()?;
                emit_sgr_delta(&mut out, prev_style.as_ref(), &style, fg, bg);
                prev_style = Some(style);

                for ch in &graphemes {
                    let mut buf = [0u8; 4];
                    out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
                }
            }
            row_index += 1;
        }

        // 3. Reset SGR before cursor placement so the cursor's visual
        // style isn't tainted by the last cell's attributes.
        out.extend_from_slice(b"\x1b[0m");

        // 4. Cursor position.
        if let Some(viewport) = snapshot.cursor_viewport()? {
            write_cup(&mut out, viewport.y, viewport.x);
        } else {
            // No viewport-resident cursor; leave at home.
            out.extend_from_slice(b"\x1b[H");
        }

        // 5. Cursor visibility + visual style.
        if snapshot.cursor_visible()? {
            out.extend_from_slice(b"\x1b[?25h");
        } else {
            out.extend_from_slice(b"\x1b[?25l");
        }
        emit_cursor_style(
            &mut out,
            snapshot.cursor_visual_style()?,
            snapshot.cursor_blinking()?,
        );

        // 6. A small set of mode bits queried from the canonical Terminal.
        // ALT_SCREEN is the load-bearing one — a snapshot taken while the
        // alt screen is active must put the receiving Terminal back into
        // alt-screen mode so subsequent live bytes apply to the right
        // surface. Bracketed paste and a handful of mouse modes are nice
        // for fidelity. More modes can land here as needed.
        emit_mode(&mut out, terminal, Mode::BRACKETED_PASTE, b"2004")?;
        emit_mode(&mut out, terminal, Mode::FOCUS_EVENT, b"1004")?;
        // Both legacy and modern alt-screen toggles map to libghostty's
        // ALT_SCREEN_LEGACY (47) and the standard pair lives at 1049.
        emit_mode(&mut out, terminal, Mode::ALT_SCREEN_LEGACY, b"47")?;

        Ok(SnapshotBytes {
            cols,
            rows: rows_n,
            bytes: out,
        })
    }
}

/// Convenience wrapper: allocate a fresh [`SnapshotSynthesizer`] for a
/// one-shot synthesis. Per-pane hot loops should reuse a
/// [`SnapshotSynthesizer`].
pub fn synthesize(terminal: &Terminal<'_, '_>) -> Result<SnapshotBytes, SynthesisError> {
    SnapshotSynthesizer::new()?.synthesize(terminal)
}

/// Result of one snapshot synthesis: the dimensions and the VT byte body.
#[derive(Debug, Clone)]
pub struct SnapshotBytes {
    /// Grid width in cells at the moment of synthesis.
    pub cols: u16,
    /// Grid height in cells at the moment of synthesis.
    pub rows: u16,
    /// VT byte sequence; opaque, mosh-style, fed to the client's `Terminal`.
    pub bytes: Vec<u8>,
}

/// 1-based CUP (`CSI <r+1>;<c+1> H`). Inputs are zero-based.
fn write_cup(out: &mut Vec<u8>, row: u16, col: u16) {
    let r = row.saturating_add(1);
    let c = col.saturating_add(1);
    let _ = write!(out, "\x1b[{r};{c}H");
}

/// Emit SGR parameters representing `style` + colors, prefixed by a
/// reset so the parameter list is independent of `prev`. Skip emission
/// entirely if nothing changed.
fn emit_sgr_delta(
    out: &mut Vec<u8>,
    prev: Option<&Style>,
    style: &Style,
    fg: Option<RgbColor>,
    bg: Option<RgbColor>,
) {
    let same = prev.is_some_and(|p| styles_equal(p, style));
    let touched = !same || prev.is_none();
    if !touched {
        return;
    }
    // Always reset first — keeps the parameter list independent of state.
    out.extend_from_slice(b"\x1b[0m");

    let mut wrote_any = false;
    let sep = |out: &mut Vec<u8>, wrote: &mut bool| {
        if *wrote {
            out.push(b';');
        } else {
            out.extend_from_slice(b"\x1b[");
            *wrote = true;
        }
    };
    if style.bold {
        sep(out, &mut wrote_any);
        out.push(b'1');
    }
    if style.faint {
        sep(out, &mut wrote_any);
        out.push(b'2');
    }
    if style.italic {
        sep(out, &mut wrote_any);
        out.push(b'3');
    }
    if style.blink {
        sep(out, &mut wrote_any);
        out.push(b'5');
    }
    if style.inverse {
        sep(out, &mut wrote_any);
        out.push(b'7');
    }
    if style.invisible {
        sep(out, &mut wrote_any);
        out.push(b'8');
    }
    if style.strikethrough {
        sep(out, &mut wrote_any);
        out.push(b'9');
    }
    if let Some(rgb) = fg {
        sep(out, &mut wrote_any);
        let _ = write!(out, "38;2;{};{};{}", rgb.r, rgb.g, rgb.b);
    }
    if let Some(rgb) = bg {
        sep(out, &mut wrote_any);
        let _ = write!(out, "48;2;{};{};{}", rgb.r, rgb.g, rgb.b);
    }
    if wrote_any {
        out.push(b'm');
    } else {
        // Already reset above; nothing else to emit. The reset is the
        // SGR. No-op past the `\x1b[0m` we already wrote.
    }
}

const fn styles_equal(a: &Style, b: &Style) -> bool {
    a.bold == b.bold
        && a.faint == b.faint
        && a.italic == b.italic
        && a.blink == b.blink
        && a.inverse == b.inverse
        && a.invisible == b.invisible
        && a.strikethrough == b.strikethrough
        && a.overline == b.overline
}

fn emit_cursor_style(out: &mut Vec<u8>, style: CursorVisualStyle, blinking: bool) {
    // DECSCUSR: `CSI <n> SP q`. Block/blink=1, Block/steady=2,
    // Underline/blink=3, steady=4, Bar/blink=5, steady=6. BlockHollow has
    // no DECSCUSR encoding; map to Block-steady.
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
        // Hollow block and any future variant — treat as steady block.
        _ => 2,
    };
    let _ = write!(out, "\x1b[{code} q");
}

/// Query `mode` on `terminal`; emit `CSI ? <code> h/l` accordingly.
fn emit_mode(
    out: &mut Vec<u8>,
    terminal: &Terminal<'_, '_>,
    mode: Mode,
    code: &[u8],
) -> Result<(), SynthesisError> {
    let on = terminal.mode(mode)?;
    out.extend_from_slice(b"\x1b[?");
    out.extend_from_slice(code);
    out.push(if on { b'h' } else { b'l' });
    Ok(())
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
    fn synthesizer_returns_dimensions() {
        let terminal = fresh(80, 24);
        let snap = synthesize(&terminal).expect("synth");
        assert_eq!(snap.cols, 80);
        assert_eq!(snap.rows, 24);
        // First bytes should be the reset prelude.
        assert!(snap.bytes.starts_with(b"\x1b[!p\x1b[2J\x1b[H"));
    }

    /// Walk the viewport of `t` and collect each row as a string,
    /// reproducing wide-cell tail handling so the comparison is grid-
    /// equivalent rather than byte-equivalent.
    fn render_grid(t: &Terminal<'_, '_>) -> Vec<String> {
        let mut rs = RenderState::new().expect("RenderState::new");
        let snap = rs.update(t).expect("update");
        let rows_n = snap.rows().expect("rows");
        let mut row_iter_storage = RowIterator::new().expect("RowIterator::new");
        let mut cell_iter_storage = CellIterator::new().expect("CellIterator::new");
        let mut row_iter = row_iter_storage.update(&snap).expect("row update");
        let mut grid: Vec<String> = Vec::with_capacity(usize::from(rows_n));
        let mut i: u16 = 0;
        while let Some(row) = row_iter.next() {
            if i >= rows_n {
                break;
            }
            let mut line = String::new();
            let mut cell_iter = cell_iter_storage.update(row).expect("cell update");
            while let Some(cell) = cell_iter.next() {
                let wide = cell.raw_cell().expect("raw_cell").wide().expect("wide");
                if matches!(wide, CellWide::SpacerTail) {
                    continue;
                }
                let graphemes = cell.graphemes().expect("graphemes");
                if graphemes.is_empty() {
                    line.push(' ');
                } else {
                    for ch in &graphemes {
                        line.push(*ch);
                    }
                }
            }
            grid.push(line);
            i += 1;
        }
        grid
    }

    #[test]
    fn synthesizer_round_trips_via_libghostty() {
        // Feed bytes into a Terminal, synthesise a snapshot, feed the
        // snapshot into a fresh Terminal — the cursor position must match.
        // We assert cursor position rather than full grid equality because
        // the byte synthesis is best-effort fidelity, not perfect diff;
        // the snapshot algorithm is allowed to use a different (but
        // equivalent) sequence of bytes.
        let mut a = fresh(20, 5);
        a.vt_write(b"hello\r\nworld");
        let synth = synthesize(&a).expect("synth");

        let mut b = fresh(synth.cols, synth.rows);
        b.vt_write(&synth.bytes);

        // Both terminals should report cursor at the end of "world" on row 1.
        let ax = a.cursor_x().expect("cursor_x a");
        let ay = a.cursor_y().expect("cursor_y a");
        let bx = b.cursor_x().expect("cursor_x b");
        let by = b.cursor_y().expect("cursor_y b");
        assert_eq!((ax, ay), (bx, by), "cursor position should round-trip");
    }

    /// Regression test for phux-073: a wide CJK glyph (here `你`) takes
    /// two columns; libghostty marks the second column as
    /// `CellWide::SpacerTail`. Before the fix the synthesizer treated
    /// that tail as a blank cell and emitted a space, producing the
    /// wrong layout on replay. After the fix the tail is skipped and
    /// the grid round-trips exactly.
    #[test]
    fn synthesizer_skips_wide_cell_tails() {
        let mut a = fresh(10, 2);
        // Two CJK glyphs (4 columns wide total) followed by ASCII.
        a.vt_write("你好ab".as_bytes());

        // Sanity: the source grid should contain both wide glyphs.
        let src_grid = render_grid(&a);
        assert_eq!(src_grid[0], "你好ab    ", "source grid layout");

        let synth = synthesize(&a).expect("synth");
        let mut b = fresh(synth.cols, synth.rows);
        b.vt_write(&synth.bytes);

        let dst_grid = render_grid(&b);
        assert_eq!(
            src_grid, dst_grid,
            "grid must round-trip through synthesizer for wide glyphs"
        );

        // Bytes must NOT contain a stray space between the two wide
        // glyphs (`你` followed by ` ` would be the wide-tail bug).
        let bytes_str = String::from_utf8_lossy(&synth.bytes);
        assert!(
            bytes_str.contains("你好"),
            "synthesized bytes should contain consecutive wide glyphs, got: {bytes_str:?}"
        );
        assert!(
            !bytes_str.contains("你 好"),
            "synthesized bytes must not insert a space between wide glyphs (wide-tail bug)"
        );
    }
}
