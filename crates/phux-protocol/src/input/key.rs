//! Key input: [`KeyEvent`], [`PhysicalKey`], [`KeyAction`], [`ModSet`].
//!
//! Mirrors libghostty-vt's `key::Event`, `key::Key`, `key::Action`, and
//! `key::Mods` one-to-one. See `SPEC.md` §9.1 and ADR-0006 for the rationale.
//!
//! Variant names and numeric discriminants are deliberately kept identical
//! to libghostty so the server-side `From<&phux_protocol::input::key::*>`
//! conversions (owned by phux-6yl.2) are field-for-field copies.

use bitflags::bitflags;

/// One key event on a pane.
///
/// Layout-independent: `key` is the *physical* key (W3C `code`-style),
/// while `text` / `unshifted_codepoint` carry the layout-resolved
/// character(s). See `SPEC.md` §9.1.
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

/// Press / release / repeat. Numeric discriminants match libghostty's
/// `key::Action`.
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KeyAction {
    /// Key was pressed.
    Press = 0,
    /// Key was released.
    Release = 1,
    /// Key is being repeated (held down).
    Repeat = 2,
}

/// Physical key code, independent of keyboard layout or modifiers.
///
/// Mirrors libghostty-vt's `key::Key` — both variant names and numeric
/// discriminants. The W3C UI Events `code`-style enum: a US-QWERTY user
/// pressing the leftmost home-row key produces [`PhysicalKey::A`]; an
/// AZERTY user pressing the *same physical key* also produces
/// [`PhysicalKey::A`]. Layout-resolved text appears in
/// [`KeyEvent::text`] / [`KeyEvent::unshifted_codepoint`].
///
/// This enum is `#[non_exhaustive]` (per `SPEC.md` §9.1.1): minor protocol
/// versions may add values. Decoders MUST treat unknown numeric values as
/// [`PhysicalKey::Unidentified`].
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
#[allow(missing_docs, reason = "self-explanatory key names")]
pub enum PhysicalKey {
    Unidentified = 0,

    // Writing-system keys (US-QWERTY positions).
    Backquote = 1,
    Backslash = 2,
    BracketLeft = 3,
    BracketRight = 4,
    Comma = 5,
    Digit0 = 6,
    Digit1 = 7,
    Digit2 = 8,
    Digit3 = 9,
    Digit4 = 10,
    Digit5 = 11,
    Digit6 = 12,
    Digit7 = 13,
    Digit8 = 14,
    Digit9 = 15,
    Equal = 16,
    IntlBackslash = 17,
    IntlRo = 18,
    IntlYen = 19,
    A = 20,
    B = 21,
    C = 22,
    D = 23,
    E = 24,
    F = 25,
    G = 26,
    H = 27,
    I = 28,
    J = 29,
    K = 30,
    L = 31,
    M = 32,
    N = 33,
    O = 34,
    P = 35,
    Q = 36,
    R = 37,
    S = 38,
    T = 39,
    U = 40,
    V = 41,
    W = 42,
    X = 43,
    Y = 44,
    Z = 45,
    Minus = 46,
    Period = 47,
    Quote = 48,
    Semicolon = 49,
    Slash = 50,

    // Functional keys.
    AltLeft = 51,
    AltRight = 52,
    Backspace = 53,
    CapsLock = 54,
    ContextMenu = 55,
    ControlLeft = 56,
    ControlRight = 57,
    Enter = 58,
    MetaLeft = 59,
    MetaRight = 60,
    ShiftLeft = 61,
    ShiftRight = 62,
    Space = 63,
    Tab = 64,
    Convert = 65,
    KanaMode = 66,
    NonConvert = 67,

    // Control pad.
    Delete = 68,
    End = 69,
    Help = 70,
    Home = 71,
    Insert = 72,
    PageDown = 73,
    PageUp = 74,

    // Arrow keys.
    ArrowDown = 75,
    ArrowLeft = 76,
    ArrowRight = 77,
    ArrowUp = 78,

    // Numpad.
    NumLock = 79,
    Numpad0 = 80,
    Numpad1 = 81,
    Numpad2 = 82,
    Numpad3 = 83,
    Numpad4 = 84,
    Numpad5 = 85,
    Numpad6 = 86,
    Numpad7 = 87,
    Numpad8 = 88,
    Numpad9 = 89,
    NumpadAdd = 90,
    NumpadBackspace = 91,
    NumpadClear = 92,
    NumpadClearEntry = 93,
    NumpadComma = 94,
    NumpadDecimal = 95,
    NumpadDivide = 96,
    NumpadEnter = 97,
    NumpadEqual = 98,
    NumpadMemoryAdd = 99,
    NumpadMemoryClear = 100,
    NumpadMemoryRecall = 101,
    NumpadMemoryStore = 102,
    NumpadMemorySubtract = 103,
    NumpadMultiply = 104,
    NumpadParenLeft = 105,
    NumpadParenRight = 106,
    NumpadSubtract = 107,
    NumpadSeparator = 108,
    NumpadUp = 109,
    NumpadDown = 110,
    NumpadRight = 111,
    NumpadLeft = 112,
    NumpadBegin = 113,
    NumpadHome = 114,
    NumpadEnd = 115,
    NumpadInsert = 116,
    NumpadDelete = 117,
    NumpadPageUp = 118,
    NumpadPageDown = 119,

    // Function keys & system.
    Escape = 120,
    F1 = 121,
    F2 = 122,
    F3 = 123,
    F4 = 124,
    F5 = 125,
    F6 = 126,
    F7 = 127,
    F8 = 128,
    F9 = 129,
    F10 = 130,
    F11 = 131,
    F12 = 132,
    F13 = 133,
    F14 = 134,
    F15 = 135,
    F16 = 136,
    F17 = 137,
    F18 = 138,
    F19 = 139,
    F20 = 140,
    F21 = 141,
    F22 = 142,
    F23 = 143,
    F24 = 144,
    F25 = 145,
    Fn = 146,
    FnLock = 147,
    PrintScreen = 148,
    ScrollLock = 149,
    Pause = 150,

    // Browser / app launch.
    BrowserBack = 151,
    BrowserFavorites = 152,
    BrowserForward = 153,
    BrowserHome = 154,
    BrowserRefresh = 155,
    BrowserSearch = 156,
    BrowserStop = 157,
    Eject = 158,
    LaunchApp1 = 159,
    LaunchApp2 = 160,
    LaunchMail = 161,

    // Media / system.
    MediaPlayPause = 162,
    MediaSelect = 163,
    MediaStop = 164,
    MediaTrackNext = 165,
    MediaTrackPrevious = 166,
    Power = 167,
    Sleep = 168,
    AudioVolumeDown = 169,
    AudioVolumeMute = 170,
    AudioVolumeUp = 171,
    WakeUp = 172,
    Copy = 173,
    Cut = 174,
    Paste = 175,
}

bitflags! {
    /// Keyboard modifier set.
    ///
    /// Mirrors libghostty-vt's `key::Mods`. Note the deliberate absence of
    /// `HYPER` and `META` as separate flags (ADR-0006): libghostty collapses
    /// them to [`ModSet::SUPER`] and platforms where they exist map them
    /// there via XKB configuration. Modeling them separately would introduce
    /// a degree of freedom no downstream encoder can honor.
    ///
    /// Each `*_SIDE` bit is only meaningful when the corresponding modifier
    /// bit is set: `0` = left key, `1` = right key. Platforms that cannot
    /// distinguish sides MUST leave these bits zero.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
    pub struct ModSet: u16 {
        /// Shift key is pressed.
        const SHIFT      = 0x0001;
        /// Alt key is pressed.
        const ALT        = 0x0002;
        /// Control key is pressed.
        const CTRL       = 0x0004;
        /// Super / Command / Windows key is pressed.
        const SUPER      = 0x0008;
        /// Caps Lock is active.
        const CAPS_LOCK  = 0x0010;
        /// Num Lock is active.
        const NUM_LOCK   = 0x0020;
        /// Right Shift pressed (unset = left, set = right). Only valid when
        /// [`ModSet::SHIFT`] is set.
        const SHIFT_SIDE = 0x0040;
        /// Right Alt pressed (unset = left, set = right). Only valid when
        /// [`ModSet::ALT`] is set.
        const ALT_SIDE   = 0x0080;
        /// Right Control pressed (unset = left, set = right). Only valid
        /// when [`ModSet::CTRL`] is set.
        const CTRL_SIDE  = 0x0100;
        /// Right Super pressed (unset = left, set = right). Only valid when
        /// [`ModSet::SUPER`] is set.
        const SUPER_SIDE = 0x0200;
    }
}

#[cfg(test)]
mod tests {
    use super::{KeyAction, KeyEvent, ModSet, PhysicalKey};

    #[test]
    fn key_action_discriminants() {
        assert_eq!(KeyAction::Press as u32, 0);
        assert_eq!(KeyAction::Release as u32, 1);
        assert_eq!(KeyAction::Repeat as u32, 2);
    }

    #[test]
    fn modset_bits_match_spec() {
        assert_eq!(ModSet::SHIFT.bits(), 0x0001);
        assert_eq!(ModSet::ALT.bits(), 0x0002);
        assert_eq!(ModSet::CTRL.bits(), 0x0004);
        assert_eq!(ModSet::SUPER.bits(), 0x0008);
        assert_eq!(ModSet::CAPS_LOCK.bits(), 0x0010);
        assert_eq!(ModSet::NUM_LOCK.bits(), 0x0020);
        assert_eq!(ModSet::SHIFT_SIDE.bits(), 0x0040);
        assert_eq!(ModSet::ALT_SIDE.bits(), 0x0080);
        assert_eq!(ModSet::CTRL_SIDE.bits(), 0x0100);
        assert_eq!(ModSet::SUPER_SIDE.bits(), 0x0200);
    }

    #[test]
    fn modset_default_is_empty() {
        assert!(ModSet::default().is_empty());
    }

    #[test]
    fn key_event_round_trips_via_clone() {
        let e = KeyEvent {
            action: KeyAction::Press,
            key: PhysicalKey::A,
            mods: ModSet::CTRL | ModSet::SHIFT,
            consumed_mods: ModSet::SHIFT,
            composing: false,
            text: Some("A".to_owned()),
            unshifted_codepoint: Some(u32::from('a')),
        };
        assert_eq!(e, e.clone());
    }

    /// Cross-task validation gate (phux-6yl.1 / phux-6yl.2 boundary).
    ///
    /// `phux-protocol` deliberately does NOT depend on `libghostty-vt`
    /// (that would invert the layering — the server, not the protocol
    /// crate, owns the libghostty seam). To still pin discriminants to
    /// libghostty's `key::Key`, we hard-code the expected values here.
    /// The mirroring `From` impl test that actually links libghostty-vt
    /// lives in phux-6yl.2 (server-side conversions).
    #[test]
    #[allow(
        clippy::too_many_lines,
        reason = "exhaustive table — one line per libghostty Key variant"
    )]
    fn physical_key_discriminants_match_libghostty() {
        // (variant, libghostty-vt::key::Key discriminant)
        // Sourced from libghostty-rs @ 31d1f70:
        //   crates/libghostty-vt/src/key.rs — `pub enum Key { ... }`.
        const EXPECTED: &[(PhysicalKey, u32)] = &[
            (PhysicalKey::Unidentified, 0),
            (PhysicalKey::Backquote, 1),
            (PhysicalKey::Backslash, 2),
            (PhysicalKey::BracketLeft, 3),
            (PhysicalKey::BracketRight, 4),
            (PhysicalKey::Comma, 5),
            (PhysicalKey::Digit0, 6),
            (PhysicalKey::Digit1, 7),
            (PhysicalKey::Digit2, 8),
            (PhysicalKey::Digit3, 9),
            (PhysicalKey::Digit4, 10),
            (PhysicalKey::Digit5, 11),
            (PhysicalKey::Digit6, 12),
            (PhysicalKey::Digit7, 13),
            (PhysicalKey::Digit8, 14),
            (PhysicalKey::Digit9, 15),
            (PhysicalKey::Equal, 16),
            (PhysicalKey::IntlBackslash, 17),
            (PhysicalKey::IntlRo, 18),
            (PhysicalKey::IntlYen, 19),
            (PhysicalKey::A, 20),
            (PhysicalKey::B, 21),
            (PhysicalKey::C, 22),
            (PhysicalKey::D, 23),
            (PhysicalKey::E, 24),
            (PhysicalKey::F, 25),
            (PhysicalKey::G, 26),
            (PhysicalKey::H, 27),
            (PhysicalKey::I, 28),
            (PhysicalKey::J, 29),
            (PhysicalKey::K, 30),
            (PhysicalKey::L, 31),
            (PhysicalKey::M, 32),
            (PhysicalKey::N, 33),
            (PhysicalKey::O, 34),
            (PhysicalKey::P, 35),
            (PhysicalKey::Q, 36),
            (PhysicalKey::R, 37),
            (PhysicalKey::S, 38),
            (PhysicalKey::T, 39),
            (PhysicalKey::U, 40),
            (PhysicalKey::V, 41),
            (PhysicalKey::W, 42),
            (PhysicalKey::X, 43),
            (PhysicalKey::Y, 44),
            (PhysicalKey::Z, 45),
            (PhysicalKey::Minus, 46),
            (PhysicalKey::Period, 47),
            (PhysicalKey::Quote, 48),
            (PhysicalKey::Semicolon, 49),
            (PhysicalKey::Slash, 50),
            (PhysicalKey::AltLeft, 51),
            (PhysicalKey::AltRight, 52),
            (PhysicalKey::Backspace, 53),
            (PhysicalKey::CapsLock, 54),
            (PhysicalKey::ContextMenu, 55),
            (PhysicalKey::ControlLeft, 56),
            (PhysicalKey::ControlRight, 57),
            (PhysicalKey::Enter, 58),
            (PhysicalKey::MetaLeft, 59),
            (PhysicalKey::MetaRight, 60),
            (PhysicalKey::ShiftLeft, 61),
            (PhysicalKey::ShiftRight, 62),
            (PhysicalKey::Space, 63),
            (PhysicalKey::Tab, 64),
            (PhysicalKey::Convert, 65),
            (PhysicalKey::KanaMode, 66),
            (PhysicalKey::NonConvert, 67),
            (PhysicalKey::Delete, 68),
            (PhysicalKey::End, 69),
            (PhysicalKey::Help, 70),
            (PhysicalKey::Home, 71),
            (PhysicalKey::Insert, 72),
            (PhysicalKey::PageDown, 73),
            (PhysicalKey::PageUp, 74),
            (PhysicalKey::ArrowDown, 75),
            (PhysicalKey::ArrowLeft, 76),
            (PhysicalKey::ArrowRight, 77),
            (PhysicalKey::ArrowUp, 78),
            (PhysicalKey::NumLock, 79),
            (PhysicalKey::Numpad0, 80),
            (PhysicalKey::Numpad1, 81),
            (PhysicalKey::Numpad2, 82),
            (PhysicalKey::Numpad3, 83),
            (PhysicalKey::Numpad4, 84),
            (PhysicalKey::Numpad5, 85),
            (PhysicalKey::Numpad6, 86),
            (PhysicalKey::Numpad7, 87),
            (PhysicalKey::Numpad8, 88),
            (PhysicalKey::Numpad9, 89),
            (PhysicalKey::NumpadAdd, 90),
            (PhysicalKey::NumpadBackspace, 91),
            (PhysicalKey::NumpadClear, 92),
            (PhysicalKey::NumpadClearEntry, 93),
            (PhysicalKey::NumpadComma, 94),
            (PhysicalKey::NumpadDecimal, 95),
            (PhysicalKey::NumpadDivide, 96),
            (PhysicalKey::NumpadEnter, 97),
            (PhysicalKey::NumpadEqual, 98),
            (PhysicalKey::NumpadMemoryAdd, 99),
            (PhysicalKey::NumpadMemoryClear, 100),
            (PhysicalKey::NumpadMemoryRecall, 101),
            (PhysicalKey::NumpadMemoryStore, 102),
            (PhysicalKey::NumpadMemorySubtract, 103),
            (PhysicalKey::NumpadMultiply, 104),
            (PhysicalKey::NumpadParenLeft, 105),
            (PhysicalKey::NumpadParenRight, 106),
            (PhysicalKey::NumpadSubtract, 107),
            (PhysicalKey::NumpadSeparator, 108),
            (PhysicalKey::NumpadUp, 109),
            (PhysicalKey::NumpadDown, 110),
            (PhysicalKey::NumpadRight, 111),
            (PhysicalKey::NumpadLeft, 112),
            (PhysicalKey::NumpadBegin, 113),
            (PhysicalKey::NumpadHome, 114),
            (PhysicalKey::NumpadEnd, 115),
            (PhysicalKey::NumpadInsert, 116),
            (PhysicalKey::NumpadDelete, 117),
            (PhysicalKey::NumpadPageUp, 118),
            (PhysicalKey::NumpadPageDown, 119),
            (PhysicalKey::Escape, 120),
            (PhysicalKey::F1, 121),
            (PhysicalKey::F2, 122),
            (PhysicalKey::F3, 123),
            (PhysicalKey::F4, 124),
            (PhysicalKey::F5, 125),
            (PhysicalKey::F6, 126),
            (PhysicalKey::F7, 127),
            (PhysicalKey::F8, 128),
            (PhysicalKey::F9, 129),
            (PhysicalKey::F10, 130),
            (PhysicalKey::F11, 131),
            (PhysicalKey::F12, 132),
            (PhysicalKey::F13, 133),
            (PhysicalKey::F14, 134),
            (PhysicalKey::F15, 135),
            (PhysicalKey::F16, 136),
            (PhysicalKey::F17, 137),
            (PhysicalKey::F18, 138),
            (PhysicalKey::F19, 139),
            (PhysicalKey::F20, 140),
            (PhysicalKey::F21, 141),
            (PhysicalKey::F22, 142),
            (PhysicalKey::F23, 143),
            (PhysicalKey::F24, 144),
            (PhysicalKey::F25, 145),
            (PhysicalKey::Fn, 146),
            (PhysicalKey::FnLock, 147),
            (PhysicalKey::PrintScreen, 148),
            (PhysicalKey::ScrollLock, 149),
            (PhysicalKey::Pause, 150),
            (PhysicalKey::BrowserBack, 151),
            (PhysicalKey::BrowserFavorites, 152),
            (PhysicalKey::BrowserForward, 153),
            (PhysicalKey::BrowserHome, 154),
            (PhysicalKey::BrowserRefresh, 155),
            (PhysicalKey::BrowserSearch, 156),
            (PhysicalKey::BrowserStop, 157),
            (PhysicalKey::Eject, 158),
            (PhysicalKey::LaunchApp1, 159),
            (PhysicalKey::LaunchApp2, 160),
            (PhysicalKey::LaunchMail, 161),
            (PhysicalKey::MediaPlayPause, 162),
            (PhysicalKey::MediaSelect, 163),
            (PhysicalKey::MediaStop, 164),
            (PhysicalKey::MediaTrackNext, 165),
            (PhysicalKey::MediaTrackPrevious, 166),
            (PhysicalKey::Power, 167),
            (PhysicalKey::Sleep, 168),
            (PhysicalKey::AudioVolumeDown, 169),
            (PhysicalKey::AudioVolumeMute, 170),
            (PhysicalKey::AudioVolumeUp, 171),
            (PhysicalKey::WakeUp, 172),
            (PhysicalKey::Copy, 173),
            (PhysicalKey::Cut, 174),
            (PhysicalKey::Paste, 175),
        ];

        assert_eq!(
            EXPECTED.len(),
            176,
            "expected all 176 libghostty Key variants",
        );
        for (i, (variant, expected)) in EXPECTED.iter().enumerate() {
            let actual = *variant as u32;
            assert_eq!(
                actual, *expected,
                "PhysicalKey discriminant mismatch at index {i}: got {actual}, expected {expected}",
            );
            let Ok(i_u32) = u32::try_from(i) else {
                unreachable!("EXPECTED has <= 256 entries");
            };
            assert_eq!(
                *expected, i_u32,
                "EXPECTED table is missing entries or has duplicates near index {i}",
            );
        }
    }
}
