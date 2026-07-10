//! Window sidebar painter (phux-4h5a, herdr-shaped by phux-p4vp/phux-fce4).
//!
//! A vertical strip of window "tabs" — one two-row block per window: the
//! window's name (which upstream already resolves to the pane's live OSC
//! title, phux-efj7, or its ADR-0040 agent label) with the active window
//! highlighted, and a dim branch line underneath when the window's focused
//! pane sits inside a git repository (phux-p4vp). The strip's last two rows
//! are the `+ new` / `= menu` affordances (display-only until phux-fce4
//! wires their hit targets). A vertical rule on the strip's last column
//! separates it from the panes. The reservation + placement is owned by the
//! driver; this type just paints into the `Rect` it is handed and caches
//! the last paint so an unchanged repaint emits nothing — the same
//! incremental discipline as the status bar.

use std::io::{self, Write};

use phux_config::widget::WindowInfo;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect as RataRect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};

use crate::layout::Rect;
use crate::render::Theme;

/// Label of the "create" affordance row (phux-fce4).
///
/// Clicking it runs the `new-window` action — the sidebar lists windows,
/// so `+ new` creates one.
pub const NEW_LABEL: &str = "+ new";
/// Label of the "menu" affordance row (phux-fce4).
///
/// Clicking it opens the command palette — the one menu that covers
/// window, session, and plugin actions (`new-session` included) through
/// the action registry.
pub const MENU_LABEL: &str = "= menu";

/// Minimum strip height (rows) at which the footer affordances render.
/// Below this every row goes to window blocks — a 2–3 row strip showing
/// only chrome and no windows would be useless.
const MIN_FOOTER_HEIGHT: u16 = 4;

/// One row of the strip, top to bottom. The painter derives from this
/// single model (and phux-fce4's hit-test will share it), so paint and
/// click targets cannot drift.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SidebarRow {
    /// Window `i`'s name row.
    WindowName(usize),
    /// Window `i`'s branch row (dim; blank when the window has no branch).
    WindowBranch(usize),
    /// Unused padding between the window blocks and the footer.
    Blank,
    /// The `+ new` affordance (create a window).
    NewWindow,
    /// The `= menu` affordance (open the command palette).
    Menu,
}

/// The strip's row model for `window_count` windows in an `h`-row rect.
///
/// Each window occupies a fixed two-row block (name + branch) from the top;
/// when `h >= MIN_FOOTER_HEIGHT` the bottom two rows are reserved for the
/// `+ new` / `= menu` affordances and window blocks that would collide are
/// truncated. Fixed-size blocks keep the model derivable from the window
/// *count* alone, so a hit-test (phux-fce4) can share it without
/// rebuilding the full window projection.
#[must_use]
pub fn row_model(window_count: usize, h: u16) -> Vec<SidebarRow> {
    let h = usize::from(h);
    let footer = if h >= usize::from(MIN_FOOTER_HEIGHT) {
        2
    } else {
        0
    };
    let window_area = h - footer;
    let mut rows = Vec::with_capacity(h);
    'blocks: for i in 0..window_count {
        for row in [SidebarRow::WindowName(i), SidebarRow::WindowBranch(i)] {
            if rows.len() >= window_area {
                break 'blocks;
            }
            rows.push(row);
        }
    }
    // A truncated block must not show a name row without room for its
    // branch row being *reserved* — but a dangling name row is still more
    // useful than a blank, so we keep it (the branch row is simply absent).
    while rows.len() < window_area {
        rows.push(SidebarRow::Blank);
    }
    if footer == 2 {
        rows.push(SidebarRow::NewWindow);
        rows.push(SidebarRow::Menu);
    }
    rows
}

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
    /// Returns `true` if the list actually changed, so a caller with no
    /// other paint trigger (the agent-event chrome path) can gate a repaint
    /// on it; the paint cache below makes an unchanged repaint free either
    /// way.
    pub fn set_windows(&mut self, windows: Vec<WindowInfo>) -> bool {
        if self.windows == windows {
            return false;
        }
        self.windows = windows;
        true
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

    /// Compose the strip into a `rect`-sized ratatui [`Buffer`] (origin
    /// `(0, 0)`), for the structured `snapshot --rendered` compositor
    /// (phux-l5xa / phux-4h5a). The VT [`Self::paint`] path uses the same
    /// `compose` step internally, so the cells match a live paint.
    #[must_use]
    pub fn compose_buffer(&self, rect: Rect) -> Buffer {
        self.compose(rect)
    }

    /// Render one window's name row.
    fn name_line(&self, w: &WindowInfo, text_w: u16) -> Line<'static> {
        let marker = if w.active { "▸ " } else { "  " };
        // phux-foz.1: reserve 2 cells for the ` !` attention
        // suffix so a long label can't push it off the strip.
        let label_w = usize::from(text_w)
            .saturating_sub(2) // marker is 2 cells
            .saturating_sub(if w.attention { 2 } else { 0 });
        let label = truncate(&w.name, label_w);
        let style = if w.active {
            Style::default()
                .fg(self.theme.accent)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(self.theme.action)
        };
        let mut spans = vec![Span::styled(format!("{marker}{label}"), style)];
        // phux-foz.1: a window holding a pane that asked for a
        // human answer (ADR-0035) gets a themed `!` marker.
        if w.attention {
            spans.push(Span::styled(
                " !",
                Style::default()
                    .fg(self.theme.attention)
                    .add_modifier(Modifier::BOLD),
            ));
        }
        Line::from(spans)
    }

    /// Render one window's branch row (phux-p4vp): the focused pane's VCS
    /// branch, dim and nested under the label. Blank when unknown.
    fn branch_line(&self, w: &WindowInfo, text_w: u16) -> Line<'static> {
        let Some(branch) = w.branch.as_deref() else {
            return Line::from("");
        };
        let label = truncate(branch, usize::from(text_w).saturating_sub(4));
        Line::from(Span::styled(
            format!("    {label}"),
            Style::default()
                .fg(self.theme.action)
                .add_modifier(Modifier::DIM),
        ))
    }

    /// Render an affordance row (phux-fce4).
    fn affordance_line(&self, label: &str, text_w: u16) -> Line<'static> {
        let label = truncate(label, usize::from(text_w).saturating_sub(2));
        Line::from(Span::styled(
            format!("  {label}"),
            Style::default().fg(self.theme.action),
        ))
    }

    /// Render the tab list + affordances + separator into a fresh
    /// `rect`-sized buffer, row-for-row from [`row_model`].
    fn compose(&self, rect: Rect) -> Buffer {
        let area = RataRect::new(0, 0, rect.w, rect.h);
        let mut buf = Buffer::empty(area);
        // Text occupies every column except the 1-cell right separator.
        let text_w = rect.w.saturating_sub(1);
        if text_w > 0 {
            let lines: Vec<Line<'static>> = row_model(self.windows.len(), rect.h)
                .into_iter()
                .map(|row| match row {
                    SidebarRow::WindowName(i) => self
                        .windows
                        .get(i)
                        .map_or_else(|| Line::from(""), |w| self.name_line(w, text_w)),
                    SidebarRow::WindowBranch(i) => self
                        .windows
                        .get(i)
                        .map_or_else(|| Line::from(""), |w| self.branch_line(w, text_w)),
                    SidebarRow::Blank => Line::from(""),
                    SidebarRow::NewWindow => self.affordance_line(NEW_LABEL, text_w),
                    SidebarRow::Menu => self.affordance_line(MENU_LABEL, text_w),
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
            attention: false,
            branch: None,
        }
    }

    fn win_attention(name: &str, active: bool) -> WindowInfo {
        WindowInfo {
            attention: true,
            ..win(name, active)
        }
    }

    fn win_branch(name: &str, active: bool, branch: &str) -> WindowInfo {
        WindowInfo {
            branch: Some(branch.to_owned()),
            ..win(name, active)
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
                h: 8,
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

    /// phux-foz.1: a window whose pane asked for a human answer (ADR-0035)
    /// carries a `!` marker on its sidebar tab; unmarked tabs stay plain.
    /// The marker change also busts the paint cache.
    #[test]
    fn attention_window_gets_a_marker() {
        let mut p = SidebarPainter::new(Theme::default());
        p.set_windows(vec![win("editor", true), win("shell", false)]);
        let rect = Rect {
            x: 0,
            y: 0,
            w: 20,
            h: 8,
        };
        let plain = strip_ansi(&paint_to_string(&mut p, rect));
        assert!(!plain.contains('!'), "no attention, no marker: {plain:?}");
        // The asking window gets the marker; the cache re-emits.
        assert!(
            p.set_windows(vec![win("editor", true), win_attention("shell", false)]),
            "attention flip must report a change"
        );
        let plain = strip_ansi(&paint_to_string(&mut p, rect));
        assert!(
            plain.contains("shell !"),
            "asking window tab must carry the marker: {plain:?}"
        );
    }

    /// An identical window list reports no change (the agent-event chrome
    /// path gates its repaint on this).
    #[test]
    fn set_windows_reports_change_only_on_difference() {
        let mut p = SidebarPainter::new(Theme::default());
        assert!(p.set_windows(vec![win("a", true)]));
        assert!(!p.set_windows(vec![win("a", true)]));
        assert!(p.set_windows(vec![win_attention("a", true)]));
        // phux-p4vp: a branch change alone busts the cache too — a
        // `git switch` must repaint the branch line.
        assert!(p.set_windows(vec![win_branch("a", true, "main")]));
        assert!(p.set_windows(vec![win_branch("a", true, "feature")]));
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

    /// phux-p4vp: a window with a branch renders it dim on the row under
    /// its label, herdr-style; a window without one leaves the row blank.
    #[test]
    fn branch_renders_on_the_row_under_the_label() {
        let mut p = SidebarPainter::new(Theme::default());
        p.set_windows(vec![
            win_branch("phux", true, "wave2/herdr"),
            win("scratch", false),
        ]);
        let rect = Rect {
            x: 0,
            y: 0,
            w: 20,
            h: 8,
        };
        let plain = strip_ansi(&paint_to_string(&mut p, rect));
        assert!(
            plain.contains("wave2/herdr"),
            "branch line missing: {plain:?}"
        );
        // Row order: name, branch, next name — check via the composed
        // buffer, whose rows are addressable.
        let buf = p.compose_buffer(rect);
        let row_text = |y: u16| -> String {
            (0..rect.w.saturating_sub(1))
                .map(|x| buf[(x, y)].symbol().chars().next().unwrap_or(' '))
                .collect()
        };
        assert!(row_text(0).contains("phux"), "row 0: {:?}", row_text(0));
        assert!(
            row_text(1).contains("wave2/herdr"),
            "row 1: {:?}",
            row_text(1)
        );
        assert!(row_text(2).contains("scratch"), "row 2: {:?}", row_text(2));
        assert!(
            row_text(3).trim().is_empty(),
            "branchless window's branch row must be blank: {:?}",
            row_text(3)
        );
    }

    /// phux-fce4: the footer affordances render on the strip's last two
    /// rows when the strip is tall enough, and drop out below the minimum.
    #[test]
    fn footer_affordances_render_on_the_last_two_rows() {
        let mut p = SidebarPainter::new(Theme::default());
        p.set_windows(vec![win("shell", true)]);
        let rect = Rect {
            x: 0,
            y: 0,
            w: 20,
            h: 8,
        };
        let buf = p.compose_buffer(rect);
        let row_text = |y: u16| -> String {
            (0..rect.w.saturating_sub(1))
                .map(|x| buf[(x, y)].symbol().chars().next().unwrap_or(' '))
                .collect()
        };
        assert!(
            row_text(6).contains(NEW_LABEL),
            "row 6 should hold the new affordance: {:?}",
            row_text(6)
        );
        assert!(
            row_text(7).contains(MENU_LABEL),
            "row 7 should hold the menu affordance: {:?}",
            row_text(7)
        );
        // A 3-row strip is below the footer minimum: no affordances.
        let short = Rect {
            x: 0,
            y: 0,
            w: 20,
            h: 3,
        };
        let plain = strip_ansi(&paint_to_string(&mut p, short));
        assert!(
            !plain.contains(NEW_LABEL) && !plain.contains(MENU_LABEL),
            "short strip must not render the footer: {plain:?}"
        );
    }

    // ---------- phux-fce4: row model + hit-test ----------

    #[test]
    fn row_model_reserves_footer_and_truncates_blocks() {
        // 3 windows in 8 rows: 6 window-area rows fit exactly 3 blocks.
        let rows = row_model(3, 8);
        assert_eq!(rows.len(), 8);
        assert_eq!(rows[0], SidebarRow::WindowName(0));
        assert_eq!(rows[1], SidebarRow::WindowBranch(0));
        assert_eq!(rows[4], SidebarRow::WindowName(2));
        assert_eq!(rows[5], SidebarRow::WindowBranch(2));
        assert_eq!(rows[6], SidebarRow::NewWindow);
        assert_eq!(rows[7], SidebarRow::Menu);
        // 3 windows in 6 rows: 4 window-area rows truncate the third block.
        let rows = row_model(3, 6);
        assert_eq!(rows[3], SidebarRow::WindowBranch(1));
        assert_eq!(rows[4], SidebarRow::NewWindow);
        assert_eq!(rows[5], SidebarRow::Menu);
        // Below the minimum height there is no footer.
        let rows = row_model(1, 3);
        assert_eq!(
            rows,
            vec![
                SidebarRow::WindowName(0),
                SidebarRow::WindowBranch(0),
                SidebarRow::Blank
            ]
        );
    }

    /// The painter fills rows exactly as [`row_model`] lays them out.
    #[test]
    fn paint_follows_the_row_model_row_for_row() {
        let rect = Rect {
            x: 0,
            y: 0,
            w: 20,
            h: 10,
        };
        let windows = vec![
            win_branch("alpha", true, "main"),
            win("beta", false),
            win_branch("gamma", false, "dev"),
        ];
        let mut p = SidebarPainter::new(Theme::default());
        p.set_windows(windows.clone());
        let buf = p.compose_buffer(rect);
        let row_text = |y: u16| -> String {
            (0..rect.w.saturating_sub(1))
                .map(|x| buf[(x, y)].symbol().chars().next().unwrap_or(' '))
                .collect()
        };
        for (y, row) in row_model(windows.len(), rect.h).iter().enumerate() {
            let y16 = u16::try_from(y).expect("row fits u16");
            match row {
                SidebarRow::WindowName(i) => {
                    assert!(row_text(y16).contains(&windows[*i].name));
                }
                SidebarRow::WindowBranch(i) => {
                    if let Some(b) = windows[*i].branch.as_deref() {
                        assert!(row_text(y16).contains(b));
                    }
                }
                SidebarRow::Blank => assert!(row_text(y16).trim().is_empty()),
                SidebarRow::NewWindow => {
                    assert!(row_text(y16).contains(NEW_LABEL));
                }
                SidebarRow::Menu => {
                    assert!(row_text(y16).contains(MENU_LABEL));
                }
            }
        }
    }
}
