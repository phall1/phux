//! Synthesize a `TERMINAL_SNAPSHOT` `vt_replay_bytes` blob from a
//! `libghostty_vt::Terminal`.
//!
//! Under [ADR-0013] the wire carries VT bytes, not structured grids.
//! When a client attaches, the server owes it a `TERMINAL_SNAPSHOT`
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
//!    `GhosttyTerminal` via [`libghostty_vt::Terminal::mode`].
//!
//! Out-of-band registries (OSC 8 hyperlinks, kitty graphics, etc.) are
//! deferred — they need their own re-emission strategy and don't appear
//! in `RenderState` directly.
//!
//! [ADR-0013]: https://github.com/phall1/phux/blob/main/ADR/0013-libghostty-bytes-on-wire.md

use std::io::Write as _;

use phux_core::screen::{
    CellColor, CellInfo, CellStyle, CursorState, SCHEMA_VERSION, ScreenState, SemanticContent,
};

use libghostty_vt::{
    RenderState, Terminal as GhosttyTerminal,
    render::{CellIteration, CellIterator, CursorVisualStyle, Dirty, RowIterator, Snapshot},
    screen::{CellSemanticContent, CellWide},
    style::{RgbColor, Style, StyleColor},
    terminal::{Mode, Point, PointCoordinate},
};

use super::reference::{ConsumerReference, ReferenceCursorMode};

/// "All retained history" sentinel for the scrollback request.
///
/// A `Some(0)` scrollback request to
/// [`SnapshotSynthesizer::screen_state_with_scrollback`] means "all
/// available history rows" — the bare `--scrollback` flag with no explicit
/// count (`phux-o1v`). A request of literally zero rows is meaningless, so
/// this reuse is unambiguous.
pub const SCROLLBACK_ALL: u32 = 0;

/// Inline grapheme-cluster buffer size for the scrollback walk.
///
/// Covers the overwhelming-common case (a base codepoint plus a few
/// combining marks) without a heap allocation per cell; deeper clusters
/// fall back to a heap retry on `OutOfSpace`.
pub const GRAPHEME_INLINE: usize = 8;

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
    /// Reusable per-row scratch buffer for the per-consumer diff
    /// ([`Self::synthesize_against_reference`]). Each tick renders every
    /// row's body into this buffer and compares it against the consumer's
    /// reference; a fresh `Vec<u8>` per row would allocate `rows` times per
    /// consumer per tick under heavy output. The buffer is `clear()`-ed
    /// (capacity retained) between rows, so steady-state ticks allocate
    /// nothing here.
    row_scratch: Vec<u8>,
}

impl<'alloc> SnapshotSynthesizer<'alloc> {
    /// Allocate a fresh pool of render iterators. Do this once per pane.
    pub fn new() -> Result<Self, SynthesisError> {
        Ok(Self {
            render_state: RenderState::new()?,
            rows: RowIterator::new()?,
            cells: CellIterator::new()?,
            row_scratch: Vec::new(),
        })
    }

    /// Walk `terminal`'s viewport and emit a VT byte sequence that
    /// reproduces it on a fresh Terminal.
    ///
    /// Returns the synthesised bytes plus the queried `(cols, rows)`
    /// dimensions, since `TERMINAL_SNAPSHOT` carries them alongside the
    /// replay body (SPEC §8.4).
    pub fn synthesize(
        &mut self,
        terminal: &GhosttyTerminal<'alloc, '_>,
    ) -> Result<SnapshotBytes, SynthesisError> {
        let snapshot = self.render_state.update(terminal)?;
        let cols = snapshot.cols()?;
        let rows_n = snapshot.rows()?;

        let mut out: Vec<u8> = Vec::with_capacity(usize::from(cols) * usize::from(rows_n) * 2);

        // 1. Reset target: DECSTR (soft reset) + ED 2 (clear screen) + CUP home.
        out.extend_from_slice(b"\x1b[!p\x1b[2J\x1b[H");

        // 1b. Select the screen buffer BEFORE painting (phux-99n). If the
        //     terminal is on the alt screen, entering it now (esp. 1049,
        //     which clears the alt buffer) means the row paint below lands
        //     on the correct buffer instead of the primary screen.
        emit_screen_mode(&mut out, terminal)?;

        // 2. Walk rows + cells, emitting SGR deltas and graphemes. The
        //    full-snapshot path paints every row unconditionally; the
        //    incremental path consults `Row::dirty()`. The inner cell loop
        //    is shared via [`emit_cell`].
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
                emit_cell(cell, &mut out, &mut prev_style)?;
            }
            row_index += 1;
        }

        emit_epilogue(&mut out, &snapshot, terminal)?;

        Ok(SnapshotBytes {
            cols,
            rows: rows_n,
            bytes: out,
        })
    }

    /// Walk `terminal`'s viewport into a structured [`ScreenState`] — the
    /// agent surface's read shape (ADR-0022 §2, `phux-oki`).
    ///
    /// Unlike [`Self::synthesize`], this does not emit VT bytes; it
    /// projects the grid to plain text rows + cursor, exactly what a
    /// reasoning agent (rather than a rendering terminal) wants. It runs
    /// on the server's own `Terminal`, so the read is side-effect-free —
    /// no attach, no resize. `pane` is the wire-local id stamped into the
    /// result for the caller.
    pub fn screen_state(
        &mut self,
        terminal: &GhosttyTerminal<'alloc, '_>,
        pane: u32,
    ) -> Result<ScreenState, SynthesisError> {
        self.screen_state_with_scrollback(terminal, pane, None, false)
    }

    /// Like [`Self::screen_state`], but additionally projects up to
    /// `scrollback` rows of history *above* the viewport into the
    /// [`ScreenState::scrollback`] field (`phux-o1v`).
    ///
    /// `scrollback` semantics:
    /// - `None` — viewport only; `scrollback` is left empty (identical to
    ///   [`Self::screen_state`]).
    /// - `Some(0)` (the [`SCROLLBACK_ALL`] sentinel) — every retained
    ///   history row (the bare `--scrollback` flag).
    /// - `Some(n)` — the most-recent `n` history rows (those nearest the
    ///   viewport); fewer if less history exists.
    ///
    /// History is read cell-by-cell via `Terminal::grid_ref` with
    /// `Point::History` coordinates. That path is side-effect-free: it
    /// neither scrolls the viewport nor mutates the canonical Terminal,
    /// so the read stays safe to poll against a live pane. The viewport
    /// walk is unchanged from [`Self::screen_state`] and still uses the
    /// pooled render iterators.
    ///
    /// When `cells` is `true`, the viewport walk additionally collects a
    /// sparse [`ScreenState::cells`] vec: per-cell OSC-133 semantic marks
    /// (via `Cell::semantic_content`) and styles (via `CellIteration::style`
    /// plus the resolved foreground/background). Only cells with a
    /// non-default style or a semantic mark are emitted, in row-major order,
    /// skipping wide-cell tails — see the private `collect_cell`. When `false`,
    /// `cells` is left `None` and the walk pays nothing (`phux-8yl`).
    pub fn screen_state_with_scrollback(
        &mut self,
        terminal: &GhosttyTerminal<'alloc, '_>,
        pane: u32,
        scrollback: Option<u32>,
        cells: bool,
    ) -> Result<ScreenState, SynthesisError> {
        // Read history first, before borrowing `render_state` for the
        // viewport snapshot: `grid_ref` borrows `terminal` immutably and
        // its references are invalidated by the next terminal operation,
        // so we read each row's text eagerly into owned `String`s here.
        let scrollback_lines = match scrollback {
            None => Vec::new(),
            Some(want) => Self::scrollback_lines(terminal, want)?,
        };

        let snapshot = self.render_state.update(terminal)?;
        let cols = snapshot.cols()?;
        let rows_n = snapshot.rows()?;

        let cursor = snapshot.cursor_viewport()?.map(|c| CursorState {
            x: c.x,
            y: c.y,
            visible: snapshot.cursor_visible().unwrap_or(true),
        });

        // Only allocate the cells vec when the caller asked; the common
        // `--cells`-absent snapshot pays nothing.
        let mut cell_infos: Option<Vec<CellInfo>> = cells.then(Vec::new);

        let mut lines: Vec<String> = Vec::with_capacity(usize::from(rows_n));
        let mut row_iter = self.rows.update(&snapshot)?;
        let mut row_index: u16 = 0;
        while let Some(row) = row_iter.next() {
            if row_index >= rows_n {
                break;
            }
            let mut buf = String::with_capacity(usize::from(cols));
            let mut col_index: u16 = 0;
            let mut cell_iter = self.cells.update(row)?;
            while let Some(cell) = cell_iter.next() {
                let wide = cell.raw_cell()?.wide()?;
                if matches!(wide, CellWide::SpacerTail) {
                    // Wide-cell tail: the base glyph spans both columns and
                    // advances col_index by its display width below, so skip
                    // the tail for both the text and the cells projection
                    // (no column emitted).
                    continue;
                }
                if let Some(infos) = cell_infos.as_mut()
                    && let Some(info) = collect_cell(cell, row_index, col_index)?
                {
                    infos.push(info);
                }
                let graphemes = cell.graphemes()?;
                if graphemes.is_empty() {
                    buf.push(' ');
                } else {
                    buf.extend(graphemes);
                }
                // Advance by the cell's display width so a styled/marked cell
                // to the right of a double-width (CJK/emoji) glyph reports the
                // true grid column — the same space cursor.x lives in. A Wide
                // base occupies two columns; its SpacerTail is skipped above.
                col_index =
                    col_index.saturating_add(if matches!(wide, CellWide::Wide) { 2 } else { 1 });
            }
            lines.push(buf.trim_end().to_owned());
            row_index += 1;
        }

        Ok(ScreenState {
            schema_version: SCHEMA_VERSION,
            pane,
            cols,
            rows: rows_n,
            cursor,
            lines,
            scrollback: scrollback_lines,
            cells: cell_infos,
        })
    }

    /// Read the history (scrollback) rows above the active viewport into
    /// owned, right-trimmed strings, oldest first.
    ///
    /// `want` follows the [`Self::screen_state_with_scrollback`] convention:
    /// [`SCROLLBACK_ALL`] (`0`) means every retained history row, any other
    /// value caps the result to the most-recent `want` rows.
    ///
    /// Each cell is read via [`Terminal::grid_ref`] in the
    /// [`Point::History`] coordinate space, mirroring the viewport walk's
    /// wide-cell-tail handling (`SpacerTail` cells advance no column and
    /// are skipped). History coordinates are local to the history region:
    /// `y = 0` is the oldest retained row, `y = scrollback_rows - 1` is the
    /// row just above the viewport.
    fn scrollback_lines(
        terminal: &GhosttyTerminal<'alloc, '_>,
        want: u32,
    ) -> Result<Vec<String>, SynthesisError> {
        let total = terminal.scrollback_rows()?;
        if total == 0 {
            return Ok(Vec::new());
        }
        let cols = terminal.cols()?;

        // Resolve the [start, total) window of history rows to emit. For a
        // bounded request we keep the rows nearest the viewport (the most
        // recent history), which is what an agent reading "the last N lines
        // of scrollback" expects.
        let start = if want == SCROLLBACK_ALL {
            0
        } else {
            total.saturating_sub(usize::try_from(want).unwrap_or(usize::MAX))
        };

        let mut out: Vec<String> = Vec::with_capacity(total - start);
        for y in start..total {
            // History `y` is a `u32` in libghostty's coordinate space.
            // `total` comes from `scrollback_rows()` (also originally a C
            // count); clamp defensively rather than truncate.
            let y = u32::try_from(y).unwrap_or(u32::MAX);
            let mut buf = String::with_capacity(usize::from(cols));
            for x in 0..cols {
                let point = Point::History(PointCoordinate { x, y });
                let grid_ref = terminal.grid_ref(point)?;
                if matches!(grid_ref.cell()?.wide()?, CellWide::SpacerTail) {
                    continue;
                }
                // Read the grapheme cluster; an empty cluster is a blank
                // cell, which advances one column with a space. A cluster
                // longer than the inline buffer (deep combining sequence)
                // surfaces as `OutOfSpace { required }`; retry on the heap.
                let mut inline = [char::from(0u8); GRAPHEME_INLINE];
                match grid_ref.graphemes(&mut inline) {
                    Ok(0) => buf.push(' '),
                    Ok(n) => buf.extend(&inline[..n]),
                    Err(libghostty_vt::Error::OutOfSpace { required }) => {
                        let mut heap = vec![char::from(0u8); required];
                        let n = grid_ref.graphemes(&mut heap)?;
                        buf.extend(&heap[..n]);
                    }
                    Err(err) => return Err(err.into()),
                }
            }
            out.push(buf.trim_end().to_owned());
        }
        Ok(out)
    }

    /// Mark this consumer's `RenderState` as fully in sync with the
    /// canonical Terminal — clears the snapshot-level dirty state and
    /// every per-row dirty bit.
    ///
    /// Per ADR-0018 (Lazy state synchronization), this is the operation
    /// the tick driver (phux-q0e.3) invokes when a `FRAME_ACK` for the
    /// matching `seq` arrives from the consumer. It is deliberately
    /// **not** called inside [`Self::synthesize_incremental`]: an unacked
    /// diff must remain re-emittable so a lost packet causes the next
    /// tick to re-diff against the same older reference rather than
    /// returning a Clean-but-incorrect empty body.
    ///
    /// After this returns successfully, the next `synthesize_incremental`
    /// against an unchanged terminal will observe `Dirty::Clean` and emit
    /// an empty body, saving wire bytes.
    pub fn mark_synced(
        &mut self,
        terminal: &GhosttyTerminal<'alloc, '_>,
    ) -> Result<(), SynthesisError> {
        let snapshot = self.render_state.update(terminal)?;
        let rows_n = snapshot.rows()?;
        // Walk rows and clear each dirty bit. The row-level clear is
        // separate from the snapshot-level clear — see render.h's "Dirty
        // Tracking" section: both must be reset to bring this consumer
        // back to Clean on the next `update`.
        let mut row_iter = self.rows.update(&snapshot)?;
        let mut row_index: u16 = 0;
        while let Some(row) = row_iter.next() {
            if row_index >= rows_n {
                break;
            }
            row.set_dirty(false)?;
            row_index += 1;
        }
        snapshot.set_dirty(Dirty::Clean)?;
        Ok(())
    }

    /// Synthesize the **incremental** VT diff: the bytes that, applied via
    /// `vt_write` to a mirror that's in sync with the per-consumer
    /// `RenderState`'s last-acked reference, advance the mirror to match
    /// the canonical Terminal now.
    ///
    /// Per ADR-0018 (Lazy state synchronization) and its 2026-05-26
    /// Addendum, this is the per-tick emission primitive. It follows the
    /// 5-step algorithm from `research/archive/2026-05-26-state-sync-algorithm.md`
    /// Dependencies §2:
    ///
    /// 1. `render_state.update(terminal)` to refresh dirty state.
    /// 2. Consult [`Snapshot::dirty`]:
    ///    - `Dirty::Clean` → empty `replay_bytes`.
    ///    - `Dirty::Full` → identical output to [`Self::synthesize`] (fall
    ///      back to the full reset + paint path).
    ///    - `Dirty::Partial` → walk rows, skip those with
    ///      `Row::dirty() == false`, CUP to each dirty row and emit the
    ///      same per-cell loop the full path uses.
    /// 3. Re-emit cursor position + visibility + visual style + mode bits.
    /// 4. **Do not clear dirty bits.** The tick driver (phux-q0e.3)
    ///    clears bits only when a `FRAME_ACK` arrives (phux-q0e.4). An
    ///    unacked diff must remain re-emittable; that is the loss-tolerance
    ///    invariant ADR-0018 rests on.
    pub fn synthesize_incremental(
        &mut self,
        terminal: &GhosttyTerminal<'alloc, '_>,
    ) -> Result<SnapshotBytes, SynthesisError> {
        let snapshot = self.render_state.update(terminal)?;
        let cols = snapshot.cols()?;
        let rows_n = snapshot.rows()?;

        let dirty = snapshot.dirty()?;
        match dirty {
            Dirty::Clean => Ok(SnapshotBytes {
                cols,
                rows: rows_n,
                bytes: Vec::new(),
            }),
            Dirty::Full => {
                // Full reset + paint everything. Identical bytes to the
                // existing [`Self::synthesize`] path; replicate the prologue
                // here rather than re-entering `synthesize` so we keep
                // `render_state` borrowed by `snapshot` for the row walk.
                let mut out: Vec<u8> =
                    Vec::with_capacity(usize::from(cols) * usize::from(rows_n) * 2);
                out.extend_from_slice(b"\x1b[!p\x1b[2J\x1b[H");
                // Select the screen buffer before painting (phux-99n).
                emit_screen_mode(&mut out, terminal)?;

                let mut prev_style: Option<Style> = None;
                let mut row_iter = self.rows.update(&snapshot)?;
                let mut row_index: u16 = 0;
                while let Some(row) = row_iter.next() {
                    if row_index >= rows_n {
                        break;
                    }
                    write_cup(&mut out, row_index, 0);
                    let mut cell_iter = self.cells.update(row)?;
                    while let Some(cell) = cell_iter.next() {
                        emit_cell(cell, &mut out, &mut prev_style)?;
                    }
                    row_index += 1;
                }

                emit_epilogue(&mut out, &snapshot, terminal)?;
                Ok(SnapshotBytes {
                    cols,
                    rows: rows_n,
                    bytes: out,
                })
            }
            Dirty::Partial => {
                // Walk rows; emit only those whose `Row::dirty() == true`.
                // No reset preamble — the mirror's state outside the dirty
                // rows is unchanged.
                let mut out: Vec<u8> = Vec::with_capacity(usize::from(cols) * usize::from(rows_n));
                let mut prev_style: Option<Style> = None;
                let mut row_iter = self.rows.update(&snapshot)?;
                let mut row_index: u16 = 0;
                while let Some(row) = row_iter.next() {
                    if row_index >= rows_n {
                        break;
                    }
                    if !row.dirty()? {
                        row_index += 1;
                        continue;
                    }
                    write_cup(&mut out, row_index, 0);
                    let mut cell_iter = self.cells.update(row)?;
                    while let Some(cell) = cell_iter.next() {
                        emit_cell(cell, &mut out, &mut prev_style)?;
                    }
                    row_index += 1;
                }

                // Always re-emit the cursor + mode epilogue. Cursor
                // position can change without any row being marked dirty
                // (e.g. a bare CUP into a position whose cell is
                // unchanged), and mode bits are diffed flat against the
                // mirror's state, so we re-emit them on every non-empty
                // tick to keep the algorithm simple. This matches the
                // research note's step 3 + 4.
                emit_epilogue(&mut out, &snapshot, terminal)?;

                // CRITICAL: do not call `snapshot.set_dirty(Clean)` or
                // `row.set_dirty(false)` here. The tick driver clears
                // bits only when FRAME_ACK arrives; an unacked diff must
                // stay re-emittable so the next tick can re-diff against
                // the same older reference if this packet is lost.

                Ok(SnapshotBytes {
                    cols,
                    rows: rows_n,
                    bytes: out,
                })
            }
        }
    }

    /// Synthesize the per-consumer incremental diff by comparing the live
    /// `terminal` against a caller-owned [`ConsumerReference`] (phux-ia4).
    ///
    /// # Why this exists (the per-consumer dirty-isolation fix)
    ///
    /// libghostty's `RenderState::update` **consumes** the shared
    /// `Terminal`'s dirty state: it clears `t.flags.dirty`, the active
    /// screen's dirty flags, and the per-page / per-row dirty bits
    /// (`render.zig` `update`, lines ~440-461 and ~647-648 of the pinned
    /// `acc4b87` checkout). A `RenderState`'s own `Snapshot::dirty()` /
    /// `Row::dirty()` are only *populated* from those shared bits during
    /// `update`. So with N consumers sharing one pane, the FIRST
    /// consumer's `update` in a tick consumes the shared dirty bits and
    /// every OTHER consumer's `update` that tick observes `Dirty::Clean`
    /// — starving all-but-one. [`Self::synthesize_incremental`] (which
    /// reads `Snapshot::dirty()`) is therefore only correct for a single
    /// consumer per tick.
    ///
    /// This method sidesteps the shared dirty bits entirely. It renders
    /// each viewport row's cell body into bytes and compares it against
    /// the per-consumer reference's stored row body. Rows whose rendered
    /// body differs from the reference are re-emitted (CUP + cells);
    /// unchanged rows are skipped. The reference is independent per
    /// consumer, so consumers that have diverged (different ack/sync
    /// points, dropped frames) each get their own correct diff regardless
    /// of what any other consumer did to the shared `Terminal` this tick.
    ///
    /// # Emit-once semantics
    ///
    /// On a non-empty diff this advances `reference` to the just-rendered
    /// state *before returning the bytes* (the caller commits by shipping
    /// the frame). A given change is therefore emitted exactly once and
    /// not re-emitted on subsequent ticks until the content changes again.
    /// This matches the v0.1 reliable-transport emission model (the
    /// broadcast pump ships each PTY byte once) and keeps a non-acking
    /// consumer from re-receiving the same diff every tick. The
    /// loss-tolerance "re-diff against an older reference" property
    /// (ADR-0018) belongs to the future lossy-transport path and is not
    /// v0.1 normative (proto.md §8); `FRAME_ACK` remains wired for
    /// backpressure accounting and forward compatibility.
    ///
    /// Returns the synthesized bytes plus the queried `(cols, rows)`. An
    /// empty body means the viewport is byte-identical to the reference.
    pub fn synthesize_against_reference(
        &mut self,
        terminal: &GhosttyTerminal<'alloc, '_>,
        reference: &mut ConsumerReference,
    ) -> Result<SnapshotBytes, SynthesisError> {
        // Where the per-tick, per-consumer CPU goes (row walk + cell render
        // + diff). Debug level so the default `phux=info` filter leaves it
        // disabled and free; when `phux=debug` is captured, the span's CLOSE
        // duration is the headline server-side lag signal, and the recorded
        // `changed_row_count` / `out_bytes` say how much this tick repainted.
        // Declared `Empty` and filled before each return.
        let synth_ref_span = tracing::debug_span!(
            "synthesize_against_reference",
            changed_row_count = tracing::field::Empty,
            out_bytes = tracing::field::Empty,
        )
        .entered();
        let snapshot = self.render_state.update(terminal)?;
        let cols = snapshot.cols()?;
        let rows_n = snapshot.rows()?;

        // Resize the reference to the current geometry. A dimension change
        // (reflow) is handled by clearing the reference so every row is
        // treated as changed: the resize resync path emits a fresh
        // snapshot, but should this diff run mid-resize the safe behavior
        // is a full repaint rather than a stale partial diff.
        if reference.cols != cols || reference.rows != rows_n {
            reference.reset_geometry(cols, rows_n);
        }

        // Render each row's body into the reusable scratch buffer and diff
        // against the reference. A changed row's index is recorded and its
        // freshly-rendered body is committed into the reference immediately
        // by swapping the scratch buffer with the reference's stored body
        // (so the reference's old allocation becomes the next row's scratch
        // — no per-row heap allocation under churn). The `out` diff is then
        // built from the just-committed reference bodies. Emit-once still
        // holds: the reference reflects the rendered state before the frame
        // ships. The walk is scoped so the disjoint `&mut` borrows of the
        // reference's fields (so the per-row swap and the changed-index push
        // don't alias each other through `reference`) drop before the
        // cursor/mode reads below.
        {
            let ConsumerReference {
                rows_body,
                changed_scratch: changed,
                ..
            } = &mut *reference;
            changed.clear();
            let row_scratch = &mut self.row_scratch;
            let mut row_iter = self.rows.update(&snapshot)?;
            let mut row_index: u16 = 0;
            while let Some(row) = row_iter.next() {
                if row_index >= rows_n {
                    break;
                }
                // Each row body is rendered with a fresh SGR pen so the
                // per-row byte sequence is self-contained and comparable
                // across ticks regardless of neighbouring rows.
                row_scratch.clear();
                let mut prev_style: Option<Style> = None;
                let mut cell_iter = self.cells.update(row)?;
                while let Some(cell) = cell_iter.next() {
                    emit_cell(cell, row_scratch, &mut prev_style)?;
                }
                let idx = usize::from(row_index);
                if rows_body[idx] != *row_scratch {
                    // Commit the new body into the reference by swapping; the
                    // displaced (old) buffer is reused as the next row's
                    // scratch.
                    std::mem::swap(&mut rows_body[idx], row_scratch);
                    changed.push(row_index);
                }
                row_index += 1;
            }
        }

        // Re-establish cursor + mode bits. Diff them flat against the
        // reference; cursor placement can move without any row changing
        // (a bare CUP onto an unchanged cell), so we capture the live
        // cursor/mode and compare.
        let live_cm = ReferenceCursorMode::capture(&snapshot, terminal)?;
        let cursor_mode_changed = reference.cursor_mode != live_cm;

        if reference.changed_scratch.is_empty() && !cursor_mode_changed {
            // Byte-identical to the reference: nothing to ship.
            synth_ref_span.record("changed_row_count", 0_usize);
            synth_ref_span.record("out_bytes", 0_usize);
            return Ok(SnapshotBytes {
                cols,
                rows: rows_n,
                bytes: Vec::new(),
            });
        }
        let changed_row_count = reference.changed_scratch.len();

        // Build the diff: per changed row, CUP to its start then the body.
        // No reset preamble — the mirror's untouched rows stay as they are.
        let mut out: Vec<u8> = Vec::new();

        // If the screen buffer changed since the reference (e.g. a program
        // entered/left the alt screen mid-session), toggle it FIRST, before
        // the row paint — same ordering rationale as the full path
        // (phux-99n). We emit the toggle only on an actual transition: an
        // unconditional `?1049h` while already on the alt screen would
        // clear the very buffer we are about to diff into.
        if reference.cursor_mode.alt_screen_set() != live_cm.alt_screen_set() {
            emit_screen_mode(&mut out, terminal)?;
        }

        // The reference bodies already hold the just-rendered state (swapped
        // in above), so read them back for the changed indices. Disjoint
        // immutable borrows of the two fields so the index iteration and the
        // body read don't alias through `reference`.
        let ConsumerReference {
            rows_body,
            changed_scratch: changed,
            ..
        } = &*reference;
        for &ri in changed {
            write_cup(&mut out, ri, 0);
            // Reset SGR at the start of each emitted row so the row body
            // (which itself starts from a fresh pen) lands on a clean pen.
            out.extend_from_slice(b"\x1b[0m");
            out.extend_from_slice(&rows_body[usize::from(ri)]);
        }
        // Always re-emit the cursor/mode epilogue on a non-empty tick (the
        // changed rows moved the cursor as a side effect of painting).
        emit_epilogue(&mut out, &snapshot, terminal)?;

        // Emit-once: the reference row bodies were committed during the
        // walk above (swap); commit the cursor/mode too.
        reference.cursor_mode = live_cm;

        synth_ref_span.record("changed_row_count", changed_row_count);
        synth_ref_span.record("out_bytes", out.len());
        Ok(SnapshotBytes {
            cols,
            rows: rows_n,
            bytes: out,
        })
    }

    /// Prime `reference` to the current `terminal` state without emitting
    /// any bytes (phux-ia4). Used at consumer registration so the first
    /// `synthesize_against_reference` only reports deltas that occur
    /// *after* attach (the `TERMINAL_SNAPSHOT` already brought the
    /// consumer's mirror to this same reference point).
    pub fn prime_reference(
        &mut self,
        terminal: &GhosttyTerminal<'alloc, '_>,
        reference: &mut ConsumerReference,
    ) -> Result<(), SynthesisError> {
        let snapshot = self.render_state.update(terminal)?;
        let cols = snapshot.cols()?;
        let rows_n = snapshot.rows()?;
        reference.reset_geometry(cols, rows_n);

        let mut row_iter = self.rows.update(&snapshot)?;
        let mut row_index: u16 = 0;
        while let Some(row) = row_iter.next() {
            if row_index >= rows_n {
                break;
            }
            let mut body: Vec<u8> = Vec::with_capacity(usize::from(cols));
            let mut prev_style: Option<Style> = None;
            let mut cell_iter = self.cells.update(row)?;
            while let Some(cell) = cell_iter.next() {
                emit_cell(cell, &mut body, &mut prev_style)?;
            }
            reference.rows_body[usize::from(row_index)] = body;
            row_index += 1;
        }
        reference.cursor_mode = ReferenceCursorMode::capture(&snapshot, terminal)?;
        Ok(())
    }
}

/// Convenience wrapper: allocate a fresh [`SnapshotSynthesizer`] for a
/// one-shot synthesis. Per-pane hot loops should reuse a
/// [`SnapshotSynthesizer`].
pub fn synthesize(terminal: &GhosttyTerminal<'_, '_>) -> Result<SnapshotBytes, SynthesisError> {
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

/// Per-cell emission shared by the full ([`SnapshotSynthesizer::synthesize`])
/// and incremental ([`SnapshotSynthesizer::synthesize_incremental`]) paths.
///
/// Tracks the active SGR pen via `prev_style`, skips wide-cell tails
/// (`CellWide::SpacerTail`, see the comment in the body), and emits the
/// cell's grapheme cluster (or a space for genuinely-blank cells).
fn emit_cell(
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
fn encode_graphemes(out: &mut Vec<u8>, graphemes: &[char]) {
    for ch in graphemes {
        let mut buf = [0u8; 4];
        out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
    }
}

/// Project one viewport cell into a [`CellInfo`], or `None` when the cell
/// carries neither a non-default style nor an OSC-133 semantic mark
/// (`phux-8yl`).
///
/// Returning `None` for plain cells keeps the [`ScreenState::cells`] vec
/// sparse: a mostly-blank grid emits almost nothing, so the JSON stays
/// small while every styled or semantically-marked cell is still reported.
/// `row`/`col` are the viewport-relative, zero-based coordinates of the
/// cell's left edge (wide-cell tails are skipped by the caller).
fn collect_cell(
    cell: &CellIteration<'_, '_>,
    row: u16,
    col: u16,
) -> Result<Option<CellInfo>, SynthesisError> {
    let style = cell.style()?;
    // libghostty defaults every cell's semantic content to `Output`,
    // whether or not the shell emitted any OSC-133 marks — so `Output` is
    // the *absence* of a meaningful mark, not a signal. Collapse it to
    // `None` and surface only `Input` / `Prompt`, the marks an agent can
    // actually act on; this also keeps the cells projection sparse (a grid
    // with no shell integration emits no semantic field at all).
    let semantic = match cell.raw_cell()?.semantic_content()? {
        CellSemanticContent::Output => None,
        CellSemanticContent::Input => Some(SemanticContent::Input),
        CellSemanticContent::Prompt => Some(SemanticContent::Prompt),
    };

    // Resolve fg/bg via the iteration's color helpers (which apply the
    // palette/default), falling back to the raw `StyleColor` so a palette
    // index survives as a palette index in the projection.
    let fg = cell_color(cell.fg_color()?, style.fg_color);
    let bg = cell_color(cell.bg_color()?, style.bg_color);

    let cell_style = CellStyle {
        bold: style.bold,
        faint: style.faint,
        italic: style.italic,
        underline: !matches!(style.underline, libghostty_vt::style::Underline::None),
        blink: style.blink,
        inverse: style.inverse,
        invisible: style.invisible,
        strikethrough: style.strikethrough,
        overline: style.overline,
        fg,
        bg,
    };

    // Sparse: drop cells that carry nothing an agent could act on.
    if semantic.is_none() && cell_style == DEFAULT_CELL_STYLE {
        return Ok(None);
    }

    Ok(Some(CellInfo {
        col,
        row,
        semantic,
        style: cell_style,
    }))
}

/// The all-off, all-default [`CellStyle`] — the sentinel `collect_cell`
/// compares against to keep the cells projection sparse.
const DEFAULT_CELL_STYLE: CellStyle = CellStyle {
    bold: false,
    faint: false,
    italic: false,
    underline: false,
    blink: false,
    inverse: false,
    invisible: false,
    strikethrough: false,
    overline: false,
    fg: CellColor::Default,
    bg: CellColor::Default,
};

/// Project a cell color to [`CellColor`].
///
/// Prefers the cell's explicit per-cell [`StyleColor`] so a palette index
/// keeps its identity (`Palette { index }`) rather than collapsing to RGB.
/// When the cell sets no explicit color (`StyleColor::None`) but the
/// iteration still resolves a concrete RGB (`resolved` — e.g. a non-default
/// background inherited from the terminal palette), that RGB is surfaced;
/// otherwise the projection is [`CellColor::Default`].
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

/// Post-row-walk epilogue shared by both synthesis paths: reset SGR,
/// re-establish cursor position + visibility + visual style, and replay
/// the load-bearing mode bits queried from the canonical [`Terminal`].
///
/// Identical to the tail of `synthesize` from before the
/// full/incremental split was introduced.
fn emit_epilogue(
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
fn emit_screen_mode(
    out: &mut Vec<u8>,
    terminal: &GhosttyTerminal<'_, '_>,
) -> Result<(), SynthesisError> {
    emit_mode(out, terminal, Mode::ALT_SCREEN_LEGACY, b"47")?;
    emit_mode(out, terminal, Mode::ALT_SCREEN, b"1047")?;
    emit_mode(out, terminal, Mode::ALT_SCREEN_SAVE, b"1049")?;
    Ok(())
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
fn emit_mode(
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

#[cfg(test)]
#[allow(clippy::expect_used, reason = "tests")]
mod tests {
    use super::*;
    use libghostty_vt::{Terminal as GhosttyTerminal, TerminalOptions};

    fn fresh(cols: u16, rows: u16) -> GhosttyTerminal<'static, 'static> {
        GhosttyTerminal::new(TerminalOptions {
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
    fn render_grid(t: &GhosttyTerminal<'_, '_>) -> Vec<String> {
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
    fn screen_state_projects_text_lines_and_dims() {
        // The agent-surface read path: walk the grid into structured text.
        let mut t = fresh(20, 5);
        t.vt_write(b"hello\r\nworld");
        let mut synth = SnapshotSynthesizer::new().expect("synth");
        let screen = synth.screen_state(&t, 7).expect("screen_state");

        assert_eq!(screen.schema_version, SCHEMA_VERSION);
        assert_eq!(screen.pane, 7, "pane id is stamped from the argument");
        assert_eq!((screen.cols, screen.rows), (20, 5));
        assert_eq!(screen.lines.len(), 5, "one entry per grid row");
        assert_eq!(screen.lines[0], "hello");
        assert_eq!(screen.lines[1], "world");
        // Trailing blank rows trim to empty strings.
        assert_eq!(screen.lines[4], "");
        // Cursor lands just past "world" on row 1 (0-based).
        let cursor = screen.cursor.expect("cursor resolvable in viewport");
        assert_eq!((cursor.x, cursor.y), (5, 1));
    }

    #[test]
    fn screen_state_without_cells_leaves_cells_none() {
        // The default read path (cells = false) must not allocate the
        // cells projection — back-compat with the pre-phux-8yl shape.
        let mut t = fresh(20, 3);
        t.vt_write(b"hello");
        let mut synth = SnapshotSynthesizer::new().expect("synth");
        let screen = synth.screen_state(&t, 1).expect("screen_state");
        assert!(screen.cells.is_none(), "cells = false leaves cells None");
    }

    #[test]
    fn screen_state_cells_collects_styles_sparsely() {
        // A bold-red "HI" followed by plain "ok": the styled cells must
        // surface in the cells projection, the plain cells must NOT (the
        // vec is sparse), and the styled cells must carry the right
        // attributes + RGB-resolved color (phux-8yl).
        let mut t = fresh(20, 2);
        // ESC[1;31m = bold + red fg; "HI"; ESC[0m reset; " ok".
        t.vt_write(b"\x1b[1;31mHI\x1b[0m ok");

        let mut synth = SnapshotSynthesizer::new().expect("synth");
        let screen = synth
            .screen_state_with_scrollback(&t, 1, None, true)
            .expect("screen_state_with_scrollback");
        let cells = screen.cells.expect("cells = true populates Some(..)");

        // Exactly the two styled cells (H, I) are emitted; the blank and
        // the plain "ok" cells are dropped by the sparse filter (no style,
        // no mark).
        assert_eq!(
            cells.len(),
            2,
            "only the two bold-red cells are emitted, got {cells:?}",
        );
        for (i, cell) in cells.iter().enumerate() {
            assert_eq!((cell.row, cell.col), (0, u16::try_from(i).unwrap()));
            assert!(cell.style.bold, "bold cell {i}");
            assert!(!cell.style.italic, "not italic {i}");
            // ANSI `31` is palette slot 1 (red); the explicit per-cell
            // palette index is preserved rather than collapsed to RGB.
            assert_eq!(
                cell.style.fg,
                CellColor::Palette { index: 1 },
                "ANSI red keeps its palette identity",
            );
            assert_eq!(cell.style.bg, CellColor::Default, "no explicit bg");
            assert!(
                cell.semantic.is_none(),
                "no OSC-133 marks written -> Output collapses to None",
            );
        }
    }

    #[test]
    fn screen_state_cells_captures_osc133_semantic_marks() {
        // OSC-133 shell-integration marks classify cells as prompt / input
        // / output. Emit a prompt mark (`OSC 133 ; A`), prompt text, then a
        // command-start mark (`OSC 133 ; B`) and typed input. The cells
        // projection must surface the per-cell semantic content (phux-8yl).
        let mut t = fresh(40, 2);
        // OSC 133 ; A  -> prompt start. Then "$ " is prompt text.
        t.vt_write(b"\x1b]133;A\x07$ ");
        // OSC 133 ; B  -> command (input) start. Then "ls" is input.
        t.vt_write(b"\x1b]133;B\x07ls");

        let mut synth = SnapshotSynthesizer::new().expect("synth");
        let screen = synth
            .screen_state_with_scrollback(&t, 1, None, true)
            .expect("screen_state_with_scrollback");
        let cells = screen.cells.expect("cells = true populates Some(..)");

        // The prompt glyph "$" must carry Prompt; the input glyph "l"/"s"
        // must carry Input. We assert on the marks rather than exact cell
        // counts so the test is robust to how libghostty attributes the
        // trailing space.
        let prompt_marked = cells
            .iter()
            .any(|c| matches!(c.semantic, Some(SemanticContent::Prompt)));
        let input_marked = cells
            .iter()
            .any(|c| matches!(c.semantic, Some(SemanticContent::Input)));
        assert!(
            prompt_marked,
            "OSC-133 ;A region must surface a Prompt cell, got {cells:?}",
        );
        assert!(
            input_marked,
            "OSC-133 ;B region must surface an Input cell, got {cells:?}",
        );
    }

    #[test]
    fn screen_state_cells_reports_true_column_after_wide_glyph() {
        // A double-width CJK glyph occupies two grid columns; libghostty
        // emits its second column as a SpacerTail. The cells projection must
        // advance its column counter by the glyph's full display width so a
        // styled cell to its right reports the true grid column — the same
        // coordinate space cursor.x lives in. Regression: the phux-8yl walk
        // advanced col_index by 1 per cell, under-counting after wide glyphs.
        let mut t = fresh(20, 2);
        // Unstyled wide glyph (你, two columns) then a bold "X" at col 2.
        t.vt_write("你".as_bytes());
        t.vt_write(b"\x1b[1mX");

        let mut synth = SnapshotSynthesizer::new().expect("synth");
        let screen = synth
            .screen_state_with_scrollback(&t, 1, None, true)
            .expect("screen_state_with_scrollback");
        let cells = screen.cells.expect("cells = true populates Some(..)");

        // The wide glyph is unstyled and dropped by the sparse filter, so the
        // bold X is the only emitted cell; it must report col 2, not col 1.
        let x = cells
            .iter()
            .find(|c| c.style.bold)
            .expect("bold X after the wide glyph must be emitted");
        assert_eq!(
            (x.row, x.col),
            (0, 2),
            "styled cell after a double-width glyph must report true column 2, got {cells:?}",
        );
    }

    #[test]
    fn screen_state_cells_accounts_for_spacer_head_at_soft_wrap() {
        // A wide glyph that does not fit in the final column of a row
        // soft-wraps to the next row; libghostty fills the vacated final
        // column with a `CellWide::SpacerHead` (grid width 1, empty
        // grapheme) and places the wide glyph at column 0 of the next row.
        //
        // The cells walk skips `SpacerTail` (a wide glyph's second column)
        // but treats `SpacerHead` as a normal width-1 cell, advancing
        // col_index by 1 — which matches libghostty's column model, where
        // `Cell.gridWidth()` returns 1 for `spacer_head` and 2 only for
        // `wide`. This test pins that accounting across the wrap boundary
        // (phux-ja1).
        //
        // Layout on a 4-column grid for bold "abc你d":
        //   row 0:  a(0) b(1) c(2) SpacerHead(3)
        //   row 1:  你(0,wide) SpacerTail(1) d(2)
        // Bold applies to every cell, so each surfaces in the projection.
        let mut t = fresh(4, 3);
        t.vt_write(b"\x1b[1m");
        t.vt_write("abc你d".as_bytes());

        let mut synth = SnapshotSynthesizer::new().expect("synth");
        let screen = synth
            .screen_state_with_scrollback(&t, 1, None, true)
            .expect("screen_state_with_scrollback");
        let cells = screen.cells.expect("cells = true populates Some(..)");

        // The (row, col) coordinates every bold cell reports. The SpacerHead
        // occupies the soft-wrap row's final column (row 0, col 3): it is a
        // real width-1 cell, not skipped like a SpacerTail, so it both
        // surfaces here and advances col_index by exactly 1.
        let coords: Vec<(u16, u16)> = cells.iter().map(|c| (c.row, c.col)).collect();
        assert_eq!(
            coords,
            vec![(0, 0), (0, 1), (0, 2), (0, 3), (1, 0), (1, 2)],
            "SpacerHead is a width-1 cell at the wrapped row's last column; \
             the wide glyph restarts column accounting at col 0 of the next \
             row and its SpacerTail (row 1, col 1) is skipped, got {cells:?}",
        );

        // The wrap resets column accounting per row: the wide glyph that the
        // SpacerHead displaced lands at col 0 of row 1, and the trailing "d"
        // reports col 2 (the wide glyph advanced two columns, its SpacerTail
        // contributing none). The SpacerHead did not leak a column into the
        // next row.
        assert!(
            cells.iter().any(|c| (c.row, c.col) == (1, 0)),
            "wide glyph wrapped to row 1 must report col 0, got {cells:?}",
        );
        assert!(
            cells.iter().any(|c| (c.row, c.col) == (1, 2)),
            "cell after the wrapped wide glyph must report true col 2, got {cells:?}",
        );
    }

    #[test]
    fn screen_state_with_scrollback_collects_history() {
        // A 3-row viewport with 5 written lines pushes the oldest two into
        // scrollback. Requesting all history (Some(SCROLLBACK_ALL)) must
        // surface them above an unchanged viewport (phux-o1v).
        let mut t = fresh(20, 3);
        t.vt_write(b"line1\r\nline2\r\nline3\r\nline4\r\nline5");
        // Sanity: libghostty must actually be retaining the two scrolled rows.
        assert_eq!(t.scrollback_rows().expect("scrollback_rows"), 2);

        let mut synth = SnapshotSynthesizer::new().expect("synth");
        let screen = synth
            .screen_state_with_scrollback(&t, 7, Some(SCROLLBACK_ALL), false)
            .expect("screen_state_with_scrollback");

        assert_eq!(screen.schema_version, SCHEMA_VERSION);
        assert_eq!(
            screen.scrollback,
            vec!["line1".to_owned(), "line2".to_owned()],
            "all history, oldest first",
        );
        assert_eq!(screen.lines.len(), 3, "viewport stays full height");
        assert_eq!(screen.lines[0], "line3");
        assert_eq!(screen.lines[1], "line4");
        assert_eq!(screen.lines[2], "line5");
    }

    #[test]
    fn screen_state_with_scrollback_bounds_to_recent_rows() {
        // A bounded request keeps the rows nearest the viewport (the most
        // recent history), not the oldest.
        let mut t = fresh(20, 2);
        // 5 lines, 2-row viewport -> 3 rows of scrollback (line1..line3).
        t.vt_write(b"line1\r\nline2\r\nline3\r\nline4\r\nline5");
        assert_eq!(t.scrollback_rows().expect("scrollback_rows"), 3);

        let mut synth = SnapshotSynthesizer::new().expect("synth");
        let screen = synth
            .screen_state_with_scrollback(&t, 1, Some(2), false)
            .expect("screen_state_with_scrollback");

        assert_eq!(
            screen.scrollback,
            vec!["line2".to_owned(), "line3".to_owned()],
            "the most-recent 2 of 3 history rows, oldest-first",
        );
    }

    #[test]
    fn screen_state_without_scrollback_leaves_history_empty() {
        // None must reproduce the legacy viewport-only shape exactly.
        let mut t = fresh(20, 3);
        t.vt_write(b"a\r\nb\r\nc\r\nd\r\ne");
        assert!(t.scrollback_rows().expect("scrollback_rows") > 0);

        let mut synth = SnapshotSynthesizer::new().expect("synth");
        let none = synth
            .screen_state_with_scrollback(&t, 0, None, false)
            .expect("with None");
        let legacy = synth.screen_state(&t, 0).expect("screen_state");

        assert!(none.scrollback.is_empty(), "no scrollback requested");
        assert_eq!(none.lines, legacy.lines, "viewport unchanged by None path");
        assert!(legacy.scrollback.is_empty());
    }

    #[test]
    fn screen_state_with_scrollback_empty_when_no_history() {
        // Requesting scrollback on a pane with no history yields an empty
        // vec, not an error.
        let mut t = fresh(20, 5);
        t.vt_write(b"only one line");
        assert_eq!(t.scrollback_rows().expect("scrollback_rows"), 0);

        let mut synth = SnapshotSynthesizer::new().expect("synth");
        let screen = synth
            .screen_state_with_scrollback(&t, 0, Some(SCROLLBACK_ALL), false)
            .expect("screen_state_with_scrollback");
        assert!(screen.scrollback.is_empty());
        assert_eq!(screen.lines[0], "only one line");
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

    /// Companion to `synthesizer_skips_wide_cell_tails`: exercise the
    /// wide-tail discriminator across a heterogeneous mix of CJK,
    /// emoji, and ASCII on multiple rows. Each emoji codepoint
    /// (e.g. `😀`, U+1F600) is double-width and produces a
    /// `CellWide::SpacerTail` neighbor in libghostty's grid, the same
    /// way CJK does. The round-trip must reproduce the source grid
    /// cell-for-cell.
    #[test]
    fn synthesizer_round_trips_cjk_and_emoji() {
        let mut a = fresh(20, 4);
        // Row 0: CJK + ASCII. Row 1: pure emoji. Row 2: mixed
        // emoji/ASCII. The CRLF sequences keep row layout deterministic.
        a.vt_write("東 hello\r\n".as_bytes());
        a.vt_write("😀😀😀\r\n".as_bytes());
        a.vt_write("a😀b".as_bytes());

        let src_grid = render_grid(&a);

        let synth = synthesize(&a).expect("synth");
        let mut b = fresh(synth.cols, synth.rows);
        b.vt_write(&synth.bytes);

        let dst_grid = render_grid(&b);
        assert_eq!(
            src_grid, dst_grid,
            "CJK + emoji content must round-trip through the synthesizer"
        );

        // The source row containing `東` must carry the literal glyph,
        // not a leading space that would indicate the wide tail leaked
        // into the leading position.
        assert!(
            src_grid[0].starts_with('東'),
            "source row 0 should start with 東, got {:?}",
            src_grid[0]
        );
    }

    /// phux-99n: a snapshot taken while a 1049-alt-screen program
    /// (vim/less/man/htop/tmux) is running MUST re-establish the alt
    /// screen via `?1049h` so the receiving mirror lands on the alt
    /// buffer — and must NOT force the primary screen via a stale `?47l`.
    ///
    /// This is the FLIP of the audit pin test
    /// `audit_snapshot_drops_alt_screen_1049_mode`: the pin asserted the
    /// buggy status quo (`?1049h` absent, `?47l` emitted); we assert the
    /// fix. 1049 also saves the cursor + clears on entry, so it must be
    /// emitted BEFORE the cursor re-establishment — verified by checking
    /// `?1049h` precedes the cursor-home/CUP in the byte stream.
    #[test]
    fn snapshot_reestablishes_alt_screen_1049() {
        let mut t = fresh(20, 4);
        t.vt_write(b"\x1b[?1049h");
        t.vt_write(b"alt-screen body");

        // libghostty keeps 47 and 1049 as distinct bits.
        assert!(
            t.mode(Mode::ALT_SCREEN_SAVE).expect("mode 1049"),
            "?1049h should set ALT_SCREEN_SAVE",
        );
        assert!(
            !t.mode(Mode::ALT_SCREEN_LEGACY).expect("mode 47"),
            "1049 must not set the legacy-47 bit",
        );

        let snap = synthesize(&t).expect("synth");
        let bytes = String::from_utf8_lossy(&snap.bytes);

        assert!(
            bytes.contains("?1049h"),
            "snapshot must re-emit ?1049h so the mirror lands on the alt screen; bytes={bytes:?}",
        );
        // 47 is off, so the snapshot reports its true (off) state. The bug
        // was emitting ?47l as the ONLY alt-screen signal; now ?1049h
        // carries the screen and ?47l is merely the honest 47 state.
        assert!(
            !bytes.contains("?47h"),
            "47 is off; snapshot must not assert ?47h; bytes={bytes:?}",
        );
        // Ordering: 1049 saves cursor + clears on entry, so it must come
        // before the epilogue's cursor re-establishment, else the CUP we
        // emit to restore the cursor is clobbered by the screen switch.
        // The epilogue's cursor-visibility CSI (`?25h`/`?25l`) is emitted
        // immediately after that CUP and only there, so it is a reliable
        // landmark for "the cursor has been re-established".
        let pos_1049 = bytes.find("?1049h").expect("?1049h present");
        let pos_cursor_vis = bytes
            .find("?25h")
            .or_else(|| bytes.find("?25l"))
            .expect("epilogue cursor-visibility present");
        assert!(
            pos_1049 < pos_cursor_vis,
            "?1049h (at {pos_1049}) must precede the epilogue cursor block (at {pos_cursor_vis}); bytes={bytes:?}",
        );
    }

    /// phux-99n: the legacy 47 alt-screen mode still round-trips. A
    /// program that uses bare `?47h` (rare, but valid) must have the
    /// snapshot re-emit `?47h`, not silently drop it.
    #[test]
    fn snapshot_reestablishes_alt_screen_47_legacy() {
        let mut t = fresh(20, 4);
        t.vt_write(b"\x1b[?47h");
        t.vt_write(b"legacy alt");
        assert!(t.mode(Mode::ALT_SCREEN_LEGACY).expect("mode 47"));

        let snap = synthesize(&t).expect("synth");
        let bytes = String::from_utf8_lossy(&snap.bytes);
        assert!(
            bytes.contains("?47h"),
            "snapshot must re-emit ?47h for a legacy alt-screen program; bytes={bytes:?}",
        );
    }

    /// phux-99n: on the PRIMARY screen the snapshot must report all three
    /// alt-screen modes as off (`?47l ?1047l ?1049l`) — i.e. it must not
    /// accidentally assert any alt-screen mode.
    #[test]
    fn snapshot_primary_screen_emits_all_alt_modes_off() {
        let mut t = fresh(20, 4);
        t.vt_write(b"primary content");

        let snap = synthesize(&t).expect("synth");
        let bytes = String::from_utf8_lossy(&snap.bytes);
        assert!(bytes.contains("?47l"), "47 off on primary; bytes={bytes:?}");
        assert!(
            bytes.contains("?1047l"),
            "1047 off on primary; bytes={bytes:?}",
        );
        assert!(
            bytes.contains("?1049l"),
            "1049 off on primary; bytes={bytes:?}",
        );
    }

    /// phux-99n: the per-consumer reference diff trips on a 47<->1049
    /// transition. The audit noted that tracking only the legacy-47 bit in
    /// `ReferenceCursorMode` would miss a transition between the two
    /// distinct alt-screen modes. After priming on 1049, switching to the
    /// primary screen must produce a non-empty diff that re-emits
    /// `?1049l`.
    #[test]
    fn reference_diff_trips_on_alt_screen_transition() {
        let mut t = fresh(20, 4);
        t.vt_write(b"\x1b[?1049h");
        t.vt_write(b"alt body");

        let mut synth = SnapshotSynthesizer::new().expect("synth");
        let mut reference = ConsumerReference::new();
        synth
            .prime_reference(&t, &mut reference)
            .expect("prime_reference");

        // Leave the alt screen: a mode-only change, no row content edits
        // beyond what 1049's restore does.
        t.vt_write(b"\x1b[?1049l");

        let diff = synth
            .synthesize_against_reference(&t, &mut reference)
            .expect("diff");
        assert!(
            !diff.bytes.is_empty(),
            "a 1049->primary transition must produce a non-empty diff",
        );
        let bytes = String::from_utf8_lossy(&diff.bytes);
        assert!(
            bytes.contains("?1049l"),
            "the diff epilogue must re-emit ?1049l on leaving the alt screen; bytes={bytes:?}",
        );
    }

    /// phux-4l0 (correctness half): an unchanged terminal diffs to an
    /// empty body across repeated calls (emit-once / steady state). The
    /// behavioral idle short-circuit lives in the actor's `tick_emit`
    /// (see `terminal_actor.rs`); this pins the synthesis-level invariant
    /// the short-circuit relies on — a clean terminal yields nothing.
    #[test]
    fn reference_diff_empty_when_unchanged() {
        let mut t = fresh(40, 10);
        t.vt_write(b"steady state line one\r\nand line two");

        let mut synth = SnapshotSynthesizer::new().expect("synth");
        let mut reference = ConsumerReference::new();
        synth
            .prime_reference(&t, &mut reference)
            .expect("prime_reference");

        for n in 0..3 {
            let diff = synth
                .synthesize_against_reference(&t, &mut reference)
                .expect("diff");
            assert!(
                diff.bytes.is_empty(),
                "unchanged terminal must diff empty on call {n}, got {:?}",
                String::from_utf8_lossy(&diff.bytes),
            );
        }
    }
}
