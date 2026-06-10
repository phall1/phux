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

use phux_config::keybind::chord_str_matches_event;
use phux_config::{Action, KeybindingsCfg};
use phux_protocol::input::key::{KeyEvent, PhysicalKey};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use super::widgets::{ChordRow, ChordSection, KeyChordTable, Modal, centered};
use super::{OverlayCommand, RenderOverlay};
use crate::render::Theme;

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
/// Rendering composes a [`KeyChordTable`] (sections grouped prefix-table
/// → global → hardcoded) into a centered [`Modal`].
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
    /// Color slots snapshotted from the active [`Theme`] at construction.
    /// Captured (not borrowed) so the overlay stays `'static`.
    theme: Theme,
}

impl HelpOverlay {
    /// Build the overlay from a config snapshot, styled with `theme`.
    /// Cheap; clones small strings and copies the `Theme`. The overlay
    /// owns its entries and colors — both `cfg` and `theme` may be
    /// dropped immediately after construction.
    #[must_use]
    pub fn from_config(cfg: &KeybindingsCfg, theme: &Theme) -> Self {
        // The numeric window-jump keys (0-9, each `select-window {index}`)
        // would otherwise be ten near-identical rows. Collect them aside
        // and render a single `C-a 0-9  select window by number` line so
        // the list stays scannable and the capability reads clearly.
        let mut prefix_entries: Vec<Entry> = Vec::new();
        let mut window_jump_keys: Vec<String> = Vec::new();
        for (chord, action) in &cfg.prefix_table {
            if is_indexed_select_window(action) {
                window_jump_keys.push(chord.clone());
            } else {
                prefix_entries.push(Entry {
                    chord: format!("{} {chord}", cfg.prefix),
                    action: action_label(action),
                });
            }
        }
        if let Some(entry) = compact_window_jump(&cfg.prefix, &window_jump_keys) {
            prefix_entries.push(entry);
        }
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
            theme: *theme,
        }
    }

    /// Group the snapshotted entries into the [`KeyChordTable`]'s
    /// sections (prefix → global → hardcoded). Empty sections are dropped
    /// by the table itself.
    fn chord_table(&self) -> KeyChordTable {
        let to_rows = |entries: &[Entry]| {
            entries
                .iter()
                .map(|e| ChordRow::new(e.chord.clone(), e.action.clone()))
                .collect::<Vec<_>>()
        };
        let sections = vec![
            ChordSection::new(
                format!("Prefix bindings ({})", self.prefix),
                to_rows(&self.prefix_entries),
            ),
            ChordSection::new("Global bindings", to_rows(&self.global_entries)),
            ChordSection::new("Hardcoded", to_rows(&self.hardcoded_entries)),
        ];
        KeyChordTable::new(&self.theme, sections).empty_notice("No keybindings configured.")
    }

    /// The dismiss-hint footer, reflecting the user-bound `show-help`
    /// chord when present.
    fn footer(&self) -> String {
        self.show_help_chord.as_deref().map_or_else(
            || "Press Esc to close".to_owned(),
            |chord| format!("Press {chord} or Esc to close"),
        )
    }
}

impl RenderOverlay for HelpOverlay {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        let modal_area = self.bounds(area).unwrap_or(area);
        let body = self.chord_table().body_lines();
        Modal::new(&self.theme, "phux help", body)
            .footer(self.footer())
            .wrap(true)
            .render_into(modal_area, buf);
    }

    fn bounds(&self, area: Rect) -> Option<Rect> {
        // ~70% of the viewport, min 40x10, clamped to the outer rect.
        Some(centered(area, 7, 40, 10))
    }

    fn handle_key(&mut self, key: &KeyEvent) -> OverlayCommand {
        // Esc always dismisses. So does the chord the user actually bound
        // to `show-help` — pressing the open-help key again is the
        // universal toggle, even when the overlay only models "show" +
        // "dismiss". We match the *configured* chord (via
        // `chord_str_matches_event`) rather than a hardcoded F1, so a user
        // who rebound `show-help` to e.g. `?` closes it with `?`
        // (phux-ahv.7). When no `show-help` binding exists, only Esc
        // closes it.
        if key.key == PhysicalKey::Escape {
            return OverlayCommand::Dismiss;
        }
        if let Some(chord) = &self.show_help_chord
            && chord_str_matches_event(chord, key)
        {
            return OverlayCommand::Dismiss;
        }
        OverlayCommand::Stay
    }
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

/// `true` for a `select-window` binding carrying an explicit `index` —
/// the numeric 0-9 window-jump keys, which the overlay collapses into one
/// row rather than listing individually.
fn is_indexed_select_window(action: &Action) -> bool {
    matches!(
        action,
        Action::Parameterized(p) if p.action == "select-window" && p.args.contains_key("index")
    )
}

/// Collapse the numeric window-jump keys into a single help row, e.g.
/// `C-a 0-9   select window by number`. `keys` arrive sorted (`BTreeMap`
/// iteration). Returns `None` when no such keys are bound.
fn compact_window_jump(prefix: &str, keys: &[String]) -> Option<Entry> {
    let first = keys.first()?;
    let last = keys.last()?;
    let chord = if keys.len() == 1 {
        format!("{prefix} {first}")
    } else {
        format!("{prefix} {first}-{last}")
    };
    Some(Entry {
        chord,
        action: "select window by number".to_owned(),
    })
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
        let overlay = HelpOverlay::from_config(&cfg(), &Theme::default());
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
        let overlay = HelpOverlay::from_config(&c, &Theme::default());
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
        let overlay = HelpOverlay::from_config(&c, &Theme::default());
        let text = render_to_string(&overlay, 80, 24);
        assert!(text.contains("C-a |"), "missing `C-a |` row:\n{text}");
        assert!(text.contains("C-a -"), "missing `C-a -` row:\n{text}");
    }

    #[test]
    fn esc_dismisses() {
        let mut overlay = HelpOverlay::from_config(&cfg(), &Theme::default());
        assert_eq!(
            overlay.handle_key(&key(PhysicalKey::Escape)),
            OverlayCommand::Dismiss
        );
    }

    #[test]
    fn f1_dismisses() {
        let mut overlay = HelpOverlay::from_config(&cfg(), &Theme::default());
        assert_eq!(
            overlay.handle_key(&key(PhysicalKey::F1)),
            OverlayCommand::Dismiss
        );
    }

    #[test]
    fn other_keys_stay() {
        let mut overlay = HelpOverlay::from_config(&cfg(), &Theme::default());
        assert_eq!(
            overlay.handle_key(&key(PhysicalKey::A)),
            OverlayCommand::Stay
        );
    }

    #[test]
    fn dismisses_on_rebound_show_help_chord_not_just_f1() {
        // phux-ahv.7: a user who rebinds `show-help` to `?` must be able
        // to dismiss with `?`, and F1 (no longer bound) must NOT dismiss.
        let mut c = cfg();
        c.global.clear();
        c.global
            .insert("?".to_owned(), Action::Bare("show-help".to_owned()));
        let mut overlay = HelpOverlay::from_config(&c, &Theme::default());
        assert_eq!(overlay.show_help_chord.as_deref(), Some("?"));

        // `?` is Shift+/ on US ANSI (the chord parser decomposes it), so
        // build the live event accordingly.
        let mut question = key(PhysicalKey::Slash);
        question.mods = ModSet::SHIFT;
        assert_eq!(
            overlay.handle_key(&question),
            OverlayCommand::Dismiss,
            "the rebound show-help chord should dismiss"
        );

        // F1 is no longer the help binding: it must be absorbed, not
        // treated as a dismiss.
        assert_eq!(
            overlay.handle_key(&key(PhysicalKey::F1)),
            OverlayCommand::Stay,
            "F1 should no longer dismiss once show-help is rebound off it"
        );
        // Esc still always dismisses.
        assert_eq!(
            overlay.handle_key(&key(PhysicalKey::Escape)),
            OverlayCommand::Dismiss
        );
    }

    #[test]
    fn numeric_window_jumps_collapse_to_one_row() {
        // phux UX: ten `select-window {index}` bindings (0-9) must render
        // as a single compact row, not ten near-identical lines.
        let mut c = cfg();
        for i in 0..10u8 {
            let mut args = BTreeMap::new();
            args.insert("index".to_owned(), toml::Value::Integer(i.into()));
            c.prefix_table.insert(
                i.to_string(),
                Action::Parameterized(ParamAction {
                    action: "select-window".to_owned(),
                    args,
                }),
            );
        }
        let overlay = HelpOverlay::from_config(&c, &Theme::default());
        // Exactly one entry carries the collapsed label, and the individual
        // C-a 0 / C-a 9 rows are gone.
        let collapsed = overlay
            .prefix_entries
            .iter()
            .filter(|e| e.action == "select window by number")
            .count();
        assert_eq!(
            collapsed, 1,
            "expected exactly one collapsed window-jump row"
        );
        assert!(
            overlay
                .prefix_entries
                .iter()
                .all(|e| e.chord != "C-a 0" && e.chord != "C-a 9"),
            "individual numeric jump rows should be collapsed away"
        );
        let text = render_to_string(&overlay, 80, 24);
        assert!(
            text.contains("C-a 0-9"),
            "expected compacted `C-a 0-9` row:\n{text}"
        );
        assert!(
            text.contains("select window by number"),
            "compacted row should make window selection clear:\n{text}"
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
        let overlay = HelpOverlay::from_config(&cfg(), &Theme::default());
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
        let overlay = HelpOverlay::from_config(&cfg(), &Theme::default());
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
    fn render_centers_within_tiny_viewport() {
        // Regression for the prior `centered` clamp: a viewport smaller
        // than the modal's preferred size must still render without
        // overflowing (the shared `centered` helper clamps to the outer
        // rect). Just assert it paints something without panicking.
        let overlay = HelpOverlay::from_config(&cfg(), &Theme::default());
        let text = render_to_string(&overlay, 20, 8);
        assert!(
            text.contains("phux help"),
            "title in tiny viewport:\n{text}"
        );
    }
}
