//! Paint composition for the attach driver.
//!
//! Two paint paths:
//! * [`paint_full_frame`] — clear viewport, render every pane, dividers,
//!   status bar. Use after layout mutations, viewport resize, or attach.
//! * [`paint_focused_pane`] + [`paint_bar_after_pane`] — incremental
//!   path for `TERMINAL_OUTPUT` arrivals where only the focused pane
//!   changed.
//!
//! [`pane_viewport`] reserves the bottom row for the status bar so pane
//! Rects never spill into it.

use std::collections::HashMap;
use std::io::{self, Write};
use std::time::SystemTime;

use phux_protocol::ids::TerminalId;

use super::driver::PaneSlot;
use super::status_bar::{StatusBarPainter, make_context};
use crate::layout::LayoutState;

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
    let _ = slot.terminal.resize(rect.w.max(1), rect.h.max(1), 0, 0);
    let _ = slot
        .renderer
        .render_at(&slot.terminal, out, (rect.x, rect.y));
    slot.renderer.last_cursor()
}

/// Clear the viewport and paint every pane + dividers + bar from
/// scratch. Use after layout mutations, viewport resize, or initial
/// attach — anything where the previous frame may not be a coherent
/// base for an incremental repaint. For "focused pane got output"
/// situations call [`paint_focused_pane`] + [`paint_bar_after_pane`]
/// instead.
pub(super) fn paint_full_frame(
    layout_state: &LayoutState,
    panes: &mut HashMap<TerminalId, PaneSlot>,
    focused_pane: Option<&TerminalId>,
    viewport_dims: (u16, u16),
    status_bar: Option<&mut StatusBarPainter>,
    session_name: &str,
) {
    let has_bar = status_bar.is_some();
    let pane_dims = pane_viewport(viewport_dims, has_bar);
    let multi = super::multi_pane::compute_layout(layout_state, pane_dims);
    let mut stdout = io::stdout().lock();
    // ED2 (clear screen) + cursor home. Cheap and unambiguous.
    let _ = stdout.write_all(b"\x1b[2J\x1b[H");
    // Non-focused panes first; focused last so its render_at is the
    // final cursor-positioning emit and `last_cursor` reflects where
    // the user is typing.
    for (id, rect) in &multi.rects {
        if Some(id) == focused_pane {
            continue;
        }
        if let Some(slot) = panes.get_mut(id) {
            let _ = slot.terminal.resize(rect.w.max(1), rect.h.max(1), 0, 0);
            let _ = slot
                .renderer
                .render_at(&slot.terminal, &mut stdout, (rect.x, rect.y));
        }
    }
    let focused_cursor = focused_pane.and_then(|fid| {
        paint_focused_pane(
            &mut stdout,
            layout_state,
            panes,
            fid,
            viewport_dims,
            has_bar,
        )
    });
    let _ = super::multi_pane::paint_dividers(&mut stdout, &multi);
    paint_bar_after_pane(
        status_bar,
        &mut stdout,
        viewport_dims,
        session_name,
        focused_cursor,
    );
}

/// phux-nz4.5: shared helper invoked after every pane render so the
/// status row is restored on top of whatever VT the pane renderer just
/// wrote. No-op when there is no painter or no live viewport.
pub(super) fn paint_bar_after_pane<W: Write>(
    status_bar: Option<&mut StatusBarPainter>,
    out: &mut W,
    viewport_dims: (u16, u16),
    session_name: &str,
    restore_cursor: Option<(u16, u16)>,
) {
    let Some(painter) = status_bar else {
        return;
    };
    // The pane renderer wrote into the bottom row — invalidate so the
    // painter unconditionally re-emits.
    painter.invalidate();
    let _ = painter.paint(
        out,
        viewport_dims.0,
        viewport_dims.1,
        &make_context(session_name, SystemTime::now()),
    );
    // `write_row` hides the cursor while painting and does not restore
    // it; without this branch the cursor stays invisible until the next
    // pane render. Restore at the focused pane's known cursor position
    // when one is available (None ⇒ cursor was hidden by the pane itself,
    // e.g. a TUI inside it — leave it hidden).
    if let Some((row, col)) = restore_cursor {
        let one_based_row = row.saturating_add(1);
        let one_based_col = col.saturating_add(1);
        let _ = write!(out, "\x1b[{one_based_row};{one_based_col}H\x1b[?25h");
    }
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
