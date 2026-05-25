//! Focus input: `FocusEvent`.
//!
//! Owned by phux-6yl.7. See SPEC.md §9.3 and ADR-0006.
//!
//! Mirrors libghostty-vt's `focus::Event` one-to-one. Where libghostty
//! models the event as a two-variant enum (`Gained` / `Lost`), the wire
//! form collapses to a single boolean `gained` field for compactness; the
//! semantic mapping is `gained == true` ↔ `focus::Event::Gained`.

#![allow(clippy::module_name_repetitions)]

/// Window focus change reported by a client to the server.
///
/// SPEC.md §9.3: the client emits `INPUT_FOCUS` when its host-OS window
/// gains or loses focus. The server forwards a `CSI I` / `CSI O` report
/// via `libghostty_vt::focus` if the target pane has DEC mode 1004
/// active, otherwise drops the event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FocusEvent {
    /// `true` if focus was gained, `false` if lost.
    pub gained: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn focus_event_construction_and_equality() {
        let gained = FocusEvent { gained: true };
        let lost = FocusEvent { gained: false };
        assert_ne!(gained, lost);
        assert_eq!(gained, FocusEvent { gained: true });
        assert_eq!(lost, FocusEvent { gained: false });
    }

    #[test]
    fn focus_event_is_copy() {
        let e = FocusEvent { gained: true };
        let copy = e;
        // Both bindings remain usable — proves `Copy` derive.
        assert_eq!(e, copy);
    }
}
