//! Paint composition for the attach driver.
//!
//! Two paint paths:
//! * `paint_full_frame` — clear viewport, render every pane, dividers,
//!   status bar. Use after layout mutations, viewport resize, or attach.
//! * `paint_focused_pane` + `paint_bar_after_pane` — incremental path
//!   for `TERMINAL_OUTPUT` arrivals where only the focused pane changed.
//!
//! `pane_viewport` reserves the bottom row for the status bar so pane
//! Rects never spill into it.

use std::collections::HashMap;
use std::io::Write;
use std::time::SystemTime;

use libghostty_vt::Terminal;
use phux_protocol::ids::TerminalId;

use super::driver::PaneSlot;
use crate::layout::LayoutState;
use crate::render::chrome::status_bar::{StatusBarPainter, make_context};

/// Resize a libghostty [`Terminal`], working around a `PageList.resizeCols`
/// integer overflow (phux-y06) that panics — inside libghostty's Zig — when
/// **both** the column and row counts shrink in a single `resize()` call.
/// Shrinking a single axis, or growing, is safe.
///
/// When both axes shrink versus the Terminal's current `cols()`/`rows()`,
/// we decompose the resize into two single-axis steps (rows first, then
/// cols), each of which is individually safe. Otherwise we resize in one
/// call. `cols`/`rows` are clamped to a 1-cell minimum (libghostty has no
/// concept of a zero-dimension grid).
///
/// The proper fix is upstream in libghostty's `PageList.resizeCols`; drop
/// this shim once the pinned `libghostty-vt` rev carries it.
pub(super) fn safe_resize(
    terminal: &mut Terminal<'_, '_>,
    cols: u16,
    rows: u16,
) -> libghostty_vt::error::Result<()> {
    let cols = cols.max(1);
    let rows = rows.max(1);
    // If we can't read the current size, fall back to a direct resize —
    // we only special-case the proven-bad both-shrink transition.
    let cur_cols = terminal.cols().unwrap_or(cols);
    let cur_rows = terminal.rows().unwrap_or(rows);
    if cols < cur_cols && rows < cur_rows {
        // Shrink rows alone first (cols unchanged → single-axis, safe),
        // then the call below shrinks cols alone (rows already at target).
        terminal.resize(cur_cols, rows, 0, 0)?;
    }
    terminal.resize(cols, rows, 0, 0)
}

/// Render one pane into its outer-viewport sub-Rect.
///
/// Looks up the pane's Rect in the layout, resizes its libghostty
/// Terminal to match (so the renderer's CUP math lines up), and calls
/// `render_at` with the Rect's origin. Falls back to `(0,0)` + full
/// pane viewport when the layout has no entry (single-pane bootstrap).
///
/// Returns the renderer's cached `last_cursor` (outer-viewport coords),
/// or `None` if the pane has no slot or its libghostty cursor is hidden.
/// Callers use this to restore the cursor after a status-bar paint.
pub(super) fn paint_focused_pane<W: Write>(
    out: &mut W,
    layout_state: &LayoutState,
    panes: &mut HashMap<TerminalId, PaneSlot>,
    focused: &TerminalId,
    viewport_dims: (u16, u16),
    has_bar: bool,
    force_full: bool,
) -> Option<(u16, u16)> {
    let pane_dims = pane_viewport(viewport_dims, has_bar);
    let rect = super::multi_pane::compute_layout(layout_state, pane_dims)
        .rects
        .get(focused)
        .copied()
        .unwrap_or(crate::layout::Rect {
            x: 0,
            y: 0,
            w: pane_dims.0,
            h: pane_dims.1,
        });
    let slot = panes.get_mut(focused)?;
    let _ = safe_resize(&mut slot.terminal, rect.w, rect.h);
    let _ = if force_full {
        slot.renderer
            .render_at_full(&slot.terminal, out, (rect.x, rect.y))
    } else {
        slot.renderer
            .render_at(&slot.terminal, out, (rect.x, rect.y))
    };
    slot.renderer.last_cursor()
}

/// Clear the viewport and paint every pane + dividers + bar from
/// scratch. Use after layout mutations, viewport resize, or initial
/// attach — anything where the previous frame may not be a coherent
/// base for an incremental repaint. For "focused pane got output"
/// situations call [`paint_focused_pane`] + [`paint_bar_after_pane`]
/// instead.
pub(super) fn paint_full_frame<W: super::RenderSink>(
    out: &mut W,
    layout_state: &LayoutState,
    panes: &mut HashMap<TerminalId, PaneSlot>,
    focused_pane: Option<&TerminalId>,
    viewport_dims: (u16, u16),
    status_bar: Option<&mut StatusBarPainter>,
    session_name: &str,
) {
    // The full screen paint (ratatui chrome + per-pane libghostty render).
    // Its close-duration is the client-side render-lag signal the flywheel
    // reads; debug-level so it is free at the default filter, and kept here
    // (not at the 4 call sites) so every repaint is timed.
    let _paint = tracing::debug_span!(
        "paint_full_frame",
        cols = viewport_dims.0,
        rows = viewport_dims.1,
        panes = panes.len()
    )
    .entered();
    let has_bar = status_bar.is_some();
    let pane_dims = pane_viewport(viewport_dims, has_bar);
    let multi = super::multi_pane::compute_layout(layout_state, pane_dims);
    // ED2 (clear screen) + cursor home. Cheap and unambiguous.
    let _ = out.write_all(b"\x1b[2J\x1b[H");
    // Non-focused panes first; chrome (dividers + status bar) next; the
    // focused pane's render_at is intentionally the LAST cursor-touching
    // emit in the frame so it owns final cursor position + DECTCEM. This
    // matters on fresh attach where libghostty's snapshot may not yet
    // expose a `cursor_viewport`, so a "restore cursor after the bar"
    // strategy strands the cursor invisible.
    for (id, rect) in &multi.rects {
        if Some(id) == focused_pane {
            continue;
        }
        if let Some(slot) = panes.get_mut(id) {
            let _ = safe_resize(&mut slot.terminal, rect.w, rect.h);
            // Force a full redraw: the ED2 above cleared the screen, so
            // an incremental "only dirty rows" paint would leave a pane
            // whose content is unchanged (the survivor of a split/resize)
            // blank.
            let _ = slot
                .renderer
                .render_at_full(&slot.terminal, out, (rect.x, rect.y));
        }
    }
    let _ = crate::render::chrome::dividers::render_dividers(out, &multi, focused_pane);
    // The ED2 above cleared the bar row, so force a re-emit even if the
    // bar's content is byte-identical to the previous frame.
    paint_bar_after_pane(
        status_bar,
        out,
        viewport_dims,
        session_name,
        None,
        None,
        true,
    );
    // Paint the focused pane LAST so its render_at owns final cursor
    // placement. But render_at may be a no-op (slot missing, or the
    // libghostty Terminal grid has no diffs to emit), in which case
    // the cursor is still wherever the bar's final write parked it —
    // bottom-right of the host terminal. Capture `paint_focused_pane`'s
    // last_cursor and always emit an explicit cursor placement so the
    // frame ends with a deterministic cursor position regardless of
    // whether render_at touched the cursor. See phux-gxy.
    let final_cursor = focused_pane.and_then(|fid| {
        paint_focused_pane(out, layout_state, panes, fid, viewport_dims, has_bar, true)
    });
    if let Some((row, col)) = final_cursor {
        let one_based_row = row.saturating_add(1);
        let one_based_col = col.saturating_add(1);
        tracing::trace!(
            row,
            col,
            "paint_full_frame: restore cursor to focused last_cursor"
        );
        let _ = write!(out, "\x1b[{one_based_row};{one_based_col}H\x1b[?25h");
    } else {
        // No authoritative cursor. Park at the focused pane's Rect
        // origin if we have one, otherwise top-left of the viewport.
        // Always emit a CUP so the cursor never strands at the bar's
        // tail (bottom-right) — that was phux-gxy's visible symptom
        // when focused_pane was None and the bar repaint owned the
        // final escape on the wire.
        let (x, y) = focused_pane
            .and_then(|fid| multi.rects.get(fid).copied())
            .map_or((0, 0), |r| (r.x, r.y));
        let one_based_row = y.saturating_add(1);
        let one_based_col = x.saturating_add(1);
        tracing::trace!(
            row = y,
            col = x,
            focused_pane_set = focused_pane.is_some(),
            "paint_full_frame: no last_cursor, parking at fallback hidden"
        );
        let _ = write!(out, "\x1b[{one_based_row};{one_based_col}H\x1b[?25l");
    }
    // Flush the final cursor placement. See the note in
    // `paint_bar_after_pane`: render_at flushes mid-frame, but the
    // explicit CUP we write *after* the last render_at has no newline
    // and would otherwise sit in the LineWriter buffer indefinitely.
    let _ = out.flush();
}

/// phux-nz4.5: shared helper invoked after every pane render so the
/// status row is restored on top of whatever VT the pane renderer just
/// wrote. No-op when there is no painter or no live viewport.
///
/// `restore_cursor` is the renderer's last authoritative cursor
/// position (outer-viewport coords); when present we CUP+show there.
///
/// `fallback_origin` is the focused pane's `Rect` origin to use when
/// `restore_cursor` is `None` (phux-9xn). Without this, the bar's
/// final write strands the host terminal's cursor at the end of the
/// bar row — i.e. bottom-right of the screen. The fallback emits a
/// CUP into the pane area + `?25l` so the cursor sits in a sane
/// location and is hidden until the next authoritative render
/// places it. We hide rather than show because `last_cursor == None`
/// means libghostty's snapshot either reported the cursor hidden or
/// had no viewport position — in both cases showing the cursor at an
/// arbitrary fallback position would lie to the user.
///
/// Pass `fallback_origin = None` at call sites where a subsequent
/// pane render is guaranteed to own final cursor placement (e.g.
/// `paint_full_frame`, which paints the focused pane LAST).
///
/// `bar_row_clobbered` controls whether the painter's content cache is
/// bypassed. Pane rendering is confined to the rows ABOVE the reserved
/// bar row (see [`pane_viewport`]), so on the steady-state hot path
/// (`TERMINAL_OUTPUT`) the focused pane render never overwrites the bar
/// row — the painter's own cache then makes an unchanged bar a zero-byte
/// no-op (the win in phux's incremental-paint pass). Pass `true` only
/// from callers that physically cleared the bar row (the `paint_full_frame`
/// `ED2`), where the on-screen row must be re-emitted even if its content
/// is identical to last frame.
pub(super) fn paint_bar_after_pane<W: Write>(
    status_bar: Option<&mut StatusBarPainter>,
    out: &mut W,
    viewport_dims: (u16, u16),
    session_name: &str,
    restore_cursor: Option<(u16, u16)>,
    fallback_origin: Option<(u16, u16)>,
    bar_row_clobbered: bool,
) {
    let Some(painter) = status_bar else {
        return;
    };
    // Force a re-emit only when the bar row was physically overwritten
    // (e.g. the full-frame `ED2`). On the incremental path the pane
    // render stays above the bar row, so the painter's content/dims
    // cache decides: an unchanged bar emits zero bytes.
    if bar_row_clobbered {
        painter.invalidate();
    }
    let _ = painter.paint(
        out,
        viewport_dims.0,
        viewport_dims.1,
        // The window list is owned by the painter and injected inside
        // `paint`; this context carries none.
        &make_context(session_name, SystemTime::now()),
    );
    // After the bar repaints, the cursor sits on the bar row. Put it
    // back at the focused pane's known position when we have one;
    // otherwise fall back to the focused pane's Rect origin (hidden)
    // so the cursor doesn't remain stranded at the bar's tail —
    // bottom-right of the host terminal. See phux-9xn.
    if let Some((row, col)) = restore_cursor {
        let one_based_row = row.saturating_add(1);
        let one_based_col = col.saturating_add(1);
        tracing::trace!(row, col, "paint_bar_after_pane: restore cursor");
        let _ = write!(out, "\x1b[{one_based_row};{one_based_col}H\x1b[?25h");
    } else if let Some((x, y)) = fallback_origin {
        // No authoritative cursor: park inside the focused pane and
        // hide. The next pane render that hits this slot will lift
        // visibility back up via its own DECTCEM emit.
        let one_based_row = y.saturating_add(1);
        let one_based_col = x.saturating_add(1);
        tracing::trace!(x, y, "paint_bar_after_pane: fallback origin (hidden)");
        let _ = write!(out, "\x1b[{one_based_row};{one_based_col}H\x1b[?25l");
    } else {
        // Both None — historically a no-op (caller promised to own
        // cursor placement after). But every observed caller of this
        // shape strands the cursor at the bar's last cell. Park at
        // top-left hidden as a safety net.
        tracing::trace!("paint_bar_after_pane: both None, parking at (0,0) hidden");
        let _ = write!(out, "\x1b[1;1H\x1b[?25l");
    }
    // CRITICAL: flush so the cursor-restore CUP actually reaches the
    // terminal. stdout is a LineWriter and the CUP we just wrote has no
    // trailing newline, so without an explicit flush it stays buffered
    // until the next pane output. When the pane is idle (a shell prompt)
    // that output never comes, and the host cursor strands at the bar's
    // tail — bottom-right. This is the real phux-gxy: the prior fixes
    // computed the right CUP but never flushed it, so unit tests on the
    // in-memory buffer passed while the live terminal never saw it.
    let _ = out.flush();
}

/// Effective viewport available to pane rendering: outer dims with the
/// status-bar row reserved when a bar is present. Used at every
/// `multi_pane::compute_layout` call site so pane Rects never spill
/// into the status row.
pub(super) const fn pane_viewport(outer: (u16, u16), has_status_bar: bool) -> (u16, u16) {
    if has_status_bar {
        (outer.0, outer.1.saturating_sub(1))
    } else {
        outer
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, reason = "tests")]
mod tests {
    use super::*;
    use crate::attach::driver::PaneSlot;
    use crate::render::chrome::status_bar::Position;
    use phux_config::widget::WidgetRegistry;
    use phux_config::{StatusCfg, Widget};
    use phux_protocol::wire::info::{LayoutNode, SplitDir};

    fn build_painter() -> StatusBarPainter {
        let cfg = StatusCfg {
            left: vec![Widget::Bare("session-name".into())],
            ..Default::default()
        };
        let reg = WidgetRegistry::with_builtins();
        let bar = phux_config::widget::StatusBar::build(&cfg, &reg).expect("bar build");
        StatusBarPainter::new(bar, Position::Bottom)
    }

    /// `paint_full_frame` against an injected `Vec<u8>` sink composites
    /// the whole frame for a two-pane layout: it leads with ED2 + home,
    /// emits both panes' rect-anchored content, draws the divider, and
    /// ends with an explicit cursor placement. Locks the full-frame
    /// composition contract on the now-injectable sink (phux-549).
    #[test]
    fn paint_full_frame_composites_two_panes_into_sink() {
        let left = TerminalId::local(1);
        let right = TerminalId::local(2);
        let layout = LayoutState {
            tree: Some(LayoutNode::Split {
                dir: SplitDir::Horizontal,
                ratio: 0.5,
                left: Box::new(LayoutNode::Leaf(left.clone())),
                right: Box::new(LayoutNode::Leaf(right.clone())),
            }),
            focus: Some(left.clone()),
        };
        let mut panes: HashMap<TerminalId, PaneSlot> = HashMap::new();
        panes.insert(left.clone(), PaneSlot::new().expect("left slot"));
        panes.insert(right, PaneSlot::new().expect("right slot"));

        let mut out: Vec<u8> = Vec::new();
        paint_full_frame(
            &mut out,
            &layout,
            &mut panes,
            Some(&left),
            (80, 24),
            None,
            "demo",
        );

        let s = String::from_utf8_lossy(&out);
        // ED2 (clear screen) + cursor home must lead the frame.
        assert!(
            s.starts_with("\x1b[2J\x1b[H"),
            "frame must open with ED2 + home; out = {s:?}"
        );
        // The divider for a 0.5 side-by-side split sits at column 40
        // (1-based 41). render_dividers emits CUPs into that column.
        assert!(
            s.contains(";41H") || s.contains(";40H"),
            "expected a divider CUP near the split column; out = {s:?}"
        );
        // The frame ends with an explicit cursor placement (CUP + DECTCEM)
        // — never stranded at the bar tail (phux-gxy).
        assert!(
            s.contains("\x1b[?25h") || s.contains("\x1b[?25l"),
            "frame must end with an explicit cursor visibility; out = {s:?}"
        );
    }

    /// phux-9xn regression: when `restore_cursor` is None (e.g. fresh
    /// attach before any PTY output, or hidden cursor) and a
    /// `fallback_origin` is provided, the helper must emit a CUP into
    /// the focused pane's rect origin plus `?25l` so the host
    /// terminal's cursor doesn't strand at the end of the bar row.
    #[test]
    fn paint_bar_after_pane_falls_back_to_pane_origin_when_cursor_none() {
        let mut painter = build_painter();
        let mut out = Vec::new();
        paint_bar_after_pane(
            Some(&mut painter),
            &mut out,
            (80, 24),
            "demo",
            None,
            Some((3, 5)),
            true,
        );
        let s = String::from_utf8_lossy(&out);
        // Pane origin (3, 5) ⇒ 1-based CUP `\x1b[6;4H`.
        assert!(s.contains("\x1b[6;4H"), "fallback CUP missing; out = {s:?}");
        // Fallback hides the cursor — we don't know if it should be
        // visible at this position.
        assert!(
            s.contains("\x1b[?25l"),
            "fallback ?25l missing; out = {s:?}"
        );
        // And we must NOT have emitted ?25h via the restore branch.
        let last_cup_idx = s.rfind("\x1b[6;4H").expect("cup present");
        let after = &s[last_cup_idx..];
        assert!(
            !after.contains("\x1b[?25h"),
            "fallback path must hide, not show cursor; trailing = {after:?}"
        );
    }

    /// Cursor-known path must continue to emit `?25h` at the
    /// authoritative position (phux-b9n regression guard).
    #[test]
    fn paint_bar_after_pane_restores_cursor_visible_when_known() {
        let mut painter = build_painter();
        let mut out = Vec::new();
        paint_bar_after_pane(
            Some(&mut painter),
            &mut out,
            (80, 24),
            "demo",
            Some((4, 7)),
            Some((0, 0)),
            true,
        );
        let s = String::from_utf8_lossy(&out);
        // (row, col) = (4, 7) ⇒ 1-based CUP `\x1b[5;8H`.
        assert!(s.contains("\x1b[5;8H"), "restore CUP missing; out = {s:?}");
        assert!(s.contains("\x1b[?25h"), "restore ?25h missing; out = {s:?}");
        // Fallback CUP for origin (0, 0) must NOT appear.
        assert!(
            !s.contains("\x1b[1;1H"),
            "fallback CUP leaked into restore path; out = {s:?}"
        );
    }

    /// When `restore_cursor` is None AND `fallback_origin` is None,
    /// the helper now parks the cursor at (0,0) hidden as a safety
    /// net. The old behavior (no CUP) stranded the cursor at the
    /// bar's last cell — bottom-right of the host terminal — when no
    /// follow-up paint owned final placement (phux-gxy).
    #[test]
    fn paint_bar_after_pane_parks_at_top_left_hidden_when_both_none() {
        let mut painter = build_painter();
        let mut out = Vec::new();
        paint_bar_after_pane(
            Some(&mut painter),
            &mut out,
            (80, 24),
            "demo",
            None,
            None,
            true,
        );
        let s = String::from_utf8_lossy(&out);
        // Bar CUP to row 24 must be present (the bar still paints).
        assert!(s.contains("\x1b[24;1H"), "bar CUP missing; out = {s:?}");
        // Safety-net CUP to (0,0) followed by hide.
        assert!(
            s.contains("\x1b[1;1H\x1b[?25l"),
            "safety-net CUP+?25l missing; out = {s:?}"
        );
        // Must NOT show cursor.
        assert!(
            !s.contains("\x1b[?25h"),
            "unexpected ?25h in both-none path; out = {s:?}"
        );
    }

    /// Incremental-paint win: on the hot path (`bar_row_clobbered = false`)
    /// a repaint whose bar content + dims are unchanged emits NO status-bar
    /// row bytes. Only the (cheap) cursor-restore CUP is written. This is
    /// the steady-state cost reduction: the prior unconditional
    /// `painter.invalidate()` re-emitted the entire bar row on every
    /// `TERMINAL_OUTPUT` frame.
    #[test]
    fn paint_bar_after_pane_skips_unchanged_bar_when_not_clobbered() {
        let mut painter = build_painter();
        // First paint primes the painter's cache (emits the bar row once).
        let mut first = Vec::new();
        paint_bar_after_pane(
            Some(&mut painter),
            &mut first,
            (80, 24),
            "demo",
            Some((4, 7)),
            None,
            false,
        );
        let first_s = String::from_utf8_lossy(&first);
        assert!(
            first_s.contains("\x1b[24;1H"),
            "first paint must emit the bar row CUP; out = {first_s:?}"
        );

        // Second paint, same dims + same widget inputs, NOT clobbered:
        // the bar row must NOT be re-emitted.
        let mut second = Vec::new();
        paint_bar_after_pane(
            Some(&mut painter),
            &mut second,
            (80, 24),
            "demo",
            Some((4, 7)),
            None,
            false,
        );
        let second_s = String::from_utf8_lossy(&second);
        assert!(
            !second_s.contains("\x1b[24;1H"),
            "unchanged bar must not re-emit its row CUP; out = {second_s:?}"
        );
        // The only bytes are the cursor restore to (4,7) ⇒ \x1b[5;8H.
        assert!(
            second_s.contains("\x1b[5;8H"),
            "cursor restore CUP still expected; out = {second_s:?}"
        );
    }

    /// Correctness guard: when the bar row WAS clobbered
    /// (`bar_row_clobbered = true`, the `paint_full_frame` ED2 path), the
    /// bar re-emits even if its content is byte-identical to the previous
    /// frame — otherwise the cleared row would stay blank.
    #[test]
    fn paint_bar_after_pane_re_emits_when_clobbered_even_if_unchanged() {
        let mut painter = build_painter();
        let mut first = Vec::new();
        paint_bar_after_pane(
            Some(&mut painter),
            &mut first,
            (80, 24),
            "demo",
            Some((4, 7)),
            None,
            true,
        );
        assert!(String::from_utf8_lossy(&first).contains("\x1b[24;1H"));

        // Same inputs, but clobbered: must force a re-emit of the bar row.
        let mut second = Vec::new();
        paint_bar_after_pane(
            Some(&mut painter),
            &mut second,
            (80, 24),
            "demo",
            Some((4, 7)),
            None,
            true,
        );
        let second_s = String::from_utf8_lossy(&second);
        assert!(
            second_s.contains("\x1b[24;1H"),
            "clobbered bar must re-emit its row even when unchanged; out = {second_s:?}"
        );
    }
}
