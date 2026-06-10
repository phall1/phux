//! Reusable selectable-list overlay (phux-ahv.8 / phux-4li.19).
//!
//! A themed [`Modal`] wrapping a one-line query input and a filtered,
//! scrollable list of items. Each item carries a display label, an
//! optional right-aligned secondary label, and the
//! [`ResolvedAction`] it commits on Enter. This is the shared primitive
//! behind both the command palette and the `<leader> w` window picker:
//! both populate a [`SelectList`] from different sources and let the
//! single `run_action()` dispatch path execute the committed action.
//!
//! ## Input model
//!
//! - Up / `C-p` and Down / `C-n` move the selection (also `j` / `k` when
//!   the query is empty, so a fresh palette is vi-navigable; once the
//!   user starts typing, `j`/`k` are treated as filter text).
//! - Printable text appends to the query and re-filters.
//! - Backspace edits the query.
//! - Enter commits the selected item's [`ResolvedAction`]
//!   ([`OverlayCommand::Commit`]); on an empty filtered list it is a
//!   no-op.
//! - Esc dismisses ([`OverlayCommand::Dismiss`]).
//!
//! ## Filtering
//!
//! Filtering is a subsequence (fuzzy) match of the lowercased query
//! against each item's `filter_text` (label + secondary). An empty query
//! matches everything. See [`fuzzy_match`].

use phux_config::keybind::ResolvedAction;
use phux_protocol::input::key::{KeyAction, KeyEvent, ModSet, PhysicalKey};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use super::widgets::{Modal, centered};
use super::{OverlayCommand, RenderOverlay};
use crate::render::Theme;

/// One selectable row in a [`SelectList`].
#[derive(Debug, Clone)]
pub struct SelectItem {
    /// Primary display label (left column), e.g. an action name or a
    /// window's `index:name`.
    pub label: String,
    /// Optional right-aligned secondary label, e.g. a bound chord or a
    /// pane count. Dimmed when present.
    pub secondary: Option<String>,
    /// The action committed when this item is chosen. Flows straight
    /// into the dispatcher's `run_action()` — the same path a keybind
    /// takes.
    pub action: ResolvedAction,
}

impl SelectItem {
    /// An item displaying `label` that commits `action`, with no
    /// secondary label.
    #[must_use]
    pub fn new(label: impl Into<String>, action: ResolvedAction) -> Self {
        Self {
            label: label.into(),
            secondary: None,
            action,
        }
    }

    /// Attach a right-aligned, dimmed secondary label.
    #[must_use]
    pub fn secondary(mut self, secondary: impl Into<String>) -> Self {
        self.secondary = Some(secondary.into());
        self
    }

    /// The text the query is fuzzy-matched against: label plus secondary
    /// (so a user can filter the palette by a binding chord too).
    fn filter_text(&self) -> String {
        self.secondary
            .as_ref()
            .map_or_else(|| self.label.clone(), |sec| format!("{} {sec}", self.label))
    }
}

/// A themed, filterable, selectable list rendered as an overlay.
///
/// Build with [`SelectList::new`], then push the boxed value onto the
/// [`OverlayState`](super::OverlayState) stack. Generic only over the
/// supplied [`SelectItem`]s — the palette and window picker differ only
/// in how they build that vector.
#[derive(Debug, Clone)]
pub struct SelectList {
    /// Modal title (e.g. `"command palette"`).
    title: String,
    /// All items, unfiltered. Filtering is recomputed from `query` on
    /// every keystroke rather than cached, since the lists are short.
    items: Vec<SelectItem>,
    /// Current query text.
    query: String,
    /// Selection index into the *filtered* list. Clamped on every filter
    /// change so it never points past the visible rows.
    selected: usize,
    /// Color slots snapshotted from the active [`Theme`] at construction
    /// (captured, not borrowed, so the overlay stays `'static`).
    theme: Theme,
}

impl SelectList {
    /// A list titled `title` over `items`, styled with `theme`. The
    /// selection starts on the first item; the query starts empty (all
    /// items visible).
    #[must_use]
    pub fn new(title: impl Into<String>, items: Vec<SelectItem>, theme: &Theme) -> Self {
        Self {
            title: title.into(),
            items,
            query: String::new(),
            selected: 0,
            theme: *theme,
        }
    }

    /// Indices of items matching the current query, in original order.
    fn filtered_indices(&self) -> Vec<usize> {
        let q = self.query.to_lowercase();
        self.items
            .iter()
            .enumerate()
            .filter(|(_, item)| fuzzy_match(&q, &item.filter_text().to_lowercase()))
            .map(|(i, _)| i)
            .collect()
    }

    /// Clamp `selected` so it always points at a visible row (or 0 when
    /// nothing matches).
    const fn clamp_selection(&mut self, visible: usize) {
        if visible == 0 {
            self.selected = 0;
        } else if self.selected >= visible {
            self.selected = visible - 1;
        }
    }

    /// Move the selection one row down within `visible` rows, saturating
    /// at the bottom (no wrap — matches the prompt overlay's restraint).
    const fn select_down(&mut self, visible: usize) {
        if visible != 0 && self.selected + 1 < visible {
            self.selected += 1;
        }
    }

    /// Move the selection one row up, saturating at the top.
    const fn select_up(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }

    /// The modal rect: 60% of the viewport, min 30x10, clamped to the
    /// outer rect (like the help overlay, but a touch narrower).
    fn modal_area(outer: Rect) -> Rect {
        centered(outer, 6, 30, 10)
    }

    /// Build the body lines: a query line, a separator, then the filtered
    /// rows (selected row reverse-video). A dimmed notice replaces the
    /// rows when nothing matches.
    fn body_lines(&self, indices: &[usize], inner_width: u16) -> Vec<Line<'static>> {
        let mut lines: Vec<Line<'static>> = Vec::new();
        // Query line: a `> ` prompt, the text, and a reverse-video caret.
        lines.push(Line::from(vec![
            Span::styled("> ".to_owned(), Style::default().fg(self.theme.accent)),
            Span::raw(self.query.clone()),
            Span::styled(" ", Style::default().add_modifier(Modifier::REVERSED)),
        ]));
        lines.push(Line::from(""));

        if indices.is_empty() {
            lines.push(Line::from(Span::styled(
                "(no matches)".to_owned(),
                Style::default().fg(self.theme.dim),
            )));
            return lines;
        }

        for (row, &idx) in indices.iter().enumerate() {
            let item = &self.items[idx];
            let selected = row == self.selected;
            lines.push(self.item_line(item, selected, inner_width));
        }
        lines
    }

    /// One list row: label on the left, optional dimmed secondary
    /// right-aligned within `inner_width`. The selected row is rendered
    /// reverse-video across its visible width.
    fn item_line(&self, item: &SelectItem, selected: bool, inner_width: u16) -> Line<'static> {
        let width = inner_width as usize;
        let label = item.label.clone();
        let secondary = item.secondary.clone().unwrap_or_default();
        // Lay out `label .... secondary` within `width`. The gap is at
        // least one space; secondary is truncated implicitly by the
        // terminal if the row is too narrow.
        let used = label.chars().count() + secondary.chars().count();
        let gap = width.saturating_sub(used).max(1);
        let padding = " ".repeat(gap);

        if selected {
            // Reverse-video the whole row so the selection reads clearly
            // regardless of theme. Secondary stays in the same run.
            let text = format!("{label}{padding}{secondary}");
            Line::from(Span::styled(
                text,
                Style::default().add_modifier(Modifier::REVERSED),
            ))
        } else {
            Line::from(vec![
                Span::styled(label, Style::default().fg(self.theme.action)),
                Span::raw(padding),
                Span::styled(secondary, Style::default().fg(self.theme.dim)),
            ])
        }
    }
}

impl RenderOverlay for SelectList {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        let modal_area = Self::modal_area(area);
        let indices = self.filtered_indices();
        // Body width is the modal interior minus the 1-cell border on
        // each side.
        let inner_width = modal_area.width.saturating_sub(2);
        let body = self.body_lines(&indices, inner_width);
        Modal::new(&self.theme, self.title.clone(), body)
            .footer("Enter select  ·  Esc cancel  ·  type to filter")
            .render_into(modal_area, buf);
    }

    fn bounds(&self, area: Rect) -> Option<Rect> {
        Some(Self::modal_area(area))
    }

    fn handle_key(&mut self, key: &KeyEvent) -> OverlayCommand {
        if key.action != KeyAction::Press {
            return OverlayCommand::Stay;
        }
        let indices = self.filtered_indices();
        let visible = indices.len();
        self.clamp_selection(visible);

        // Ctrl-modified navigation works regardless of query content.
        if key.mods.contains(ModSet::CTRL) {
            match key.key {
                PhysicalKey::N => {
                    self.select_down(visible);
                    return OverlayCommand::Stay;
                }
                PhysicalKey::P => {
                    self.select_up();
                    return OverlayCommand::Stay;
                }
                _ => {}
            }
        }

        match key.key {
            PhysicalKey::Escape => OverlayCommand::Dismiss,
            PhysicalKey::Enter => indices
                .get(self.selected)
                .map_or(OverlayCommand::Stay, |&idx| {
                    OverlayCommand::Commit(self.items[idx].action.clone())
                }),
            PhysicalKey::ArrowDown => {
                self.select_down(visible);
                OverlayCommand::Stay
            }
            PhysicalKey::ArrowUp => {
                self.select_up();
                OverlayCommand::Stay
            }
            PhysicalKey::Backspace => {
                self.query.pop();
                let visible = self.filtered_indices().len();
                self.clamp_selection(visible);
                OverlayCommand::Stay
            }
            // `j`/`k` navigate only while the query is empty (so a fresh
            // list is vi-navigable); once the user types, they're filter
            // text like any other letter.
            PhysicalKey::J if self.query.is_empty() => {
                self.select_down(visible);
                OverlayCommand::Stay
            }
            PhysicalKey::K if self.query.is_empty() => {
                self.select_up();
                OverlayCommand::Stay
            }
            _ => {
                if let Some(t) = &key.text
                    && !t.chars().any(char::is_control)
                {
                    self.query.push_str(t);
                    let visible = self.filtered_indices().len();
                    self.clamp_selection(visible);
                }
                OverlayCommand::Stay
            }
        }
    }
}

/// Subsequence (fuzzy) match.
///
/// Every char of `needle` appears in `haystack` in order (not necessarily
/// contiguously). An empty needle matches everything. Both arguments are
/// expected lowercased by the caller.
#[must_use]
pub fn fuzzy_match(needle: &str, haystack: &str) -> bool {
    let mut hay = haystack.chars();
    'outer: for nc in needle.chars() {
        for hc in hay.by_ref() {
            if hc == nc {
                continue 'outer;
            }
        }
        return false;
    }
    true
}

#[cfg(test)]
#[allow(clippy::expect_used, reason = "tests")]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn action(name: &str) -> ResolvedAction {
        ResolvedAction {
            action: name.to_owned(),
            args: BTreeMap::new(),
        }
    }

    fn press(key: PhysicalKey, text: Option<&str>) -> KeyEvent {
        KeyEvent {
            action: KeyAction::Press,
            key,
            mods: ModSet::empty(),
            consumed_mods: ModSet::empty(),
            composing: false,
            text: text.map(ToOwned::to_owned),
            unshifted_codepoint: None,
        }
    }

    fn ctrl(key: PhysicalKey) -> KeyEvent {
        let mut ev = press(key, None);
        ev.mods = ModSet::CTRL;
        ev
    }

    fn sample() -> SelectList {
        let items = vec![
            SelectItem::new("split-pane", action("split-pane")).secondary("C-a |"),
            SelectItem::new("new-window", action("new-window")).secondary("C-a c"),
            SelectItem::new("detach", action("detach")).secondary("C-a d"),
        ];
        SelectList::new("command palette", items, &Theme::default())
    }

    #[test]
    fn fuzzy_match_subsequence() {
        assert!(fuzzy_match("", "anything"));
        assert!(fuzzy_match("sp", "split-pane"));
        assert!(fuzzy_match("spn", "split-pane")); // s-p-(a)n
        assert!(fuzzy_match("nw", "new-window"));
        assert!(!fuzzy_match("zzz", "new-window"));
        // Order matters: "wen" can't be a subsequence — there is no 'e'
        // after the first 'w' in "new-window".
        assert!(!fuzzy_match("wen", "new-window"));
    }

    #[test]
    fn typing_narrows_the_list() {
        let mut sl = sample();
        assert_eq!(sl.filtered_indices().len(), 3);
        sl.handle_key(&press(PhysicalKey::N, Some("n")));
        sl.handle_key(&press(PhysicalKey::W, Some("w")));
        // "nw" subsequence matches only "new-window".
        let idx = sl.filtered_indices();
        assert_eq!(idx.len(), 1);
        assert_eq!(sl.items[idx[0]].label, "new-window");
    }

    #[test]
    fn filter_can_match_secondary_chord() {
        let mut sl = sample();
        // The detach chord is "C-a d"; querying "det" via the label is the
        // simple case, but the filter text includes the secondary too.
        for ch in ['d', 'e', 't'] {
            sl.handle_key(&press(PhysicalKey::A, Some(&ch.to_string())));
        }
        let idx = sl.filtered_indices();
        assert_eq!(idx.len(), 1);
        assert_eq!(sl.items[idx[0]].label, "detach");
    }

    #[test]
    fn enter_commits_selected_action() {
        let mut sl = sample();
        // Move down once → "new-window".
        sl.handle_key(&press(PhysicalKey::ArrowDown, None));
        let cmd = sl.handle_key(&press(PhysicalKey::Enter, None));
        let OverlayCommand::Commit(a) = cmd else {
            panic!("expected Commit, got {cmd:?}");
        };
        assert_eq!(a.action, "new-window");
    }

    #[test]
    fn enter_on_empty_filter_is_noop() {
        let mut sl = sample();
        for ch in ['z', 'z', 'z'] {
            sl.handle_key(&press(PhysicalKey::A, Some(&ch.to_string())));
        }
        assert_eq!(sl.filtered_indices().len(), 0);
        assert_eq!(
            sl.handle_key(&press(PhysicalKey::Enter, None)),
            OverlayCommand::Stay,
            "Enter with no matches should not commit"
        );
    }

    #[test]
    fn esc_dismisses() {
        let mut sl = sample();
        assert_eq!(
            sl.handle_key(&press(PhysicalKey::Escape, None)),
            OverlayCommand::Dismiss
        );
    }

    #[test]
    fn ctrl_n_p_navigate() {
        let mut sl = sample();
        sl.handle_key(&ctrl(PhysicalKey::N));
        sl.handle_key(&ctrl(PhysicalKey::N));
        // Two downs → third item "detach". Saturates (no wrap).
        sl.handle_key(&ctrl(PhysicalKey::N));
        let cmd = sl.handle_key(&press(PhysicalKey::Enter, None));
        let OverlayCommand::Commit(a) = cmd else {
            panic!("expected Commit");
        };
        assert_eq!(a.action, "detach");
        sl.handle_key(&ctrl(PhysicalKey::P));
        let cmd = sl.handle_key(&press(PhysicalKey::Enter, None));
        let OverlayCommand::Commit(a) = cmd else {
            panic!("expected Commit");
        };
        assert_eq!(a.action, "new-window");
    }

    #[test]
    fn jk_navigate_only_when_query_empty() {
        let mut sl = sample();
        // j with empty query moves down.
        sl.handle_key(&press(PhysicalKey::J, Some("j")));
        let cmd = sl.handle_key(&press(PhysicalKey::Enter, None));
        let OverlayCommand::Commit(a) = cmd else {
            panic!("expected Commit");
        };
        assert_eq!(a.action, "new-window");
    }

    #[test]
    fn j_becomes_filter_text_once_query_nonempty() {
        let mut sl = sample();
        // Type "d" (query now non-empty), then "j" should be filter text,
        // not navigation. "dj" matches nothing.
        sl.handle_key(&press(PhysicalKey::A, Some("d")));
        sl.handle_key(&press(PhysicalKey::J, Some("j")));
        assert_eq!(sl.query, "dj");
        assert_eq!(sl.filtered_indices().len(), 0);
    }

    #[test]
    fn render_byte_output_is_stable() {
        // Pin the painted layout (query line, separator, rows with the
        // first selected reverse-video) so accidental layout churn is
        // caught. Fixed small viewport for a compact snapshot.
        let sl = sample();
        let area = Rect::new(0, 0, 44, 12);
        let mut buf = Buffer::empty(area);
        sl.render(area, &mut buf);
        let mut out = String::new();
        for y in 0..area.height {
            let mut row = String::new();
            for x in 0..area.width {
                row.push_str(buf[(x, y)].symbol());
            }
            out.push_str(row.trim_end());
            out.push('\n');
        }
        insta::assert_snapshot!(out);
    }

    #[test]
    fn render_does_not_panic_and_shows_title() {
        let sl = sample();
        let area = Rect::new(0, 0, 80, 24);
        let mut buf = Buffer::empty(area);
        sl.render(area, &mut buf);
        let mut text = String::new();
        for y in 0..area.height {
            for x in 0..area.width {
                text.push_str(buf[(x, y)].symbol());
            }
            text.push('\n');
        }
        assert!(text.contains("command palette"), "{text}");
        assert!(text.contains("split-pane"), "{text}");
    }
}
