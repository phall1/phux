//! Render the client's local `libghostty_vt::Terminal` to the outer
//! terminal as VT escape sequences.
//!
//! Under ADR-0013 the client owns one `Terminal` per attached pane;
//! `TERMINAL_OUTPUT` byte frames are fed into it via `vt_write`. This
//! module reads the resulting structured state back out via
//! `RenderState` (per-row dirty tracking) and emits VT to stdout.
//!
//! v0 emits a **dirty-row redraw**: for each row reported dirty by
//! `RenderState`, position the cursor at the row, then walk its cells
//! emitting an SGR sequence only when a cell's style differs from the one
//! currently active on the outer terminal (a run of same-style cells costs
//! one SGR plus the glyphs) and writing each cell's graphemes. Per-row dirty
//! bits are reset after the row is drawn so subsequent renders skip clean
//! rows.
//!
//! No raw-mode or alt-screen toggling happens here; the [`super::driver`]
//! owns those transitions via an RAII guard so they survive panics and
//! early returns.
//!
//! See `research/2026-05-25-libghostty-renderstate.md` for the
//! renderer-side contract this module implements.

use std::io::{self, Write};

use libghostty_vt::{
    RenderState, Terminal as GhosttyTerminal,
    render::{CellIterator, CursorVisualStyle, Dirty, RowIterator},
    style::{RgbColor, Style},
};
use phux_protocol::sgr::write_reset_and_sgr;

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

/// A copy-mode selection in pane-local viewport cells (inclusive), for the
/// renderer to reverse-video while painting (phux copy-mode).
///
/// Linear (text-flow) selection, matching the copy-mode overlay: full interior
/// rows, partial first/last rows. Carrying the highlight here — in the same
/// per-cell render that emits the pane's real styles — is what lets copy-mode
/// leave the screen untouched except for inverting the selected cells, instead
/// of clearing and repainting a separate overlay surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SelectionRect {
    /// First selected row (inclusive).
    pub start_row: u16,
    /// First selected column, on `start_row` (inclusive).
    pub start_col: u16,
    /// Last selected row (inclusive).
    pub end_row: u16,
    /// Last selected column, on `end_row` (inclusive).
    pub end_col: u16,
}

impl SelectionRect {
    /// Whether the pane-local cell `(row, col)` falls inside the selection.
    #[must_use]
    pub const fn contains(self, row: u16, col: u16) -> bool {
        if row < self.start_row || row > self.end_row {
            return false;
        }
        if row == self.start_row && col < self.start_col {
            return false;
        }
        if row == self.end_row && col > self.end_col {
            return false;
        }
        true
    }
}

/// Per-pane render scaffolding.
///
/// Owns the libghostty render iterators so they're reused across frames
/// instead of reallocated each tick.
#[derive(Debug)]
pub struct TerminalRenderer<'alloc> {
    state: RenderState<'alloc>,
    rows: RowIterator<'alloc>,
    cells: CellIterator<'alloc>,
    /// Last-seen authoritative cursor position (viewport coords). Updated
    /// at the end of [`Self::render`] so the predictive-echo layer
    /// (`phux-9gw.1`) can re-anchor its cursor estimate without doing a
    /// second snapshot pass. `None` while the cursor is hidden.
    last_cursor: Option<(u16, u16)>,
    /// Copy-mode selection to reverse-video on the next render, if any.
    ///
    /// Transient: the driver sets it on the focused pane's renderer just
    /// before a copy-mode repaint and clears it immediately after, so
    /// ordinary renders are unaffected and no other paint path needs to know
    /// copy-mode exists.
    selection: Option<SelectionRect>,
}

impl<'alloc> TerminalRenderer<'alloc> {
    /// Allocate render scaffolding for one pane. Do this once per pane,
    /// not per frame.
    pub fn new() -> Result<Self, RenderError> {
        Ok(Self {
            state: RenderState::new()?,
            rows: RowIterator::new()?,
            cells: CellIterator::new()?,
            last_cursor: None,
            selection: None,
        })
    }

    /// Set (or clear) the copy-mode selection to reverse-video on the next
    /// render. Transient — see [`SelectionRect`]; callers set it before a
    /// copy-mode repaint and clear it (`None`) immediately after.
    pub const fn set_selection(&mut self, selection: Option<SelectionRect>) {
        self.selection = selection;
    }

    /// Cursor (row, col) as of the most recent [`Self::render`] call.
    /// Returns `None` if the cursor was hidden or no render has yet
    /// occurred. The predictive-echo layer reads this to re-anchor its
    /// cursor estimate after a server frame.
    #[must_use]
    pub const fn last_cursor(&self) -> Option<(u16, u16)> {
        self.last_cursor
    }

    /// Read the base grapheme of the cell at `(row, col)` in `terminal`.
    ///
    /// Returns `Some(ch)` if the cell has a base grapheme, `None` if it
    /// is blank (no grapheme, wide-tail placeholder, or out of range).
    /// A `' '` (space) cell yields `Some(' ')` so callers can distinguish
    /// "explicitly blanked" from "out of range" — the predict-layer
    /// reconcile treats `' '` and `None` as the same "blank" verdict.
    ///
    /// This takes a fresh snapshot of `terminal` — it must not be called
    /// concurrently with [`Self::render`] (the `&mut self` receiver
    /// guarantees that statically). Used by the per-cell reconcile in
    /// the predict layer (phux-9gw.1.1) to confirm or contradict
    /// predictions against the authoritative cell grid.
    pub fn read_grapheme_at(
        &mut self,
        terminal: &GhosttyTerminal<'alloc, '_>,
        row: u16,
        col: u16,
    ) -> Result<Option<char>, RenderError> {
        Ok(self
            .read_cell_graphemes(terminal, row, col)?
            .and_then(|g| g.first().copied()))
    }

    /// Read the full grapheme cluster of the cell at `(row, col)` as a
    /// `String`, joining every scalar in the cell.
    ///
    /// Returns `Some(s)` if the cell has any grapheme (`s` may be a
    /// multi-codepoint cluster — a flag emoji, a ZWJ family sequence, or
    /// a base plus combining marks), `None` if the cell is blank
    /// (no grapheme, wide-tail placeholder, or out of range). Unlike
    /// [`Self::read_grapheme_at`], which truncates to the base scalar,
    /// this preserves the whole cluster so the predict-layer reconcile
    /// (phux-9gw.1.6) can compare it against a predicted multi-codepoint
    /// cluster.
    ///
    /// Same snapshot semantics as [`Self::read_grapheme_at`]: takes a
    /// fresh snapshot of `terminal`; the `&mut self` receiver guarantees
    /// it is not called concurrently with [`Self::render`].
    pub fn read_grapheme_string_at(
        &mut self,
        terminal: &GhosttyTerminal<'alloc, '_>,
        row: u16,
        col: u16,
    ) -> Result<Option<String>, RenderError> {
        Ok(self.read_cell_graphemes(terminal, row, col)?.and_then(|g| {
            if g.is_empty() {
                None
            } else {
                Some(g.into_iter().collect())
            }
        }))
    }

    /// Shared cell-grapheme lookup backing [`Self::read_grapheme_at`] and
    /// [`Self::read_grapheme_string_at`]. Returns the cell's scalar vec,
    /// or `None` when `(row, col)` is out of range.
    fn read_cell_graphemes(
        &mut self,
        terminal: &GhosttyTerminal<'alloc, '_>,
        row: u16,
        col: u16,
    ) -> Result<Option<Vec<char>>, RenderError> {
        let snapshot = self.state.update(terminal)?;
        let rows_total = snapshot.rows()?;
        let cols_total = snapshot.cols()?;
        if row >= rows_total || col >= cols_total {
            return Ok(None);
        }
        let mut row_iter = self.rows.update(&snapshot)?;
        let mut row_index: u16 = 0;
        while let Some(this_row) = row_iter.next() {
            if row_index == row {
                let mut cell_iter = self.cells.update(this_row)?;
                cell_iter.select(col)?;
                return Ok(Some(cell_iter.graphemes()?));
            }
            row_index = row_index.saturating_add(1);
            if row_index >= rows_total {
                break;
            }
        }
        Ok(None)
    }

    /// Render dirty rows of `terminal` to `out`. Returns the dirty
    /// classification observed; the caller can use it to decide whether
    /// to flush.
    ///
    /// After this returns, every dirty bit (global + per-row) is reset,
    /// per the libghostty contract documented in
    /// `research/2026-05-25-libghostty-renderstate.md` §3.
    ///
    /// This is the single-pane entry point — equivalent to
    /// [`Self::render_at`] with origin `(0, 0)`. Multi-pane callers
    /// (see `attach::multi_pane`, phux-4li.4) use [`Self::render_at`] to
    /// position the terminal's content inside a sub-rectangle of the
    /// outer viewport.
    pub fn render(
        &mut self,
        terminal: &GhosttyTerminal<'alloc, '_>,
        out: &mut impl Write,
    ) -> Result<Dirty, RenderError> {
        // No pane rect to clip against; the terminal's own grid defines the
        // extent (`u16::MAX` clamps to the grid size on both axes).
        self.render_at(terminal, out, (0, 0), (u16::MAX, u16::MAX))
    }

    /// Render `terminal` into the outer viewport with its top-left at
    /// `origin = (x, y)` in outer-viewport cell coordinates, clipped to
    /// `clip = (cols, rows)` of the pane's render rect.
    ///
    /// The painted extent is `min(terminal grid, clip)` on each axis. The
    /// mirror's libghostty grid size is server-authoritative and may
    /// transiently exceed the client's layout rect during a resize
    /// handshake; `clip` confines the paint to the rect so a wider mirror
    /// never spills past the rect (into a divider or a neighbour pane) and
    /// a narrower mirror never paints beyond its own grid. Every row CUP is
    /// shifted by `origin.1` and every column by `origin.0`; the final
    /// cursor placement (cached in [`Self::last_cursor`]) is reported in
    /// **outer-viewport** coordinates, not pane-local — that's what the
    /// predictive-echo overlay needs for direct stdout writes.
    ///
    /// Multi-pane drivers call this once per visible pane; dividers are
    /// painted separately via
    /// [`crate::render::chrome::dividers::render_dividers`].
    pub fn render_at(
        &mut self,
        terminal: &GhosttyTerminal<'alloc, '_>,
        out: &mut impl Write,
        origin: (u16, u16),
        clip: (u16, u16),
    ) -> Result<Dirty, RenderError> {
        self.render_at_inner(terminal, out, origin, clip, false)
    }

    /// Like [`Self::render_at`] but unconditionally repaints every row,
    /// ignoring the incremental dirty tracking.
    ///
    /// Required by the full-frame paint path: that path emits `ED2`
    /// (clear screen) before re-rendering each pane, which wipes the
    /// terminal but leaves libghostty's per-row dirty bits clean for a
    /// pane whose *content* didn't change (e.g. the surviving pane after
    /// a split or resize). A plain `render_at` would see `Dirty::Clean`,
    /// early-return, and leave that pane blank on the freshly-cleared
    /// screen. Forcing a full redraw repaints it from the grid. See the
    /// split-leaves-original-pane-blank bug.
    pub fn render_at_full(
        &mut self,
        terminal: &GhosttyTerminal<'alloc, '_>,
        out: &mut impl Write,
        origin: (u16, u16),
        clip: (u16, u16),
    ) -> Result<Dirty, RenderError> {
        self.render_at_inner(terminal, out, origin, clip, true)
    }

    fn render_at_inner(
        &mut self,
        terminal: &GhosttyTerminal<'alloc, '_>,
        out: &mut impl Write,
        origin: (u16, u16),
        clip: (u16, u16),
        force_full: bool,
    ) -> Result<Dirty, RenderError> {
        let (ox, oy) = origin;
        let (clip_cols, clip_rows) = clip;
        let snapshot = self.state.update(terminal)?;
        let dirty = if force_full {
            Dirty::Full
        } else {
            snapshot.dirty()?
        };

        match dirty {
            Dirty::Clean => return Ok(dirty),
            Dirty::Partial | Dirty::Full => {
                out.write_all(b"\x1b[?25l")?;
            }
        }

        // Walk rows. Under `Dirty::Full` paint every row; under
        // `Dirty::Partial` skip rows whose per-row dirty bit is clear.
        let mut row_iter = self.rows.update(&snapshot)?;
        let mut row_index: u16 = 0;
        // Clip to the render rect: a server-authoritative mirror may be
        // larger than the client's layout rect during a resize handshake;
        // painting past the rect would spill into a divider or neighbour
        // pane. `min` also keeps a smaller mirror within its own grid.
        let rows_total = snapshot.rows()?.min(clip_rows);
        let cols_total = snapshot.cols()?.min(clip_cols);
        while let Some(row) = row_iter.next() {
            if row_index >= rows_total {
                break;
            }
            let must_draw = matches!(dirty, Dirty::Full) || row.dirty()?;
            if must_draw {
                write_cup(out, row_index.saturating_add(oy), ox)?;
                // Force a reset at row start so the previous row's tail
                // style can't leak into the current row. After this the
                // active outer-terminal SGR state is the default style,
                // which `emitted = None` represents.
                out.write_all(b"\x1b[0m")?;
                let mut emitted: Option<EmittedStyle> = None;

                // Copy-mode selection (pane-local cells), reverse-videoed as
                // each cell is emitted with its real style — see `SelectionRect`.
                let selection = self.selection;
                let mut col: u16 = 0;
                let mut cell_iter = self.cells.update(row)?;
                while let Some(cell) = cell_iter.next() {
                    if col >= cols_total {
                        break;
                    }
                    let graphemes = cell.graphemes()?;
                    let mut style = cell.style()?;
                    let fg = cell.fg_color()?;
                    let bg = cell.bg_color()?;
                    // Invert the cell when it falls in the copy-mode selection:
                    // toggle so an already-reverse cell flips back, making every
                    // selected cell visibly distinct from its normal state.
                    if selection.is_some_and(|s| s.contains(row_index, col)) {
                        style.inverse = !style.inverse;
                    }
                    col = col.saturating_add(1);
                    // Coalesce: emit an SGR sequence only when the cell's
                    // effective style differs from the one currently active
                    // on the outer terminal. A run of same-style cells then
                    // costs one SGR sequence plus the glyphs, not one SGR per
                    // cell. `emitted` tracks the active state for this row
                    // (reset to default = `None` at row start).
                    emit_sgr_if_changed(out, &mut emitted, style, fg, bg)?;
                    if graphemes.is_empty() {
                        // Blank or wide-tail cell — advance one column with a
                        // space. Wide-tail mis-emission overwrites the
                        // right half of a wide cell; the base grapheme
                        // emitted on the previous cell already covered that
                        // column. End-state stays equivalent.
                        out.write_all(b" ")?;
                        continue;
                    }
                    let mut buf = [0u8; 4];
                    for ch in &graphemes {
                        out.write_all(ch.encode_utf8(&mut buf).as_bytes())?;
                    }
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
        // Final cursor placement + visibility. Cache the (row, col) for
        // the predictive-echo layer to read via [`Self::last_cursor`].
        self.last_cursor = if let Some(viewport) = snapshot.cursor_viewport()? {
            let abs_y = viewport.y.saturating_add(oy);
            let abs_x = viewport.x.saturating_add(ox);
            write_cup(out, abs_y, abs_x)?;
            Some((abs_y, abs_x))
        } else {
            None
        };
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

// CURSOR-AUTHORITY: the canonical CUP formatter. The composite end-of-frame
// emitter (paint::end_of_frame_cursor) and the pane-interior renderer both
// route cursor moves through this one place (ADR-0029); raw `\x1b[..H`
// elsewhere under attach/ is banned.
pub(super) fn write_cup(out: &mut impl Write, row: u16, col: u16) -> io::Result<()> {
    let r = row.saturating_add(1);
    let c = col.saturating_add(1);
    write!(out, "\x1b[{r};{c}H")
}

/// The cell style currently active on the outer terminal, as a comparable
/// key for run coalescing. `fg`/`bg` are tracked alongside `Style` because
/// the renderer sources the resolved RGB foreground/background from the
/// per-cell [`libghostty_vt::render::CellIterator`] (`cell.fg_color()`/`cell.bg_color()`)
/// rather than from `Style`'s palette-indexed color fields.
type EmittedStyle = (Style, Option<RgbColor>, Option<RgbColor>);

/// Whether a `(style, fg, bg)` triple renders as the terminal default — no
/// attributes and no explicit colors. Such a run needs only a plain `\x1b[0m`
/// reset (which `emit_sgr_set` already produces), and at row start the active
/// state is already default, so it emits nothing at all.
fn is_default_render(style: &Style, fg: Option<RgbColor>, bg: Option<RgbColor>) -> bool {
    fg.is_none() && bg.is_none() && *style == Style::default()
}

/// Emit an SGR sequence only when `(style, fg, bg)` differs from the style
/// currently active on the outer terminal (`emitted`).
///
/// `emitted` is the per-row coalescing state: `None` means the default style
/// is active (true at row start, just after the row-leading `\x1b[0m`), and
/// `Some(key)` means `key` was the last sequence written on this row. A run
/// of cells sharing a style therefore emits a single SGR sequence; only a
/// real style change writes another. The bytes for an isolated style change
/// are identical to the pre-coalescing per-cell emission (a `\x1b[0m` reset
/// followed by the attribute/color set), so the rendered screen is unchanged.
fn emit_sgr_if_changed(
    out: &mut impl Write,
    emitted: &mut Option<EmittedStyle>,
    style: Style,
    fg: Option<RgbColor>,
    bg: Option<RgbColor>,
) -> io::Result<()> {
    if is_default_render(&style, fg, bg) {
        // Returning to default mid-row needs an explicit reset; at row start
        // (`emitted == None`) the default is already active, so skip it.
        if emitted.is_some() {
            out.write_all(b"\x1b[0m")?;
            *emitted = None;
        }
        return Ok(());
    }

    let key = (style, fg, bg);
    if *emitted == Some(key) {
        return Ok(());
    }
    emit_sgr_set(out, &style, fg, bg)?;
    *emitted = Some(key);
    Ok(())
}

/// Write a full `\x1b[0m` reset followed by the SGR set for `(style, fg, bg)`.
///
/// The leading reset clears any prior attributes so the resulting outer-
/// terminal state is exactly `(style, fg, bg)` regardless of what preceded
/// it; coalescing in [`emit_sgr_if_changed`] decides *when* this runs.
fn emit_sgr_set(
    out: &mut impl Write,
    style: &Style,
    fg: Option<RgbColor>,
    bg: Option<RgbColor>,
) -> io::Result<()> {
    // Encode via the shared server/client SGR emitter (phux-protocol) so the
    // two ends cannot drift — they previously both dropped underline/overline.
    // Build into a small scratch buffer (this runs once per coalesced style
    // run, not per cell) then write it in one call.
    let mut buf = Vec::with_capacity(32);
    write_reset_and_sgr(&mut buf, style, fg, bg);
    out.write_all(&buf)
}

fn emit_cursor_style(
    out: &mut impl Write,
    style: CursorVisualStyle,
    blinking: bool,
) -> io::Result<()> {
    let code: u8 = match (style, blinking) {
        (CursorVisualStyle::Block, true) => 1,
        (CursorVisualStyle::Underline, true) => 3,
        (CursorVisualStyle::Underline, false) => 4,
        (CursorVisualStyle::Bar, true) => 5,
        (CursorVisualStyle::Bar, false) => 6,
        _ => 2,
    };
    write!(out, "\x1b[{code} q")
}

#[cfg(test)]
#[allow(clippy::expect_used, reason = "tests")]
mod tests {
    use super::*;
    use libghostty_vt::{Terminal as GhosttyTerminal, TerminalOptions};

    /// The core copy-mode fix: with a selection set, the renderer emits the
    /// real pane content and reverse-videos (SGR 7) the selected cells — no
    /// screen clear, no separate overlay surface.
    #[test]
    fn selection_emits_reverse_video_for_selected_cells() {
        let mut t = fresh(10, 2);
        t.vt_write(b"hello");
        let mut r = TerminalRenderer::new().expect("renderer");
        r.set_selection(Some(SelectionRect {
            start_row: 0,
            start_col: 0,
            end_row: 0,
            end_col: 1,
        }));
        let mut out: Vec<u8> = Vec::new();
        let _ = r.render_at_full(&t, &mut out, (0, 0), (10, 2));
        let s = String::from_utf8_lossy(&out);
        // Inverse is emitted first in emit_sgr_set, so the param leads the CSI.
        assert!(
            s.contains("\x1b[7"),
            "expected reverse-video (SGR 7) for the selection, got {s:?}"
        );
        // The real content is still there (no blank/clear). The glyphs are
        // split by the selection's SGR runs (`\x1b[7mhe\x1b[0mllo`), so check
        // the selected and unselected halves separately.
        assert!(s.contains("he"), "selected glyphs must render, got {s:?}");
        assert!(
            s.contains("llo"),
            "unselected glyphs must render, got {s:?}"
        );
        // And without a selection the same render has no reverse-video.
        r.set_selection(None);
        let mut plain: Vec<u8> = Vec::new();
        let _ = r.render_at_full(&t, &mut plain, (0, 0), (10, 2));
        assert!(!String::from_utf8_lossy(&plain).contains("\x1b[7"));
    }

    #[test]
    fn selection_rect_contains_is_linear() {
        // Linear/text selection: full interior rows, partial first/last rows.
        let sel = SelectionRect {
            start_row: 1,
            start_col: 1,
            end_row: 3,
            end_col: 5,
        };
        assert!(sel.contains(1, 1)); // start corner
        assert!(sel.contains(2, 0)); // interior row, any col
        assert!(sel.contains(3, 5)); // end corner
        assert!(!sel.contains(0, 1)); // above
        assert!(!sel.contains(1, 0)); // before start on start row
        assert!(!sel.contains(3, 6)); // after end on end row
        assert!(!sel.contains(4, 1)); // below
    }

    fn fresh(cols: u16, rows: u16) -> GhosttyTerminal<'static, 'static> {
        GhosttyTerminal::new(TerminalOptions {
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
        let mut renderer = TerminalRenderer::new().expect("TerminalRenderer::new");
        let mut buf = Vec::new();
        let _ = renderer.render(&terminal, &mut buf).expect("render");
        // Must start by hiding the cursor.
        assert!(buf.starts_with(b"\x1b[?25l"));
        // Should contain the literal characters "a" and "b" somewhere.
        let s = String::from_utf8_lossy(&buf);
        assert!(s.contains('a') && s.contains('b'));
    }

    /// Incremental-paint baseline: a second `render` of a terminal with no
    /// new input is `Dirty::Clean` and emits ZERO bytes. This is what the
    /// status-bar cache change leans on — the focused pane render is
    /// already a no-op on a steady screen, so the per-frame paint cost
    /// collapses toward zero when nothing changed.
    #[test]
    fn second_render_of_unchanged_terminal_emits_nothing() {
        let mut terminal = fresh(10, 3);
        terminal.vt_write(b"hello");
        let mut renderer = TerminalRenderer::new().expect("TerminalRenderer::new");
        let mut first = Vec::new();
        let _ = renderer
            .render(&terminal, &mut first)
            .expect("first render");
        assert!(!first.is_empty(), "first render must emit content");

        // No new vt_write — the grid is unchanged, so render is Clean.
        let mut second = Vec::new();
        let dirty = renderer
            .render(&terminal, &mut second)
            .expect("second render");
        assert!(
            matches!(dirty, Dirty::Clean),
            "unchanged terminal must report Clean, got {dirty:?}"
        );
        assert!(
            second.is_empty(),
            "unchanged repaint must emit zero bytes; got {:?}",
            String::from_utf8_lossy(&second)
        );
    }

    /// A single changed row repaints only that row: the emitted CUP
    /// targets the touched row and the untouched row's content is absent.
    #[test]
    fn single_row_change_repaints_only_that_row() {
        let mut terminal = fresh(10, 3);
        // Row 0 = "top", row 1 = "mid" (CRLF between).
        terminal.vt_write(b"top\r\nmid");
        let mut renderer = TerminalRenderer::new().expect("TerminalRenderer::new");
        let mut first = Vec::new();
        let _ = renderer
            .render(&terminal, &mut first)
            .expect("first render");

        // Park the cursor on row 1 (CUP row 2, col 1) and overwrite it.
        terminal.vt_write(b"\x1b[2;1HNEW");
        let mut second = Vec::new();
        let _ = renderer
            .render(&terminal, &mut second)
            .expect("second render");
        let s = String::from_utf8_lossy(&second);
        // The changed row (row index 1 ⇒ CUP row 2) must be re-emitted with
        // its new content. The renderer interleaves an SGR reset between
        // cells, so "NEW" is not contiguous — assert on each glyph.
        assert!(
            s.contains("\x1b[2;1H"),
            "changed row CUP missing; out = {s:?}"
        );
        assert!(
            s.contains('N') && s.contains('E') && s.contains('W'),
            "changed row content missing; out = {s:?}"
        );
        // The unchanged row 0 ("top") must NOT be re-emitted: no CUP to
        // row 1 (1-based) and no "top" text on the wire.
        assert!(
            !s.contains("\x1b[1;1H"),
            "unchanged row 0 should not be repainted (CUP leaked); out = {s:?}"
        );
        assert!(
            !s.contains("top"),
            "unchanged row 0 content should not be repainted; out = {s:?}"
        );
    }

    /// Count occurrences of `needle` in `hay`.
    fn count(hay: &[u8], needle: &[u8]) -> usize {
        if needle.is_empty() {
            return 0;
        }
        hay.windows(needle.len()).filter(|w| *w == needle).count()
    }

    /// Render a single terminal once and return the emitted bytes.
    fn render_once(terminal: &GhosttyTerminal<'_, '_>) -> Vec<u8> {
        let mut renderer = TerminalRenderer::new().expect("TerminalRenderer::new");
        let mut buf = Vec::new();
        let _ = renderer.render(terminal, &mut buf).expect("render");
        buf
    }

    /// One visible cell, normalized for grid comparison: a blank cell
    /// (no grapheme) and a single space are the same visible verdict, so
    /// both collapse to `None`. Colors are kept even on blanks because a
    /// colored gap (e.g. a bg run) is visually distinct.
    type VisCell = (Option<char>, Option<RgbColor>, Option<RgbColor>);

    fn vis_cell(graphemes: &[char], fg: Option<RgbColor>, bg: Option<RgbColor>) -> VisCell {
        let ch = match graphemes {
            [] | [' '] => None,
            [c, ..] => Some(*c),
        };
        (ch, fg, bg)
    }

    /// Read the visible grid of a live terminal, normalized via [`vis_cell`].
    fn read_grid(terminal: &GhosttyTerminal<'_, '_>, cols: u16, rows: u16) -> Vec<VisCell> {
        let mut state = RenderState::new().expect("RenderState");
        let mut rows_it = RowIterator::new().expect("RowIterator");
        let mut cells_it = CellIterator::new().expect("CellIterator");
        let snap = state.update(terminal).expect("snapshot");
        let mut out = Vec::new();
        let mut row_iter = rows_it.update(&snap).expect("rows");
        let mut ri: u16 = 0;
        while let Some(row) = row_iter.next() {
            if ri >= rows {
                break;
            }
            let mut cell_iter = cells_it.update(row).expect("cells");
            let mut ci: u16 = 0;
            while let Some(cell) = cell_iter.next() {
                if ci >= cols {
                    break;
                }
                out.push(vis_cell(
                    &cell.graphemes().expect("graphemes"),
                    cell.fg_color().expect("fg"),
                    cell.bg_color().expect("bg"),
                ));
                ci += 1;
            }
            ri += 1;
        }
        out
    }

    /// Decode `bytes` into a fresh `cols`x`rows` terminal and return its
    /// normalized visible grid. This is the in-crate stand-in for the Screen
    /// oracle: it proves the coalesced byte stream reconstructs the same
    /// visible grid, not just the same byte count.
    fn decode_grid(bytes: &[u8], cols: u16, rows: u16) -> Vec<VisCell> {
        let mut term = fresh(cols, rows);
        term.vt_write(bytes);
        read_grid(&term, cols, rows)
    }

    /// (a) A row of N identical-style colored cells emits exactly ONE SGR
    /// set for the run, not one per cell.
    #[test]
    fn identical_colored_run_emits_single_sgr() {
        let cols = 20u16;
        let mut terminal = fresh(cols, 1);
        // Set a truecolor fg, then write a full row of the same color.
        terminal.vt_write(b"\x1b[38;2;10;20;30m");
        terminal.vt_write(&vec![b'x'; cols as usize]);
        let buf = render_once(&terminal);

        // The truecolor fg set appears exactly once for the whole run.
        assert_eq!(
            count(&buf, b"38;2;10;20;30"),
            1,
            "expected a single fg SGR for the identical-style run; out = {:?}",
            String::from_utf8_lossy(&buf)
        );
        // All N glyphs are present.
        assert_eq!(
            count(&buf, b"x"),
            cols as usize,
            "all glyphs must be emitted"
        );
    }

    /// (b) A row alternating two styles emits an SGR per style change, not
    /// per cell.
    #[test]
    fn alternating_styles_emit_one_sgr_per_change() {
        let cols = 10u16;
        let mut terminal = fresh(cols, 1);
        // Alternate red / green truecolor fg per cell.
        for i in 0..cols {
            if i % 2 == 0 {
                terminal.vt_write(b"\x1b[38;2;255;0;0m");
            } else {
                terminal.vt_write(b"\x1b[38;2;0;255;0m");
            }
            terminal.vt_write(b"z");
        }
        let buf = render_once(&terminal);

        let reds = count(&buf, b"38;2;255;0;0");
        let greens = count(&buf, b"38;2;0;255;0");
        // 10 cells alternating ⇒ 5 reds, 5 greens — one SGR per change, i.e.
        // one per cell here because every adjacent pair differs. The point is
        // we emit no MORE than the number of style changes.
        assert_eq!(reds, 5, "one red SGR per red cell; out = {reds}");
        assert_eq!(greens, 5, "one green SGR per green cell; out = {greens}");
    }

    /// A run of three same-color cells between two differently-colored
    /// neighbors collapses to one SGR for the middle run.
    #[test]
    fn middle_run_collapses_to_single_sgr() {
        let cols = 5u16;
        let mut terminal = fresh(cols, 1);
        terminal.vt_write(b"\x1b[38;2;1;1;1mA"); // cell 0: color A
        terminal.vt_write(b"\x1b[38;2;2;2;2mBBB"); // cells 1-3: color B (run)
        terminal.vt_write(b"\x1b[38;2;3;3;3mC"); // cell 4: color C
        let buf = render_once(&terminal);
        assert_eq!(count(&buf, b"38;2;2;2;2"), 1, "middle run is one SGR");
        assert_eq!(count(&buf, b"BBB"), 1, "run glyphs are contiguous");
    }

    /// (c) Round-trip: feed the coalesced output back through a fresh
    /// libghostty Terminal and assert the reconstructed grid equals the
    /// source grid — coalesced output renders identically.
    #[test]
    fn coalesced_output_round_trips_to_identical_grid() {
        let cols = 24u16;
        let rows = 3u16;
        let mut terminal = fresh(cols, rows);
        // A mix that exercises runs, a return-to-default gap, a bg color, and
        // attributes, across rows.
        terminal.vt_write(b"\x1b[38;2;200;100;50mHELLO");
        terminal.vt_write(b"\x1b[0m   "); // default-style gap
        terminal.vt_write(b"\x1b[1;48;2;0;0;255mWORLD"); // bold + bg
        terminal.vt_write(b"\r\n");
        terminal.vt_write(b"\x1b[3;38;2;9;9;9mitalics same color run");
        let buf = render_once(&terminal);

        let src = read_grid(&terminal, cols, rows);
        let reconstructed = decode_grid(&buf, cols, rows);
        assert_eq!(
            src, reconstructed,
            "coalesced output must reconstruct the source grid exactly"
        );
    }

    /// (verify the win) A heavy-colored full-width dirty row emits
    /// substantially fewer bytes coalesced than the per-cell baseline would.
    #[test]
    fn colored_full_row_emits_far_fewer_bytes_than_per_cell() {
        let cols = 80u16;
        let mut terminal = fresh(cols, 1);
        terminal.vt_write(b"\x1b[38;2;120;200;40m");
        terminal.vt_write(&vec![b'#'; cols as usize]);
        let buf = render_once(&terminal);

        // Pre-coalescing, each of the 80 cells emitted `\x1b[0m` (4 bytes)
        // plus `\x1b[38;2;120;200;40m` (18 bytes) plus the glyph (1) ≈ 23
        // bytes/cell ⇒ ~1840 bytes for the run alone. Coalesced, the run is
        // one such sequence (~22 bytes) plus 80 glyphs.
        let per_cell_baseline = cols as usize * (4 + 18 + 1);
        assert!(
            buf.len() * 3 < per_cell_baseline,
            "coalesced row ({} bytes) should be far smaller than the \
             per-cell baseline (~{} bytes)",
            buf.len(),
            per_cell_baseline
        );
        // Exactly one fg SGR for the whole run is the source of the win.
        assert_eq!(count(&buf, b"38;2;120;200;40"), 1);
    }

    /// Returning to the default style mid-row emits a single reset, and a
    /// default run at row start emits no SGR at all.
    #[test]
    fn default_run_emits_at_most_one_reset() {
        let cols = 10u16;
        let mut terminal = fresh(cols, 1);
        // First half colored, second half default.
        terminal.vt_write(b"\x1b[38;2;7;7;7mAAAAA");
        terminal.vt_write(b"\x1b[0mBBBBB");
        let buf = render_once(&terminal);
        // Row leads with one reset (row start) + the colored SGR + a reset
        // when returning to default. Count total `\x1b[0m`: row-start reset
        // (1) + return-to-default (1) + the colored set's leading reset (1)
        // + final cursor reset (1) = 4. The key invariant: the default run
        // (BBBBB) added exactly one reset, not one per cell.
        assert_eq!(count(&buf, b"BBBBB"), 1, "default run glyphs contiguous");
        // The colored fg appears once.
        assert_eq!(count(&buf, b"38;2;7;7;7"), 1);
        // Round-trip equality as the real correctness guard.
        let reconstructed = decode_grid(&buf, cols, 1);
        let src = read_grid(&terminal, cols, 1);
        assert_eq!(
            src, reconstructed,
            "default-gap row round-trips identically"
        );
    }

    /// phux-wurs: the render must clip to the pane's rect, not to the
    /// (server-authoritative) mirror grid. When the mirror is WIDER than the
    /// rect — the resize-handshake window where the server's grid is still
    /// width N while the client layout reports width M < N — `render_at` must
    /// emit at most M columns per row. Painting the mirror's full width would
    /// spill prior content past the rect (the ghost cells / divider overrun).
    #[test]
    fn render_at_clips_columns_to_rect_not_mirror_width() {
        // Mirror is 20 cols wide, full of distinct content across the row.
        let mirror_cols = 20u16;
        let mut terminal = fresh(mirror_cols, 1);
        terminal.vt_write(b"ABCDEFGHIJKLMNOPQRST"); // 20 glyphs, cols 0..20
        let mut renderer = TerminalRenderer::new().expect("renderer");
        // Rect is only 12 cols wide.
        let rect_cols = 12u16;
        let mut out: Vec<u8> = Vec::new();
        let _ = renderer
            .render_at(&terminal, &mut out, (0, 0), (rect_cols, 1))
            .expect("render");
        let s = String::from_utf8_lossy(&out);
        // Columns inside the rect (0..12 ⇒ 'A'..'L') are painted.
        assert!(
            s.contains('A') && s.contains('L'),
            "in-rect glyphs must paint; out = {s:?}"
        );
        // Columns past the rect (12..20 ⇒ 'M'..'T') must NOT be emitted — they
        // would land beyond the pane's rect (divider / neighbour pane), which
        // is exactly the right-side ghost.
        for ch in ['M', 'N', 'O', 'P', 'Q', 'R', 'S', 'T'] {
            assert!(
                !s.contains(ch),
                "column {ch} past the rect must not be painted; out = {s:?}"
            );
        }
    }

    /// phux-wurs: the row walk clips to the rect height too — a mirror taller
    /// than the rect must not paint rows below the rect.
    #[test]
    fn render_at_clips_rows_to_rect_not_mirror_height() {
        let cols = 6u16;
        let mut terminal = fresh(cols, 4);
        terminal.vt_write(b"row0\r\nrow1\r\nrow2\r\nrow3");
        let mut renderer = TerminalRenderer::new().expect("renderer");
        // Rect is only 2 rows tall.
        let mut out: Vec<u8> = Vec::new();
        let _ = renderer
            .render_at(&terminal, &mut out, (0, 0), (cols, 2))
            .expect("render");
        let s = String::from_utf8_lossy(&out);
        // Rows 0..2 emit a CUP (1-based rows 1 and 2); row 2/3 (1-based 3/4)
        // must not.
        assert!(s.contains("\x1b[1;1H"), "row 0 CUP missing; out = {s:?}");
        assert!(s.contains("\x1b[2;1H"), "row 1 CUP missing; out = {s:?}");
        assert!(
            !s.contains("\x1b[3;1H"),
            "row 2 past the rect must not paint; out = {s:?}"
        );
        assert!(
            !s.contains("\x1b[4;1H"),
            "row 3 past the rect must not paint; out = {s:?}"
        );
    }
}
