//! Mouse input — `MouseEvent` plus re-exports of libghostty's canonical atoms.
//!
//! Per ADR-0008, `MouseAction` and `MouseButton` are libghostty-vt's types
//! under phux-flavored names. The wrapper struct [`MouseEvent`] is
//! phux-defined because libghostty's `mouse::Event<'alloc>` is allocator-
//! lifetime-bound and therefore not directly serializable.
//!
//! Coordinates are pane-local surface-space pixels (NOT cells), matching
//! libghostty's `mouse::Position` shape exactly — see SPEC.md §9.2.1 for the
//! cell-geometry contract.

use super::key::ModSet;

pub use libghostty_vt::mouse::{Action as MouseAction, Button as MouseButton};

/// A normalized mouse input event flowing from client to server.
///
/// Composes libghostty's `MouseAction`, `MouseButton`, and our `ModSet`
/// (which is itself libghostty's `key::Mods`). The server reconstructs a
/// `libghostty_vt::mouse::Event` from these fields, derives a
/// `mouse::EncoderSize` from the latest `VIEWPORT_RESIZE`, and hands the
/// event to a per-pane `mouse::Encoder`.
///
/// # Coordinate system — cell-geometry contract
///
/// `x` and `y` are **pane-local surface-space pixels**, NOT cell indices.
/// SPEC.md §9.2.1 makes this load-bearing:
///
/// * Cell-quantized clients (TUIs without true pixel-precision input) MUST
///   emit positions at `cell_index × cell_size` — the server's encoder then
///   produces correct output in both cell-format (SGR, URXVT) and
///   pixel-format (SGR-Pixels) mouse protocols.
/// * Clients with real pixel input pass it through unchanged.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MouseEvent {
    /// What the mouse did: press, release, or motion.
    pub action: MouseAction,
    /// Which button was involved.
    pub button: MouseButton,
    /// Keyboard modifiers held during the event.
    pub mods: ModSet,
    /// Pane-local surface-space pixel X.
    pub x: f64,
    /// Pane-local surface-space pixel Y.
    pub y: f64,
}

// `MouseEvent` is `PartialEq` but not `Eq` because `f64` is not `Eq`.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mouse_event_composes_libghostty_atoms() {
        let ev = MouseEvent {
            action: MouseAction::Press,
            button: MouseButton::Left,
            mods: ModSet::empty(),
            x: 12.5,
            y: 34.25,
        };
        assert_eq!(ev.action, MouseAction::Press);
        assert_eq!(ev.button, MouseButton::Left);
        assert!((ev.x - 12.5).abs() < f64::EPSILON);
    }

    #[test]
    fn mouse_event_is_copy() {
        fn assert_copy<T: Copy>() {}
        assert_copy::<MouseEvent>();
    }
}
