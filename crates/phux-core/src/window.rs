//! [`Window`] — a session's tab-like container of panes.
//!
//! The layout is a binary split tree ([`LayoutNode`]). Each interior
//! [`LayoutNode::Split`] divides its rectangle along one axis at a `ratio`;
//! each [`LayoutNode::Leaf`] is a single pane. The tree is the auxiliary
//! structure; [`Window::panes`] remains the insertion-ordered source of
//! truth for which panes are in the window.
//!
//! Spec ref: `SPEC.md` §10.3 Layout (binary subset; `TABBED` is reserved for
//! a later version and is intentionally absent here).

use std::collections::HashMap;

use thiserror::Error;

use crate::ids::{PaneId, SessionId, WindowId};

/// A window: an ordered collection of panes belonging to a session.
///
/// `panes` is the insertion-ordered source of truth. `layout` is a binary
/// split tree over the same set of panes; the two are kept in sync by
/// [`Window::split`] and [`Window::kill_pane`] (and by the
/// [`Registry`](crate::registry::Registry) that owns the [`Window`]).
#[derive(Debug, Clone)]
pub struct Window {
    /// The stable identifier issued by the [`Registry`].
    ///
    /// [`Registry`]: crate::registry::Registry
    pub id: WindowId,
    /// The session that owns this window.
    pub session: SessionId,
    /// Panes belonging to this window, in insertion order.
    pub panes: Vec<PaneId>,
    /// The pane layout as a binary split tree, or `None` when no panes exist.
    pub layout: Option<LayoutNode>,
    /// The currently focused pane, if any.
    pub active: Option<PaneId>,
}

/// A node in the binary split tree.
///
/// A `Leaf` holds a single [`PaneId`]; a `Split` divides its rectangle
/// between two children along [`SplitDir`] at `ratio` (the left/top child
/// gets `ratio` of the parent's dimension along the split axis).
#[derive(Debug, Clone, PartialEq)]
pub enum LayoutNode {
    /// A single pane — the recursion base.
    Leaf(PaneId),
    /// An interior node that splits its rectangle in two.
    Split {
        /// The axis the split is taken along.
        dir: SplitDir,
        /// Fraction of the parent dim given to `left` (range `0.0..=1.0`).
        ratio: f32,
        /// Left (for [`SplitDir::Horizontal`]) or top (for [`SplitDir::Vertical`]) child.
        left: Box<LayoutNode>,
        /// Right (for [`SplitDir::Horizontal`]) or bottom (for [`SplitDir::Vertical`]) child.
        right: Box<LayoutNode>,
    },
}

/// Axis along which a [`LayoutNode::Split`] divides its rectangle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitDir {
    /// Split side-by-side (a vertical bar between left and right).
    Horizontal,
    /// Split stacked (a horizontal bar between top and bottom).
    Vertical,
}

/// Cardinal direction for focus movement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// Move focus upward.
    Up,
    /// Move focus downward.
    Down,
    /// Move focus left.
    Left,
    /// Move focus right.
    Right,
}

/// An axis-aligned rectangle in cell coordinates.
///
/// `x`/`y` are the top-left corner; `w`/`h` are the width and height. The
/// origin is the window's top-left.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    /// Top-left x coordinate (column).
    pub x: u16,
    /// Top-left y coordinate (row).
    pub y: u16,
    /// Width in cells.
    pub w: u16,
    /// Height in cells.
    pub h: u16,
}

/// Errors returned by layout operations on a [`Window`].
#[derive(Debug, Clone, Copy, PartialEq, Error)]
pub enum LayoutError {
    /// The target [`PaneId`] is not present in this window's layout.
    #[error("pane not in layout: {0:?}")]
    PaneNotInLayout(PaneId),
    /// The requested split ratio is outside the half-open `(0.0, 1.0)` range,
    /// or is NaN.
    #[error("invalid split ratio: {0}")]
    InvalidRatio(f32),
    /// The layout has only one pane — `kill_pane` would empty the window.
    /// Callers may choose to remove the window itself instead.
    #[error("cannot kill the last pane in the layout")]
    LastPane,
}

impl LayoutNode {
    /// Return `true` if this subtree contains a [`Leaf`] for `pane`.
    ///
    /// [`Leaf`]: LayoutNode::Leaf
    #[must_use]
    pub fn contains(&self, pane: PaneId) -> bool {
        match self {
            Self::Leaf(p) => *p == pane,
            Self::Split { left, right, .. } => left.contains(pane) || right.contains(pane),
        }
    }

    /// Collect every [`PaneId`] in this subtree in left-to-right traversal order.
    #[must_use]
    pub fn leaves(&self) -> Vec<PaneId> {
        let mut out = Vec::new();
        self.collect_leaves(&mut out);
        out
    }

    fn collect_leaves(&self, out: &mut Vec<PaneId>) {
        match self {
            Self::Leaf(p) => out.push(*p),
            Self::Split { left, right, .. } => {
                left.collect_leaves(out);
                right.collect_leaves(out);
            }
        }
    }

    /// Split the [`Leaf`] for `target` into a [`Split`] whose `left` keeps
    /// `target` and whose `right` is a new [`Leaf`] for `new_pane`.
    ///
    /// Returns `Ok(())` if `target` was found and replaced, or
    /// [`LayoutError::PaneNotInLayout`] otherwise.
    ///
    /// [`Leaf`]: LayoutNode::Leaf
    /// [`Split`]: LayoutNode::Split
    fn split_at(
        &mut self,
        target: PaneId,
        new_pane: PaneId,
        dir: SplitDir,
        ratio: f32,
    ) -> Result<(), LayoutError> {
        match self {
            Self::Leaf(p) if *p == target => {
                *self = Self::Split {
                    dir,
                    ratio,
                    left: Box::new(Self::Leaf(target)),
                    right: Box::new(Self::Leaf(new_pane)),
                };
                Ok(())
            }
            Self::Leaf(_) => Err(LayoutError::PaneNotInLayout(target)),
            Self::Split { left, right, .. } => {
                if left.contains(target) {
                    left.split_at(target, new_pane, dir, ratio)
                } else if right.contains(target) {
                    right.split_at(target, new_pane, dir, ratio)
                } else {
                    Err(LayoutError::PaneNotInLayout(target))
                }
            }
        }
    }

    /// Recursively compute pane rectangles given the bounding rectangle.
    ///
    /// Rounding ensures the children's dims sum exactly to the parent's
    /// dim along the split axis — no slop, no overlap.
    fn fill_rects(&self, bounds: Rect, out: &mut HashMap<PaneId, Rect>) {
        match self {
            Self::Leaf(p) => {
                out.insert(*p, bounds);
            }
            Self::Split {
                dir,
                ratio,
                left,
                right,
            } => match dir {
                SplitDir::Horizontal => {
                    let left_w = split_dim(bounds.w, *ratio);
                    let right_w = bounds.w - left_w;
                    let left_rect = Rect {
                        x: bounds.x,
                        y: bounds.y,
                        w: left_w,
                        h: bounds.h,
                    };
                    let right_rect = Rect {
                        x: bounds.x + left_w,
                        y: bounds.y,
                        w: right_w,
                        h: bounds.h,
                    };
                    left.fill_rects(left_rect, out);
                    right.fill_rects(right_rect, out);
                }
                SplitDir::Vertical => {
                    let top_h = split_dim(bounds.h, *ratio);
                    let bot_h = bounds.h - top_h;
                    let top_rect = Rect {
                        x: bounds.x,
                        y: bounds.y,
                        w: bounds.w,
                        h: top_h,
                    };
                    let bot_rect = Rect {
                        x: bounds.x,
                        y: bounds.y + top_h,
                        w: bounds.w,
                        h: bot_h,
                    };
                    left.fill_rects(top_rect, out);
                    right.fill_rects(bot_rect, out);
                }
            },
        }
    }
}

/// Compute the left/top child's dim, clamped so the right/bottom child's dim
/// (`total - returned`) is non-negative.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]
fn split_dim(total: u16, ratio: f32) -> u16 {
    let raw = (f32::from(total) * ratio).round();
    if raw < 0.0 {
        0
    } else if raw > f32::from(total) {
        total
    } else {
        raw as u16
    }
}

fn validate_ratio(ratio: f32) -> Result<(), LayoutError> {
    if ratio.is_nan() || ratio <= 0.0 || ratio >= 1.0 {
        Err(LayoutError::InvalidRatio(ratio))
    } else {
        Ok(())
    }
}

impl Window {
    /// Split the leaf for `target` into two, placing `new_pane` as the new
    /// sibling along `dir` with the given `ratio`.
    ///
    /// On success the layout grows by one [`Leaf`](LayoutNode::Leaf) and one
    /// [`Split`](LayoutNode::Split); `target` and `new_pane` are siblings.
    ///
    /// # Errors
    /// * [`LayoutError::PaneNotInLayout`] if `target` is not present.
    /// * [`LayoutError::InvalidRatio`] if `ratio` is NaN or outside `(0, 1)`.
    pub fn split(
        &mut self,
        target: PaneId,
        new_pane: PaneId,
        dir: SplitDir,
        ratio: f32,
    ) -> Result<(), LayoutError> {
        validate_ratio(ratio)?;
        let layout = self
            .layout
            .as_mut()
            .ok_or(LayoutError::PaneNotInLayout(target))?;
        layout.split_at(target, new_pane, dir, ratio)
    }

    /// Initialize the layout with `pane` as the sole [`Leaf`](LayoutNode::Leaf).
    ///
    /// Idempotent only when the layout is currently empty; if the window
    /// already has a layout this returns [`LayoutError::PaneNotInLayout`]
    /// (the caller should use [`Window::split`] instead).
    ///
    /// # Errors
    /// Returns [`LayoutError::PaneNotInLayout`] if the layout is already
    /// initialized — a guard against silently clobbering the tree.
    pub fn seed_layout(&mut self, pane: PaneId) -> Result<(), LayoutError> {
        if self.layout.is_some() {
            return Err(LayoutError::PaneNotInLayout(pane));
        }
        self.layout = Some(LayoutNode::Leaf(pane));
        Ok(())
    }

    /// Remove the leaf for `target` from the layout, collapsing its parent
    /// [`Split`](LayoutNode::Split) so the remaining sibling takes its
    /// grandparent's slot.
    ///
    /// # Errors
    /// * [`LayoutError::PaneNotInLayout`] if `target` is not present.
    /// * [`LayoutError::LastPane`] if `target` is the only leaf — the caller
    ///   must remove the whole window.
    pub fn kill_pane(&mut self, target: PaneId) -> Result<(), LayoutError> {
        let Some(layout) = self.layout.as_mut() else {
            return Err(LayoutError::PaneNotInLayout(target));
        };
        match layout {
            LayoutNode::Leaf(p) if *p == target => {
                self.layout = None;
                Err(LayoutError::LastPane)
            }
            LayoutNode::Leaf(_) => Err(LayoutError::PaneNotInLayout(target)),
            LayoutNode::Split { .. } => {
                // Replace `layout` with the collapsed subtree.
                let Some(owned) = self.layout.take() else {
                    // Unreachable: we matched Some(Split{..}) above.
                    return Err(LayoutError::PaneNotInLayout(target));
                };
                let (new_root, found) = collapse(owned, target);
                self.layout = Some(new_root);
                if found {
                    Ok(())
                } else {
                    Err(LayoutError::PaneNotInLayout(target))
                }
            }
        }
    }

    /// Return the neighbouring [`PaneId`] in `dir` from `current`, if any.
    ///
    /// The algorithm:
    /// 1. Record the root-to-leaf path of (Split, `ChildSide`) steps to
    ///    `current`.
    /// 2. Walk that path in reverse; the first Split whose axis matches
    ///    `dir` and from which we came from the appropriate side identifies
    ///    the boundary to cross.
    /// 3. Descend into the sibling subtree, choosing children whose split
    ///    axis is perpendicular to `dir` so as to preserve the source's
    ///    position on that axis; when the axis is parallel we hug the
    ///    shared edge.
    ///
    /// Returns `None` if `current` is not in the layout or if no neighbour
    /// exists in that direction.
    #[must_use]
    pub fn focus_direction(&self, current: PaneId, dir: Direction) -> Option<PaneId> {
        let layout = self.layout.as_ref()?;
        let mut path: Vec<(SplitDir, ChildSide)> = Vec::new();
        if !record_path(layout, current, &mut path) {
            return None;
        }
        // Walk up the path. At each step we exit a node into its parent.
        // The last entry corresponds to the deepest Split — the parent of
        // the current leaf.
        for i in (0..path.len()).rev() {
            let (split_dir, came_from) = path[i];
            if matches_to_sibling(split_dir, dir, came_from) {
                // Locate the sibling subtree: re-traverse the layout to depth `i`,
                // then take the *other* child.
                let sibling = sibling_at_depth(layout, &path, i)?;
                // The suffix below the boundary records perpendicular-axis
                // choices that locate the source within the sibling's
                // mirror. The prefix above the boundary is irrelevant for
                // perpendicular-axis preservation here.
                return Some(descend_to_leaf(sibling, dir, &path[i + 1..]));
            }
        }
        None
    }

    /// Compute the bounding rectangle of every leaf given the window dims.
    ///
    /// Returns an empty map if the layout is empty. Rectangles tile the
    /// `(0, 0, dims.0, dims.1)` rect exactly: dims sum to the parent's dim
    /// along the split axis at every interior node.
    #[must_use]
    pub fn pane_rects(&self, dims: (u16, u16)) -> HashMap<PaneId, Rect> {
        let mut out = HashMap::new();
        if let Some(layout) = self.layout.as_ref() {
            layout.fill_rects(
                Rect {
                    x: 0,
                    y: 0,
                    w: dims.0,
                    h: dims.1,
                },
                &mut out,
            );
        }
        out
    }
}

/// Walk `node`, removing the leaf for `target`, collapsing the parent Split
/// so the sibling takes its place. Returns the rewritten tree and whether
/// `target` was found.
fn collapse(node: LayoutNode, target: PaneId) -> (LayoutNode, bool) {
    match node {
        LayoutNode::Leaf(p) => (LayoutNode::Leaf(p), false),
        LayoutNode::Split {
            dir,
            ratio,
            left,
            right,
        } => {
            // If either direct child is the target leaf, collapse to the sibling.
            if let LayoutNode::Leaf(p) = *left
                && p == target
            {
                return (*right, true);
            }
            if let LayoutNode::Leaf(p) = *right
                && p == target
            {
                return (*left, true);
            }
            // Otherwise recurse.
            let (new_left, found_l) = collapse(*left, target);
            if found_l {
                return (
                    LayoutNode::Split {
                        dir,
                        ratio,
                        left: Box::new(new_left),
                        right,
                    },
                    true,
                );
            }
            let (new_right, found_r) = collapse(*right, target);
            (
                LayoutNode::Split {
                    dir,
                    ratio,
                    left: Box::new(new_left),
                    right: Box::new(new_right),
                },
                found_r,
            )
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ChildSide {
    Left,
    Right,
}

/// Record the root-to-leaf path to `target`. Returns `true` iff found.
fn record_path(node: &LayoutNode, target: PaneId, out: &mut Vec<(SplitDir, ChildSide)>) -> bool {
    match node {
        LayoutNode::Leaf(p) => *p == target,
        LayoutNode::Split {
            dir: sd,
            left,
            right,
            ..
        } => {
            out.push((*sd, ChildSide::Left));
            if record_path(left, target, out) {
                return true;
            }
            out.pop();
            out.push((*sd, ChildSide::Right));
            if record_path(right, target, out) {
                return true;
            }
            out.pop();
            false
        }
    }
}

/// Does a move in `dir` from `came_from` cross the [`SplitDir`] axis toward
/// the sibling?
const fn matches_to_sibling(split: SplitDir, dir: Direction, came_from: ChildSide) -> bool {
    matches!(
        (split, dir, came_from),
        (SplitDir::Horizontal, Direction::Right, ChildSide::Left)
            | (SplitDir::Horizontal, Direction::Left, ChildSide::Right)
            | (SplitDir::Vertical, Direction::Down, ChildSide::Left)
            | (SplitDir::Vertical, Direction::Up, ChildSide::Right)
    )
}

/// Re-traverse `root` along `path[..depth]` then return the sibling of
/// `path[depth].1`.
fn sibling_at_depth<'a>(
    root: &'a LayoutNode,
    path: &[(SplitDir, ChildSide)],
    depth: usize,
) -> Option<&'a LayoutNode> {
    let mut cur = root;
    for (_, side) in &path[..depth] {
        let LayoutNode::Split { left, right, .. } = cur else {
            return None;
        };
        cur = match side {
            ChildSide::Left => left,
            ChildSide::Right => right,
        };
    }
    let LayoutNode::Split { left, right, .. } = cur else {
        return None;
    };
    let (_, came_from) = path[depth];
    Some(match came_from {
        ChildSide::Left => right,
        ChildSide::Right => left,
    })
}

/// Descend into `node`, preserving the source's perpendicular-axis position
/// where possible. `suffix` is the source's path *below* the boundary
/// split (closer to the source leaf); reading it from the shallow end
/// gives the source's perpendicular-axis choice when the descent
/// encounters a perpendicular split.
///
/// When the descent encounters a Split parallel to `dir`, hug the shared
/// edge (leftmost/topmost for moves into a sibling rightward/downward;
/// rightmost/bottommost for the reverse).
fn descend_to_leaf(node: &LayoutNode, dir: Direction, suffix: &[(SplitDir, ChildSide)]) -> PaneId {
    let perp = perpendicular_axis(dir);
    // Hints in shallow→deep order — same order as we'll encounter them
    // during descent.
    let hints: Vec<ChildSide> = suffix
        .iter()
        .filter_map(|(sd, side)| if *sd == perp { Some(*side) } else { None })
        .collect();
    let mut hint_idx = 0;
    let mut cur = node;
    loop {
        match cur {
            LayoutNode::Leaf(p) => return *p,
            LayoutNode::Split {
                dir: sd,
                left,
                right,
                ..
            } => {
                if axis_parallel(*sd, dir) {
                    // Hug the shared edge.
                    cur = match dir {
                        Direction::Right | Direction::Down => left,
                        Direction::Left | Direction::Up => right,
                    };
                } else {
                    // Perpendicular: take the source's hint if available;
                    // otherwise default to Left.
                    let side = hints.get(hint_idx).copied().unwrap_or(ChildSide::Left);
                    hint_idx += 1;
                    cur = match side {
                        ChildSide::Left => left,
                        ChildSide::Right => right,
                    };
                }
            }
        }
    }
}

const fn axis_parallel(split: SplitDir, dir: Direction) -> bool {
    matches!(
        (split, dir),
        (SplitDir::Horizontal, Direction::Left | Direction::Right)
            | (SplitDir::Vertical, Direction::Up | Direction::Down)
    )
}

const fn perpendicular_axis(dir: Direction) -> SplitDir {
    match dir {
        Direction::Left | Direction::Right => SplitDir::Vertical,
        Direction::Up | Direction::Down => SplitDir::Horizontal,
    }
}
