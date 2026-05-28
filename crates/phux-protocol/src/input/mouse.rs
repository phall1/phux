//! Mouse input — `MouseEvent` plus re-exports of libghostty's canonical atoms.
//!
//! Per ADR-0008, `MouseAction` and `MouseButton` are libghostty-vt's types
//! under phux-flavored names. The wrapper struct [`MouseEvent`] is
//! phux-defined because libghostty's `mouse::Event<'alloc>` is allocator-
//! lifetime-bound and therefore not directly serializable.
//!
//! Coordinates are pane-local surface-space pixels (NOT cells), matching
//! libghostty's `mouse::Position` shape exactly — see docs/spec/input.md §3.1 for the
//! cell-geometry contract.
//!
//! # Mouse mode bits ([`MouseProtocol`] / [`MouseEncoding`])
//!
//! Per docs/spec/L1.md §2.5, cursor and mode state — including the inner program's
//! mouse-tracking protocol and the wire format it asks for — live entirely
//! inside each end's `libghostty_vt::Terminal`. They are **not** separate
//! wire concepts: clients query their local `Terminal::modes()` to learn
//! whether to forward mouse events, and the server reads its `Terminal`
//! directly via `Encoder::set_options_from_terminal` when emitting PTY
//! bytes for an inbound `INPUT_MOUSE`.
//!
//! [`MouseProtocol`] and [`MouseEncoding`] are therefore re-exports of
//! libghostty's canonical `mouse::TrackingMode` and `mouse::Format` (per
//! ADR-0008): they exist as named handles for the server-side state the
//! inner program toggles via DECSET, not as wire fields.

use super::key::ModSet;

pub use libghostty_vt::mouse::{
    Action as MouseAction, Button as MouseButton, Format as MouseEncoding,
    TrackingMode as MouseProtocol,
};

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
/// docs/spec/input.md §3.1 makes this load-bearing:
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

    /// `MouseProtocol` covers the five DECSET tracking modes the inner
    /// program may select (docs/spec/L1.md §2.5). Names follow libghostty's
    /// `TrackingMode` — see ADR-0008.
    #[test]
    fn mouse_protocol_variants_present() {
        // None      → tracking disabled
        // X10       → DECSET 9, press-only
        // Normal    → DECSET 1000, press + release
        // Button    → DECSET 1002, press + release + drag (button-event)
        // Any       → DECSET 1003, all motion (any-event)
        let _ = MouseProtocol::None;
        let _ = MouseProtocol::X10;
        let _ = MouseProtocol::Normal;
        let _ = MouseProtocol::Button;
        let _ = MouseProtocol::Any;
        assert_ne!(MouseProtocol::None, MouseProtocol::X10);
        assert_ne!(MouseProtocol::Normal, MouseProtocol::Button);
        assert_ne!(MouseProtocol::Button, MouseProtocol::Any);
    }

    /// `MouseEncoding` covers the five wire formats the inner program may
    /// select via DECSET 1005 / 1006 / 1015 / 1016 (docs/spec/L1.md §2.5). Names
    /// follow libghostty's `Format` — see ADR-0008.
    #[test]
    fn mouse_encoding_variants_present() {
        // X10       → legacy (CSI M Cb Cx Cy), DECSET 1006/1015/1016 all off
        // Utf8      → DECSET 1005 (UTF-8 extended)
        // Sgr       → DECSET 1006 (SGR)
        // Urxvt     → DECSET 1015 (urxvt)
        // SgrPixels → DECSET 1016 (SGR with pixel coordinates)
        let _ = MouseEncoding::X10;
        let _ = MouseEncoding::Utf8;
        let _ = MouseEncoding::Sgr;
        let _ = MouseEncoding::Urxvt;
        let _ = MouseEncoding::SgrPixels;
        assert_ne!(MouseEncoding::X10, MouseEncoding::Sgr);
        assert_ne!(MouseEncoding::Sgr, MouseEncoding::SgrPixels);
        assert_ne!(MouseEncoding::Urxvt, MouseEncoding::Utf8);
    }

    #[test]
    fn mouse_protocol_and_encoding_are_copy() {
        fn assert_copy<T: Copy>() {}
        assert_copy::<MouseProtocol>();
        assert_copy::<MouseEncoding>();
    }
}
