use std::collections::HashMap;

use phux_protocol::TerminalId;

use crate::layout::{LayoutNode, LayoutState, Rect};

use super::rasterize::{DividerCell, DividerSegment, rasterize, walk_layout};

/// Result of [`compute_layout`]: per-pane rectangles plus the cells
/// occupied by dividers between them.
///
/// The pane `Rect`s are exactly those [`pane_rects`] returns; the divider
/// cells fill the **gaps** the layout algorithm carved out for them.
/// Together they cover the outer viewport with no overlap and no holes —
/// the `proptest_rects_and_dividers_tile_exactly` invariant.
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

/// Per-leaf rectangle map for `tree` inside a `viewport_dims` outer
/// viewport — the canonical client tiling.
///
/// This is the exact same local-divider walk [`compute_layout`] paints
/// with, minus the divider rasterization. Reflow-emit
/// (`phux_client::attach::reflow`) and the min-cell gate
/// (`phux_client::attach::actions`) both call it so the size a pane's
/// PTY is told to be (via `TERMINAL_RESIZE`) equals the rect it is
/// painted into, by construction — closing the gap/overlap class of bug
/// that arose when reflow and paint used divergent algorithms.
///
/// Every leaf of `tree` receives a rect; sub-viable splits yield
/// zero-size rects rather than dropping leaves (the same exact-tiling
/// walk [`compute_layout`] uses). `viewport_dims` is the **outer** viewport —
/// divider accounting happens *inside* the walk, so callers pass the
/// full pane viewport, never a pre-deducted content rectangle.
#[must_use]
pub fn pane_rects(tree: &LayoutNode, viewport_dims: (u16, u16)) -> HashMap<TerminalId, Rect> {
    let mut segments: Vec<DividerSegment> = Vec::new();
    let mut rects: HashMap<TerminalId, Rect> = HashMap::new();
    walk_layout(
        tree,
        Rect {
            x: 0,
            y: 0,
            w: viewport_dims.0,
            h: viewport_dims.1,
        },
        &mut segments,
        &mut rects,
    );
    rects
}
