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

use libghostty_vt::Terminal as GhosttyTerminal;
use phux_protocol::ids::TerminalId;

use super::driver::PaneSlot;
use crate::layout::LayoutState;
use crate::render::chrome::status_bar::{StatusBarPainter, make_context};

/// Fallback per-cell pixel size for client-side libghostty mirrors.
///
/// The server-side actor uses the same conventional 8x16 default until a real
/// viewport pixel report arrives. The client mirror also needs nonzero cell
/// pixels: classic Kitty placements without explicit `c/r` dimensions infer
/// their grid footprint from pixel geometry, and a zero value makes the first
/// live render skip the placement until a later snapshot supplies `c/r`.
pub(super) const FALLBACK_CELL_PX: (u32, u32) = (8, 16);

/// Resize a libghostty [`GhosttyTerminal`] to `cols`x`rows`, clamping each axis to
/// a 1-cell minimum (libghostty has no concept of a zero-dimension grid, so
/// a `0`-col or `0`-row request fails with `InvalidValue` and leaves the
/// grid unchanged).
///
/// The both-axes-shrink overflow in libghostty's `PageList.resizeCols`
/// (phux-y06) is fixed in the vendored ghostty (phall1/ghostty 6d89054f3,
/// "fix: resize overflow"), so a both-shrink is a single safe `resize()`
/// call — no axis decomposition needed.
pub(super) fn safe_resize(
    terminal: &mut GhosttyTerminal<'_, '_>,
    cols: u16,
    rows: u16,
) -> libghostty_vt::error::Result<()> {
    let cols = cols.max(1);
    let rows = rows.max(1);
    terminal.resize(cols, rows, FALLBACK_CELL_PX.0, FALLBACK_CELL_PX.1)
}

/// The server-authoritative mirror grid `(cols, rows)` used to letterbox a
/// pane within its render rect (phux-7ubw).
///
/// Reads the libghostty mirror's own grid size. On the (unexpected) error
/// path it falls back to the rect dims, which makes [`render_at_letterboxed`]
/// degrade to the prior rect-clamp paint (zero pad, no margins) rather than
/// mis-centring on a bogus size.
///
/// [`render_at_letterboxed`]: super::render::TerminalRenderer::render_at_letterboxed
fn mirror_dims(terminal: &GhosttyTerminal<'_, '_>, rect: crate::layout::Rect) -> (u16, u16) {
    let cols = terminal.cols().unwrap_or(rect.w);
    let rows = terminal.rows().unwrap_or(rect.h);
    (cols, rows)
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
#[allow(
    clippy::too_many_arguments,
    reason = "phux-4h5a adds the sidebar reservation to the pane paint context; same arg-list refactor follow-up as paint_full_frame / handle_server_frame"
)]
pub(super) fn paint_focused_pane<W: Write>(
    out: &mut W,
    layout_state: &LayoutState,
    panes: &mut HashMap<TerminalId, PaneSlot>,
    focused: &TerminalId,
    viewport_dims: (u16, u16),
    has_bar: bool,
    sidebar: Option<SidebarReservation>,
    force_full: bool,
) -> Option<(u16, u16)> {
    let content = content_rect(viewport_dims, has_bar, sidebar);
    let rect = super::multi_pane::compute_layout_in(layout_state, content, viewport_dims)
        .rects
        .get(focused)
        .copied()
        .unwrap_or(content);
    let slot = panes.get_mut(focused)?;
    // The mirror grid size is server-authoritative (set only at the
    // snapshot / resize-ack handler); the layout rect clips and positions
    // the paint but never resizes the pane's libghostty Terminal. Resizing
    // the alt-screen mirror to a transient client-rect width during a resize
    // handshake strands previous-screen content in the dropped columns (the
    // ghost cells — alt screen does not reflow), which `render_at` would then
    // faithfully paint. Clipping confines the paint to the rect instead.
    // Letterbox: when the server-authoritative mirror grid is smaller than the
    // rect, centre it and blank the surrounding margins instead of pinning it
    // to the rect's top-left (phux-7ubw, ADR-0027 single-view letterbox). When
    // the mirror is >= the rect this degrades to the existing clamp, so a
    // mirror that fills the rect is byte-identical to the prior `render_at`.
    let mirror = mirror_dims(&slot.terminal, rect);
    let _ = slot.renderer.render_at_letterboxed(
        &slot.terminal,
        out,
        (rect.x, rect.y),
        (rect.w, rect.h),
        mirror,
        force_full,
    );
    slot.renderer.last_cursor()
}

/// The single composite end-of-frame cursor authority (ADR-0029, phux-gxy/
/// 9xn/b9n/d69/549). Every frame ends here: this is the SOLE place that emits
/// the composite cursor placement (CUP + DECTCEM) and the SOLE place that
/// flushes the pane/chrome composite. Routing all paint paths through it keeps
/// ADR-0020 invariant 4 ("exactly one renderer positions the cursor per
/// frame") true and collapses the three-way None-fallback policy — previously
/// copy-pasted across several paint sites — into one body.
///
/// `cursor` is the focused pane's authoritative last cursor as `(row, col)`
/// (0-based). When `None`, `fallback_origin` (`(x, y)` = the focused pane's
/// `Rect` origin) parks the cursor inside the pane area and HIDES it (`?25l`),
/// so a `None` cursor never strands the host cursor at the status bar's tail
/// (bottom-right) — the visible phux-gxy/9xn symptom. `None` + `None` parks at
/// the viewport origin, hidden, as a safety net.
///
/// The trailing flush is load-bearing: stdout is a `LineWriter` and the CUP we
/// write has no newline, so without the flush it sits buffered until the next
/// pane output — which never comes for an idle pane (a shell prompt). That was
/// the real phux-gxy: prior fixes computed the right CUP but never flushed it,
/// so in-memory unit tests passed while the live terminal never saw it.
// CURSOR-AUTHORITY: composite
pub(super) fn end_of_frame_cursor<W: Write>(
    out: &mut W,
    cursor: Option<(u16, u16)>,
    fallback_origin: Option<(u16, u16)>,
) -> std::io::Result<()> {
    if let Some((row, col)) = cursor {
        tracing::trace!(row, col, "end_of_frame_cursor: restore focused cursor");
        super::render::write_cup(out, row, col)?;
        out.write_all(b"\x1b[?25h")?;
    } else {
        // No authoritative cursor: park at the focused pane's origin (or the
        // viewport origin) and hide. `fallback_origin` is `(x, y)`.
        let (x, y) = fallback_origin.unwrap_or((0, 0));
        tracing::trace!(x, y, "end_of_frame_cursor: no cursor, parking hidden");
        super::render::write_cup(out, y, x)?;
        out.write_all(b"\x1b[?25l")?;
    }
    out.flush()
}

/// Clear the viewport and paint every pane + dividers + bar from
/// scratch. Use after layout mutations, viewport resize, or initial
/// attach — anything where the previous frame may not be a coherent
/// base for an incremental repaint. For "focused pane got output"
/// situations call [`paint_focused_pane`] + [`paint_bar_after_pane`]
/// instead.
#[allow(
    clippy::too_many_arguments,
    reason = "phux-4h5a adds the sidebar reservation + painter to the existing paint context; same arg-list refactor follow-up as handle_server_frame"
)]
pub(super) fn paint_full_frame<W: super::RenderSink>(
    out: &mut W,
    layout_state: &LayoutState,
    panes: &mut HashMap<TerminalId, PaneSlot>,
    focused_pane: Option<&TerminalId>,
    viewport_dims: (u16, u16),
    status_bar: Option<&mut StatusBarPainter>,
    sidebar: Option<SidebarReservation>,
    sidebar_painter: Option<&mut crate::render::chrome::sidebar::SidebarPainter>,
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
    let content = content_rect(viewport_dims, has_bar, sidebar);
    let multi = super::multi_pane::compute_layout_in(layout_state, content, viewport_dims);
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
            // Force a full redraw: the ED2 above cleared the screen, so
            // an incremental "only dirty rows" paint would leave a pane
            // whose content is unchanged (the survivor of a split/resize)
            // blank. The rect clips the paint; it never resizes the
            // server-authoritative mirror grid. Letterboxed: an undersized
            // mirror is centred and its margins blanked (phux-7ubw); a mirror
            // that fills/exceeds the rect degrades to the prior clamp.
            let mirror = mirror_dims(&slot.terminal, *rect);
            let _ = slot.renderer.render_at_letterboxed(
                &slot.terminal,
                out,
                (rect.x, rect.y),
                (rect.w, rect.h),
                mirror,
                true,
            );
        }
    }
    let _ = crate::render::chrome::dividers::render_dividers(out, &multi, focused_pane);
    // Paint the sidebar strip into its reserved columns. The ED2 above cleared
    // it, so invalidate the painter's cache to force a re-emit even if the
    // window list is byte-identical to the previous frame. The strip occupies
    // the columns `content_rect` carved out, so it never overlaps pane content.
    if let (Some(res), Some(painter)) = (sidebar, sidebar_painter) {
        painter.invalidate();
        let _ = painter.paint(out, sidebar_rect(viewport_dims, has_bar, res));
    }
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
        paint_focused_pane(
            out,
            layout_state,
            panes,
            fid,
            viewport_dims,
            has_bar,
            sidebar,
            true,
        )
    });
    // The focused pane's Rect origin is the fallback cursor parking spot when
    // `final_cursor` is None (phux-gxy/9xn). All cursor placement + the flush
    // is owned by the one composite authority.
    let fallback_origin = focused_pane
        .and_then(|fid| multi.rects.get(fid).copied())
        .map(|r| (r.x, r.y));
    let _ = end_of_frame_cursor(out, final_cursor, fallback_origin);
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
    // All cursor placement (restore / fallback / safety-net) and the
    // load-bearing flush are owned by the one composite authority (ADR-0029).
    let _ = end_of_frame_cursor(out, restore_cursor, fallback_origin);
}

/// Effective viewport available to pane rendering: outer dims with the
/// status-bar row reserved when a bar is present.
///
/// Equivalent to `content_rect(outer, has_status_bar, None)`'s `(w, h)` —
/// the no-sidebar content rect is anchored at `(0, 0)` with these dims, which
/// is what keeps the disabled path byte-identical to the pre-sidebar tiling.
/// phux-4h5a converted every production call site to `content_rect`, so this
/// now survives only as the reference half of the disabled-path invariant test
/// [`tests::content_rect_disabled_equals_pane_viewport_rect`].
#[cfg_attr(not(test), allow(dead_code, reason = "test-only invariant reference"))]
pub(super) const fn pane_viewport(outer: (u16, u16), has_status_bar: bool) -> (u16, u16) {
    if has_status_bar {
        (outer.0, outer.1.saturating_sub(1))
    } else {
        outer
    }
}

/// Which edge a reserved sidebar strip docks to. Mirrors
/// [`phux_config::SidebarPosition`]; kept local so `paint`'s geometry doesn't
/// depend on the config crate's enum directly (the driver maps one to the
/// other).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SidebarEdge {
    /// Dock on the left; panes tile to its right.
    Left,
    /// Dock on the right; panes tile to its left.
    Right,
}

/// A chrome-region reservation for the window sidebar (phux-4h5a): `width`
/// columns reserved on `edge`. The driver builds this from `[sidebar]` config
/// each frame (`None` when the sidebar is disabled) and threads the SAME value
/// to every layout site so panes, dividers, reflow, mouse, and predict agree
/// on the inset.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct SidebarReservation {
    /// The edge the strip docks to.
    pub edge: SidebarEdge,
    /// Strip width in columns.
    pub width: u16,
}

/// The residual content `Rect` panes tile into after the status bar and the
/// (optional) sidebar are folded off the outer viewport.
///
/// Height drops one row for the status bar (mirroring [`pane_viewport`]).
/// Width and x-origin inset for the sidebar: a left strip pushes the origin
/// right by `width`; a right strip just narrows the width. `width` is clamped
/// to the viewport so an over-wide sidebar yields a zero-width content rect
/// rather than underflowing.
///
/// CRITICAL: with `sidebar = None` this is exactly `Rect { x: 0, y: 0, w, h }`
/// where `(w, h) == pane_viewport(outer, has_bar)`, so
/// `compute_layout_in(ls, content_rect(outer, has_bar, None), outer)` is
/// byte-identical to the pre-sidebar `compute_layout(ls, pane_viewport(..))`.
pub(super) fn content_rect(
    outer: (u16, u16),
    has_bar: bool,
    sidebar: Option<SidebarReservation>,
) -> crate::layout::Rect {
    let (cols, rows) = outer;
    let h = if has_bar {
        rows.saturating_sub(1)
    } else {
        rows
    };
    sidebar.map_or(
        crate::layout::Rect {
            x: 0,
            y: 0,
            w: cols,
            h,
        },
        |res| {
            let width = res.width.min(cols);
            let w = cols - width;
            let x = match res.edge {
                SidebarEdge::Left => width,
                SidebarEdge::Right => 0,
            };
            crate::layout::Rect { x, y: 0, w, h }
        },
    )
}

/// The sidebar strip's own `Rect` — the columns [`content_rect`] reserved for
/// it. Shares the status-bar height reservation so the strip stops above the
/// bar row (the bar spans the full width below both panes and sidebar). The
/// strip docks flush to the left or right outer edge per `res.edge`.
pub(super) fn sidebar_rect(
    outer: (u16, u16),
    has_bar: bool,
    res: SidebarReservation,
) -> crate::layout::Rect {
    let (cols, rows) = outer;
    let h = if has_bar {
        rows.saturating_sub(1)
    } else {
        rows
    };
    let width = res.width.min(cols);
    let x = match res.edge {
        SidebarEdge::Left => 0,
        SidebarEdge::Right => cols - width,
    };
    crate::layout::Rect {
        x,
        y: 0,
        w: width,
        h,
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, reason = "tests")]
mod tests {
    use super::*;

    /// phux-4h5a: the disabled-path invariant. `content_rect(.., None)` must
    /// yield exactly `Rect { 0, 0, pane_viewport(..).0, pane_viewport(..).1 }`
    /// so the sidebar-off tiling stays byte-identical to the pre-sidebar one.
    #[test]
    fn content_rect_disabled_equals_pane_viewport_rect() {
        for outer in [(80u16, 24u16), (1, 1), (200, 50), (0, 0)] {
            for has_bar in [false, true] {
                let (vw, vh) = pane_viewport(outer, has_bar);
                assert_eq!(
                    content_rect(outer, has_bar, None),
                    crate::layout::Rect {
                        x: 0,
                        y: 0,
                        w: vw,
                        h: vh,
                    },
                    "outer={outer:?} has_bar={has_bar}"
                );
            }
        }
    }

    /// A left sidebar pushes the content origin right by `width` and narrows
    /// the width; a right sidebar leaves the origin at 0 and just narrows.
    /// Height tracks the status-bar reservation in both cases.
    #[test]
    fn content_rect_insets_for_left_and_right_sidebar() {
        let outer = (80, 24);
        // No bar, left dock, width 20: x = 20, w = 60, h = 24.
        let left = content_rect(
            outer,
            false,
            Some(SidebarReservation {
                edge: SidebarEdge::Left,
                width: 20,
            }),
        );
        assert_eq!(
            left,
            crate::layout::Rect {
                x: 20,
                y: 0,
                w: 60,
                h: 24,
            }
        );
        // With bar, right dock, width 20: x = 0, w = 60, h = 23.
        let right = content_rect(
            outer,
            true,
            Some(SidebarReservation {
                edge: SidebarEdge::Right,
                width: 20,
            }),
        );
        assert_eq!(
            right,
            crate::layout::Rect {
                x: 0,
                y: 0,
                w: 60,
                h: 23,
            }
        );
        // An over-wide sidebar clamps to the viewport: zero content width, no
        // underflow.
        let huge = content_rect(
            outer,
            false,
            Some(SidebarReservation {
                edge: SidebarEdge::Left,
                width: 999,
            }),
        );
        assert_eq!(huge.w, 0);
        assert_eq!(huge.x, 80);
    }

    /// The strip rect docks flush to the outer edge, spans `width` columns, and
    /// shares the content height (stops above the status bar row).
    #[test]
    fn sidebar_rect_docks_flush_to_the_edge() {
        let outer = (80, 24);
        let left = sidebar_rect(
            outer,
            true,
            SidebarReservation {
                edge: SidebarEdge::Left,
                width: 20,
            },
        );
        assert_eq!(
            left,
            crate::layout::Rect {
                x: 0,
                y: 0,
                w: 20,
                h: 23,
            }
        );
        let right = sidebar_rect(
            outer,
            false,
            SidebarReservation {
                edge: SidebarEdge::Right,
                width: 20,
            },
        );
        assert_eq!(
            right,
            crate::layout::Rect {
                x: 60,
                y: 0,
                w: 20,
                h: 24,
            }
        );
    }

    /// ADR-0029: the one composite cursor emitter resolves the three-way
    /// None-fallback policy and always ends with a flush. Pins the byte output
    /// for each case (the cursor-matrix the phux-gxy/9xn/b9n scars chased).
    #[test]
    fn end_of_frame_cursor_resolves_all_three_cases() {
        // Some(cursor) -> CUP(row,col) + show. (2,4) 0-based -> CUP 3;5.
        let mut out = Vec::new();
        end_of_frame_cursor(&mut out, Some((2, 4)), None).expect("write");
        assert_eq!(String::from_utf8(out).unwrap(), "\x1b[3;5H\x1b[?25h");

        // None + fallback origin (x=3, y=5) -> CUP(y,x)=6;4 + hide.
        let mut out = Vec::new();
        end_of_frame_cursor(&mut out, None, Some((3, 5))).expect("write");
        assert_eq!(String::from_utf8(out).unwrap(), "\x1b[6;4H\x1b[?25l");

        // None + None -> safety net: viewport origin, hidden.
        let mut out = Vec::new();
        end_of_frame_cursor(&mut out, None, None).expect("write");
        assert_eq!(String::from_utf8(out).unwrap(), "\x1b[1;1H\x1b[?25l");
    }

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
            None,
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

    /// phux-wurs: `paint_focused_pane` must NOT resize the pane's libghostty
    /// mirror to the client layout rect. The mirror grid size is
    /// server-authoritative (set only at the snapshot / resize-ack handler).
    /// Resizing the alt-screen mirror to a transient client-rect width during
    /// a resize handshake strands previous-screen content in the dropped
    /// columns (the right-side ghost), because the alternate screen does not
    /// reflow. Single-pane (`tree: None`) takes the full-viewport rect
    /// fallback, so the rect width (M) differs from the mirror width (N).
    #[test]
    fn paint_focused_pane_does_not_resize_server_authoritative_mirror() {
        use libghostty_vt::TerminalOptions;

        let id = TerminalId::local(1);
        // Single-pane: no layout tree ⇒ compute_layout yields no rect, so
        // paint_focused_pane falls back to the full pane viewport.
        let layout = LayoutState {
            tree: None,
            focus: Some(id.clone()),
        };

        // Mirror is server-authoritative at 20x4 on the ALT screen, filled
        // with full-width content (the "top-of-file" the ghost is made of).
        let mirror_cols = 20u16;
        let mirror_rows = 4u16;
        let mut slot = PaneSlot::new_with_size(mirror_cols, mirror_rows).expect("slot");
        slot.terminal.vt_write(b"\x1b[?1049h"); // enter alt screen (no reflow)
        slot.terminal
            .vt_write(b"ABCDEFGHIJKLMNOPQRST\r\nABCDEFGHIJKLMNOPQRST");
        let mut panes: HashMap<TerminalId, PaneSlot> = HashMap::new();
        panes.insert(id.clone(), slot);

        // Client viewport is far wider/taller than the mirror, so the rect
        // (M) disagrees with the mirror (N). With a bar, pane_dims = (80, 23).
        let viewport = (80u16, 24u16);
        let mut out: Vec<u8> = Vec::new();
        let _ = paint_focused_pane(
            &mut out, &layout, &mut panes, &id, viewport, true, None, false,
        );

        // The mirror grid size is unchanged — the layout rect did not resize it.
        let slot = panes.get(&id).expect("slot");
        assert_eq!(
            slot.terminal.cols().expect("cols"),
            mirror_cols,
            "focused paint must not widen the server-authoritative mirror"
        );
        assert_eq!(
            slot.terminal.rows().expect("rows"),
            mirror_rows,
            "focused paint must not grow the server-authoritative mirror"
        );

        // And the paint is clipped to the mirror's real width: no spill past
        // column 20 (the rect is 80 wide, but the mirror is only 20).
        // Reference: re-read the mirror grid and confirm the painted glyphs
        // match, with nothing beyond. A spill would emit extra glyphs from a
        // stale wider grid; here the grid is 20 wide so the clip equals the
        // mirror. The regression we guard is the resize, asserted above.
        let s = String::from_utf8_lossy(&out);
        assert!(s.contains('A') && s.contains('T'), "content painted; {s:?}");

        // A no-grow probe via an explicit alt-screen reference: a 20-wide
        // mirror written the same way, never resized, has the identical grid.
        let mut reference = GhosttyTerminal::new(TerminalOptions {
            cols: mirror_cols,
            rows: mirror_rows,
            max_scrollback: 10_000,
        })
        .expect("reference");
        reference.vt_write(b"\x1b[?1049h");
        reference.vt_write(b"ABCDEFGHIJKLMNOPQRST\r\nABCDEFGHIJKLMNOPQRST");
        assert_eq!(
            reference.cols().expect("ref cols"),
            slot.terminal.cols().expect("slot cols"),
            "mirror width must equal the never-resized reference"
        );
    }
}
