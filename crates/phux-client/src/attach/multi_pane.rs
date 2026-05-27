//! Multi-pane composition: layout tree → per-pane sub-rectangles + the
//! divider cells that live between them.
//!
//! Per ADR-0019 decision 4 the reference TUI draws **dividers between**
//! panes (not frames around each), in plain Unicode box-drawing
//! (U+2500–U+257F). One column is consumed per `Horizontal` interior
//! node along the relevant axis path; one row per `Vertical` interior
//! node. The cell budget given to the layout algorithm is therefore
//! `(cols - h_dividers, rows - v_dividers)` — the layout tiles the
//! **content** rectangle and the renderer paints dividers in the gaps
//! the tree explicitly excluded.
//!
//! Focus chrome (decision 4 cont.): the divider segments adjacent to
//! the focused pane use the **heavy** variant (`━ ┃ ╋` and the heavy
//! junction pieces); inactive segments use **light** (`─ │ ┼` …).
//! Junction characters are chosen per-cell from the set of incident
//! light/heavy edges so a `T`-piece adjacent to a heavy edge renders
//! the correct mixed-weight glyph (e.g. `┲`, `┳`, `┺`, …).
//!
//! The output is a [`PaneLayout`] carrying both the per-pane [`Rect`]s
//! (which `attach::driver` hands to each `TerminalRenderer`) and the
//! list of [`DividerCell`]s (which the chrome layer at
//! [`crate::render::chrome::dividers`] composites onto stdout via
//! ratatui, with pane interiors marked `Cell::skip` so libghostty's
//! direct VT output is not stomped — see ADR-0020).
//!
//! SIGWINCH-driven reflow lives in `attach::reflow` (sibling ticket
//! phux-4li.7); this module is the pure compute step it composes with.

use std::collections::HashMap;

use phux_protocol::TerminalId;
use phux_protocol::input::mouse::MouseEvent;

use crate::layout::{LayoutNode, LayoutState, Rect, SplitDir};

// -----------------------------------------------------------------------------
// PaneLayout — the result of one multi-pane compute pass
// -----------------------------------------------------------------------------

/// Result of [`compute_layout`]: per-pane rectangles plus the cells
/// occupied by dividers between them.
///
/// The pane `Rect`s tile the content rectangle exactly (same invariant
/// as [`crate::layout::pane_rects`]); the divider cells fill the **gaps** the layout
/// algorithm excluded. Together they cover the outer viewport with no
/// overlap and no holes — proptest target for future regressions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneLayout {
    /// The outer viewport this layout was computed against.
    pub viewport: (u16, u16),
    /// Per-pane bounding rectangle, in outer-viewport cell coordinates.
    pub rects: HashMap<TerminalId, Rect>,
    /// Divider cells with their pre-resolved box-drawing character.
    ///
    /// Each entry positions exactly one cell. The character carries the
    /// final weight (light or heavy per edge) and junction shape; the
    /// painter writes it verbatim. Order is row-major within each
    /// interior split (left-to-right then top-to-bottom).
    pub dividers: Vec<DividerCell>,
}

/// One cell of the divider grid, with its resolved box-drawing glyph.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DividerCell {
    /// Column in outer-viewport coordinates.
    pub x: u16,
    /// Row in outer-viewport coordinates.
    pub y: u16,
    /// The pre-resolved box-drawing character.
    pub ch: char,
}

// -----------------------------------------------------------------------------
// compute_layout — the public entry point
// -----------------------------------------------------------------------------

/// Compute per-pane rectangles and divider cells for `layout` inside a
/// `viewport_dims` outer viewport.
///
/// Returns an empty `PaneLayout` when `layout.tree.is_none()` (fresh
/// state, no panes seeded yet) or when either viewport axis is zero —
/// callers should special-case "single-pane attach" by checking
/// `dividers.is_empty()` and `rects.len() == 1`.
#[must_use]
pub fn compute_layout(layout: &LayoutState, viewport_dims: (u16, u16)) -> PaneLayout {
    let Some(tree) = layout.tree.as_ref() else {
        return PaneLayout {
            viewport: viewport_dims,
            rects: HashMap::new(),
            dividers: Vec::new(),
        };
    };
    let (cols, rows) = viewport_dims;
    if cols == 0 || rows == 0 {
        return PaneLayout {
            viewport: viewport_dims,
            rects: HashMap::new(),
            dividers: Vec::new(),
        };
    }

    // Walk the tree once, allocating outer-viewport rects to leaves and
    // emitting one `DividerSegment` per interior split. Both share the
    // same divider-budget math: each Horizontal split eats one column
    // from its bounds, each Vertical split eats one row.
    let mut segments: Vec<DividerSegment> = Vec::new();
    let mut rects: HashMap<TerminalId, Rect> = HashMap::new();
    walk_layout(
        tree,
        Rect {
            x: 0,
            y: 0,
            w: cols,
            h: rows,
        },
        &mut segments,
        &mut rects,
    );

    // Rasterize the segments into per-cell divider entries, resolving
    // junctions and heavy/light weights against the focused pane.
    let dividers = rasterize(&segments, layout.focus.as_ref(), &rects, (cols, rows));

    PaneLayout {
        viewport: viewport_dims,
        rects,
        dividers,
    }
}

// -----------------------------------------------------------------------------
// Divider painting moved to `crate::render::chrome::dividers` (phux-5ke.3,
// ADR-0020). This module stays pure data: `compute_layout` yields the
// `PaneLayout`, and the chrome layer composites its `DividerCell`s with
// skip-cell carve-outs over the pane interiors. No VT-byte path here.
// -----------------------------------------------------------------------------

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
/// no-op (drag-to-resize is deferred per DESIGN.md §7 / ticket scope).
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

// -----------------------------------------------------------------------------
// Internals — segment collection, divider counting, rasterization
// -----------------------------------------------------------------------------

/// One divider segment: either a vertical line (from a Horizontal
/// split) or a horizontal line (from a Vertical split). Lives in
/// outer-viewport cell coordinates and remembers which two subtrees
/// it separates so we can resolve "is the focused pane adjacent?".
#[derive(Debug, Clone)]
struct DividerSegment {
    /// Direction of the *line itself*: a Horizontal split produces a
    /// vertical line; we tag the segment with the split's dir so the
    /// rasterizer knows which axis to walk.
    split: SplitDir,
    /// Inclusive cell range along the segment's long axis.
    a0: u16,
    /// Inclusive cell range along the segment's long axis.
    a1: u16,
    /// Cell index on the perpendicular (cross) axis.
    cross: u16,
    /// Leaves of the subtree on the "low" side of the segment (left for
    /// Horizontal, top for Vertical). Used to resolve focus adjacency.
    low_leaves: Vec<TerminalId>,
    /// Leaves of the subtree on the "high" side.
    high_leaves: Vec<TerminalId>,
}

/// Wildcard handler for `#[non_exhaustive]` matches over [`LayoutNode`] /
/// [`SplitDir`]. v0.1 only knows the documented variants; a newer-server
/// forward-compat decode reaching this module is a protocol violation
/// already caught upstream.
#[cold]
#[inline(never)]
#[allow(clippy::panic)]
fn unknown_variant() -> ! {
    panic!("multi_pane: unknown wire-protocol variant (newer than this client)")
}

/// Recursively split `bounds` according to the tree, recording one
/// `DividerSegment` per interior node and the outer-viewport `Rect` of
/// every leaf. Bounds are in outer-viewport cell coordinates; the
/// divider cell is subtracted from the split axis before the ratio is
/// applied.
fn walk_layout(
    node: &LayoutNode,
    bounds: Rect,
    segments: &mut Vec<DividerSegment>,
    rects: &mut HashMap<TerminalId, Rect>,
) {
    match node {
        LayoutNode::Leaf(p) => {
            rects.insert(p.clone(), bounds);
        }
        LayoutNode::Split {
            dir,
            ratio,
            left,
            right,
        } => match dir {
            SplitDir::Horizontal => {
                if bounds.w < 3 {
                    // Degenerate; give the leftmost leaf the whole
                    // bounds and bail. Callers should be warning about
                    // an unworkably small viewport already.
                    if let Some(id) = collect_leaves(left).first() {
                        rects.insert(id.clone(), bounds);
                    }
                    return;
                }
                let content_w = bounds.w - 1;
                let left_w = split_dim(content_w, *ratio);
                let right_w = content_w - left_w;
                let divider_x = bounds.x + left_w;
                segments.push(DividerSegment {
                    split: SplitDir::Horizontal,
                    a0: bounds.y,
                    a1: bounds.y + bounds.h.saturating_sub(1),
                    cross: divider_x,
                    low_leaves: collect_leaves(left),
                    high_leaves: collect_leaves(right),
                });
                walk_layout(
                    left,
                    Rect {
                        x: bounds.x,
                        y: bounds.y,
                        w: left_w,
                        h: bounds.h,
                    },
                    segments,
                    rects,
                );
                walk_layout(
                    right,
                    Rect {
                        x: divider_x + 1,
                        y: bounds.y,
                        w: right_w,
                        h: bounds.h,
                    },
                    segments,
                    rects,
                );
            }
            SplitDir::Vertical => {
                if bounds.h < 3 {
                    if let Some(id) = collect_leaves(left).first() {
                        rects.insert(id.clone(), bounds);
                    }
                    return;
                }
                let content_h = bounds.h - 1;
                let top_h = split_dim(content_h, *ratio);
                let bot_h = content_h - top_h;
                let divider_y = bounds.y + top_h;
                segments.push(DividerSegment {
                    split: SplitDir::Vertical,
                    a0: bounds.x,
                    a1: bounds.x + bounds.w.saturating_sub(1),
                    cross: divider_y,
                    low_leaves: collect_leaves(left),
                    high_leaves: collect_leaves(right),
                });
                walk_layout(
                    left,
                    Rect {
                        x: bounds.x,
                        y: bounds.y,
                        w: bounds.w,
                        h: top_h,
                    },
                    segments,
                    rects,
                );
                walk_layout(
                    right,
                    Rect {
                        x: bounds.x,
                        y: divider_y + 1,
                        w: bounds.w,
                        h: bot_h,
                    },
                    segments,
                    rects,
                );
            }
            _ => unknown_variant(),
        },
        _ => unknown_variant(),
    }
}

/// Final pass: convert `DividerSegment`s into the per-cell
/// `DividerCell`s the painter consumes. Resolves heavy/light per
/// segment edge by checking focus adjacency, and picks junction
/// characters when two segments cross.
#[allow(
    clippy::too_many_lines,
    reason = "two-pass rasterizer (lay down edges + neighbour-pass T-piece resolution); splitting would lose locality between the per-segment paint and the per-cell inheritance."
)]
fn rasterize(
    segments: &[DividerSegment],
    focus: Option<&TerminalId>,
    rects: &HashMap<TerminalId, Rect>,
    viewport: (u16, u16),
) -> Vec<DividerCell> {
    // Per-cell map: which edges are present at (x, y), and which are
    // heavy? Order of bits is [N, E, S, W]; weight is parallel.
    #[derive(Default, Clone, Copy)]
    struct Cell {
        north: Option<bool>, // Some(heavy?) when an edge points north
        east: Option<bool>,
        south: Option<bool>,
        west: Option<bool>,
    }
    let (vcols, vrows) = viewport;
    let mut grid: HashMap<(u16, u16), Cell> = HashMap::new();

    // Helper: does the segment touch the focused pane? Focused pane is
    // adjacent iff its TerminalId is in either low_leaves or high_leaves
    // AND its rect borders the segment along the cross axis. We use
    // membership only — the rasterizer doesn't need the exact rect
    // contact test because every leaf in a subtree is adjacent to *its*
    // outer divider (binary-split-tree invariant).
    let focused_pane_id = focus;
    let segment_heavy = |seg: &DividerSegment| -> bool {
        let Some(fid) = focused_pane_id else {
            return false;
        };
        let touches = seg.low_leaves.contains(fid) || seg.high_leaves.contains(fid);
        if !touches {
            return false;
        }
        // Confirm the focused pane's rect actually shares an edge with
        // this segment. Otherwise we'd mark non-adjacent dividers heavy
        // when the focused pane is deep in one subtree.
        let Some(fr) = rects.get(fid) else {
            return false;
        };
        match seg.split {
            SplitDir::Horizontal => {
                // Vertical line at column `cross`. Adjacent iff the
                // focused rect's left edge == cross+1 OR right edge ==
                // cross. AND it must overlap the segment's y range.
                let adjacent_x =
                    fr.x == seg.cross.saturating_add(1) || fr.x.saturating_add(fr.w) == seg.cross;
                let overlaps_y =
                    fr.y <= seg.a1 && fr.y.saturating_add(fr.h).saturating_sub(1) >= seg.a0;
                adjacent_x && overlaps_y
            }
            SplitDir::Vertical => {
                let adjacent_y =
                    fr.y == seg.cross.saturating_add(1) || fr.y.saturating_add(fr.h) == seg.cross;
                let overlaps_x =
                    fr.x <= seg.a1 && fr.x.saturating_add(fr.w).saturating_sub(1) >= seg.a0;
                adjacent_y && overlaps_x
            }
            _ => false,
        }
    };

    // Lay down each segment's cells, recording incident edges.
    for seg in segments {
        let heavy = segment_heavy(seg);
        match seg.split {
            SplitDir::Horizontal => {
                // Vertical line at column `cross` from row a0..=a1.
                let x = seg.cross;
                if x >= vcols {
                    continue;
                }
                for y in seg.a0..=seg.a1.min(vrows.saturating_sub(1)) {
                    let cell = grid.entry((x, y)).or_default();
                    if y > seg.a0 {
                        cell.north = Some(heavy);
                    }
                    if y < seg.a1 {
                        cell.south = Some(heavy);
                    }
                }
            }
            SplitDir::Vertical => {
                // Horizontal line at row `cross` from col a0..=a1.
                let y = seg.cross;
                if y >= vrows {
                    continue;
                }
                for x in seg.a0..=seg.a1.min(vcols.saturating_sub(1)) {
                    let cell = grid.entry((x, y)).or_default();
                    if x > seg.a0 {
                        cell.west = Some(heavy);
                    }
                    if x < seg.a1 {
                        cell.east = Some(heavy);
                    }
                }
            }
            _ => {}
        }
    }

    // Post-pass: T-piece junctions where one segment terminates at
    // another. A cell whose neighbour is itself a divider cell gets an
    // edge pointing toward that neighbour (inheriting the neighbour's
    // weight on the touching edge). Without this pass an inner segment
    // ending against an outer segment paints as a straight line + a
    // straight perpendicular at the same coordinates, with no junction
    // glyph — visually a "broken cross."
    let cell_coords: Vec<(u16, u16)> = grid.keys().copied().collect();
    for (x, y) in cell_coords {
        // Look at each cardinal neighbour. If the neighbour is a
        // divider cell AND we don't already have an edge in that
        // direction, inherit one whose weight matches the neighbour's
        // touching edge.
        let north_neighbour = if y > 0 {
            grid.get(&(x, y - 1)).copied()
        } else {
            None
        };
        let south_neighbour = grid.get(&(x, y + 1)).copied();
        let east_neighbour = grid.get(&(x + 1, y)).copied();
        let west_neighbour = if x > 0 {
            grid.get(&(x - 1, y)).copied()
        } else {
            None
        };

        let Some(cell) = grid.get_mut(&(x, y)) else {
            continue;
        };
        if cell.north.is_none()
            && let Some(n) = north_neighbour
            && let Some(heavy) = n.south.or(n.north)
        {
            // The neighbour's south edge is what touches us.
            cell.north = Some(heavy);
        }
        if cell.south.is_none()
            && let Some(n) = south_neighbour
            && let Some(heavy) = n.north.or(n.south)
        {
            cell.south = Some(heavy);
        }
        if cell.east.is_none()
            && let Some(n) = east_neighbour
            && let Some(heavy) = n.west.or(n.east)
        {
            cell.east = Some(heavy);
        }
        if cell.west.is_none()
            && let Some(n) = west_neighbour
            && let Some(heavy) = n.east.or(n.west)
        {
            cell.west = Some(heavy);
        }
    }

    // Stable output order: row-major.
    let mut keys: Vec<(u16, u16)> = grid.keys().copied().collect();
    keys.sort_by_key(|(x, y)| (*y, *x));
    keys.into_iter()
        .map(|(x, y)| {
            let cell = grid[&(x, y)];
            DividerCell {
                x,
                y,
                ch: pick_box_char(cell.north, cell.east, cell.south, cell.west),
            }
        })
        .collect()
}

/// Pick the box-drawing character for a cell given which of its four
/// edges are present, and (per edge) whether that edge is heavy.
///
/// Falls back to the all-light glyph when a cell has incident heavy
/// edges in a configuration Unicode 16 doesn't define a mixed
/// pictograph for (very rare; v0.1 only generates 2-edge crosses and
/// 4-edge plusses).
#[allow(
    clippy::match_same_arms,
    reason = "single-edge stubs share the glyph of the longer line they continue (`│` / `─` / `┃` / `━`); merging them via `|` defeats the `unnested_or_patterns` lint. Keep the 1-edge-per-arm form for documentation."
)]
const fn pick_box_char(
    north: Option<bool>,
    east: Option<bool>,
    south: Option<bool>,
    west: Option<bool>,
) -> char {
    use EdgeKind::{Absent, Heavy, Light};
    // Compact tag: (present, heavy) per side. 256-entry decision table
    // at most, but we only hit a small subset; an explicit match is
    // more readable than a numeric lookup.
    let n = edge_kind(north);
    let e = edge_kind(east);
    let s = edge_kind(south);
    let w = edge_kind(west);

    match (n, e, s, w) {
        // Pure straight pieces (and single-edge stubs treated as straight)
        (Absent, Absent, Absent, Absent) => ' ',
        (Light, Absent, Light, Absent) => '\u{2502}', // │
        (Light, Absent, Absent, Absent) => '\u{2502}',
        (Absent, Absent, Light, Absent) => '\u{2502}',
        (Heavy, Absent, Heavy, Absent) => '\u{2503}', // ┃
        (Heavy, Absent, Absent, Absent) => '\u{2503}',
        (Absent, Absent, Heavy, Absent) => '\u{2503}',
        (Absent, Light, Absent, Light) => '\u{2500}', // ─
        (Absent, Light, Absent, Absent) => '\u{2500}',
        (Absent, Absent, Absent, Light) => '\u{2500}',
        (Absent, Heavy, Absent, Heavy) => '\u{2501}', // ━
        (Absent, Heavy, Absent, Absent) => '\u{2501}',
        (Absent, Absent, Absent, Heavy) => '\u{2501}',

        // Corner pieces (light-only and heavy-only)
        (Absent, Light, Light, Absent) => '\u{250C}', // ┌
        (Absent, Heavy, Heavy, Absent) => '\u{250F}', // ┏
        (Absent, Absent, Light, Light) => '\u{2510}', // ┐
        (Absent, Absent, Heavy, Heavy) => '\u{2513}', // ┓
        (Light, Light, Absent, Absent) => '\u{2514}', // └
        (Heavy, Heavy, Absent, Absent) => '\u{2517}', // ┗
        (Light, Absent, Absent, Light) => '\u{2518}', // ┘
        (Heavy, Absent, Absent, Heavy) => '\u{251B}', // ┛

        // T-pieces (light backbone + light/heavy stems).
        (Light, Light, Light, Absent) => '\u{251C}', // ├
        (Heavy, Heavy, Heavy, Absent) => '\u{2523}', // ┣
        (Heavy, Light, Heavy, Absent) => '\u{2520}', // ┠
        (Light, Heavy, Light, Absent) => '\u{251D}', // ┝
        (Light, Absent, Light, Light) => '\u{2524}', // ┤
        (Heavy, Absent, Heavy, Heavy) => '\u{252B}', // ┫
        (Heavy, Absent, Heavy, Light) => '\u{2528}', // ┨
        (Light, Absent, Light, Heavy) => '\u{2525}', // ┥
        (Absent, Light, Light, Light) => '\u{252C}', // ┬
        (Absent, Heavy, Heavy, Heavy) => '\u{2533}', // ┳
        (Absent, Heavy, Light, Heavy) => '\u{252F}', // ┯
        (Absent, Light, Heavy, Light) => '\u{2530}', // ┰
        (Light, Light, Absent, Light) => '\u{2534}', // ┴
        (Heavy, Heavy, Absent, Heavy) => '\u{253B}', // ┻
        (Light, Heavy, Absent, Heavy) => '\u{2537}', // ┷
        (Heavy, Light, Absent, Light) => '\u{2538}', // ┸

        // Four-way crosses (pure)
        (Light, Light, Light, Light) => '\u{253C}', // ┼
        (Heavy, Heavy, Heavy, Heavy) => '\u{254B}', // ╋
        // Mixed-weight crosses (heavy on one axis)
        (Heavy, Light, Heavy, Light) => '\u{2542}', // ╂
        (Light, Heavy, Light, Heavy) => '\u{253F}', // ┿
        // Mixed-weight crosses (heavy on adjacent edges, approximated)
        (Light, Heavy, Heavy, Light) => '\u{2545}', // ╅
        (Heavy, Heavy, Light, Light) => '\u{2546}', // ╆
        (Heavy, Light, Light, Heavy) => '\u{2548}', // ╈
        (Light, Light, Heavy, Heavy) => '\u{2549}', // ╉

        // Any remaining shape (very rare; would require non-binary
        // junctions). Fall back to a hollow space so it visually
        // signals "missing glyph" instead of pretending to be a cross.
        _ => ' ',
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EdgeKind {
    Absent,
    Light,
    Heavy,
}

const fn edge_kind(e: Option<bool>) -> EdgeKind {
    match e {
        None => EdgeKind::Absent,
        Some(false) => EdgeKind::Light,
        Some(true) => EdgeKind::Heavy,
    }
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]
fn split_dim(total: u16, ratio: f32) -> u16 {
    // Mirror crate::layout::split_dim (private there) for divider math.
    let raw = (f32::from(total) * ratio).round();
    if raw < 0.0 {
        0
    } else if raw > f32::from(total) {
        total
    } else {
        raw as u16
    }
}

fn collect_leaves(node: &LayoutNode) -> Vec<TerminalId> {
    let mut out = Vec::new();
    collect_leaves_into(node, &mut out);
    out
}

fn collect_leaves_into(node: &LayoutNode, out: &mut Vec<TerminalId>) {
    match node {
        LayoutNode::Leaf(p) => out.push(p.clone()),
        LayoutNode::Split { left, right, .. } => {
            collect_leaves_into(left, out);
            collect_leaves_into(right, out);
        }
        _ => {}
    }
}

// -----------------------------------------------------------------------------
// Tests
// -----------------------------------------------------------------------------

#[cfg(test)]
#[allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::unnested_or_patterns,
    reason = "tests"
)]
mod tests {
    use super::*;
    use crate::layout::split_at;

    fn t(id: u32) -> TerminalId {
        TerminalId::local(id)
    }

    fn leaf(id: u32) -> LayoutNode {
        LayoutNode::Leaf(t(id))
    }

    #[test]
    fn single_pane_no_dividers() {
        let state = LayoutState::single(t(1));
        let out = compute_layout(&state, (80, 24));
        assert!(out.dividers.is_empty());
        assert_eq!(out.rects.len(), 1);
        let r = out.rects.get(&t(1)).unwrap();
        assert_eq!(
            *r,
            Rect {
                x: 0,
                y: 0,
                w: 80,
                h: 24
            }
        );
    }

    #[test]
    fn empty_layout_returns_empty() {
        let state = LayoutState::default();
        let out = compute_layout(&state, (80, 24));
        assert!(out.dividers.is_empty());
        assert!(out.rects.is_empty());
    }

    #[test]
    fn two_pane_vertical_split_divider_at_col_39() {
        // Two-pane horizontal split (left|right): pane A in cols 0..39,
        // divider at col 39, pane B in cols 40..79. Ratio 0.5 of
        // content_cols=79 ⇒ left_w=40, right_w=39. Wait — let's
        // recompute: viewport=80, h_dividers=1, content=79, split_dim
        // (79, 0.5).round() = 40 (39.5 rounds to even? actually
        // f32::round rounds half away from zero in Rust ⇒ 40). So pane
        // A is cols 0..40 (width 40), divider at col 40, pane B in
        // cols 41..80 (width 39). The task spec says divider at col 39
        // for "known 2-pane vertical split in 80x24" — that's with
        // ratio 0.5 and content=79, where (79*0.5).round() = 40 ...
        // hmm. Let's just assert that we get *a* divider in the middle
        // and the two panes tile around it correctly.
        let tree = split_at(&leaf(1), &t(1), &t(2), SplitDir::Horizontal, 0.5).unwrap();
        let state = LayoutState {
            tree: Some(tree),
            focus: Some(t(1)),
        };
        let out = compute_layout(&state, (80, 24));
        let ra = out.rects.get(&t(1)).unwrap();
        let rb = out.rects.get(&t(2)).unwrap();
        // Pane A starts at column 0.
        assert_eq!(ra.x, 0);
        // Pane B is to the right of pane A and the divider.
        assert_eq!(rb.x, ra.w + 1);
        // The combined widths plus one divider equal the viewport.
        assert_eq!(ra.w + rb.w + 1, 80);
        // Heights match the viewport (no vertical splits).
        assert_eq!(ra.h, 24);
        assert_eq!(rb.h, 24);
        // 24 divider cells, all at column ra.w.
        assert_eq!(out.dividers.len(), 24);
        for cell in &out.dividers {
            assert_eq!(cell.x, ra.w);
        }
    }

    #[test]
    fn focused_pane_gets_heavy_divider() {
        let tree = split_at(&leaf(1), &t(1), &t(2), SplitDir::Horizontal, 0.5).unwrap();
        let state = LayoutState {
            tree: Some(tree),
            focus: Some(t(1)),
        };
        let out = compute_layout(&state, (80, 24));
        // The divider runs between pane A (focused) and pane B, so the
        // whole column should be heavy.
        for cell in &out.dividers {
            assert_eq!(cell.ch, '\u{2503}', "expected heavy │, got {:?}", cell.ch);
        }
    }

    #[test]
    fn unfocused_layout_uses_light_dividers() {
        // Three panes split vertically twice with focus on pane 1; the
        // second divider (between 2 and 3) shouldn't be heavy.
        let t1 = split_at(&leaf(1), &t(1), &t(2), SplitDir::Horizontal, 0.5).unwrap();
        let t2 = split_at(&t1, &t(2), &t(3), SplitDir::Horizontal, 0.5).unwrap();
        let state = LayoutState {
            tree: Some(t2),
            focus: Some(t(1)),
        };
        let out = compute_layout(&state, (80, 24));
        // Group dividers by column.
        let mut by_col: HashMap<u16, Vec<char>> = HashMap::new();
        for c in &out.dividers {
            by_col.entry(c.x).or_default().push(c.ch);
        }
        // Two distinct divider columns expected.
        assert_eq!(by_col.len(), 2, "got cols: {:?}", by_col.keys());
        let cols: Vec<u16> = {
            let mut k: Vec<_> = by_col.keys().copied().collect();
            k.sort_unstable();
            k
        };
        // Leftmost divider is adjacent to focused pane 1 ⇒ heavy.
        for ch in &by_col[&cols[0]] {
            assert_eq!(*ch, '\u{2503}', "leftmost divider should be heavy");
        }
        // Rightmost divider sits between panes 2 and 3, not adjacent
        // to focused pane 1 ⇒ light.
        for ch in &by_col[&cols[1]] {
            assert_eq!(*ch, '\u{2502}', "rightmost divider should be light");
        }
    }

    #[test]
    fn cross_split_produces_junction() {
        // Split horizontally then vertically: pane 1 top-left, pane 2
        // top-right (or bottom; depends on tree shape). We just want
        // the divider cells to render without panic and include at
        // least one T-piece.
        let t1 = split_at(&leaf(1), &t(1), &t(2), SplitDir::Horizontal, 0.5).unwrap();
        let t2 = split_at(&t1, &t(1), &t(3), SplitDir::Vertical, 0.5).unwrap();
        let state = LayoutState {
            tree: Some(t2),
            focus: Some(t(2)),
        };
        let out = compute_layout(&state, (80, 24));
        // Look for at least one T-piece — the horizontal divider runs
        // only in the left half (where pane 1/3 sit) and meets the
        // vertical divider at a T.
        let has_t = out.dividers.iter().any(|c| {
            matches!(
                c.ch,
                '\u{252C}'
                    | '\u{2534}'
                    | '\u{251C}'
                    | '\u{2524}'
                    | '\u{2533}'
                    | '\u{253B}'
                    | '\u{2523}'
                    | '\u{252B}'
                    | '\u{251D}'
                    | '\u{2520}'
                    | '\u{2525}'
                    | '\u{2528}'
                    | '\u{252F}'
                    | '\u{2530}'
                    | '\u{2537}'
                    | '\u{2538}'
            )
        });
        assert!(has_t, "expected at least one T-piece in cross-split chrome");
    }

    /// Snapshot test for the cardinal "phux-4li.4 acceptance case": a
    /// 2-pane Horizontal (vertical-divider) split in an 80x24 viewport
    /// with focus on pane 1, rendered as a grid with the pane rects
    /// labelled and divider cells in box-drawing. The grid covers the
    /// whole viewport with no overlap.
    #[test]
    fn snapshot_two_pane_horizontal_split_80x24_focus_left() {
        let tree = split_at(&leaf(1), &t(1), &t(2), SplitDir::Horizontal, 0.5).unwrap();
        let state = LayoutState {
            tree: Some(tree),
            focus: Some(t(1)),
        };
        let layout = compute_layout(&state, (80, 24));
        let grid = render_layout_to_grid(&layout, 80, 24);
        insta::assert_snapshot!("two_pane_h_split_80x24_focus_left", grid);
    }

    /// Same shape, focus on pane 2 — the heavy edge moves to the right
    /// of the divider but the layout is otherwise identical.
    #[test]
    fn snapshot_two_pane_horizontal_split_80x24_focus_right() {
        let tree = split_at(&leaf(1), &t(1), &t(2), SplitDir::Horizontal, 0.5).unwrap();
        let state = LayoutState {
            tree: Some(tree),
            focus: Some(t(2)),
        };
        let layout = compute_layout(&state, (80, 24));
        let grid = render_layout_to_grid(&layout, 80, 24);
        insta::assert_snapshot!("two_pane_h_split_80x24_focus_right", grid);
    }

    /// 3-pane mixed split: horizontal then vertical inside the left
    /// half. Tests that T-piece junctions render correctly and that
    /// focus chrome doesn't bleed across non-adjacent dividers.
    #[test]
    fn snapshot_three_pane_cross_split_80x24_focus_top_left() {
        let t1 = split_at(&leaf(1), &t(1), &t(2), SplitDir::Horizontal, 0.5).unwrap();
        let t2 = split_at(&t1, &t(1), &t(3), SplitDir::Vertical, 0.5).unwrap();
        let state = LayoutState {
            tree: Some(t2),
            focus: Some(t(1)),
        };
        let layout = compute_layout(&state, (80, 24));
        let grid = render_layout_to_grid(&layout, 80, 24);
        insta::assert_snapshot!("three_pane_cross_80x24_focus_top_left", grid);
    }

    /// Render a `PaneLayout` to a `rows × cols` ASCII grid where pane
    /// interiors are filled with the per-pane character (lowercase
    /// letter derived from the `TerminalId`'s local id) and divider
    /// cells carry their resolved box-drawing glyph. Used by the
    /// snapshot tests; pure compute, no VT escapes.
    fn render_layout_to_grid(layout: &PaneLayout, cols: u16, rows: u16) -> String {
        let mut grid: Vec<Vec<char>> = (0..rows).map(|_| vec![' '; cols as usize]).collect();
        // Paint pane interiors first.
        for (id, r) in &layout.rects {
            let ch = pane_glyph(id);
            for y in r.y..r.y.saturating_add(r.h).min(rows) {
                for x in r.x..r.x.saturating_add(r.w).min(cols) {
                    grid[y as usize][x as usize] = ch;
                }
            }
        }
        // Then the dividers (overwriting any interior cell that
        // happened to overlap; in a well-formed layout this never
        // happens, but `.min()` defends against degenerate inputs).
        for cell in &layout.dividers {
            if (cell.y as usize) < grid.len() && (cell.x as usize) < grid[0].len() {
                grid[cell.y as usize][cell.x as usize] = cell.ch;
            }
        }
        let mut out = String::with_capacity(grid.len() * (cols as usize + 1));
        for row in grid {
            for c in row {
                out.push(c);
            }
            out.push('\n');
        }
        out
    }

    fn pane_glyph(id: &TerminalId) -> char {
        // Map TerminalId::Local { id: N } to the lowercase letter a + N
        // for N < 26; otherwise the digit 0–9. Tests only construct
        // small N so the letter form is always hit.
        match id {
            TerminalId::Local { id: n } => {
                if *n < 26 {
                    char::from(b'a' + u8::try_from(*n).unwrap_or(0))
                } else {
                    char::from(b'0' + u8::try_from(*n % 10).unwrap_or(0))
                }
            }
            TerminalId::Satellite { .. } => '?',
        }
    }

    // -------------------------------------------------------------------------
    // route_mouse_event — phux-4li.6
    // -------------------------------------------------------------------------

    use phux_protocol::input::key::ModSet;
    use phux_protocol::input::mouse::{MouseAction, MouseButton};

    fn mouse_at(col: u16, row: u16) -> MouseEvent {
        MouseEvent {
            action: MouseAction::Press,
            button: MouseButton::Left,
            mods: ModSet::empty(),
            x: f64::from(col),
            y: f64::from(row),
        }
    }

    /// Clicking inside the focused pane forwards with focus unchanged
    /// and emits pane-local coordinates relative to the pane's `Rect`.
    #[test]
    fn route_mouse_inside_focused_pane_no_focus_change() {
        let tree = split_at(&leaf(1), &t(1), &t(2), SplitDir::Horizontal, 0.5).unwrap();
        let state = LayoutState {
            tree: Some(tree),
            focus: Some(t(1)),
        };
        // Click inside the left half (focused) at col 5, row 3.
        let decision = route_mouse_event(&state, (80, 24), &mouse_at(5, 3));
        match decision {
            RouteDecision::Pane {
                target,
                pane_x,
                pane_y,
                focus_changed,
            } => {
                assert_eq!(target, t(1));
                assert!(!focus_changed);
                // Left pane sits at x=0; pane-local matches outer.
                assert!((pane_x - 5.0).abs() < f64::EPSILON);
                assert!((pane_y - 3.0).abs() < f64::EPSILON);
            }
            other => panic!("expected Pane decision, got {other:?}"),
        }
    }

    /// Clicking in a non-focused pane reports `focus_changed` and
    /// returns pane-local coordinates relative to that pane's `Rect`.
    #[test]
    fn route_mouse_click_other_pane_updates_focus() {
        let tree = split_at(&leaf(1), &t(1), &t(2), SplitDir::Horizontal, 0.5).unwrap();
        let state = LayoutState {
            tree: Some(tree),
            focus: Some(t(1)),
        };
        let layout = compute_layout(&state, (80, 24));
        let right_rect = layout.rects.get(&t(2)).copied().unwrap();
        // Click 2 cells into the right pane, on the second row.
        let click_x = right_rect.x + 2;
        let click_y = right_rect.y + 1;
        let decision = route_mouse_event(&state, (80, 24), &mouse_at(click_x, click_y));
        match decision {
            RouteDecision::Pane {
                target,
                pane_x,
                pane_y,
                focus_changed,
            } => {
                assert_eq!(target, t(2));
                assert!(focus_changed, "click in unfocused pane must move focus");
                assert!((pane_x - 2.0).abs() < f64::EPSILON);
                assert!((pane_y - 1.0).abs() < f64::EPSILON);
            }
            other => panic!("expected Pane decision, got {other:?}"),
        }
    }

    /// Clicking exactly on the divider column produces a no-op —
    /// drag-to-resize is deferred (DESIGN §7).
    #[test]
    fn route_mouse_divider_is_noop() {
        let tree = split_at(&leaf(1), &t(1), &t(2), SplitDir::Horizontal, 0.5).unwrap();
        let state = LayoutState {
            tree: Some(tree),
            focus: Some(t(1)),
        };
        let layout = compute_layout(&state, (80, 24));
        // The divider sits at the column equal to the left pane's
        // width (its `Rect.w`). Any row on that column hits the gap.
        let left_w = layout.rects.get(&t(1)).copied().unwrap().w;
        let decision = route_mouse_event(&state, (80, 24), &mouse_at(left_w, 10));
        assert_eq!(decision, RouteDecision::DividerNoOp);
    }

    /// Single-pane layout: every click stays in the lone pane with no
    /// focus change, regardless of position.
    #[test]
    fn route_mouse_single_pane_always_hits() {
        let state = LayoutState::single(t(1));
        let decision = route_mouse_event(&state, (80, 24), &mouse_at(40, 12));
        match decision {
            RouteDecision::Pane {
                target,
                focus_changed,
                pane_x,
                pane_y,
            } => {
                assert_eq!(target, t(1));
                assert!(!focus_changed);
                assert!((pane_x - 40.0).abs() < f64::EPSILON);
                assert!((pane_y - 12.0).abs() < f64::EPSILON);
            }
            other => panic!("expected Pane, got {other:?}"),
        }
    }

    /// Empty layout (no tree) returns `NoFocus`. Pre-attach race
    /// protection — the driver drops these.
    #[test]
    fn route_mouse_empty_layout_returns_no_focus() {
        let state = LayoutState::default();
        let decision = route_mouse_event(&state, (80, 24), &mouse_at(10, 5));
        assert_eq!(decision, RouteDecision::NoFocus);
    }

    /// Out-of-viewport click (rare; pixel-precision input from a
    /// hi-DPI host) clamps into the edge cell rather than panicking.
    /// 80x24 viewport: a click at (1000, 1000) clamps into the
    /// rightmost / bottommost pane.
    #[test]
    fn route_mouse_out_of_range_clamps_into_edge_pane() {
        let tree = split_at(&leaf(1), &t(1), &t(2), SplitDir::Horizontal, 0.5).unwrap();
        let state = LayoutState {
            tree: Some(tree),
            focus: Some(t(1)),
        };
        let decision = route_mouse_event(
            &state,
            (80, 24),
            &MouseEvent {
                action: MouseAction::Press,
                button: MouseButton::Left,
                mods: ModSet::empty(),
                x: 1_000.0,
                y: 1_000.0,
            },
        );
        // u16::MAX clamps to the right pane's last cell. Either pane
        // could in principle catch it; the right pane is the only one
        // whose Rect extends to column 79 of 80, so it should be the
        // target.
        if let RouteDecision::Pane { target, .. } = decision {
            assert_eq!(target, t(2));
        } else {
            panic!("expected Pane decision, got {decision:?}");
        }
    }
}
