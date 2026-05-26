//! Proptest: layout-tree invariants under random `Registry` operations.
//!
//! The byc.1 registry proptest covers parent↔child symmetry. This one
//! adds the layout-specific invariants: `pane_rects` tiles exactly, and
//! `LayoutNode::leaves()` matches `Window::panes` as a set.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::collections::HashSet;

use phux_core::Registry;
use proptest::prelude::*;

#[derive(Debug, Clone, Copy)]
enum Op {
    AddPane,
    KillPaneAt(usize),
}

fn arb_op() -> impl Strategy<Value = Op> {
    prop_oneof![
        4 => Just(Op::AddPane),
        1 => (0_usize..16).prop_map(Op::KillPaneAt),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 64, ..ProptestConfig::default() })]

    #[test]
    fn pane_rects_tile_after_random_ops(ops in prop::collection::vec(arb_op(), 1..20)) {
        let mut reg = Registry::new();
        let s = reg.new_session("p".to_owned());
        let w = reg.new_window(s).unwrap();
        // Bootstrap one pane.
        reg.new_terminal(w).unwrap();

        for op in &ops {
            match op {
                Op::AddPane => {
                    let _ = reg.new_terminal(w);
                }
                Op::KillPaneAt(idx) => {
                    let target = {
                        let win = reg.window(w).unwrap();
                        if win.panes.is_empty() {
                            None
                        } else {
                            Some(win.panes[idx % win.panes.len()])
                        }
                    };
                    if let Some(t) = target {
                        let _ = reg.remove_terminal(t);
                    }
                }
            }
        }

        let win = reg.window(w).unwrap();

        // Invariant 1: layout leaves == window.panes (as sets).
        let panes_set: HashSet<_> = win.panes.iter().copied().collect();
        let leaf_set: HashSet<_> = win
            .layout
            .as_ref()
            .map_or_else(HashSet::new, |node| node.leaves().into_iter().collect());
        prop_assert_eq!(&panes_set, &leaf_set);

        // Invariant 2: with at least one pane, pane_rects tiles 80x24 exactly.
        if !panes_set.is_empty() {
            let rects = win.pane_rects((80, 24));
            let total: u32 = rects.values()
                .map(|r| u32::from(r.w) * u32::from(r.h))
                .sum();
            prop_assert_eq!(total, 80 * 24);

            // No overlap.
            let mut covered: HashSet<(u16, u16)> = HashSet::new();
            for r in rects.values() {
                for y in r.y..r.y.saturating_add(r.h) {
                    for x in r.x..r.x.saturating_add(r.w) {
                        prop_assert!(covered.insert((x, y)));
                    }
                }
            }
            prop_assert_eq!(covered.len(), 80 * 24);
        }
    }
}
