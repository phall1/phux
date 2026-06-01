//! Selection input — `SelectionEvent` + `SelectionMode`.
//!
//! Selection is client-owned state: the server receives selection frames,
//! updates per-terminal selection state, and emits no output to the PTY.
//! The client uses selection for copy-mode UI; the server uses the stored
//! selection state to drive extraction (plaintext via libghostty's
//! format_selection_alloc) when the client requests it via a COMMAND.
//! See ADR-0025 (rectangular selection rationale) and docs/spec/input.md §6.

#![allow(clippy::module_name_repetitions)]

/// Selection mode for copy-mode operations.
///
/// Describes the type of selection the client is performing or has performed.
/// Passed in InputEvent::Selection frames to the server; the server stores
/// the mode and uses it to interpret selection boundaries during extraction.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectionMode {
    /// No selection active.
    Off = 0,
    /// Character-wise selection (default copy mode).
    Char = 1,
    /// Line-wise selection (whole lines).
    Line = 2,
    /// Rectangular (block) selection. Mosh-style RECT mode.
    Rect = 3,
}

impl SelectionMode {
    /// Attempt to parse a SelectionMode from a wire discriminant.
    ///
    /// Returns None for unknown values; the wire codec uses this to reject
    /// out-of-range mode values.
    pub fn try_from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(SelectionMode::Off),
            1 => Some(SelectionMode::Char),
            2 => Some(SelectionMode::Line),
            3 => Some(SelectionMode::Rect),
            _ => None,
        }
    }

    /// Convert to wire discriminant.
    pub fn as_u8(self) -> u8 {
        self as u8
    }
}

/// A selection event from a client.
///
/// SPEC §6 / ADR-0025. The client emits this to inform the server that
/// the user is performing or has completed a selection. The mode and
/// rectangular flag together describe what the client intends to extract.
/// The server stores the selection state (start, end, mode, rect flag)
/// per terminal and emits no output.
///
/// Extraction (plaintext copy via libghostty format_selection_alloc) is
/// requested separately via a COMMAND frame carrying a RouteInput payload
/// with InputEvent::Selection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectionEvent {
    /// Selection mode: off, character-wise, line-wise, or rectangular.
    pub mode: SelectionMode,
    /// Rectangular-mode flag. True enables Mosh-style block-mode selection;
    /// false uses linear selection. Only meaningful when mode is Char or Line.
    pub rectangle: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection_mode_roundtrip() {
        for v in [0u8, 1, 2, 3] {
            let mode = SelectionMode::try_from_u8(v).expect("valid mode");
            assert_eq!(mode.as_u8(), v);
        }
    }

    #[test]
    fn selection_mode_unknown_rejected() {
        assert_eq!(SelectionMode::try_from_u8(99), None);
    }

    #[test]
    fn selection_event_construction() {
        let ev = SelectionEvent {
            mode: SelectionMode::Rect,
            rectangle: true,
        };
        assert_eq!(ev.mode, SelectionMode::Rect);
        assert!(ev.rectangle);
    }

    #[test]
    fn selection_event_equality() {
        let a = SelectionEvent {
            mode: SelectionMode::Char,
            rectangle: false,
        };
        let b = SelectionEvent {
            mode: SelectionMode::Char,
            rectangle: false,
        };
        let c = SelectionEvent {
            mode: SelectionMode::Rect,
            rectangle: true,
        };
        assert_eq!(a, b);
        assert_ne!(a, c);
    }
}
