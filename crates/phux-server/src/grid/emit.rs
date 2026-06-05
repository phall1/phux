use std::io::Write as _;

use libghostty_vt::{
    Terminal as GhosttyTerminal,
    render::{CellIteration, CursorVisualStyle, Snapshot},
    style::{RgbColor, Style},
    terminal::Mode,
};

use super::synthesizer::{GRAPHEME_INLINE, SynthesisError};

/// Per-cell emission shared by the full ([`SnapshotSynthesizer::synthesize`])
/// and incremental ([`SnapshotSynthesizer::synthesize_incremental`]) paths.
///
/// Tracks the active SGR pen via `prev_style`, skips wide-cell tails
/// (`CellWide::SpacerTail`, see the comment in the body), and emits the
/// cell's grapheme cluster (or a space for genuinely-blank cells).
pub(crate) fn emit_cell(
    cell: &CellIteration<'_, '_>,
    out: &mut Vec<u8>,
    prev_style: &mut Option<Style>,
) -> Result<(), SynthesisError> {
    // Discriminate wide-cell tails (the right half of a double-width
    // glyph) from genuinely-blank cells. The base grapheme on the wide
    // cell already advanced the cursor across both columns, so the tail
    // must NOT emit a space (which would clobber the right half of the
    // wide glyph). See libghostty's `CellWide`: `SpacerTail` is
    // documented as "do not render".
    let wide = cell.raw_cell()?.wide()?;
    if matches!(wide, CellWide::SpacerTail) {
        return Ok(());
    }

    // Read the grapheme cluster into a stack buffer rather than the
    // allocating [`CellIteration::graphemes`] (`vec!['\0'; len]` per cell).
    // The emit path visits every cell of every changed row each tick under
    // heavy output; a heap allocation per cell dominated the hot path
    // (~50 allocations per row in the bursty-colored-output stress probe).
    // `GRAPHEME_INLINE` covers the common base-codepoint-plus-a-few-marks
    // case; deeper clusters fall back to a one-shot heap retry.
    let len = cell.graphemes_len()?;
    if len == 0 {
        // Genuinely blank cell — emit a space so the column advances.
        // (Wide-tail case was handled above.)
        out.push(b' ');
        return Ok(());
    }

    let style = cell.style()?;
    let fg = cell.fg_color()?;
    let bg = cell.bg_color()?;
    emit_sgr_delta(out, prev_style.as_ref(), &style, fg, bg);
    *prev_style = Some(style);

    let mut inline = [char::from(0u8); GRAPHEME_INLINE];
    if len <= GRAPHEME_INLINE {
        cell.graphemes_buf(&mut inline[..len])?;
        encode_graphemes(out, &inline[..len]);
    } else {
        let mut heap = vec![char::from(0u8); len];
        cell.graphemes_buf(&mut heap)?;
        encode_graphemes(out, &heap);
    }
    Ok(())
}

/// UTF-8 encode a grapheme cluster's codepoints into `out`.
pub(crate) fn encode_graphemes(out: &mut Vec<u8>, graphemes: &[char]) {
    for ch in graphemes {
        let mut buf = [0u8; 4];
        out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
    }
}

/// Post-row-walk epilogue shared by both synthesis paths: reset SGR,
/// re-establish cursor position + visibility + visual style, and replay
/// the load-bearing mode bits queried from the canonical [`Terminal`].
///
/// Identical to the tail of `synthesize` from before the
/// full/incremental split was introduced.
pub(crate) fn emit_epilogue(
    out: &mut Vec<u8>,
    snapshot: &Snapshot<'_, '_>,
    terminal: &GhosttyTerminal<'_, '_>,
) -> Result<(), SynthesisError> {
    // Reset SGR before cursor placement so the cursor's visual style
    // isn't tainted by the last cell's attributes.
    out.extend_from_slice(b"\x1b[0m");

    // Cursor position.
    if let Some(viewport) = snapshot.cursor_viewport()? {
        write_cup(out, viewport.y, viewport.x);
    } else {
        // No viewport-resident cursor; leave at home.
        out.extend_from_slice(b"\x1b[H");
    }

    // Cursor visibility + visual style.
    if snapshot.cursor_visible()? {
        out.extend_from_slice(b"\x1b[?25h");
    } else {
        out.extend_from_slice(b"\x1b[?25l");
    }
    emit_cursor_style(
        out,
        snapshot.cursor_visual_style()?,
        snapshot.cursor_blinking()?,
    );

    // Remaining load-bearing mode bits. Bracketed paste and focus-event
    // reporting are independent of the screen buffer, so their order
    // relative to the cursor does not matter. The alt-screen modes are
    // NOT emitted here — they must precede the row paint (see
    // [`emit_screen_mode`]), or the content lands on the wrong buffer and
    // a `?1049h` after it would clear what we just painted.
    emit_mode(out, terminal, Mode::BRACKETED_PASTE, b"2004")?;
    emit_mode(out, terminal, Mode::FOCUS_EVENT, b"1004")?;
    Ok(())
}

/// Emit the alt-screen DEC mode toggles (47 / 1047 / 1049) that select
/// which screen buffer subsequent content paints into.
///
/// libghostty tracks 47 (`ALT_SCREEN_LEGACY`), 1047 (`ALT_SCREEN`), and
/// 1049 (`ALT_SCREEN_SAVE`) as three independent bits; a full-screen
/// program (vim/less/man/htop/tmux) typically sets 1049, which on entry
/// saves the cursor and clears the alt buffer. Each is queried
/// independently so the synthesis reproduces the terminal's exact
/// alt-screen state rather than forcing the primary screen via a stale
/// `?47l`.
///
/// CRITICAL ordering: this MUST be emitted BEFORE the row paint and the
/// cursor re-establishment. `?1049h` clears the alt buffer and saves the
/// cursor on entry, so emitting it after painting would wipe the content
/// and clobber the restored cursor. Both the full-reset prologue and the
/// per-row diff therefore call this ahead of any cell bytes.
pub(crate) fn emit_screen_mode(
    out: &mut Vec<u8>,
    terminal: &GhosttyTerminal<'_, '_>,
) -> Result<(), SynthesisError> {
    emit_mode(out, terminal, Mode::ALT_SCREEN_LEGACY, b"47")?;
    emit_mode(out, terminal, Mode::ALT_SCREEN, b"1047")?;
    emit_mode(out, terminal, Mode::ALT_SCREEN_SAVE, b"1049")?;
    Ok(())
}

/// 1-based CUP (`CSI <r+1>;<c+1> H`). Inputs are zero-based.
pub(crate) fn write_cup(out: &mut Vec<u8>, row: u16, col: u16) {
    let r = row.saturating_add(1);
    let c = col.saturating_add(1);
    let _ = write!(out, "\x1b[{r};{c}H");
}

/// Emit SGR parameters representing `style` + colors, prefixed by a
/// reset so the parameter list is independent of `prev`. Skip emission
/// entirely if nothing changed.
pub(crate) fn emit_sgr_delta(
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

pub(crate) fn emit_cursor_style(out: &mut Vec<u8>, style: CursorVisualStyle, blinking: bool) {
    // DECSCUSR: `CSI <n> SP q`. Block/blink=1, Block/steady=2,
    // Underline/blink=3, steady=4, Bar/blink=5, steady=6. BlockHollow has
    // no DECSCUSR encoding; map to Block-steady.
    let code: u8 = match (style, blinking) {
        (CursorVisualStyle::Block, true) => 1,
        (CursorVisualStyle::Underline, true) => 3,
        (CursorVisualStyle::Underline, false) => 4,
        (CursorVisualStyle::Bar, true) => 5,
        (CursorVisualStyle::Bar, false) => 6,
        // Steady block, hollow block, and any future variant — treat as steady block.
        _ => 2,
    };
    let _ = write!(out, "\x1b[{code} q");
}

/// Query `mode` on `terminal`; emit `CSI ? <code> h/l` accordingly.
pub(crate) fn emit_mode(
    out: &mut Vec<u8>,
    terminal: &GhosttyTerminal<'_, '_>,
    mode: Mode,
    code: &[u8],
) -> Result<(), SynthesisError> {
    let on = terminal.mode(mode)?;
    out.extend_from_slice(b"\x1b[?");
    out.extend_from_slice(code);
    out.push(if on { b'h' } else { b'l' });
    Ok(())
}
