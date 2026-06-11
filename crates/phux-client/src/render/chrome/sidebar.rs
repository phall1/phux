//! Window sidebar painter (phux-4h5a).
//!
//! A vertical strip of window "tabs" — one row per window, labelled by the
//! window's name (which upstream already resolves to the pane's live OSC
//! title, phux-efj7), the active window highlighted. A vertical rule on the
//! strip's last column separates it from the panes. The reservation +
//! placement is owned by the driver; this type just paints into the `Rect`
//! it is handed and caches the last paint so an unchanged repaint emits
//! nothing — the same incremental discipline as the status bar.

use std::io::{self, Write};

use phux_config::widget::WindowInfo;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect as RataRect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};

use crate::layout::Rect;
use crate::render::Theme;

/// VT painter for the window sidebar.
#[derive(Debug)]
pub struct SidebarPainter {
    windows: Vec<WindowInfo>,
    theme: Theme,
    /// Cache: the `(rect, windows)` of the last paint. An identical repaint
    /// is a zero-byte no-op.
    last: Option<(Rect, Vec<WindowInfo>)>,
}

impl SidebarPainter {
    /// A painter styled by `theme`, initially showing no windows.
    #[must_use]
    pub const fn new(theme: Theme) -> Self {
        Self {
            windows: Vec::new(),
            theme,
            last: None,
        }
    }

    /// Replace the window list (driver calls this from the same
    /// `window_infos` snapshot that feeds the status-bar tab strip).
    pub fn set_windows(&mut self, windows: Vec<WindowInfo>) {
        self.windows = windows;
    }

    /// Drop the paint cache so the next [`Self::paint`] re-emits even if its
    /// inputs are unchanged (e.g. after a full-frame clear).
    pub fn invalidate(&mut self) {
        self.last = None;
    }

    /// Paint the sidebar into `rect` (outer-viewport cells). No-op when the
    /// rect is empty or unchanged since the last paint.
    pub fn paint<W: Write>(&mut self, out: &mut W, rect: Rect) -> io::Result<()> {
        if rect.w == 0 || rect.h == 0 {
            return Ok(());
        }
        if self
            .last
            .as_ref()
            .is_some_and(|(r, w)| *r == rect && *w == self.windows)
        {
            return Ok(());
        }
        let buf = self.compose(rect);
        emit(out, &buf, rect)?;
        self.last = Some((rect, self.windows.clone()));
        Ok(())
    }

    /// Render the tab list + separator into a fresh `rect`-sized buffer.
    fn compose(&self, rect: Rect) -> Buffer {
        let area = RataRect::new(0, 0, rect.w, rect.h);
        let mut buf = Buffer::empty(area);
        // Text occupies every column except the 1-cell right separator.
        let text_w = rect.w.saturating_sub(1);
        if text_w > 0 {
            let label_w = usize::from(text_w).saturating_sub(2); // marker is 2 cells
            let lines: Vec<Line<'static>> = self
                .windows
                .iter()
                .map(|w| {
                    let marker = if w.active { "▸ " } else { "  " };
                    let label = truncate(&w.name, label_w);
                    let style = if w.active {
                        Style::default()
                            .fg(self.theme.accent)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(self.theme.action)
                    };
                    Line::from(Span::styled(format!("{marker}{label}"), style))
                })
                .collect();
            Paragraph::new(lines).render(RataRect::new(0, 0, text_w, rect.h), &mut buf);
        }
        // Vertical rule down the strip's last column.
        let sep_x = rect.w.saturating_sub(1);
        for y in 0..rect.h {
            if let Some(cell) = buf.cell_mut((sep_x, y)) {
                cell.set_symbol("│");
                cell.set_style(Style::default().fg(self.theme.border));
            }
        }
        buf
    }
}

/// Truncate `s` to `max` cells, appending `…` when it overflows. A `max` of
/// 0 yields the empty string.
fn truncate(s: &str, max: usize) -> String {
    if max == 0 {
        return String::new();
    }
    if s.chars().count() <= max {
        return s.to_owned();
    }
    s.chars()
        .take(max.saturating_sub(1))
        .chain(std::iter::once('…'))
        .collect()
}

/// Emit `buf` to `out` at `rect`'s origin, row by row, with a per-cell SGR
/// delta (shared with the overlay + status-bar painters).
fn emit<W: Write>(out: &mut W, buf: &Buffer, rect: Rect) -> io::Result<()> {
    for row in 0..rect.h {
        write!(out, "\x1b[{};{}H\x1b[0m", rect.y + row + 1, rect.x + 1)?;
        let mut prev_styled = false;
        for col in 0..rect.w {
            let cell = &buf[(col, row)];
            crate::render::sgr::emit_cell_sgr(out, cell, &mut prev_styled)?;
            let sym = cell.symbol();
            if sym.is_empty() {
                out.write_all(b" ")?;
            } else {
                out.write_all(sym.as_bytes())?;
            }
        }
        out.write_all(b"\x1b[0m")?;
    }
    out.flush()
}

#[cfg(test)]
#[allow(clippy::expect_used, reason = "tests")]
mod tests {
    use super::*;

    fn win(name: &str, active: bool) -> WindowInfo {
        WindowInfo {
            name: name.to_owned(),
            active,
            zoomed: false,
        }
    }

    fn paint_to_string(painter: &mut SidebarPainter, rect: Rect) -> String {
        let mut out = Vec::new();
        painter.paint(&mut out, rect).expect("paint");
        String::from_utf8(out).expect("utf8")
    }

    /// Strip CSI escape sequences so an assertion can read the plain glyphs —
    /// a styled (active) row interleaves a per-cell SGR between every cell, so
    /// its label is not a contiguous substring of the raw byte stream.
    fn strip_ansi(s: &str) -> String {
        let mut out = String::new();
        let mut chars = s.chars();
        while let Some(c) = chars.next() {
            if c == '\u{1b}' {
                // CSI is `ESC [ params... final`; the final byte of the
                // sequences we emit (`H`, `m`) is an ASCII letter, while the
                // introducer `[`, digits, and `;` are not — consume through
                // the first letter.
                for d in chars.by_ref() {
                    if d.is_ascii_alphabetic() {
                        break;
                    }
                }
            } else {
                out.push(c);
            }
        }
        out
    }

    #[test]
    fn renders_each_window_label() {
        let mut p = SidebarPainter::new(Theme::default());
        p.set_windows(vec![win("editor", false), win("shell", true)]);
        let raw = paint_to_string(
            &mut p,
            Rect {
                x: 0,
                y: 0,
                w: 20,
                h: 6,
            },
        );
        let plain = strip_ansi(&raw);
        assert!(plain.contains("editor"), "first tab label: {plain:?}");
        assert!(plain.contains("shell"), "second tab label: {plain:?}");
        // The active window gets the focus marker.
        assert!(plain.contains('▸'), "active marker missing: {plain:?}");
        // Separator rule present.
        assert!(plain.contains('│'), "separator missing: {plain:?}");
    }

    #[test]
    fn places_rows_at_the_rect_origin() {
        let mut p = SidebarPainter::new(Theme::default());
        p.set_windows(vec![win("a", true)]);
        // Right-docked: rect origin at column 60.
        let s = paint_to_string(
            &mut p,
            Rect {
                x: 60,
                y: 0,
                w: 20,
                h: 4,
            },
        );
        // First row CUP targets the rect's column (61, 1-based).
        assert!(s.contains("\x1b[1;61H"), "origin CUP missing: {s:?}");
    }

    #[test]
    fn unchanged_repaint_is_a_no_op() {
        let mut p = SidebarPainter::new(Theme::default());
        p.set_windows(vec![win("a", true)]);
        let rect = Rect {
            x: 0,
            y: 0,
            w: 16,
            h: 4,
        };
        assert!(
            !paint_to_string(&mut p, rect).is_empty(),
            "first paint emits"
        );
        // Same inputs ⇒ cached ⇒ nothing emitted.
        assert!(
            paint_to_string(&mut p, rect).is_empty(),
            "unchanged repaint must emit nothing"
        );
        // A window change invalidates the cache.
        p.set_windows(vec![win("b", true)]);
        assert!(
            !paint_to_string(&mut p, rect).is_empty(),
            "changed windows must re-emit"
        );
    }

    #[test]
    fn long_label_is_truncated_with_ellipsis() {
        let mut p = SidebarPainter::new(Theme::default());
        p.set_windows(vec![win("a-very-long-window-title-indeed", true)]);
        let s = paint_to_string(
            &mut p,
            Rect {
                x: 0,
                y: 0,
                w: 12,
                h: 3,
            },
        );
        assert!(s.contains('…'), "overflowing label should be elided: {s:?}");
    }
}
