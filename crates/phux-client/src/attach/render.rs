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
    render::{CellIterator, CursorVisualStyle, Dirty, RowIterator, Snapshot},
    screen::CellWide,
    style::{RgbColor, Style, StyleColor, Underline},
};
use phux_core::screen::{CellColor, CellStyle, CursorState, RenderedFrame};
use phux_protocol::{kitty_replay, sgr::write_reset_and_sgr};

/// Errors the renderer can surface.
#[derive(Debug, thiserror::Error)]
pub enum RenderError {
    /// libghostty surfaced an error from a render-state operation.
    #[error("libghostty: {0}")]
    Ghostty(#[from] libghostty_vt::Error),
    /// stdout (or the test buffer) returned an I/O error.
    #[error("io: {0}")]
    Io(#[from] io::Error),
    /// Kitty graphics replay failed while projecting libghostty image state.
    #[error("kitty replay: {0}")]
    KittyReplay(#[from] kitty_replay::KittyReplayError),
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
    kitty_placements: libghostty_vt::kitty::graphics::PlacementIterator<'alloc>,
    /// Last-seen authoritative cursor position (outer-viewport coords:
    /// pane-local cursor plus [`Self::last_origin`]). Updated at the end of
    /// [`Self::render`]. The host-cursor restore paths read this. `None`
    /// while the cursor is hidden.
    last_cursor: Option<(u16, u16)>,
    /// Pane-local cursor `(row, col)` as of the most recent render — the
    /// libghostty viewport cursor BEFORE [`Self::last_origin`] is added.
    /// This is the authoritative anchor the predictive-echo layer
    /// (`phux-9gw.1`) re-syncs from: predictions are pane-local, so feeding
    /// the layer the outer-absolute [`Self::last_cursor`] instead would
    /// clamp a lower pane's cursor up into the wrong region (the mid-screen
    /// ghost echo after a split, phux-7ry0). `None` while the cursor is hidden.
    last_cursor_local: Option<(u16, u16)>,
    /// Outer-viewport origin `(x, y)` of the most recent `render_at` paint.
    /// The predictive-echo overlay adds this to each pane-local prediction
    /// so a pane offset from the viewport origin (any split that isn't the
    /// top-left leaf) paints its echo over the pane's real cells rather than
    /// at the viewport-absolute coordinate. Defaults to `(0, 0)`.
    last_origin: (u16, u16),
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
            kitty_placements: libghostty_vt::kitty::graphics::PlacementIterator::new()?,
            last_cursor: None,
            last_cursor_local: None,
            last_origin: (0, 0),
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

    /// Pane-local cursor `(row, col)` as of the most recent render — the
    /// cursor BEFORE the pane's outer-viewport origin is added. The
    /// predictive-echo layer re-anchors from this (predictions are
    /// pane-local); see [`Self::last_cursor_local`]'s field docs for why
    /// feeding it [`Self::last_cursor`] strands the echo mid-screen
    /// (phux-7ry0). `None` if the cursor was hidden or no render has occurred.
    #[must_use]
    pub const fn last_cursor_local(&self) -> Option<(u16, u16)> {
        self.last_cursor_local
    }

    /// Outer-viewport origin `(x, y)` of the most recent `render_at` paint.
    /// The predictive-echo overlay adds this to each pane-local prediction
    /// to position it over the focused pane's cells.
    #[must_use]
    pub const fn last_origin(&self) -> (u16, u16) {
        self.last_origin
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

    /// Project this pane's grid into a region of a dense [`RenderedFrame`]
    /// instead of emitting VT (`phux-l5xa`).
    ///
    /// Walks the **same** `RenderState` snapshot + `RowIterator` /
    /// `CellIterator` as [`Self::render_at`], but writes each cell's
    /// grapheme + resolved style into `frame` at `(row + origin.1, col +
    /// origin.0)`, clipped to `clip = (cols, rows)` of the pane's render
    /// rect exactly as the VT path clips. This is the structured-cells
    /// counterpart to the byte renderer: no VT, no re-parse, so the
    /// composited view can be introspected with no external emulator.
    ///
    /// Wide glyphs are mirrored faithfully: the base cell carries the
    /// cluster, and its `SpacerTail` column is left as the empty grapheme
    /// (`""`) so a consumer reconstructs exact widths (see [`RenderedCell`]).
    /// Copy-mode selection inversion is intentionally *not* applied — this
    /// is a side-effect-free introspection path, not the live overlay.
    ///
    /// Returns the pane's cursor in **frame-absolute** coordinates (pane
    /// viewport cursor shifted by `origin`), or `None` when the cursor is
    /// off-viewport or clipped away. The compositor elects which pane's
    /// cursor becomes the frame cursor.
    ///
    /// [`RenderedCell`]: phux_core::screen::RenderedCell
    pub fn render_at_cells(
        &mut self,
        terminal: &GhosttyTerminal<'alloc, '_>,
        frame: &mut RenderedFrame,
        origin: (u16, u16),
        clip: (u16, u16),
    ) -> Result<Option<CursorState>, RenderError> {
        let (ox, oy) = origin;
        let (clip_cols, clip_rows) = clip;
        let snapshot = self.state.update(terminal)?;
        // Clip to the render rect, mirroring `render_at_inner`: a
        // server-authoritative mirror may transiently exceed the client's
        // layout rect during a resize handshake; confine the walk so a wider
        // mirror never spills past the rect and a smaller one stays in-grid.
        let rows_total = snapshot.rows()?.min(clip_rows);
        let cols_total = snapshot.cols()?.min(clip_cols);

        let mut row_iter = self.rows.update(&snapshot)?;
        let mut row_index: u16 = 0;
        while let Some(row) = row_iter.next() {
            if row_index >= rows_total {
                break;
            }
            let mut col: u16 = 0;
            let mut cell_iter = self.cells.update(row)?;
            while let Some(cell) = cell_iter.next() {
                if col >= cols_total {
                    break;
                }
                let wide = cell.raw_cell()?.wide()?;
                let graphemes = cell.graphemes()?;
                let grapheme = if matches!(wide, CellWide::SpacerTail) {
                    // Right half of a wide glyph: the base cell already
                    // carries the cluster; emit no glyph so widths stay exact.
                    String::new()
                } else if graphemes.is_empty() {
                    " ".to_owned()
                } else {
                    graphemes.iter().collect()
                };
                let style = to_cell_style(&cell.style()?, cell.fg_color()?, cell.bg_color()?);
                if let Some(dst) =
                    frame.cell_mut(row_index.saturating_add(oy), col.saturating_add(ox))
                {
                    dst.grapheme = grapheme;
                    dst.style = style;
                }
                col = col.saturating_add(1);
            }
            row_index = row_index.saturating_add(1);
        }

        // Cursor, shifted into frame-absolute coords, dropped when it sits
        // outside the painted (clipped) region.
        let cursor = match snapshot.cursor_viewport()? {
            Some(v) if v.y < rows_total && v.x < cols_total => Some(CursorState {
                x: v.x.saturating_add(ox),
                y: v.y.saturating_add(oy),
                visible: snapshot.cursor_visible()?,
            }),
            _ => None,
        };
        Ok(cursor)
    }

    /// Render `terminal` into the outer-viewport rect at `rect_origin =
    /// (x, y)` spanning `rect_clip = (cols, rows)`, **letterboxed**: when the
    /// server-authoritative mirror grid (`mirror = (cols, rows)`) is smaller
    /// than the rect on an axis, centre the content within the rect and blank
    /// the surrounding margin bars rather than painting at the rect origin
    /// (which would pin an undersized mirror to the top-left and leave stale
    /// cells along the bottom/right of the rect).
    ///
    /// When the mirror is >= the rect on an axis, this degrades to the
    /// existing [`Self::render_at`] clamp on that axis (no pad, clip to the
    /// rect) — a wider/taller mirror is confined to the rect exactly as
    /// before (phux-wurs). The mirror-equals-rect case is byte-identical to
    /// [`Self::render_at_full`]: zero pad ⇒ no margin bars ⇒ the same core
    /// paint at the same origin.
    ///
    /// `force_full` forwards to the core paint (the full-frame path forces a
    /// redraw after its `ED2`). The centring math (floor split, the extra pad
    /// cell on the bottom/right of an odd gap) lives in the private
    /// `letterbox_rect` helper.
    ///
    /// This is the single-view letterbox of ADR-0027 decision points 1-2:
    /// one Terminal rendered into one slot under the nk07/xjgs geometry
    /// policy. True multi-leaf mirroring (the same Terminal in N slots) is a
    /// layout-model change and is out of scope here.
    pub fn render_at_letterboxed(
        &mut self,
        terminal: &GhosttyTerminal<'alloc, '_>,
        out: &mut impl Write,
        rect_origin: (u16, u16),
        rect_clip: (u16, u16),
        mirror: (u16, u16),
        force_full: bool,
    ) -> Result<Dirty, RenderError> {
        let lb = letterbox_rect(rect_origin, rect_clip, mirror);
        // Blank the four margin bars first so an undersized mirror's
        // surrounding cells are cleared before the centred content paints
        // over the interior. Skipped entirely when there is no pad (the
        // mirror-fills-the-rect / clamp case), keeping that path byte-identical.
        emit_letterbox_margins(out, lb)?;
        self.render_at_inner(terminal, out, lb.inner_origin, lb.inner_clip, force_full)
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
        // Record where this pane is anchored before any early-return: the
        // predictive-echo overlay reads `last_origin` to place pane-local
        // echoes, and the pane stays at this origin even on a clean (no-op)
        // render.
        self.last_origin = origin;
        let snapshot = self.state.update(terminal)?;
        let dirty = if force_full {
            Dirty::Full
        } else {
            snapshot.dirty()?
        };

        let emitted_kitty = matches!(dirty, Dirty::Clean)
            && kitty_replay::emit_kitty_graphics_replay(
                terminal,
                &mut self.kitty_placements,
                out,
                origin,
                clip,
            )?;

        if matches!(dirty, Dirty::Clean) {
            render_clean_frame_cursor(
                &snapshot,
                out,
                origin,
                emitted_kitty,
                &mut self.last_cursor,
                &mut self.last_cursor_local,
            )?;
            return Ok(dirty);
        }
        out.write_all(b"\x1b[?25l")?;

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

        let _ = kitty_replay::emit_kitty_graphics_replay(
            terminal,
            &mut self.kitty_placements,
            out,
            origin,
            clip,
        )?;

        // Reset SGR before the final cursor placement so the visual
        // cursor isn't tainted by the last cell's attributes.
        out.write_all(b"\x1b[0m")?;
        // Final cursor placement + visibility. Cache the (row, col) for
        // the predictive-echo layer to read via [`Self::last_cursor`].
        self.last_cursor = if let Some(viewport) = snapshot.cursor_viewport()? {
            let abs_y = viewport.y.saturating_add(oy);
            let abs_x = viewport.x.saturating_add(ox);
            write_cup(out, abs_y, abs_x)?;
            // Cache the pane-local cursor (pre-offset) for the predict layer
            // alongside the outer-absolute one for the host-cursor restore.
            self.last_cursor_local = Some((viewport.y, viewport.x));
            Some((abs_y, abs_x))
        } else {
            self.last_cursor_local = None;
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

fn render_clean_frame_cursor(
    snapshot: &Snapshot<'_, '_>,
    out: &mut impl Write,
    origin: (u16, u16),
    emitted_kitty: bool,
    last_cursor: &mut Option<(u16, u16)>,
    last_cursor_local: &mut Option<(u16, u16)>,
) -> Result<(), RenderError> {
    // No row content changed, but the cursor may have MOVED — a pure cursor
    // advance. Reposition + refresh the cached cursor when it changed.
    let (ox, oy) = origin;
    let new_local = snapshot.cursor_viewport()?.map(|v| (v.y, v.x));
    let new_abs = new_local.map(|(y, x)| (y.saturating_add(oy), x.saturating_add(ox)));
    if new_abs == *last_cursor && !emitted_kitty {
        return Ok(());
    }

    if let Some((abs_y, abs_x)) = new_abs {
        write_cup(out, abs_y, abs_x)?;
        if snapshot.cursor_visible()? {
            out.write_all(b"\x1b[?25h")?;
        }
        out.flush()?;
    } else if emitted_kitty {
        out.flush()?;
    }
    *last_cursor = new_abs;
    *last_cursor_local = new_local;
    Ok(())
}

/// Project a libghostty cell's `(Style, resolved fg, resolved bg)` into a
/// plain-data [`CellStyle`] for the rendered-frame introspection path
/// (`phux-l5xa`).
///
/// This mirrors the server synthesizer's `collect_cell` (`phux-8yl`) — the
/// two can't share code because that projection lives in `phux-server` and
/// this walk runs client-side, but they must agree cell-for-cell so a
/// `--rendered` frame and a `--cells` snapshot describe the same glyph
/// identically.
fn to_cell_style(style: &Style, fg: Option<RgbColor>, bg: Option<RgbColor>) -> CellStyle {
    CellStyle {
        bold: style.bold,
        faint: style.faint,
        italic: style.italic,
        underline: !matches!(style.underline, Underline::None),
        blink: style.blink,
        inverse: style.inverse,
        invisible: style.invisible,
        strikethrough: style.strikethrough,
        overline: style.overline,
        fg: cell_color(fg, style.fg_color),
        bg: cell_color(bg, style.bg_color),
    }
}

/// Project a cell color to [`CellColor`], preferring the explicit per-cell
/// [`StyleColor`] so a palette index keeps its identity, and falling back to
/// the iteration's resolved RGB. Mirrors the synthesizer's `cell_color`.
fn cell_color(resolved: Option<RgbColor>, raw: StyleColor) -> CellColor {
    match raw {
        StyleColor::Palette(index) => CellColor::Palette { index: index.0 },
        StyleColor::Rgb(rgb) => CellColor::Rgb {
            r: rgb.r,
            g: rgb.g,
            b: rgb.b,
        },
        StyleColor::None => resolved.map_or(CellColor::Default, |rgb| CellColor::Rgb {
            r: rgb.r,
            g: rgb.g,
            b: rgb.b,
        }),
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

/// The centred placement of a mirror within a render rect, plus the margin
/// bars to blank around it (ADR-0027 single-view letterbox, phux-7ubw).
///
/// All coordinates are outer-viewport cells. `inner_origin`/`inner_clip` are
/// what the core paint ([`TerminalRenderer::render_at_inner`]) consumes:
/// the content's centred top-left and its clamped extent. The four `margin_*`
/// fields are the surrounding gap the mirror does not cover and that
/// [`emit_letterbox_margins`] blanks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Letterbox {
    /// Centred top-left of the mirror content (rect origin + pad).
    inner_origin: (u16, u16),
    /// Painted extent `min(mirror, rect)` on each axis — the clamp the
    /// existing `render_at` already applies, so a mirror >= the rect is
    /// confined to the rect (phux-wurs) with no pad.
    inner_clip: (u16, u16),
    /// Left pad width in columns (`= inner_origin.0 - rect_origin.0`).
    margin_left: u16,
    /// Right pad width in columns (the floor split's extra cell lands here).
    margin_right: u16,
    /// Top pad height in rows (`= inner_origin.1 - rect_origin.1`).
    margin_top: u16,
    /// Bottom pad height in rows (the floor split's extra cell lands here).
    margin_bottom: u16,
    /// The rect's outer origin `(x, y)`, retained so the margin emitter can
    /// position the bars without re-deriving it from the pads.
    rect_origin: (u16, u16),
    /// The rect's full extent `(cols, rows)`, retained for the same reason.
    rect_clip: (u16, u16),
}

/// Centre a mirror of `mirror = (cols, rows)` within the render rect at
/// `rect_origin = (x, y)` spanning `rect_clip = (cols, rows)`, returning the
/// centred [`Letterbox`].
///
/// Per axis: when the mirror is smaller than the rect, the gap
/// `rect - mirror` is split with `pad = gap / 2` on the leading edge
/// (left/top) and the remainder `gap - pad` on the trailing edge
/// (right/bottom) — a floor split that puts the extra cell of an odd gap on
/// the bottom/right. When the mirror is `>=` the rect, the pad is `0` and the
/// clip clamps to the rect (the existing `render_at` behaviour, phux-wurs).
///
/// Pure — no I/O, no `terminal` access — so it is unit-testable in isolation.
fn letterbox_rect(rect_origin: (u16, u16), rect_clip: (u16, u16), mirror: (u16, u16)) -> Letterbox {
    let (rx, ry) = rect_origin;
    let (rect_cols, rect_rows) = rect_clip;
    let (mirror_cols, mirror_rows) = mirror;

    // Per-axis: clamp the painted extent to the rect, then centre the gap with
    // the floor split (extra cell on the trailing edge).
    let inner_cols = mirror_cols.min(rect_cols);
    let inner_rows = mirror_rows.min(rect_rows);
    let gap_x = rect_cols.saturating_sub(mirror_cols);
    let gap_y = rect_rows.saturating_sub(mirror_rows);
    let margin_left = gap_x / 2;
    let margin_right = gap_x - margin_left;
    let margin_top = gap_y / 2;
    let margin_bottom = gap_y - margin_top;

    Letterbox {
        inner_origin: (
            rx.saturating_add(margin_left),
            ry.saturating_add(margin_top),
        ),
        inner_clip: (inner_cols, inner_rows),
        margin_left,
        margin_right,
        margin_top,
        margin_bottom,
        rect_origin,
        rect_clip,
    }
}

/// Blank the four margin bars of a [`Letterbox`] so an undersized mirror's
/// surrounding rect cells are cleared before the centred content paints.
///
/// Each bar is a sequence of `CUP` + an SGR-reset blank run: top and bottom
/// bars span the full rect width; left and right bars span only the interior
/// rows (between the top and bottom bars) so the corners are written once, by
/// the top/bottom bars. A `Letterbox` with no pad (the mirror fills or
/// exceeds the rect) emits nothing, keeping the clamp path byte-identical to
/// [`TerminalRenderer::render_at`].
fn emit_letterbox_margins(out: &mut impl Write, lb: Letterbox) -> io::Result<()> {
    let (rx, ry) = lb.rect_origin;
    let (rect_cols, rect_rows) = lb.rect_clip;
    if lb.margin_left == 0 && lb.margin_right == 0 && lb.margin_top == 0 && lb.margin_bottom == 0 {
        return Ok(());
    }
    // Reset SGR so the blanks paint in the default (background) style and no
    // prior run's attributes leak into the bars.
    out.write_all(b"\x1b[0m")?;

    // Top bar: full-width rows above the centred content.
    for row in ry..ry.saturating_add(lb.margin_top) {
        write_cup(out, row, rx)?;
        write_blank_run(out, rect_cols)?;
    }
    // Bottom bar: full-width rows below the centred content.
    let content_bottom = ry
        .saturating_add(lb.margin_top)
        .saturating_add(lb.inner_clip.1);
    for row in content_bottom..ry.saturating_add(rect_rows) {
        write_cup(out, row, rx)?;
        write_blank_run(out, rect_cols)?;
    }
    // Left/right bars: only the interior rows (the top/bottom bars already
    // cleared the corners).
    let interior_top = ry.saturating_add(lb.margin_top);
    let interior_bottom = content_bottom;
    let right_col = rx
        .saturating_add(lb.margin_left)
        .saturating_add(lb.inner_clip.0);
    for row in interior_top..interior_bottom {
        if lb.margin_left > 0 {
            write_cup(out, row, rx)?;
            write_blank_run(out, lb.margin_left)?;
        }
        if lb.margin_right > 0 {
            write_cup(out, row, right_col)?;
            write_blank_run(out, lb.margin_right)?;
        }
    }
    Ok(())
}

/// Write `n` blank (space) cells in one call — the margin-bar fill.
fn write_blank_run(out: &mut impl Write, n: u16) -> io::Result<()> {
    if n == 0 {
        return Ok(());
    }
    let blanks = vec![b' '; n as usize];
    out.write_all(&blanks)
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

    /// phux-7ry0 regression: rendering a pane at a non-zero outer origin
    /// (a lower split leaf) must cache the cursor BOTH ways — outer-absolute
    /// in `last_cursor` (for the host-cursor restore) and pane-local in
    /// `last_cursor_local` (for the predictive-echo anchor) — and record the
    /// paint origin. Feeding the predict layer the outer-absolute cursor was
    /// the bug: its pane-grid clamp dragged a lower pane's cursor up into the
    /// middle of the screen (the ghost echo).
    #[test]
    fn render_at_offset_caches_pane_local_cursor_and_origin() {
        let mut terminal = fresh(5, 2);
        terminal.vt_write(b"ab"); // cursor lands pane-local at (row 0, col 2)
        let mut renderer = TerminalRenderer::new().expect("TerminalRenderer::new");
        let mut buf = Vec::new();
        // Paint as the bottom leaf of a 24-row split: origin (x=0, y=13).
        let _ = renderer
            .render_at(&terminal, &mut buf, (0, 13), (5, 2))
            .expect("render_at");
        assert_eq!(
            renderer.last_cursor_local(),
            Some((0, 2)),
            "pane-local cursor must be origin-free (the predict anchor)"
        );
        assert_eq!(
            renderer.last_cursor(),
            Some((13, 2)),
            "outer cursor must include the pane origin offset"
        );
        assert_eq!(
            renderer.last_origin(),
            (0, 13),
            "last_origin must record where the pane was painted"
        );
    }

    /// Alt-screen exit must repaint the restored primary screen. A TUI app
    /// (claude, vim, htop) enters 1049h, paints, then exits with 1049l; the
    /// restored primary rows + the shell's fresh prompt must be emitted, not
    /// skipped as Clean with only a cursor reposition.
    #[test]
    fn alt_screen_exit_repaints_restored_primary_screen() {
        let mut terminal = fresh(20, 5);
        terminal.vt_write(b"$ old-prompt");
        let mut renderer = TerminalRenderer::new().expect("TerminalRenderer::new");
        let mut buf = Vec::new();
        let _ = renderer.render(&terminal, &mut buf).expect("render 1");

        // Enter alt screen, paint a TUI frame, render it.
        terminal.vt_write(b"\x1b[?1049h\x1b[2J\x1b[HTUI-FRAME");
        buf.clear();
        let _ = renderer.render(&terminal, &mut buf).expect("render 2");
        assert!(
            String::from_utf8_lossy(&buf).contains("TUI-FRAME"),
            "alt-screen content must paint"
        );

        // Exit alt screen; the shell prints a fresh prompt.
        terminal.vt_write(b"\x1b[?1049l\r\n$ new-prompt");
        buf.clear();
        let dirty = renderer.render(&terminal, &mut buf).expect("render 3");
        let s = String::from_utf8_lossy(&buf);
        assert!(
            !matches!(dirty, Dirty::Clean),
            "alt-screen exit must not classify as Clean"
        );
        assert!(
            s.contains("old-prompt") && s.contains("new-prompt"),
            "restored primary rows + new prompt must repaint, got {s:?}"
        );
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

    /// A pure cursor move — no cell changed, so libghostty reports
    /// `Dirty::Clean` — must still reposition the on-screen cursor (and
    /// refresh `last_cursor`), or the cursor lags a frame behind arrow-key
    /// navigation / autosuggestion-accept until the next dirtying keystroke.
    #[test]
    fn cursor_only_move_repositions_on_a_clean_render() {
        let mut terminal = fresh(10, 2);
        terminal.vt_write(b"hello"); // cursor lands at (row 0, col 5)
        let mut renderer = TerminalRenderer::new().expect("TerminalRenderer::new");
        let mut first = Vec::new();
        let _ = renderer
            .render(&terminal, &mut first)
            .expect("first render");
        assert_eq!(renderer.last_cursor(), Some((0, 5)));

        // Move the cursor only — `\x1b[1;3H` ⇒ row 0, col 2 — no cell changes.
        terminal.vt_write(b"\x1b[1;3H");
        let mut second = Vec::new();
        let dirty = renderer
            .render(&terminal, &mut second)
            .expect("second render");
        assert!(
            matches!(dirty, Dirty::Clean),
            "a cursor-only move leaves rows Clean, got {dirty:?}"
        );
        let s = String::from_utf8_lossy(&second);
        assert!(
            s.contains("\x1b[1;3H"),
            "Clean render must reposition the cursor to (0,2) ⇒ CUP 1;3; got {s:?}"
        );
        assert_eq!(
            renderer.last_cursor(),
            Some((0, 2)),
            "cached cursor must follow the move so the bar-restore agrees"
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

    /// phux-l5xa: `render_at_cells` projects graphemes + resolved style into
    /// a dense frame, shifted by the origin, and returns the cursor in
    /// frame-absolute coordinates.
    #[test]
    fn render_at_cells_projects_graphemes_style_and_cursor() {
        let mut terminal = fresh(10, 3);
        // Bold "Hi", reset, then " X": cols 0..1 bold, col 2 a default space,
        // col 3 a default 'X'. Cursor parks pane-local at col 4.
        terminal.vt_write(b"\x1b[1mHi\x1b[0m X");
        let mut renderer = TerminalRenderer::new().expect("renderer");
        let mut frame = RenderedFrame::blank(12, 4);
        let cursor = renderer
            .render_at_cells(&terminal, &mut frame, (1, 1), (10, 3))
            .expect("render_at_cells");

        assert_eq!(frame.cell(1, 1).expect("in range").grapheme, "H");
        assert!(frame.cell(1, 1).expect("in range").style.bold, "H is bold");
        assert_eq!(frame.cell(1, 2).expect("in range").grapheme, "i");
        assert!(frame.cell(1, 2).expect("in range").style.bold);
        assert_eq!(frame.cell(1, 3).expect("in range").grapheme, " ");
        assert!(
            !frame.cell(1, 3).expect("in range").style.bold,
            "the space after the reset is default style"
        );
        assert_eq!(frame.cell(1, 4).expect("in range").grapheme, "X");
        // Cells outside the painted rect stay blank.
        assert_eq!(frame.cell(0, 0).expect("in range").grapheme, " ");

        let c = cursor.expect("cursor present");
        assert_eq!((c.x, c.y), (5, 1), "pane col 4 + origin (1,1)");
    }

    // ── phux-7ubw: single-view letterbox ─────────────────────────────────

    /// The centring math: a mirror smaller than the rect on both axes is
    /// centred with a floor split, the extra cell of an odd gap landing on
    /// the bottom/right margin.
    #[test]
    fn letterbox_rect_centers_with_floor_split() {
        // Rect 10x6 at origin (0,0), mirror 6x4: even gaps (4 cols, 2 rows).
        let lb = letterbox_rect((0, 0), (10, 6), (6, 4));
        assert_eq!(lb.inner_origin, (2, 1), "even gap centres symmetrically");
        assert_eq!(lb.inner_clip, (6, 4), "clip is the mirror size");
        assert_eq!((lb.margin_left, lb.margin_right), (2, 2));
        assert_eq!((lb.margin_top, lb.margin_bottom), (1, 1));

        // Odd gaps: rect 9x5, mirror 6x4 ⇒ gap 3 cols / 1 row. Floor split
        // puts the extra pad on the right/bottom.
        let lb = letterbox_rect((0, 0), (9, 5), (6, 4));
        assert_eq!(
            (lb.margin_left, lb.margin_right),
            (1, 2),
            "extra col on right"
        );
        assert_eq!(
            (lb.margin_top, lb.margin_bottom),
            (0, 1),
            "extra row on bottom"
        );
        assert_eq!(lb.inner_origin, (1, 0));
    }

    /// The centred origin is offset by the rect origin too, so a pane that is
    /// not the top-left leaf letterboxes within its own rect.
    #[test]
    fn letterbox_rect_offsets_by_rect_origin() {
        let lb = letterbox_rect((4, 3), (10, 6), (6, 4));
        // rect origin (4,3) + pad (2,1).
        assert_eq!(lb.inner_origin, (6, 4));
    }

    /// A mirror that fills or exceeds the rect produces no pad and clamps the
    /// clip to the rect — the existing `render_at` behaviour (phux-wurs).
    #[test]
    fn letterbox_rect_clamps_when_mirror_ge_rect() {
        // Equal: no pad, clip == rect.
        let lb = letterbox_rect((0, 0), (8, 4), (8, 4));
        assert_eq!(lb.inner_origin, (0, 0));
        assert_eq!(lb.inner_clip, (8, 4));
        assert_eq!(
            (
                lb.margin_left,
                lb.margin_right,
                lb.margin_top,
                lb.margin_bottom
            ),
            (0, 0, 0, 0)
        );

        // Larger: still no pad, clip clamps DOWN to the rect.
        let lb = letterbox_rect((0, 0), (8, 4), (20, 10));
        assert_eq!(lb.inner_origin, (0, 0));
        assert_eq!(
            lb.inner_clip,
            (8, 4),
            "clip clamps to the rect, not the mirror"
        );
        assert_eq!(
            (
                lb.margin_left,
                lb.margin_right,
                lb.margin_top,
                lb.margin_bottom
            ),
            (0, 0, 0, 0)
        );
    }

    /// An undersized mirror renders centred: its content's CUP is shifted by
    /// the pad, and the margin rows/cols are blanked.
    #[test]
    fn render_at_letterboxed_centers_undersized_mirror() {
        // Mirror is 4x2 of "ab"/"cd"; rect is 8x4 ⇒ pad (2 cols, 1 row) each.
        let mut terminal = fresh(4, 2);
        terminal.vt_write(b"ab\r\ncd");
        let mut renderer = TerminalRenderer::new().expect("renderer");
        let mut out: Vec<u8> = Vec::new();
        let _ = renderer
            .render_at_letterboxed(&terminal, &mut out, (0, 0), (8, 4), (4, 2), true)
            .expect("render");
        let s = String::from_utf8_lossy(&out);

        // Content is centred: row 0 of the mirror lands at outer row 1
        // (0-based) ⇒ 1-based CUP row 2, col = pad_left 2 ⇒ 1-based col 3.
        assert!(
            s.contains("\x1b[2;3H"),
            "centred content CUP (row 2, col 3) missing; out = {s:?}"
        );
        // The top margin row (outer row 0 ⇒ CUP 1;1) is blanked full-width.
        assert!(
            s.contains("\x1b[1;1H"),
            "top margin bar CUP missing; out = {s:?}"
        );
        // The bottom margin row: content occupies outer rows 1..3, so the
        // bottom bar is outer row 3 ⇒ CUP 4;1.
        assert!(
            s.contains("\x1b[4;1H"),
            "bottom margin bar CUP missing; out = {s:?}"
        );
        // The content glyphs are present.
        assert!(s.contains('a') && s.contains('d'), "content missing; {s:?}");
    }

    /// An undersized mirror blanks exactly N margin rows + the left/right
    /// margin columns: decode the emitted bytes into an 8x4 grid and confirm
    /// the centred 4x2 content sits in the middle with blank borders.
    #[test]
    fn render_at_letterboxed_blanks_margins_around_content() {
        let mut terminal = fresh(4, 2);
        terminal.vt_write(b"WXYZ\r\nMNOP"); // 4 cols x 2 rows of content
        let mut renderer = TerminalRenderer::new().expect("renderer");
        let mut out: Vec<u8> = Vec::new();
        let _ = renderer
            .render_at_letterboxed(&terminal, &mut out, (0, 0), (8, 4), (4, 2), true)
            .expect("render");

        // Decode into an 8x4 grid: content centred at cols 2..6, rows 1..3.
        let grid = decode_grid(&out, 8, 4);
        let at = |r: usize, c: usize| grid[r * 8 + c].0;
        // Top + bottom margin rows are entirely blank.
        for c in 0..8 {
            assert_eq!(at(0, c), None, "top margin row must be blank at col {c}");
            assert_eq!(at(3, c), None, "bottom margin row must be blank at col {c}");
        }
        // Interior rows: left (cols 0,1) and right (cols 6,7) margins blank,
        // content in cols 2..6.
        for r in 1..3 {
            assert_eq!(at(r, 0), None, "left margin blank, row {r}");
            assert_eq!(at(r, 1), None, "left margin blank, row {r}");
            assert_eq!(at(r, 6), None, "right margin blank, row {r}");
            assert_eq!(at(r, 7), None, "right margin blank, row {r}");
        }
        assert_eq!(at(1, 2), Some('W'), "content top-left");
        assert_eq!(at(1, 5), Some('Z'), "content top-right");
        assert_eq!(at(2, 2), Some('M'), "content bottom-left");
        assert_eq!(at(2, 5), Some('P'), "content bottom-right");
    }

    /// A mirror equal to the rect is byte-identical to today's
    /// `render_at_full`: no pad ⇒ no margin bars ⇒ the same core paint.
    #[test]
    fn render_at_letterboxed_equal_size_is_byte_identical() {
        let make = || {
            let mut t = fresh(10, 3);
            t.vt_write(b"\x1b[1mHELLO\x1b[0m world\r\nsecond row\r\nthird");
            t
        };

        let t_a = make();
        let mut r_a = TerminalRenderer::new().expect("renderer");
        let mut today: Vec<u8> = Vec::new();
        let _ = r_a
            .render_at_full(&t_a, &mut today, (0, 0), (10, 3))
            .expect("render_at_full");

        let t_b = make();
        let mut r_b = TerminalRenderer::new().expect("renderer");
        let mut letterboxed: Vec<u8> = Vec::new();
        let _ = r_b
            .render_at_letterboxed(&t_b, &mut letterboxed, (0, 0), (10, 3), (10, 3), true)
            .expect("render_at_letterboxed");

        assert_eq!(
            today, letterboxed,
            "mirror==rect letterbox must be byte-identical to render_at_full"
        );
    }

    /// A mirror larger than the rect clamps exactly as `render_at` does (the
    /// phux-wurs clip): no margin bars, content confined to the rect.
    #[test]
    fn render_at_letterboxed_larger_mirror_clamps_like_render_at() {
        let make = || {
            let mut t = fresh(20, 4);
            t.vt_write(b"ABCDEFGHIJKLMNOPQRST\r\nabcdefghijklmnopqrst");
            t
        };

        // Today's clamp path.
        let t_a = make();
        let mut r_a = TerminalRenderer::new().expect("renderer");
        let mut clamp: Vec<u8> = Vec::new();
        let _ = r_a
            .render_at_full(&t_a, &mut clamp, (0, 0), (12, 2))
            .expect("render_at_full");

        // Letterboxed path with mirror 20x4 > rect 12x2: must match.
        let t_b = make();
        let mut r_b = TerminalRenderer::new().expect("renderer");
        let mut letterboxed: Vec<u8> = Vec::new();
        let _ = r_b
            .render_at_letterboxed(&t_b, &mut letterboxed, (0, 0), (12, 2), (20, 4), true)
            .expect("render_at_letterboxed");

        assert_eq!(
            clamp, letterboxed,
            "mirror>rect letterbox must clamp byte-identically to render_at_full"
        );
    }

    /// The cursor cached in `last_cursor` (and the recorded `last_origin`)
    /// include the letterbox pad offset, so the composite bar-restore agrees
    /// with where the content was actually painted.
    #[test]
    fn render_at_letterboxed_cursor_includes_pad_offset() {
        let mut terminal = fresh(4, 2);
        terminal.vt_write(b"ab"); // cursor parks pane-local at (row 0, col 2)
        let mut renderer = TerminalRenderer::new().expect("renderer");
        let mut out: Vec<u8> = Vec::new();
        // Rect 8x4, mirror 4x2 ⇒ pad (2 cols, 1 row).
        let _ = renderer
            .render_at_letterboxed(&terminal, &mut out, (0, 0), (8, 4), (4, 2), true)
            .expect("render");
        // Pane-local cursor is origin-free (the predict anchor).
        assert_eq!(renderer.last_cursor_local(), Some((0, 2)));
        // Outer cursor includes the pad: (row 0 + pad_top 1, col 2 + pad_left 2).
        assert_eq!(
            renderer.last_cursor(),
            Some((1, 4)),
            "last_cursor must include the letterbox pad offset"
        );
        // The recorded paint origin is the centred (padded) origin.
        assert_eq!(renderer.last_origin(), (2, 1));
    }

    /// phux-l5xa: a double-width glyph occupies its base cell; the
    /// `SpacerTail` column is the empty grapheme so widths stay exact.
    #[test]
    fn render_at_cells_marks_wide_glyph_tail_empty() {
        let mut terminal = fresh(6, 2);
        terminal.vt_write("世".as_bytes());
        let mut renderer = TerminalRenderer::new().expect("renderer");
        let mut frame = RenderedFrame::blank(6, 2);
        let _ = renderer
            .render_at_cells(&terminal, &mut frame, (0, 0), (6, 2))
            .expect("render_at_cells");
        assert_eq!(frame.cell(0, 0).expect("in range").grapheme, "世");
        assert_eq!(
            frame.cell(0, 1).expect("in range").grapheme,
            "",
            "the wide glyph's tail column is the empty grapheme"
        );
        assert_eq!(
            frame.cell(0, 2).expect("in range").grapheme,
            " ",
            "the cell after the wide glyph is a normal blank"
        );
    }
}
