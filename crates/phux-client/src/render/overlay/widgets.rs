//! Reusable themed overlay primitives (phux-ahv.5).
//!
//! Overlays ([`help`], [`prompt`], and future ones like a command palette)
//! share two visual building blocks:
//!
//! - [`Modal`] — a centered bordered box with a title, body, and optional
//!   footer, styled through [`Theme`] slots (`border`, `accent`, `dim`).
//!   Built on ratatui [`Block`] + [`Paragraph`].
//! - [`KeyChordTable`] — the chord/description columns the help overlay
//!   shows, grouped into titled sections and column-aligned across
//!   section boundaries. Styled through the `chord`, `action`, and
//!   `section_header` slots.
//!
//! Both render into a ratatui [`Buffer`] so they compose with the overlay
//! paint path. They own (copy) their [`Theme`] so the overlay that holds
//! them stays `'static`.
//!
//! [`help`]: super::help
//! [`prompt`]: super::prompt
//! [`Block`]: ratatui::widgets::Block
//! [`Paragraph`]: ratatui::widgets::Paragraph

use ratatui::buffer::Buffer;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Widget, Wrap};

use crate::render::Theme;

/// A centered, bordered modal box: themed border + centered title, a body
/// of pre-built [`Line`]s, and an optional dimmed footer line.
///
/// The caller supplies the body content (already styled) and the
/// [`Modal`] owns the chrome — border color from [`Theme::border`], title
/// from [`Theme::accent`], footer from [`Theme::dim`]. Render with
/// [`Modal::render_into`], passing the modal rect (use [`centered`] to
/// compute one).
#[derive(Debug, Clone)]
pub struct Modal<'a> {
    theme: Theme,
    title: String,
    body: Vec<Line<'a>>,
    footer: Option<String>,
    wrap: bool,
}

impl<'a> Modal<'a> {
    /// A modal titled `title` with `body` lines. No footer; body wrapping
    /// off by default (use [`Self::wrap`] to enable). Title is rendered
    /// centered as ` title ` in the border.
    #[must_use]
    pub fn new(theme: &Theme, title: impl Into<String>, body: Vec<Line<'a>>) -> Self {
        Self {
            theme: *theme,
            title: title.into(),
            body,
            footer: None,
            wrap: false,
        }
    }

    /// Attach a dimmed, italic footer line painted as the last body row.
    #[must_use]
    pub fn footer(mut self, footer: impl Into<String>) -> Self {
        self.footer = Some(footer.into());
        self
    }

    /// Enable word wrapping of the body (preserving leading whitespace).
    #[must_use]
    pub const fn wrap(mut self, wrap: bool) -> Self {
        self.wrap = wrap;
        self
    }

    /// Paint the modal into `buf`, filling `area` (the modal rect — the
    /// caller centers it). Border + title chrome come from the theme;
    /// body lines are painted as-is.
    pub fn render_into(&self, area: Rect, buf: &mut Buffer) {
        let block = Block::default()
            .borders(Borders::ALL)
            // Fill the box with the theme surface so the modal reads as a
            // solid panel floating over the live panes. Default `Reset`
            // inherits the terminal background (no visible change).
            .style(Style::default().bg(self.theme.surface))
            .border_style(Style::default().fg(self.theme.border))
            .title(Span::styled(
                format!(" {} ", self.title),
                Style::default()
                    .fg(self.theme.accent)
                    .add_modifier(Modifier::BOLD),
            ))
            .title_alignment(Alignment::Center);

        let mut lines = self.body.clone();
        if let Some(footer) = &self.footer {
            // Blank spacer + dimmed italic footer, matching the help
            // overlay's prior layout.
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                footer.clone(),
                Style::default()
                    .fg(self.theme.dim)
                    .add_modifier(Modifier::ITALIC),
            )));
        }

        let mut para = Paragraph::new(lines).block(block);
        if self.wrap {
            para = para.wrap(Wrap { trim: false });
        }
        para.render(area, buf);
    }
}

/// Compute a centered [`Rect`] inside `outer`.
///
/// Sized to `frac_num`/10 of the outer dimensions, clamped to at least
/// `min_w`×`min_h` (themselves clamped to the outer bounds so tiny
/// terminals still show something) and never exceeding `outer`.
#[must_use]
pub fn centered(outer: Rect, frac_num: u16, min_w: u16, min_h: u16) -> Rect {
    let w = outer.width.saturating_mul(frac_num) / 10;
    let h = outer.height.saturating_mul(frac_num) / 10;
    let w = w.clamp(min_w.min(outer.width), outer.width);
    let h = h.clamp(min_h.min(outer.height), outer.height);
    let x = outer.x + (outer.width.saturating_sub(w)) / 2;
    let y = outer.y + (outer.height.saturating_sub(h)) / 2;
    Rect::new(x, y, w, h)
}

/// One row in a [`KeyChordTable`] section: a chord (left column) and its
/// description (right column).
#[derive(Debug, Clone)]
pub struct ChordRow {
    /// The chord as the user types it, e.g. `"C-a v"`.
    pub chord: String,
    /// What it does, e.g. `"split-pane(direction=vertical)"`.
    pub description: String,
}

impl ChordRow {
    /// A row pairing `chord` with `description`.
    #[must_use]
    pub fn new(chord: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            chord: chord.into(),
            description: description.into(),
        }
    }
}

/// A titled group of [`ChordRow`]s.
#[derive(Debug, Clone)]
pub struct ChordSection {
    /// Section heading, e.g. `"Global bindings"`.
    pub title: String,
    /// Rows under this heading.
    pub rows: Vec<ChordRow>,
}

impl ChordSection {
    /// A section titled `title` with `rows`.
    #[must_use]
    pub fn new(title: impl Into<String>, rows: Vec<ChordRow>) -> Self {
        Self {
            title: title.into(),
            rows,
        }
    }
}

/// The chord/description table the help overlay shows.
///
/// Renders sections top-to-bottom (blank spacer between them), each with a
/// bold [`Theme::section_header`] heading, then its rows with the chord
/// column ([`Theme::chord`], bold) padded to align with every other
/// section's chords and the description column ([`Theme::action`]).
///
/// Build with [`KeyChordTable::new`] then turn into renderable body lines
/// with [`KeyChordTable::body_lines`] (so a caller can fold them into a
/// [`Modal`]'s body).
#[derive(Debug, Clone)]
pub struct KeyChordTable {
    theme: Theme,
    sections: Vec<ChordSection>,
    /// Shown when every section is empty (e.g. "No keybindings
    /// configured."), dimmed.
    empty_notice: Option<String>,
}

impl KeyChordTable {
    /// A table over `sections`, styled with `theme`.
    #[must_use]
    pub const fn new(theme: &Theme, sections: Vec<ChordSection>) -> Self {
        Self {
            theme: *theme,
            sections,
            empty_notice: None,
        }
    }

    /// Set the dimmed line shown when no section has any rows.
    #[must_use]
    pub fn empty_notice(mut self, notice: impl Into<String>) -> Self {
        self.empty_notice = Some(notice.into());
        self
    }

    /// Build the body lines: each non-empty section's bold header
    /// followed by its aligned rows, blank-separated. Returns the empty
    /// notice (if set) when no section has rows.
    #[must_use]
    pub fn body_lines(&self) -> Vec<Line<'static>> {
        // Chord column width = longest chord across ALL sections, so the
        // description column lines up through section boundaries. Default
        // of 8 matches the prior help-overlay behavior for the
        // no-bindings case.
        let chord_width = self
            .sections
            .iter()
            .flat_map(|s| s.rows.iter())
            .map(|r| r.chord.len())
            .max()
            .unwrap_or(8);

        let mut lines: Vec<Line<'static>> = Vec::new();
        for section in &self.sections {
            if section.rows.is_empty() {
                continue;
            }
            if !lines.is_empty() {
                lines.push(Line::from(""));
            }
            lines.push(Line::from(Span::styled(
                section.title.clone(),
                Style::default()
                    .fg(self.theme.section_header)
                    .add_modifier(Modifier::BOLD),
            )));
            for row in &section.rows {
                lines.push(self.row_line(row, chord_width));
            }
        }

        if lines.is_empty()
            && let Some(notice) = &self.empty_notice
        {
            lines.push(Line::from(Span::styled(
                notice.clone(),
                Style::default().fg(self.theme.dim),
            )));
        }
        lines
    }

    /// One table row: bold chord padded to `width`, two-space gutter, then
    /// the description.
    fn row_line(&self, row: &ChordRow, width: usize) -> Line<'static> {
        let pad = width.saturating_sub(row.chord.len());
        let padding = " ".repeat(pad);
        Line::from(vec![
            Span::styled(
                row.chord.clone(),
                Style::default()
                    .fg(self.theme.chord)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(padding),
            Span::raw("  "),
            Span::styled(
                row.description.clone(),
                Style::default().fg(self.theme.action),
            ),
        ])
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, reason = "tests")]
mod tests {
    use super::*;

    /// Flatten a rendered buffer to a `\n`-joined string with trailing
    /// spaces trimmed per row.
    fn buf_to_string(buf: &Buffer) -> String {
        let area = buf.area;
        let mut out = String::new();
        for y in 0..area.height {
            let mut row = String::new();
            for x in 0..area.width {
                row.push_str(buf[(area.x + x, area.y + y)].symbol());
            }
            out.push_str(row.trim_end());
            out.push('\n');
        }
        out
    }

    fn render_modal(modal: &Modal<'_>, w: u16, h: u16) -> String {
        let area = Rect::new(0, 0, w, h);
        let mut buf = Buffer::empty(area);
        modal.render_into(area, &mut buf);
        buf_to_string(&buf)
    }

    #[test]
    fn modal_renders_title_body_and_footer() {
        let theme = Theme::default();
        let modal = Modal::new(
            &theme,
            "demo",
            vec![Line::from("hello"), Line::from("world")],
        )
        .footer("Press Esc to close");
        let text = render_modal(&modal, 40, 10);
        assert!(text.contains("demo"), "title:\n{text}");
        assert!(text.contains("hello"), "body line 1:\n{text}");
        assert!(text.contains("world"), "body line 2:\n{text}");
        assert!(text.contains("Press Esc to close"), "footer:\n{text}");
    }

    #[test]
    fn modal_byte_output_is_stable() {
        let theme = Theme::default();
        let modal = Modal::new(&theme, "box", vec![Line::from("body")]);
        let area = Rect::new(0, 0, 16, 5);
        let mut buf = Buffer::empty(area);
        modal.render_into(area, &mut buf);
        insta::assert_snapshot!(buf_to_string(&buf));
    }

    #[test]
    fn chord_table_aligns_columns_across_sections() {
        let theme = Theme::default();
        let table = KeyChordTable::new(
            &theme,
            vec![
                ChordSection::new(
                    "Prefix bindings (C-a)",
                    vec![ChordRow::new("C-a d", "detach")],
                ),
                ChordSection::new("Global bindings", vec![ChordRow::new("F1", "show-help")]),
            ],
        );
        let modal = Modal::new(&theme, "phux help", table.body_lines());
        let text = render_modal(&modal, 60, 16);
        assert!(text.contains("Prefix bindings (C-a)"), "{text}");
        assert!(text.contains("Global bindings"), "{text}");
        assert!(text.contains("C-a d"), "{text}");
        assert!(text.contains("detach"), "{text}");
        assert!(text.contains("show-help"), "{text}");
    }

    #[test]
    fn chord_table_empty_shows_notice() {
        let theme = Theme::default();
        let table = KeyChordTable::new(&theme, vec![ChordSection::new("Empty", Vec::new())])
            .empty_notice("No keybindings configured.");
        let lines = table.body_lines();
        assert_eq!(lines.len(), 1);
    }

    #[test]
    fn chord_table_skips_empty_sections() {
        let theme = Theme::default();
        let table = KeyChordTable::new(
            &theme,
            vec![
                ChordSection::new("Has rows", vec![ChordRow::new("a", "act")]),
                ChordSection::new("Empty", Vec::new()),
            ],
        );
        let lines = table.body_lines();
        // Header + one row only; the empty section contributes nothing
        // and no trailing spacer is appended.
        assert_eq!(lines.len(), 2);
    }

    #[test]
    fn chord_table_byte_output_is_stable() {
        let theme = Theme::default();
        let table = KeyChordTable::new(
            &theme,
            vec![ChordSection::new(
                "Section",
                vec![
                    ChordRow::new("C-a d", "detach"),
                    ChordRow::new("C-a x", "kill-pane"),
                ],
            )],
        );
        let area = Rect::new(0, 0, 32, 6);
        let mut buf = Buffer::empty(area);
        // Render the lines bare (no modal chrome) so the snapshot pins the
        // table's own alignment.
        let para = Paragraph::new(table.body_lines());
        para.render(area, &mut buf);
        insta::assert_snapshot!(buf_to_string(&buf));
    }

    #[test]
    fn centered_clamps_to_outer() {
        let outer = Rect::new(0, 0, 20, 8);
        let inner = centered(outer, 7, 40, 10);
        assert!(inner.width <= outer.width);
        assert!(inner.height <= outer.height);
        assert!(inner.x + inner.width <= outer.x + outer.width);
        assert!(inner.y + inner.height <= outer.y + outer.height);
    }

    /// phux-foz.14: when the outer rect is the pane content rect (viewport
    /// inset by a left sidebar strip), the centered modal must stay fully
    /// inside it — its left edge lands right of the sidebar divider, never on
    /// the strip columns. This is the exact geometry the floating-modal path
    /// now feeds `centered`.
    #[test]
    fn centered_against_inset_rect_clears_the_sidebar() {
        // 80-col viewport, a 20-col left sidebar ⇒ content rect x∈[20, 80).
        let sidebar_w = 20;
        let content = Rect::new(sidebar_w, 0, 80 - sidebar_w, 24);
        let modal = centered(content, 6, 30, 10);
        // Fully within the content rect on every edge.
        assert!(
            modal.x >= content.x,
            "modal left edge {} must not enter the sidebar (divider at {})",
            modal.x,
            content.x
        );
        assert!(modal.x + modal.width <= content.x + content.width);
        assert!(modal.y >= content.y);
        assert!(modal.y + modal.height <= content.y + content.height);
        // And horizontally centered *within the content rect*, not the raw
        // viewport: the left and right margins inside the content match.
        let left_margin = modal.x - content.x;
        let right_margin = (content.x + content.width) - (modal.x + modal.width);
        assert!(
            left_margin.abs_diff(right_margin) <= 1,
            "modal must be centered in the content rect: L={left_margin} R={right_margin}"
        );
    }
}
