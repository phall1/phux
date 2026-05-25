//! Focus input — direct re-export of libghostty's `focus::Event`.
//!
//! Per ADR-0008, `FocusEvent` IS libghostty's `focus::Event` (a plain enum
//! `Gained`/`Lost`). No wrapping; no `gained: bool` indirection.

pub use libghostty_vt::focus::Event as FocusEvent;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn focus_event_variants() {
        assert_ne!(FocusEvent::Gained, FocusEvent::Lost);
    }

    #[test]
    fn focus_event_is_copy() {
        fn assert_copy<T: Copy>() {}
        assert_copy::<FocusEvent>();
    }
}
