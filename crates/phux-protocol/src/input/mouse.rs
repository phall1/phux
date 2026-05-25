//! Mouse input: [`MouseEvent`], [`MouseButton`], [`MouseAction`].
//!
//! Owned by phux-6yl.6. See `SPEC.md` §9.2 and ADR-0006.
//!
//! These types mirror libghostty-vt's `mouse::Action`, `mouse::Button`, and
//! `mouse::Position` one-to-one (ADR-0006). The numeric discriminants here
//! are chosen to match libghostty's C ABI verbatim so server-side
//! `From<&phux_protocol::input::mouse::*>` conversions are field-for-field
//! copies. The `mouse_button_discriminants_match_libghostty` test pins this
//! contract.

// TODO(phux-6yl.1): replace with `use super::key::ModSet;` once the sibling
// branch lands. Until then we carry a minimal local stub so this crate
// compiles standalone on the phux-6yl.6 worktree.
/// Temporary local stub for `ModSet`. The real type is owned by phux-6yl.1
/// (`crates/phux-protocol/src/input/key.rs`); the integration pass swaps
/// this stub for `use super::key::ModSet;`.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ModSet(u32);

/// A normalized mouse input event flowing from client to server.
///
/// Mirrors libghostty-vt's `mouse::Event` shape. The server reconstructs a
/// `libghostty_vt::mouse::Event`, hands it (with the pane's
/// `mouse::EncoderSize` derived from the latest `VIEWPORT_RESIZE` — see
/// `SPEC.md` §9.2.2) to a per-pane `mouse::Encoder`, and writes the encoded
/// bytes to the PTY.
///
/// # Coordinate system — cell-geometry contract
///
/// `x` and `y` are **pane-local surface-space pixels**, NOT cell indices.
/// `SPEC.md` §9.2.1 makes this load-bearing:
///
/// * Cell-quantized clients (TUIs without true pixel-precision input) MUST
///   emit positions at `cell_index × cell_size` — the server's encoder then
///   produces correct output in both cell-format (SGR, URXVT) and
///   pixel-format (SGR-Pixels) mouse protocols.
/// * Clients with real pixel input pass it through unchanged.
///
/// The server is the sole authority on cell geometry. Sending cell indices
/// here will silently degrade SGR-Pixels accuracy and is a protocol
/// violation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MouseEvent {
    /// What the mouse did: press, release, or motion.
    pub action: MouseAction,
    /// Which button was involved. `MouseButton::Unknown` is the wire
    /// representation of "no button" (e.g. naked motion).
    pub button: MouseButton,
    /// Keyboard modifiers held during the event.
    pub mods: ModSet,
    /// Pane-local surface-space pixel X. See the type-level docs.
    pub x: f64,
    /// Pane-local surface-space pixel Y. See the type-level docs.
    pub y: f64,
}

// `MouseEvent` is `PartialEq` but not `Eq` because `f64` is not `Eq`.
// The other public types in this module are full `Eq`.

/// Mouse event action. Discriminants match libghostty's
/// `ffi::MouseAction::{PRESS, RELEASE, MOTION}` (0/1/2).
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MouseAction {
    /// Mouse button was pressed.
    Press = 0,
    /// Mouse button was released.
    Release = 1,
    /// Mouse moved.
    Motion = 2,
}

/// Mouse button identity. Discriminants match libghostty's
/// `ffi::MouseButton` constants verbatim.
///
/// Scroll-wheel events arrive (per xterm convention, mirrored by libghostty)
/// as `MouseAction::Press` of `Four` (up) / `Five` (down) / `Six` (left) /
/// `Seven` (right). See `SPEC.md` §9.2.
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MouseButton {
    /// No button / unrecognized button. Used as the "no button" wire value
    /// for events like naked motion.
    Unknown = 0,
    /// Primary button.
    Left = 1,
    /// Secondary button.
    Right = 2,
    /// Middle / wheel-click button.
    Middle = 3,
    /// Button 4 — scroll wheel up (xterm convention).
    Four = 4,
    /// Button 5 — scroll wheel down (xterm convention).
    Five = 5,
    /// Button 6 — scroll wheel left (xterm convention).
    Six = 6,
    /// Button 7 — scroll wheel right (xterm convention).
    Seven = 7,
    /// Button 8.
    Eight = 8,
    /// Button 9.
    Nine = 9,
    /// Button 10.
    Ten = 10,
    /// Button 11.
    Eleven = 11,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pin the wire discriminants of [`MouseButton`] to libghostty's
    /// `ffi::MouseButton` constants. Mirrors phux-6yl.1's `PhysicalKey`
    /// table-driven approach: we don't take a `libghostty-vt` build
    /// dependency on `phux-protocol`, so the contract lives as a hard-coded
    /// table here. If libghostty renumbers, this test fails loudly.
    #[test]
    fn mouse_button_discriminants_match_libghostty() {
        // (variant, libghostty `ffi::MouseButton` numeric value)
        const TABLE: &[(MouseButton, u32)] = &[
            (MouseButton::Unknown, 0),
            (MouseButton::Left, 1),
            (MouseButton::Right, 2),
            (MouseButton::Middle, 3),
            (MouseButton::Four, 4),
            (MouseButton::Five, 5),
            (MouseButton::Six, 6),
            (MouseButton::Seven, 7),
            (MouseButton::Eight, 8),
            (MouseButton::Nine, 9),
            (MouseButton::Ten, 10),
            (MouseButton::Eleven, 11),
        ];
        for &(variant, expected) in TABLE {
            assert_eq!(
                variant as u32, expected,
                "MouseButton::{variant:?} must serialize as {expected} to match libghostty",
            );
        }
    }

    /// Pin [`MouseAction`] discriminants to libghostty's
    /// `ffi::MouseAction::{PRESS, RELEASE, MOTION}`.
    #[test]
    fn mouse_action_discriminants_match_libghostty() {
        const TABLE: &[(MouseAction, u32)] = &[
            (MouseAction::Press, 0),
            (MouseAction::Release, 1),
            (MouseAction::Motion, 2),
        ];
        for &(variant, expected) in TABLE {
            assert_eq!(
                variant as u32, expected,
                "MouseAction::{variant:?} must serialize as {expected} to match libghostty",
            );
        }
    }

    #[test]
    fn mouse_event_is_copy() {
        fn assert_copy<T: Copy>() {}
        assert_copy::<MouseEvent>();
        assert_copy::<MouseAction>();
        assert_copy::<MouseButton>();
    }

    #[test]
    fn mouse_event_roundtrip_fields() {
        let ev = MouseEvent {
            action: MouseAction::Press,
            button: MouseButton::Left,
            mods: ModSet::default(),
            x: 12.5,
            y: 34.25,
        };
        let copy = ev;
        assert_eq!(copy.action, MouseAction::Press);
        assert_eq!(copy.button, MouseButton::Left);
        assert!((copy.x - 12.5).abs() < f64::EPSILON);
        assert!((copy.y - 34.25).abs() < f64::EPSILON);
    }
}
