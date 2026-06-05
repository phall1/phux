use std::collections::HashMap;

use phux_protocol::TerminalId;

use crate::layout::{LayoutState, Rect};

use super::rasterize::{DividerCell, DividerSegment, rasterize, walk_layout};

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
