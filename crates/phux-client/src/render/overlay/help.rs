//! Help overlay (phux-5ke.4).
//!
//! Renders current keybindings as a centered modal. Dismisses on Esc
//! (or any key already bound to `show-help`, since "pressing the help
//! binding while help is up" is the universal "close it" gesture).
//!
//! Bindings are snapshotted at construction time — the overlay does not
//! re-read config while it's up. If the user reloads config while help
//! is open, they'll see the stale view; dismissing and re-opening picks
//! up the new bindings. This avoids the overlay holding any reference
//! into the live config, which keeps `Box<dyn RenderOverlay>` `'static`.

use phux_config::{Action, KeybindingsCfg};
use phux_protocol::input::key::{KeyEvent, PhysicalKey};
use ratatui::buffer::Buffer;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Widget, Wrap};

use super::{OverlayCommand, RenderOverlay};

/// One row in the help table: a chord (or chord sequence) and the
/// action it resolves to. `chord_text` already includes the prefix
/// (`"C-a v"`) for prefix-table entries so the user sees the literal
/// keystrokes they need to type.
#[derive(Debug, Clone)]
struct Entry {
    chord: String,
    action: String,
}

/// Help overlay — keybindings reference modal.
///
/// Built from a [`KeybindingsCfg`] snapshot via [`Self::from_config`].
/// Rendering is a single centered [`Paragraph`] inside a bordered
/// [`Block`]; bindings are grouped into prefix-table vs global vs
/// hardcoded sections.
#[derive(Debug)]
pub struct HelpOverlay {
    /// Pretty-printed prefix chord (e.g. `"C-a"`), or empty if no
    /// prefix-table entries exist.
    prefix: String,
    /// Prefix-table entries — chord shown is `"<prefix> <key>"`.
    prefix_entries: Vec<Entry>,
    /// Global entries — chord shown as-is.
    global_entries: Vec<Entry>,
    /// Hardcoded driver-owned chords (detach, etc.). Authored at
    /// construction time so the overlay stays `'static`.
    hardcoded_entries: Vec<Entry>,
    /// Chord (as authored in cfg) that opens this overlay. Used in
    /// the footer hint so a user who rebound `show-help` to e.g. `?`
    /// sees `Press ? or Esc to close` rather than a stale `F1`.
    /// `None` when no global binding maps to `show-help`.
    show_help_chord: Option<String>,
}

impl HelpOverlay {
    /// Build the overlay from a config snapshot. Cheap; clones small
    /// strings. The overlay owns its entries — the source `cfg` may be
    /// dropped immediately after construction.
    #[must_use]
    pub fn from_config(cfg: &KeybindingsCfg) -> Self {
        let prefix_entries: Vec<Entry> = cfg
            .prefix_table
            .iter()
            .map(|(chord, action)| Entry {
                chord: format!("{} {chord}", cfg.prefix),
                action: action_label(action),
            })
            .collect();
        let global_entries: Vec<Entry> = cfg
            .global
            .iter()
            .map(|(chord, action)| Entry {
                chord: chord.clone(),
                action: action_label(action),
            })
            .collect();
        let hardcoded_entries = Vec::new();
        // Find the chord (if any) that the user bound to `show-help`
        // for the footer hint. Scans `cfg.global` only — `show-help`
        // is a global by default and surfacing a prefix-table entry
        // would lie about how the dismiss path works (the prefix is
        // captured by the resolver, but while the overlay is active
        // the resolver is bypassed — only the overlay's own
        // `handle_key` runs).
        let show_help_chord = cfg
            .global
            .iter()
            .find(|(_, action)| is_show_help(action))
            .map(|(chord, _)| chord.clone());
        Self {
            prefix: if prefix_entries.is_empty() {
                String::new()
            } else {
                cfg.prefix.clone()
            },
            prefix_entries,
            global_entries,
            hardcoded_entries,
            show_help_chord,
        }
    }

    /// Compute a centered Rect inside `outer`. ~70% width and ~70%
    /// height, clamped to a sensible min (40x10) so tiny terminals still
    /// show something, and clamped to `outer` so we never overflow.
    fn centered(outer: Rect) -> Rect {
        let w = outer.width.saturating_mul(7) / 10;
        let h = outer.height.saturating_mul(7) / 10;
        let w = w.clamp(40.min(outer.width), outer.width);
        let h = h.clamp(10.min(outer.height), outer.height);
        let x = outer.x + (outer.width.saturating_sub(w)) / 2;
        let y = outer.y + (outer.height.saturating_sub(h)) / 2;
        Rect::new(x, y, w, h)
    }

    /// Build the body lines: section headers + chord→action rows.
    fn body_lines(&self) -> Vec<Line<'_>> {
        let mut lines: Vec<Line<'_>> = Vec::new();
        // Chord column is widened to the longest chord across ALL
        // sections so the action column lines up vertically through
        // section boundaries (prefix → global → hardcoded). Keeps
        // long chords like `Ctrl-b d` from squashing the prefix
        // section.
        let chord_width = self
            .prefix_entries
            .iter()
            .chain(self.global_entries.iter())
            .chain(self.hardcoded_entries.iter())
            .map(|e| e.chord.len())
            .max()
            .unwrap_or(8);
        if !self.prefix_entries.is_empty() {
            lines.push(Line::from(Span::styled(
                format!("Prefix bindings ({})", self.prefix),
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )));
            for e in &self.prefix_entries {
                lines.push(entry_line(e, chord_width));
            }
        }
        if !self.global_entries.is_empty() {
            if !lines.is_empty() {
                lines.push(Line::from(""));
            }
            lines.push(Line::from(Span::styled(
                "Global bindings",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )));
            for e in &self.global_entries {
                lines.push(entry_line(e, chord_width));
            }
        }
        if !self.hardcoded_entries.is_empty() {
            if !lines.is_empty() {
                lines.push(Line::from(""));
            }
            lines.push(Line::from(Span::styled(
                "Hardcoded",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )));
            for e in &self.hardcoded_entries {
                lines.push(entry_line(e, chord_width));
            }
        }
        if lines.is_empty() {
            lines.push(Line::from(Span::styled(
                "No keybindings configured.",
                Style::default().fg(Color::DarkGray),
            )));
        }
        lines.push(Line::from(""));
        let footer = self.show_help_chord.as_deref().map_or_else(
            || "Press Esc to close".to_owned(),
            |chord| format!("Press {chord} or Esc to close"),
        );
        lines.push(Line::from(Span::styled(
            footer,
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::ITALIC),
        )));
        lines
    }
}

impl RenderOverlay for HelpOverlay {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        let modal = Self::centered(area);
        let block = Block::default()
            .borders(Borders::ALL)
            .title(Span::styled(
                " phux help ",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ))
            .title_alignment(Alignment::Center);
        let lines = self.body_lines();
        let para = Paragraph::new(lines)
            .block(block)
            .wrap(Wrap { trim: false });
        para.render(modal, buf);
    }

    fn handle_key(&mut self, key: &KeyEvent) -> OverlayCommand {
        // Esc → dismiss. F1 → dismiss (matches the default global binding
        // to open it; pressing the same key again is the universal "toggle"
        // even when the overlay only models "show" + "dismiss"). Anything
        // else is absorbed.
        match key.key {
            PhysicalKey::Escape | PhysicalKey::F1 => OverlayCommand::Dismiss,
            _ => OverlayCommand::Stay,
        }
    }
}

/// One row in the help body — chord left-aligned, padded to `width`,
/// then a separator and the action name.
fn entry_line(e: &Entry, width: usize) -> Line<'_> {
    let pad = width.saturating_sub(e.chord.len());
    let padding = " ".repeat(pad);
    Line::from(vec![
        Span::styled(
            e.chord.clone(),
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(padding),
        Span::raw("  "),
        Span::raw(e.action.clone()),
    ])
}

/// Human-readable label for an [`Action`]. Bare actions show the name;
/// parameterized actions show `name(key=value, ...)` so the user sees
/// the args their binding actually carries (e.g. `split-pane(direction=vertical)`).
fn action_label(action: &Action) -> String {
    match action {
        Action::Bare(name) => name.clone(),
        Action::Parameterized(p) => {
            if p.args.is_empty() {
                p.action.clone()
            } else {
                let args: Vec<String> = p
                    .args
                    .iter()
                    .map(|(k, v)| format!("{k}={}", value_label(v)))
                    .collect();
                format!("{}({})", p.action, args.join(", "))
            }
        }
    }
}

fn value_label(v: &toml::Value) -> String {
    match v {
        toml::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// `true` when `action` resolves to `show-help` (bare or parameterized).
/// Used to find the user-bound chord for the dismiss footer hint.
fn is_show_help(action: &Action) -> bool {
    match action {
        Action::Bare(name) => name == "show-help",
        Action::Parameterized(p) => p.action == "show-help",
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, reason = "tests")]
mod tests {
    use super::*;
    use phux_config::ParamAction;
    use phux_protocol::input::key::{KeyAction, ModSet};
    use std::collections::BTreeMap;

    fn cfg() -> KeybindingsCfg {
        let mut prefix_table = BTreeMap::new();
        prefix_table.insert("d".to_owned(), Action::Bare("detach".to_owned()));
        prefix_table.insert("x".to_owned(), Action::Bare("kill-pane".to_owned()));
        let mut split_args = BTreeMap::new();
        split_args.insert(
            "direction".to_owned(),
            toml::Value::String("vertical".to_owned()),
        );
        prefix_table.insert(
            "v".to_owned(),
            Action::Parameterized(ParamAction {
                action: "split-pane".to_owned(),
                args: split_args,
            }),
        );
        let mut global = BTreeMap::new();
        global.insert("F1".to_owned(), Action::Bare("show-help".to_owned()));
        KeybindingsCfg {
            prefix: "C-a".to_owned(),
            prefix_table,
            global,
        }
    }

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

    #[test]
    fn from_config_collects_prefix_and_global() {
        let overlay = HelpOverlay::from_config(&cfg());
        assert_eq!(overlay.prefix, "C-a");
        assert_eq!(overlay.prefix_entries.len(), 3);
        assert_eq!(overlay.global_entries.len(), 1);
        assert!(overlay.hardcoded_entries.is_empty());
        assert!(overlay.prefix_entries.iter().any(|e| e.action == "detach"));
        // show-help is bound to F1 in the test cfg.
        assert_eq!(overlay.show_help_chord.as_deref(), Some("F1"));
    }

    #[test]
    fn no_show_help_binding_falls_back_to_esc_only_footer() {
        let mut c = cfg();
        c.global.clear();
        let overlay = HelpOverlay::from_config(&c);
        assert_eq!(overlay.show_help_chord, None);
        let text = render_to_string(&overlay, 80, 24);
        assert!(text.contains("Press Esc to close"), "text was:\n{text}");
    }

    #[test]
    fn punctuation_chords_render_verbatim() {
        // phux-9fu shipped `|` and `-` as default prefix-table keys.
        // They must render as the literal glyph the user typed in
        // TOML, not as escape sequences or raw bytes.
        let mut c = cfg();
        c.prefix_table
            .insert("|".to_owned(), Action::Bare("split-pane".to_owned()));
        c.prefix_table
            .insert("-".to_owned(), Action::Bare("split-pane".to_owned()));
        let overlay = HelpOverlay::from_config(&c);
        let text = render_to_string(&overlay, 80, 24);
        assert!(text.contains("C-a |"), "missing `C-a |` row:\n{text}");
        assert!(text.contains("C-a -"), "missing `C-a -` row:\n{text}");
    }

    #[test]
    fn esc_dismisses() {
        let mut overlay = HelpOverlay::from_config(&cfg());
        assert_eq!(
            overlay.handle_key(&key(PhysicalKey::Escape)),
            OverlayCommand::Dismiss
        );
    }

    #[test]
    fn f1_dismisses() {
        let mut overlay = HelpOverlay::from_config(&cfg());
        assert_eq!(
            overlay.handle_key(&key(PhysicalKey::F1)),
            OverlayCommand::Dismiss
        );
    }

    #[test]
    fn other_keys_stay() {
        let mut overlay = HelpOverlay::from_config(&cfg());
        assert_eq!(
            overlay.handle_key(&key(PhysicalKey::A)),
            OverlayCommand::Stay
        );
    }

    #[test]
    fn parameterized_action_label_shows_args() {
        let mut args = BTreeMap::new();
        args.insert(
            "direction".to_owned(),
            toml::Value::String("vertical".to_owned()),
        );
        let a = Action::Parameterized(ParamAction {
            action: "split-pane".to_owned(),
            args,
        });
        assert_eq!(action_label(&a), "split-pane(direction=vertical)");
    }

    #[test]
    fn bare_action_label_is_just_the_name() {
        assert_eq!(
            action_label(&Action::Bare("kill-pane".to_owned())),
            "kill-pane"
        );
    }

    /// Render `overlay` into a fresh `width × height` buffer and
    /// flatten to a `\n`-separated string with trailing spaces
    /// trimmed per row. Trimming keeps assertions terse but does
    /// not change the visible cell content.
    fn render_to_string(overlay: &HelpOverlay, width: u16, height: u16) -> String {
        let area = Rect::new(0, 0, width, height);
        let mut buf = Buffer::empty(area);
        overlay.render(area, &mut buf);
        let mut out = String::new();
        for y in 0..area.height {
            let mut row = String::new();
            for x in 0..area.width {
                row.push_str(buf[(x, y)].symbol());
            }
            out.push_str(row.trim_end());
            out.push('\n');
        }
        out
    }

    #[test]
    fn render_into_buffer_does_not_panic() {
        let overlay = HelpOverlay::from_config(&cfg());
        let text = render_to_string(&overlay, 80, 24);
        assert!(
            text.contains("phux help"),
            "expected 'phux help' title in rendered buffer:\n{text}"
        );
    }

    #[test]
    fn render_surfaces_all_three_sections() {
        // Plain-string assertion (not `insta`) — keeps the test
        // robust against width-driven padding while still proving
        // that every section the overlay promises is actually
        // painted. Covers the regression that motivated phux-i08:
        // configured prefix/global chords were missing from the overlay.
        let overlay = HelpOverlay::from_config(&cfg());
        let text = render_to_string(&overlay, 80, 24);
        // Section headers.
        assert!(
            text.contains("Prefix bindings (C-a)"),
            "missing prefix header:\n{text}"
        );
        assert!(
            text.contains("Global bindings"),
            "missing global header:\n{text}"
        );
        // At least one row from each section.
        assert!(text.contains("C-a d"), "missing detach row:\n{text}");
        assert!(text.contains("C-a x"), "missing prefix-table row:\n{text}");
        assert!(
            text.contains("C-a v"),
            "missing parameterized prefix row:\n{text}"
        );
        assert!(text.contains("F1"), "missing global row:\n{text}");
        // Footer reflects the bound chord, not a hardcoded "F1".
        assert!(
            text.contains("Press F1 or Esc to close"),
            "missing dynamic footer:\n{text}"
        );
    }

    #[test]
    fn centered_clamps_to_outer() {
        let outer = Rect::new(0, 0, 20, 8);
        let inner = HelpOverlay::centered(outer);
        assert!(inner.width <= outer.width);
        assert!(inner.height <= outer.height);
        assert!(inner.x + inner.width <= outer.x + outer.width);
        assert!(inner.y + inner.height <= outer.y + outer.height);
    }
}
