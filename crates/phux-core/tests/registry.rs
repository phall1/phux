//! Integration tests for [`phux_core::Registry`].

use phux_core::{PaneId, Registry, RegistryError, SessionId, WindowId};

#[test]
fn new_session_window_pane_chain_yields_distinct_lookups() {
    let mut reg = Registry::new();
    let s = reg.new_session("main".to_string());
    let w = reg.new_window(s).expect("session exists");
    let p = reg.new_pane(w).expect("window exists");

    assert!(reg.session(s).is_some());
    assert!(reg.window(w).is_some());
    let pane = reg.pane(p).expect("pane exists");
    assert_eq!(pane.id, p);
    assert_eq!(pane.window, w);

    // Parent linkage is bidirectional.
    let session = reg.session(s).expect("session exists");
    assert_eq!(session.windows, vec![w]);
    assert_eq!(session.active, Some(w));

    let window = reg.window(w).expect("window exists");
    assert_eq!(window.session, s);
    assert_eq!(window.panes, vec![p]);
    assert_eq!(window.layout.panes, vec![p]);
    assert_eq!(window.active, Some(p));
}

#[test]
fn new_window_against_unknown_session_errors() {
    let mut reg = Registry::new();
    let bogus: SessionId = SessionId::default();
    match reg.new_window(bogus) {
        Err(RegistryError::UnknownSession(id)) => assert_eq!(id, bogus),
        other => panic!("expected UnknownSession, got {other:?}"),
    }
}

#[test]
fn new_pane_against_unknown_window_errors() {
    let mut reg = Registry::new();
    let bogus: WindowId = WindowId::default();
    match reg.new_pane(bogus) {
        Err(RegistryError::UnknownWindow(id)) => assert_eq!(id, bogus),
        other => panic!("expected UnknownWindow, got {other:?}"),
    }
}

#[test]
fn remove_pane_invalidates_key_and_unlinks_from_window() {
    let mut reg = Registry::new();
    let s = reg.new_session("s".to_string());
    let w = reg.new_window(s).expect("session exists");
    let p1 = reg.new_pane(w).expect("window exists");
    let p2 = reg.new_pane(w).expect("window exists");

    let removed = reg.remove_pane(p1).expect("pane existed");
    assert_eq!(removed.id, p1);
    assert!(reg.pane(p1).is_none());

    let window = reg.window(w).expect("window exists");
    assert!(!window.panes.contains(&p1));
    assert!(!window.layout.panes.contains(&p1));
    assert_eq!(window.panes, vec![p2]);
    // Active rolled forward to the remaining pane.
    assert_eq!(window.active, Some(p2));
}

#[test]
fn remove_pane_clears_active_when_last() {
    let mut reg = Registry::new();
    let s = reg.new_session("s".to_string());
    let w = reg.new_window(s).expect("session exists");
    let p = reg.new_pane(w).expect("window exists");

    reg.remove_pane(p).expect("pane existed");
    let window = reg.window(w).expect("window exists");
    assert_eq!(window.active, None);
    assert!(window.panes.is_empty());
}

#[test]
fn remove_pane_unknown_returns_none() {
    let mut reg = Registry::new();
    let bogus: PaneId = PaneId::default();
    assert!(reg.remove_pane(bogus).is_none());
}

#[test]
fn remove_window_cascades_to_panes() {
    let mut reg = Registry::new();
    let s = reg.new_session("s".to_string());
    let w = reg.new_window(s).expect("session exists");
    let p1 = reg.new_pane(w).expect("window exists");
    let p2 = reg.new_pane(w).expect("window exists");

    let removed = reg.remove_window(w).expect("window existed");
    assert_eq!(removed.id, w);
    assert!(reg.window(w).is_none());
    assert!(reg.pane(p1).is_none());
    assert!(reg.pane(p2).is_none());

    let session = reg.session(s).expect("session exists");
    assert!(!session.windows.contains(&w));
    assert_eq!(session.active, None);
    assert_eq!(reg.pane_count(), 0);
}

#[test]
fn remove_window_active_rolls_forward() {
    let mut reg = Registry::new();
    let s = reg.new_session("s".to_string());
    let w1 = reg.new_window(s).expect("session exists");
    let w2 = reg.new_window(s).expect("session exists");

    reg.remove_window(w1).expect("window existed");
    let session = reg.session(s).expect("session exists");
    assert_eq!(session.active, Some(w2));
    assert_eq!(session.windows, vec![w2]);
}

#[test]
fn remove_session_cascades_fully() {
    let mut reg = Registry::new();
    let s = reg.new_session("s".to_string());
    let w1 = reg.new_window(s).expect("session exists");
    let w2 = reg.new_window(s).expect("session exists");
    let p1 = reg.new_pane(w1).expect("window exists");
    let p2 = reg.new_pane(w2).expect("window exists");
    let p3 = reg.new_pane(w2).expect("window exists");

    let removed = reg.remove_session(s).expect("session existed");
    assert_eq!(removed.id, s);
    assert!(reg.session(s).is_none());
    assert!(reg.window(w1).is_none());
    assert!(reg.window(w2).is_none());
    assert!(reg.pane(p1).is_none());
    assert!(reg.pane(p2).is_none());
    assert!(reg.pane(p3).is_none());
    assert_eq!(reg.session_count(), 0);
    assert_eq!(reg.window_count(), 0);
    assert_eq!(reg.pane_count(), 0);
}

#[test]
fn ids_are_distinct_across_kinds() {
    // Compile-time: SessionId/WindowId/PaneId cannot be mixed up. This test
    // exists to anchor the property in the suite — the assertion is trivial,
    // the value is the *types* of the locals.
    let mut reg = Registry::new();
    let s: SessionId = reg.new_session("s".to_string());
    let w: WindowId = reg.new_window(s).expect("session exists");
    let p: PaneId = reg.new_pane(w).expect("window exists");
    assert_eq!(reg.session_count(), 1);
    assert_eq!(reg.window_count(), 1);
    assert_eq!(reg.pane_count(), 1);
    // Force the IDs to be used so the bindings are not dead code.
    assert_eq!(reg.pane(p).map(|x| x.id), Some(p));
}

// ---- proptest: random op sequences keep the registry self-consistent ------

use proptest::prelude::*;

#[derive(Debug, Clone)]
enum Op {
    NewSession,
    NewWindow(usize),
    NewPane(usize),
    RemovePane(usize),
    RemoveWindow(usize),
    RemoveSession(usize),
}

fn op_strategy() -> impl Strategy<Value = Op> {
    prop_oneof![
        Just(Op::NewSession),
        any::<usize>().prop_map(Op::NewWindow),
        any::<usize>().prop_map(Op::NewPane),
        any::<usize>().prop_map(Op::RemovePane),
        any::<usize>().prop_map(Op::RemoveWindow),
        any::<usize>().prop_map(Op::RemoveSession),
    ]
}

proptest! {
    #[test]
    fn registry_invariants_hold_under_random_ops(ops in proptest::collection::vec(op_strategy(), 0..64)) {
        let mut reg = Registry::new();
        let mut sessions: Vec<SessionId> = Vec::new();
        let mut windows: Vec<WindowId> = Vec::new();
        let mut panes: Vec<PaneId> = Vec::new();

        for op in ops {
            match op {
                Op::NewSession => {
                    let id = reg.new_session("p".to_string());
                    sessions.push(id);
                }
                Op::NewWindow(i) => {
                    if !sessions.is_empty() {
                        let s = sessions[i % sessions.len()];
                        if let Ok(w) = reg.new_window(s) {
                            windows.push(w);
                        }
                    }
                }
                Op::NewPane(i) => {
                    if !windows.is_empty() {
                        let w = windows[i % windows.len()];
                        if let Ok(p) = reg.new_pane(w) {
                            panes.push(p);
                        }
                    }
                }
                Op::RemovePane(i) => {
                    if !panes.is_empty() {
                        let p = panes.swap_remove(i % panes.len());
                        let _ = reg.remove_pane(p);
                    }
                }
                Op::RemoveWindow(i) => {
                    if !windows.is_empty() {
                        let w = windows.swap_remove(i % windows.len());
                        let _ = reg.remove_window(w);
                    }
                }
                Op::RemoveSession(i) => {
                    if !sessions.is_empty() {
                        let s = sessions.swap_remove(i % sessions.len());
                        let _ = reg.remove_session(s);
                    }
                }
            }

            // Invariant: every WindowId in a Session.windows resolves and
            // links back; every PaneId in a Window.panes resolves and links
            // back; layout.panes mirrors panes; active references are live.
            let session_ids: Vec<SessionId> = sessions.iter().copied().filter(|id| reg.session(*id).is_some()).collect();
            for sid in session_ids {
                let session = reg.session(sid).expect("filtered to live");
                for wid in &session.windows {
                    let window = reg.window(*wid).expect("session points at live window");
                    prop_assert_eq!(window.session, sid);
                    prop_assert_eq!(&window.panes, &window.layout.panes);
                    for pid in &window.panes {
                        let pane = reg.pane(*pid).expect("window points at live pane");
                        prop_assert_eq!(pane.window, *wid);
                    }
                    if let Some(a) = window.active {
                        prop_assert!(window.panes.contains(&a));
                    }
                }
                if let Some(a) = session.active {
                    prop_assert!(session.windows.contains(&a));
                }
            }

            // Invariant 3: no orphan panes — every live pane's window is live
            // and lists it.
            // We can't iterate panes directly without exposing slotmap;
            // instead, iterate our tracked panes vec and check those still
            // present in the registry.
            for pid in &panes {
                if let Some(pane) = reg.pane(*pid) {
                    let window = reg.window(pane.window).expect("pane's parent window must be live");
                    prop_assert!(window.panes.contains(pid));
                }
            }
        }
    }
}
