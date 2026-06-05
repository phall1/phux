use phux_protocol::TerminalId;
use phux_protocol::input::mouse::MouseEvent;

use crate::layout::LayoutState;
use crate::layout::Rect;

use super::layout::{compute_layout, PaneLayout};

// -----------------------------------------------------------------------------
// route_mouse_event — pure hit-test for INPUT_MOUSE routing (phux-4li.6)
// -----------------------------------------------------------------------------

/// Outcome of a click hit-test against the current multi-pane composition.
///
/// The driver consumes this to decide three independent things:
///
/// 1. Which `TerminalId` (if any) the resulting `INPUT_MOUSE` frame
///    targets — and what the pane-local coordinates are.
/// 2. Whether `LayoutState.focus` needs to swap to a different pane
///    (click-to-focus, per ADR-0019 decision 6 + DESIGN §7).
/// 3. Whether a divider repaint is required because focus changed
///    (heavy / light chrome moves with focus).
///
/// A divider hit returns [`RouteDecision::DividerNoOp`]: the click
/// landed on a between-pane cell, which v0.1 explicitly treats as a
/// no-op (drag-to-resize is deferred per docs/consumers/tui.md §7 / ticket scope).
#[derive(Debug, Clone, PartialEq)]
pub enum RouteDecision {
    /// The click hit a pane. The driver should:
    /// - swap `LayoutState.focus` to `target` iff `focus_changed`;
    /// - forward an `INPUT_MOUSE` whose payload's `(x, y)` are
    ///   replaced with `(pane_x, pane_y)` (pane-local cells);
    /// - repaint the multi-pane composition iff `focus_changed`
    ///   (so the heavy-edge chrome follows focus).
    Pane {
        /// The pane the mouse event addresses.
        target: TerminalId,
        /// Pane-local 0-indexed cell x (treated as f64 pixels per
        /// SPEC §9.2.1 — the cell-quantising client contract).
        pane_x: f64,
        /// Pane-local 0-indexed cell y.
        pane_y: f64,
        /// `true` iff this click moves focus.
        focus_changed: bool,
    },
    /// The click hit a divider cell (or fell outside every pane Rect).
    /// Per DESIGN §7 the v0.1 driver drops the event entirely.
    DividerNoOp,
    /// No tree to hit-test against (fresh `LayoutState::default()`),
    /// or focus is unset. Caller falls back to "no input goes anywhere
    /// until ATTACHED seeds focus".
    NoFocus,
}

/// Pure routing decision for an `INPUT_MOUSE` event.
///
/// Hit-tests `mouse.(x, y)` (interpreted as outer-viewport cell
/// coordinates per the parser in `attach::input` — the inbound SGR /
/// X10 / urxvt-1015 dispatchers all emit `f64` pixels with "1 cell ==
/// 1 pixel" because the client does not know cell-size at parse time)
/// against the pane `Rect`s yielded by [`compute_layout`].
///
/// Behavior matrix:
///
/// * No tree / no focus ⇒ [`RouteDecision::NoFocus`]. Caller drops.
/// * Click inside a pane's `Rect` ⇒ [`RouteDecision::Pane`] with
///   pane-local coords; `focus_changed = true` iff target ≠
///   `layout.focus`.
/// * Click on a divider cell or outside any pane (degenerate viewport,
///   undersized tree) ⇒ [`RouteDecision::DividerNoOp`].
///
/// The function is pure (no allocation aside from the internal
/// [`compute_layout`] call's `HashMap`) and synchronous; the driver's
/// async input loop calls it once per `InputEvent::Mouse`.
#[must_use]
pub fn route_mouse_event(
    layout: &LayoutState,
    viewport: (u16, u16),
    mouse: &MouseEvent,
) -> RouteDecision {
    // No tree means no panes to address yet — the driver dropped the
    // event already by the time `dispatch_input_events` runs, but the
    // helper stays defensive for direct callers.
    if layout.tree.is_none() {
        return RouteDecision::NoFocus;
    }

    // Single-pane fast path: skip the HashMap roundtrip. With no
    // dividers there is exactly one rect covering the whole viewport
    // and every click lands in it.
    let multi = compute_layout(layout, viewport);
    if multi.rects.is_empty() {
        return RouteDecision::NoFocus;
    }

    // Find the pane whose Rect contains the (cell) click. Iteration
    // order is arbitrary but rects tile the viewport exactly, so at
    // most one Rect matches. f64 → u16 via floor: the parser emits
    // integer-valued f64s for cell-quantised input, so a `as` cast is
    // exact for inputs in `0..=u16::MAX`. The trailing `.min(...)`
    // clamps a stray over-edge click into the last cell of the
    // viewport so a hi-DPI host's pixel report doesn't escape the
    // pane tiling.
    let cell_x = clamp_cell(mouse.x).min(viewport.0.saturating_sub(1));
    let cell_y = clamp_cell(mouse.y).min(viewport.1.saturating_sub(1));

    let mut hit: Option<(TerminalId, Rect)> = None;
    for (id, rect) in &multi.rects {
        if rect_contains(*rect, cell_x, cell_y) {
            hit = Some((id.clone(), *rect));
            break;
        }
    }

    match hit {
        Some((target, rect)) => {
            let focus_changed = layout.focus.as_ref() != Some(&target);
            // Translate to pane-local. `rect.x <= cell_x` is
            // guaranteed by `rect_contains`, so the subtraction never
            // underflows.
            #[allow(clippy::cast_lossless, reason = "u16 → f64 is exact for our range")]
            let pane_x = f64::from(cell_x - rect.x);
            #[allow(clippy::cast_lossless, reason = "u16 → f64 is exact for our range")]
            let pane_y = f64::from(cell_y - rect.y);
            RouteDecision::Pane {
                target,
                pane_x,
                pane_y,
                focus_changed,
            }
        }
        // Cell lies in a divider gap or outside any rect (degenerate
        // viewport). Drag-to-resize on divider is deferred.
        None => RouteDecision::DividerNoOp,
    }
}

/// Clamp an f64 cell position to `u16`. Pixel-precision input that
/// exceeds the viewport falls into the edge cells rather than wrapping
/// or panicking. Negative input is clamped at 0.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "input is the result of cell-quantising the SGR/X10 mouse stream; saturate to keep malformed peers from breaking the routing path"
)]
fn clamp_cell(p: f64) -> u16 {
    if p.is_nan() || p < 0.0 {
        0
    } else if p >= f64::from(u16::MAX) {
        u16::MAX
    } else {
        p as u16
    }
}

/// Half-open rectangle membership test: `[x, x+w) × [y, y+h)`. Mirrors
/// the convention `compute_layout` uses when tiling the viewport.
const fn rect_contains(r: Rect, x: u16, y: u16) -> bool {
    x >= r.x && y >= r.y && x < r.x.saturating_add(r.w) && y < r.y.saturating_add(r.h)
}
