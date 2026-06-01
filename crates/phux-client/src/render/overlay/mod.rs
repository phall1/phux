//! Overlay layer — modals, help, command palette.
//!
//! An overlay is a chrome-layer widget that takes over the outer terminal
//! while it's active: input is captured (no keystrokes reach the focused
//! pane's stdin) and pane stdout flushing is paused (per ADR-0020 §Decision
//! invariant 5). Pane libghostty mirrors keep consuming server VT bytes —
//! we only pause the *outbound* flush so the modal doesn't get trampled by
//! a `TERMINAL_OUTPUT` repaint. On dismiss, the driver triggers a full
//! repaint to restore pane content.
//!
//! [`OverlayState`] carries a *stack* of overlays. The top of the stack
//! captures input ([`RenderOverlay::handle_key`]); rendering walks the
//! stack bottom-up so stacked overlays compose (e.g. a command palette
//! painted on top of an open help modal). A single active overlay is the
//! one-element case — the common path is unchanged from a UX standpoint.
//!
//! Submodules:
//! - [`help`] — keybindings reference modal (phux-5ke.4)
//! - [`prompt`] — single-line text-input modal (phux-ahv.1)
//! - [`widgets`] — reusable themed primitives ([`Modal`], [`KeyChordTable`])
//!
//! [`Modal`]: widgets::Modal
//! [`KeyChordTable`]: widgets::KeyChordTable

use std::io::{self, Write};

use phux_protocol::input::key::KeyEvent;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

pub mod copy_mode;
pub mod help;
pub mod prompt;
pub mod select_list;
pub mod widgets;

pub use copy_mode::CopyModeOverlay;
pub use help::HelpOverlay;
pub use prompt::PromptOverlay;
pub use select_list::{SelectItem, SelectList};

/// Test double: a [`RenderOverlay`] that records every key handed to it and
/// never dismisses, so a test can assert exactly which keystrokes reached
/// the overlay. Lives here (not in `attach/`) because implementing
/// `RenderOverlay::render` names ratatui types, which the boundary guard
/// confines to `render/`. Used by the `attach::input_dispatch` overlay-input
/// routing regression test.
#[cfg(test)]
pub(crate) struct RecordingOverlay {
    pub(crate) keys: std::rc::Rc<std::cell::RefCell<Vec<KeyEvent>>>,
}

#[cfg(test)]
impl RenderOverlay for RecordingOverlay {
    fn render(&self, _area: Rect, _buf: &mut Buffer) {}
    fn handle_key(&mut self, key: &KeyEvent) -> OverlayCommand {
        self.keys.borrow_mut().push(key.clone());
        OverlayCommand::Stay
    }
}

/// A chrome-layer overlay rendered above pane interiors.
///
/// Implementors paint into a ratatui [`Buffer`] sized to the outer
/// viewport. [`handle_key`] receives structured [`KeyEvent`]s from the
/// driver — phux uses libghostty/protocol input atoms per ADR-0006 and
/// ADR-0008, NOT crossterm's event types, even though the rendering
/// toolkit (ratatui) is crossterm-adjacent. The overlay is responsible
/// for deciding when it's done; the driver inspects the returned
/// [`OverlayCommand`].
///
/// [`handle_key`]: RenderOverlay::handle_key
pub trait RenderOverlay {
    /// Paint into `buf` covering `area` (typically the full outer
    /// viewport). Cells the overlay does not write to are left as the
    /// `Buffer`'s default (blank black-on-default).
    fn render(&self, area: Rect, buf: &mut Buffer);

    /// React to a key event. Return [`OverlayCommand::Dismiss`] to close
    /// the overlay; [`OverlayCommand::Stay`] to keep it open and consume
    /// the key.
    fn handle_key(&mut self, key: &KeyEvent) -> OverlayCommand;
}

/// What an overlay wants the driver to do after [`RenderOverlay::handle_key`].
#[derive(Debug, Clone, PartialEq)]
pub enum OverlayCommand {
    /// Keep the overlay active; the key was consumed.
    Stay,
    /// Close the overlay. The driver triggers a full repaint to restore
    /// pane content on the next loop iteration.
    Dismiss,
    /// Close the overlay and run this action in the dispatcher — e.g. a
    /// committed rename prompt returning `rename-window { name }`. The
    /// dispatcher feeds it through the normal `run_action` path.
    Commit(phux_config::keybind::ResolvedAction),
    /// Keep the overlay active, but send a selection event to the server
    /// (used by copy-mode). The dispatcher fills in the `terminal_id`.
    SendSelection(phux_protocol::input::selection::SelectionEvent),
}

/// What [`OverlayState::handle_key`] hands back to the dispatcher.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum OverlayOutcome {
    /// Nothing to do (key consumed, overlay stayed or dismissed).
    #[default]
    None,
    /// The overlay committed; run this action.
    RunAction(phux_config::keybind::ResolvedAction),
    /// Send a selection event to the server (copy-mode).
    SendSelection(phux_protocol::input::selection::SelectionEvent),
}

/// Stacked overlay state.
///
/// The top of the stack captures input; rendering walks the stack
/// bottom-up so stacked overlays compose (palette painted on top of
/// help). A single active overlay is the one-element case.
#[derive(Default)]
pub struct OverlayState {
    /// Bottom-to-top overlay stack. The last element is the top — it
    /// receives input and is painted last (on top of the others).
    stack: Vec<Box<dyn RenderOverlay>>,
}

impl std::fmt::Debug for OverlayState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OverlayState")
            .field("depth", &self.stack.len())
            .finish()
    }
}

impl OverlayState {
    /// Empty state — no overlay active.
    #[must_use]
    pub const fn new() -> Self {
        Self { stack: Vec::new() }
    }

    /// `true` when at least one overlay is active (capturing input +
    /// pausing pane stdout). The driver loop reads this *only* via this
    /// accessor — it must not see ratatui types directly (CI grep guard
    /// enforces).
    #[must_use]
    pub const fn is_active(&self) -> bool {
        !self.stack.is_empty()
    }

    /// Number of overlays currently stacked (0 when inactive).
    #[must_use]
    pub fn depth(&self) -> usize {
        self.stack.len()
    }

    /// Push `overlay` onto the top of the stack. It becomes the input
    /// target and is painted last (above any overlays beneath it).
    pub fn push(&mut self, overlay: Box<dyn RenderOverlay>) {
        self.stack.push(overlay);
    }

    /// Dismiss (pop) the top overlay, revealing whatever was beneath it.
    /// No-op when the stack is empty.
    pub fn dismiss(&mut self) {
        self.stack.pop();
    }

    /// Dispatch a key event to the top overlay. Auto-dismisses (pops) on
    /// [`OverlayCommand::Dismiss`] and [`OverlayCommand::Commit`]; the
    /// latter also returns the action for the dispatcher to run. No-op
    /// (returns [`OverlayOutcome::None`]) when no overlay is active.
    pub fn handle_key(&mut self, key: &KeyEvent) -> OverlayOutcome {
        let Some(top) = self.stack.last_mut() else {
            return OverlayOutcome::None;
        };
        match top.handle_key(key) {
            OverlayCommand::Stay => OverlayOutcome::None,
            OverlayCommand::Dismiss => {
                self.dismiss();
                OverlayOutcome::None
            }
            OverlayCommand::Commit(action) => {
                self.dismiss();
                OverlayOutcome::RunAction(action)
            }
            OverlayCommand::SendSelection(event) => {
                // Copy-mode sends selection events; the overlay stays active.
                OverlayOutcome::SendSelection(event)
            }
        }
    }

    /// Paint the overlay stack into a fresh full-viewport buffer and emit
    /// it to `out` as VT bytes. No-op when no overlay is active.
    ///
    /// Overlays paint bottom-up into one shared buffer (top last), so a
    /// stacked overlay composes over the ones beneath it. The paint is a
    /// from-scratch render, not a diff: callers should clear-screen
    /// before invoking so leftover pane content doesn't bleed through the
    /// overlays' blank cells.
    pub fn paint(&self, out: &mut impl Write, viewport_dims: (u16, u16)) -> io::Result<()> {
        if self.stack.is_empty() {
            return Ok(());
        }
        let area = Rect::new(0, 0, viewport_dims.0, viewport_dims.1);
        let mut buf = Buffer::empty(area);
        for overlay in &self.stack {
            overlay.render(area, &mut buf);
        }
        emit_buffer(out, &buf)
    }
}

/// Emit a ratatui [`Buffer`] to `out` as VT cursor + SGR + glyph bytes.
///
/// Walks row by row; emits CUP at column 0 of each row, then writes each
/// cell's symbol with an SGR delta. This is a deliberately simple
/// renderer (one CUP per row, full SGR re-emit per cell-with-style) —
/// overlays repaint on input, not on every frame, so we trade per-cell
/// efficiency for code that's obvious to audit. The chrome submodule
/// (status bar + dividers) may grow a shared writer later; for now the
/// overlay path stays self-contained.
fn emit_buffer(out: &mut impl Write, buf: &Buffer) -> io::Result<()> {
    let area = buf.area;
    // Hide cursor for the duration of the modal paint.
    out.write_all(b"\x1b[?25l")?;
    for row in 0..area.height {
        // CUP to start of row (1-based).
        write!(out, "\x1b[{};{}H", row + 1, 1)?;
        // Reset SGR so the previous row's tail style can't leak.
        out.write_all(b"\x1b[0m")?;
        let mut prev_styled = false;
        for col in 0..area.width {
            let cell = &buf[(area.x + col, area.y + row)];
            crate::render::sgr::emit_cell_sgr(out, cell, &mut prev_styled)?;
            let sym = cell.symbol();
            if sym.is_empty() {
                out.write_all(b" ")?;
            } else {
                out.write_all(sym.as_bytes())?;
            }
        }
        if prev_styled {
            out.write_all(b"\x1b[0m")?;
        }
    }
    // Park the cursor at (1,1) — overlay-active state implies no pane
    // cursor visible. Stays hidden until the overlay dismisses and the
    // pane re-paint emits its own DECTCEM.
    out.write_all(b"\x1b[1;1H")?;
    out.flush()
}

#[cfg(test)]
#[allow(clippy::expect_used, reason = "tests")]
mod tests {
    use super::*;
    use phux_protocol::input::key::{KeyAction, ModSet, PhysicalKey};

    fn key(k: PhysicalKey) -> KeyEvent {
        KeyEvent {
            action: KeyAction::Press,
            key: k,
            mods: ModSet::empty(),
            consumed_mods: ModSet::empty(),
            composing: false,
            text: None,
            unshifted_codepoint: None,
        }
    }

    struct EscDismiss;
    impl RenderOverlay for EscDismiss {
        fn render(&self, _area: Rect, _buf: &mut Buffer) {}
        fn handle_key(&mut self, key: &KeyEvent) -> OverlayCommand {
            if key.key == PhysicalKey::Escape {
                OverlayCommand::Dismiss
            } else {
                OverlayCommand::Stay
            }
        }
    }

    #[test]
    fn state_starts_inactive() {
        let s = OverlayState::new();
        assert!(!s.is_active());
    }

    #[test]
    fn push_then_dismiss_round_trip() {
        let mut s = OverlayState::new();
        s.push(Box::new(EscDismiss));
        assert!(s.is_active());
        s.dismiss();
        assert!(!s.is_active());
    }

    #[test]
    fn handle_key_auto_dismisses_on_esc() {
        let mut s = OverlayState::new();
        s.push(Box::new(EscDismiss));
        s.handle_key(&key(PhysicalKey::Escape));
        assert!(!s.is_active(), "Esc should dismiss");
    }

    #[test]
    fn handle_key_stays_on_other_keys() {
        let mut s = OverlayState::new();
        s.push(Box::new(EscDismiss));
        s.handle_key(&key(PhysicalKey::A));
        assert!(s.is_active(), "non-Esc should not dismiss");
    }

    #[test]
    fn dismiss_pops_only_the_top() {
        let mut s = OverlayState::new();
        s.push(Box::new(EscDismiss));
        s.push(Box::new(EscDismiss));
        assert_eq!(s.depth(), 2);
        s.dismiss();
        assert_eq!(s.depth(), 1, "dismiss pops one, not clear-all");
        assert!(s.is_active());
        s.dismiss();
        assert!(!s.is_active());
    }

    #[test]
    fn handle_key_targets_only_the_top_overlay() {
        // A stay-forever overlay on top must shield the Esc-dismiss
        // overlay beneath it: keys go to the top only.
        struct StayForever;
        impl RenderOverlay for StayForever {
            fn render(&self, _area: Rect, _buf: &mut Buffer) {}
            fn handle_key(&mut self, _key: &KeyEvent) -> OverlayCommand {
                OverlayCommand::Stay
            }
        }
        let mut s = OverlayState::new();
        s.push(Box::new(EscDismiss));
        s.push(Box::new(StayForever));
        s.handle_key(&key(PhysicalKey::Escape));
        assert_eq!(
            s.depth(),
            2,
            "Esc reached the StayForever top, not the EscDismiss beneath"
        );
        // Pop the top; now Esc reaches the EscDismiss overlay.
        s.dismiss();
        s.handle_key(&key(PhysicalKey::Escape));
        assert!(!s.is_active());
    }

    #[test]
    fn paint_composes_stack_bottom_up() {
        // Two overlays writing to distinct cells: both appear. The top
        // overlay overwrites the bottom on any shared cell because it
        // paints last.
        struct WriteAt(u16, &'static str);
        impl RenderOverlay for WriteAt {
            fn render(&self, area: Rect, buf: &mut Buffer) {
                buf.set_string(
                    area.x,
                    area.y + self.0,
                    self.1,
                    ratatui::style::Style::default(),
                );
            }
            fn handle_key(&mut self, _key: &KeyEvent) -> OverlayCommand {
                OverlayCommand::Stay
            }
        }
        let mut s = OverlayState::new();
        s.push(Box::new(WriteAt(0, "bottom")));
        s.push(Box::new(WriteAt(1, "topp")));
        let mut out = Vec::new();
        s.paint(&mut out, (20, 5)).expect("paint");
        let txt = String::from_utf8_lossy(&out);
        assert!(
            txt.contains("bottom"),
            "bottom overlay should be painted:\n{txt}"
        );
        assert!(
            txt.contains("topp"),
            "top overlay should be painted:\n{txt}"
        );
    }

    #[test]
    fn paint_inactive_is_noop() {
        let s = OverlayState::new();
        let mut buf = Vec::new();
        s.paint(&mut buf, (80, 24)).expect("paint");
        assert!(buf.is_empty());
    }

    #[test]
    fn paint_active_emits_some_bytes() {
        struct Filled;
        impl RenderOverlay for Filled {
            fn render(&self, area: Rect, buf: &mut Buffer) {
                buf.set_string(area.x, area.y, "hello", ratatui::style::Style::default());
            }
            fn handle_key(&mut self, _key: &KeyEvent) -> OverlayCommand {
                OverlayCommand::Stay
            }
        }
        let mut s = OverlayState::new();
        s.push(Box::new(Filled));
        let mut out = Vec::new();
        s.paint(&mut out, (20, 5)).expect("paint");
        let txt = String::from_utf8_lossy(&out);
        assert!(txt.contains("hello"));
        // Cursor hide at the top.
        assert!(out.starts_with(b"\x1b[?25l"));
    }
}
