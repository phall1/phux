//! Wire-level input event types.
//!
//! These mirror libghostty-vt's `key::Event`, `mouse::Event`, `focus::Event`,
//! and paste utilities one-to-one — see ADR-0006. Numeric discriminants are
//! chosen to match libghostty's enums verbatim so the server-side
//! `From<&phux_protocol::input::*>` conversions are field-for-field copies.
//!
//! Wire encoding for these types lives in [`crate::wire`].

pub mod focus;
pub mod key;
pub mod mouse;
pub mod paste;
pub mod selection;

use focus::FocusEvent;
use key::KeyEvent;
use mouse::MouseEvent;
use paste::PasteEvent;
use selection::SelectionEvent;

use crate::ids::TerminalId;
use crate::wire::frame::FrameKind;

/// The tagged union of client-to-server input events.
///
/// These atoms are carried by the `INPUT_KEY` / `INPUT_MOUSE` / `INPUT_FOCUS`
/// / `INPUT_PASTE` / `INPUT_SELECTION` frames ([`docs/spec/input.md`]).
/// Bundling them lets a single command carry an already-built input event
/// without one frame variant per atom — used by `ROUTE_INPUT` (L1.md §5.1),
/// the side-effect-free input route that feeds a pane without an attach.
///
/// `#[non_exhaustive]` so a future minor protocol version can add an atom
/// without breaking downstream matches.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum InputEvent {
    /// A structured key event (`INPUT_KEY` — `docs/spec/input.md` §2).
    Key(KeyEvent),
    /// A structured mouse event (`INPUT_MOUSE` — `docs/spec/input.md` §3).
    Mouse(MouseEvent),
    /// A focus state change (`INPUT_FOCUS` — `docs/spec/input.md` §4).
    Focus(FocusEvent),
    /// A paste payload (`INPUT_PASTE` — `docs/spec/input.md` §5).
    Paste(PasteEvent),
    /// A selection mode change (`INPUT_SELECTION` — `docs/spec/input.md` §6).
    /// Selection is client-owned state; the server stores it per-terminal and
    /// emits no output to the PTY. Extraction is requested via COMMAND.
    Selection(SelectionEvent),
}

impl InputEvent {
    /// Wrap this event in the matching per-atom input [`FrameKind`]
    /// addressed to `terminal_id` (`INPUT_KEY` / `INPUT_MOUSE` /
    /// `INPUT_FOCUS` / `INPUT_PASTE` / `INPUT_SELECTION`). Used by the attach
    /// loop to ship a parsed event to its focused pane.
    #[must_use]
    pub fn into_frame(self, terminal_id: TerminalId) -> FrameKind {
        match self {
            Self::Key(event) => FrameKind::InputKey { terminal_id, event },
            Self::Mouse(event) => FrameKind::InputMouse { terminal_id, event },
            Self::Focus(event) => FrameKind::InputFocus { terminal_id, event },
            Self::Paste(event) => FrameKind::InputPaste { terminal_id, event },
            Self::Selection(event) => FrameKind::InputSelection { terminal_id, event },
        }
    }
}
