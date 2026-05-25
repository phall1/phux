//! Layout-tree unit tests for [`Window`].
//!
//! Uses [`Registry`] to bootstrap real `PaneId`s, then exercises the
//! tree operations on the `Window` directly. The layout invariants live in
//! `tests/layout_proptest.rs`.

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::similar_names,
    clippy::doc_markdown
)]

use std::collections::HashSet;

use phux_core::{Direction, LayoutNode, PaneId, Rect, Registry, SplitDir};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Build a Registry seeded with one session, one window, and one initial
/// pane. Returns the (window_id, pane_id) plus the Registry.
fn seeded() -> (Registry, phux_core::WindowId, PaneId) {
    let mut reg = Registry::new();
    let s = reg.new_session("test".to_owned());
    let w = reg.new_window(s).expect("session exists");
    let p = reg.new_pane(w).expect("window exists");
    (reg, w, p)
}

// ---------------------------------------------------------------------------
// pane_rects
// ---------------------------------------------------------------------------

#[test]
fn single_pane_layout_is_leaf_and_fills_the_window() {
    let (reg, w, p) = seeded();
    let win = reg.window(w).expect("window exists");

    assert_eq!(win.layout, Some(LayoutNode::Leaf(p)));

    let rects = win.pane_rects((80, 24));
    assert_eq!(rects.len(), 1);
    assert_eq!(
        rects.get(&p).copied(),
        Some(Rect {
            x: 0,
            y: 0,
            w: 80,
            h: 24
        })
    );
}

#[test]
fn horizontal_split_half_tiles_exactly() {
    let (mut reg, w, p1) = seeded();
    // Allocate a second pane id by creating it then re-splitting at p1.
    let p2 = reg.new_pane(w).expect("window exists");
    // Reset the layout to a known shape via the Window API: byc.1's Registry
    // auto-splits, but we want a controlled state.
    {
        let win = reg.window_mut(w).expect("window exists");
        win.layout = Some(LayoutNode::Leaf(p1));
        win.split(p1, p2, SplitDir::Horizontal, 0.5)
            .expect("split p1");
    }

    let win = reg.window(w).expect("window exists");
    let rects = win.pane_rects((80, 24));
    assert_eq!(rects.len(), 2);
    let r1 = rects[&p1];
    let r2 = rects[&p2];
    // Horizontal split divides width: left keeps origin, right starts at left.w.
    assert_eq!(r1.x, 0);
    assert_eq!(r1.y, 0);
    assert_eq!(r1.h, 24);
    assert_eq!(r2.y, 0);
    assert_eq!(r2.h, 24);
    // Widths sum exactly to the parent.
    assert_eq!(u32::from(r1.w) + u32::from(r2.w), 80);
    // 0.5 ratio → r1.w == 40 (round-to-nearest).
    assert_eq!(r1.w, 40);
    assert_eq!(r2.w, 40);
    assert_eq!(r2.x, 40);
}

#[test]
fn vertical_split_half_tiles_exactly() {
    let (mut reg, w, p1) = seeded();
    let p2 = reg.new_pane(w).expect("window exists");
    {
        let win = reg.window_mut(w).expect("window exists");
        win.layout = Some(LayoutNode::Leaf(p1));
        win.split(p1, p2, SplitDir::Vertical, 0.5)
            .expect("split p1");
    }

    let win = reg.window(w).expect("window exists");
    let rects = win.pane_rects((80, 24));
    let r1 = rects[&p1];
    let r2 = rects[&p2];
    assert_eq!(r1.x, 0);
    assert_eq!(r2.x, 0);
    assert_eq!(r1.w, 80);
    assert_eq!(r2.w, 80);
    assert_eq!(u32::from(r1.h) + u32::from(r2.h), 24);
    assert_eq!(r1.y, 0);
    assert_eq!(r2.y, r1.h);
}

#[test]
fn split_with_awkward_ratio_still_tiles_exactly() {
    let (mut reg, w, p1) = seeded();
    let p2 = reg.new_pane(w).expect("window exists");
    {
        let win = reg.window_mut(w).expect("window exists");
        win.layout = Some(LayoutNode::Leaf(p1));
        // 0.33 of 100 is 33 (round-half-away-from-zero), so r1.w == 33,
        // r2.w == 67, and they sum to exactly 100 — no slop.
        win.split(p1, p2, SplitDir::Horizontal, 0.33)
            .expect("split p1");
    }

    let win = reg.window(w).expect("window exists");
    let rects = win.pane_rects((100, 24));
    let r1 = rects[&p1];
    let r2 = rects[&p2];
    assert_eq!(u32::from(r1.w) + u32::from(r2.w), 100);
}

// ---------------------------------------------------------------------------
// kill_pane
// ---------------------------------------------------------------------------

#[test]
fn kill_pane_collapses_parent_split() {
    let (mut reg, w, p1) = seeded();
    let p2 = reg.new_pane(w).expect("window exists");
    {
        let win = reg.window_mut(w).expect("window exists");
        win.layout = Some(LayoutNode::Leaf(p1));
        win.split(p1, p2, SplitDir::Horizontal, 0.5)
            .expect("split p1");
        // Tree is Split { Leaf(p1), Leaf(p2) }. Kill p1; the surviving leaf
        // should collapse up — layout becomes Leaf(p2).
        win.kill_pane(p1).expect("kill p1");
    }

    let win = reg.window(w).expect("window exists");
    assert_eq!(win.layout, Some(LayoutNode::Leaf(p2)));
}

#[test]
fn kill_pane_not_in_layout_errors() {
    let (mut reg, w, p1) = seeded();
    let bogus = PaneId::default();
    let win = reg.window_mut(w).expect("window exists");
    win.layout = Some(LayoutNode::Leaf(p1));
    let err = win.kill_pane(bogus).unwrap_err();
    assert!(matches!(
        err,
        phux_core::LayoutError::PaneNotInLayout(_) | phux_core::LayoutError::LastPane
    ));
}

#[test]
fn kill_last_pane_returns_last_pane_error() {
    let (mut reg, w, p1) = seeded();
    let win = reg.window_mut(w).expect("window exists");
    assert_eq!(win.layout, Some(LayoutNode::Leaf(p1)));
    let err = win.kill_pane(p1).unwrap_err();
    assert!(matches!(err, phux_core::LayoutError::LastPane));
}

// ---------------------------------------------------------------------------
// focus_direction
// ---------------------------------------------------------------------------

#[test]
fn focus_direction_across_balanced_grid() {
    // Build a 2x2 grid:
    //   Horizontal split at root:
    //     left  = Vertical { top: p_tl, bottom: p_bl }
    //     right = Vertical { top: p_tr, bottom: p_br }
    let (mut reg, w, p_tl) = seeded();
    let p_tr = reg.new_pane(w).expect("window exists");
    let p_bl = reg.new_pane(w).expect("window exists");
    let p_br = reg.new_pane(w).expect("window exists");
    {
        let win = reg.window_mut(w).expect("window exists");
        win.layout = Some(LayoutNode::Leaf(p_tl));
        win.split(p_tl, p_tr, SplitDir::Horizontal, 0.5)
            .expect("split tl/tr");
        win.split(p_tl, p_bl, SplitDir::Vertical, 0.5)
            .expect("split tl/bl");
        win.split(p_tr, p_br, SplitDir::Vertical, 0.5)
            .expect("split tr/br");
    }

    let win = reg.window(w).expect("window exists");

    // From the top-left:
    //   Right -> top-right
    //   Down  -> bottom-left
    //   Up    -> None (at top edge)
    //   Left  -> None (at left edge)
    assert_eq!(win.focus_direction(p_tl, Direction::Right), Some(p_tr));
    assert_eq!(win.focus_direction(p_tl, Direction::Down), Some(p_bl));
    assert_eq!(win.focus_direction(p_tl, Direction::Up), None);
    assert_eq!(win.focus_direction(p_tl, Direction::Left), None);

    // From the bottom-right:
    assert_eq!(win.focus_direction(p_br, Direction::Left), Some(p_bl));
    assert_eq!(win.focus_direction(p_br, Direction::Up), Some(p_tr));
    assert_eq!(win.focus_direction(p_br, Direction::Down), None);
    assert_eq!(win.focus_direction(p_br, Direction::Right), None);
}

// ---------------------------------------------------------------------------
// Internal consistency
// ---------------------------------------------------------------------------

#[test]
fn leaves_match_panes_after_a_sequence_of_splits() {
    let (mut reg, w, p1) = seeded();
    let p2 = reg.new_pane(w).expect("window exists");
    let p3 = reg.new_pane(w).expect("window exists");
    let p4 = reg.new_pane(w).expect("window exists");
    {
        let win = reg.window_mut(w).expect("window exists");
        win.layout = Some(LayoutNode::Leaf(p1));
        win.split(p1, p2, SplitDir::Horizontal, 0.5).unwrap();
        win.split(p2, p3, SplitDir::Vertical, 0.5).unwrap();
        win.split(p1, p4, SplitDir::Vertical, 0.5).unwrap();
    }

    let win = reg.window(w).expect("window exists");
    let leaves: HashSet<PaneId> = win.layout.as_ref().unwrap().leaves().into_iter().collect();
    let expected: HashSet<PaneId> = [p1, p2, p3, p4].iter().copied().collect();
    assert_eq!(leaves, expected);
}
