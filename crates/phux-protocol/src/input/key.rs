//! Key input — `KeyEvent` plus re-exports of libghostty's canonical atoms.
//!
//! Per ADR-0008, phux does not maintain a parallel universe of input-event
//! types. `PhysicalKey`, `KeyAction`, and `ModSet` are libghostty-vt's types
//! under phux-flavored names. Upstream evolution (new keys, new modifier
//! bits) lands automatically on `cargo update`.
//!
//! The wrapper struct [`KeyEvent`] *is* phux-defined because libghostty's
//! `key::Event<'alloc>` is FFI-handle-backed with an allocator lifetime —
//! not suitable as a wire-protocol value. Its fields are libghostty's types
//! though, so there is no enum mirroring: just composition.

pub use libghostty_vt::key::{Action as KeyAction, Key as PhysicalKey, Mods as ModSet};

/// One key event on a pane.
///
/// Wire-shape composition of libghostty's atoms. Layout-independent: `key`
/// is the physical (W3C `code`-style) key; `text` and `unshifted_codepoint`
/// carry the layout-resolved character.
///
/// See SPEC.md §9.1 for field semantics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyEvent {
    /// Press, release, or repeat.
    pub action: KeyAction,
    /// The physical key, independent of layout or modifiers.
    pub key: PhysicalKey,
    /// Modifier bitset at the moment of the event.
    pub mods: ModSet,
    /// Subset of `mods` consumed by the OS to produce `text`. KIP's encoder
    /// uses this to avoid double-applying modifiers in escape sequences.
    /// Clients without this information SHOULD pass [`ModSet::empty`].
    pub consumed_mods: ModSet,
    /// True if this event is part of an active IME composition sequence.
    pub composing: bool,
    /// UTF-8 text produced by this keypress under the current layout, before
    /// any Ctrl/Meta transformation. MUST NOT contain C0 control characters
    /// (`U+0000..=U+001F`, `U+007F`) nor platform PUA function-key codes
    /// (`U+F700..=U+F8FF`) — pass `None` and let the encoder derive the
    /// bytes from `key + mods`.
    pub text: Option<String>,
    /// Layout-resolved codepoint that would have been produced with no
    /// modifiers held. Used by KIP `REPORT_ALTERNATES`.
    pub unshifted_codepoint: Option<u32>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_event_composes_libghostty_atoms() {
        let ev = KeyEvent {
            action: KeyAction::Press,
            key: PhysicalKey::A,
            mods: ModSet::empty(),
            consumed_mods: ModSet::empty(),
            composing: false,
            text: Some("a".to_owned()),
            unshifted_codepoint: Some(u32::from('a')),
        };
        assert_eq!(ev.action, KeyAction::Press);
        assert_eq!(ev.key, PhysicalKey::A);
    }

    #[test]
    fn key_event_equality_includes_text() {
        let mk = |text: &str| KeyEvent {
            action: KeyAction::Press,
            key: PhysicalKey::A,
            mods: ModSet::empty(),
            consumed_mods: ModSet::empty(),
            composing: false,
            text: Some(text.to_owned()),
            unshifted_codepoint: None,
        };
        assert_ne!(mk("a"), mk("b"));
        assert_eq!(mk("a"), mk("a"));
    }
}
