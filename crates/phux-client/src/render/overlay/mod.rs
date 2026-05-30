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
//! v1 carries a single active overlay ([`OverlayState`]). Multi-overlay
//! stacking (palette-on-top-of-help, etc.) is intentionally future work;
//! the type signature is `Option<Box<dyn RenderOverlay>>`, not a real
//! stack.
//!
//! Submodules:
//! - [`help`] — keybindings reference modal (phux-5ke.4)

use std::io::{self, Write};

use phux_protocol::input::key::KeyEvent;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

pub mod help;

pub use help::HelpOverlay;

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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverlayCommand {
    /// Keep the overlay active; the key was consumed.
    Stay,
    /// Close the overlay. The driver triggers a full repaint to restore
    /// pane content on the next loop iteration.
    Dismiss,
}

/// One-slot overlay state. v1 carries at most one active overlay; the
/// `Option<Box<…>>` shape leaves room for a future stack without forcing
/// every call site through `Vec` indexing.
#[derive(Default)]
pub struct OverlayState {
    active: Option<Box<dyn RenderOverlay>>,
}

impl std::fmt::Debug for OverlayState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OverlayState")
            .field("active", &self.active.is_some())
            .finish()
    }
}

impl OverlayState {
    /// Empty state — no overlay active.
    #[must_use]
    pub const fn new() -> Self {
        Self { active: None }
    }

    /// `true` when an overlay is currently capturing input + pausing pane
    /// stdout. The driver loop reads this *only* via this accessor — it
    /// must not see ratatui types directly (CI grep guard enforces).
    #[must_use]
    pub const fn is_active(&self) -> bool {
        self.active.is_some()
    }

    /// Push `overlay` as the active one. v1: silently replaces any
    /// previous overlay; logged as a debug because the only path that
    /// can hit this is the user binding two `show-*` chords without
    /// dismissing in between.
    pub fn push(&mut self, overlay: Box<dyn RenderOverlay>) {
        if self.active.is_some() {
            tracing::debug!("replacing active overlay (v1 has no real stack)");
        }
        self.active = Some(overlay);
    }

    /// Dismiss the active overlay (if any).
    pub fn dismiss(&mut self) {
        self.active = None;
    }

    /// Dispatch a key event to the active overlay. Auto-dismisses on
    /// [`OverlayCommand::Dismiss`]. No-op when no overlay is active.
    pub fn handle_key(&mut self, key: &KeyEvent) {
        if let Some(active) = self.active.as_mut() {
            match active.handle_key(key) {
                OverlayCommand::Stay => {}
                OverlayCommand::Dismiss => self.dismiss(),
            }
        }
    }

    /// Paint the active overlay into a fresh full-viewport buffer and
    /// emit it to `out` as VT bytes. No-op when no overlay is active.
    ///
    /// The overlay paint is a from-scratch render, not a diff: callers
    /// should clear-screen before invoking so leftover pane content
    /// doesn't bleed through the overlay's blank cells.
    pub fn paint(&self, out: &mut impl Write, viewport_dims: (u16, u16)) -> io::Result<()> {
        let Some(active) = self.active.as_ref() else {
            return Ok(());
        };
        let area = Rect::new(0, 0, viewport_dims.0, viewport_dims.1);
        let mut buf = Buffer::empty(area);
        active.render(area, &mut buf);
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
