//! Chrome + overlay color theme (phux-ahv.4).
//!
//! Single source of truth for the hand-picked colors that the chrome
//! (status bar, dividers) and overlays (help, prompt) paint with. Before
//! this module those colors were scattered `Color::Cyan` / `Color::Yellow`
//! literals inside each overlay's `render`; now every chrome/overlay slot
//! resolves through one [`Theme`] value, owned by the attach driver
//! alongside the keybindings snapshot and threaded into the paint path.
//!
//! ## Slots
//!
//! A [`Theme`] is a flat set of named [`Color`] slots, each mapped to one
//! semantic role:
//!
//! - [`accent`] — modal titles (e.g. the help / prompt border title).
//! - [`chord`] — keybinding chords in the help table.
//! - [`action`] — reserved for action labels (kept distinct from `chord`
//!   so a future restyle can split them without churning callers).
//! - [`dim`] — de-emphasized text (footer hints, "no bindings" notice).
//! - [`border`] — modal borders.
//! - [`title`] — alias slot for window/section titles distinct from
//!   `accent` when a theme wants them to diverge.
//! - [`section_header`] — section headings inside the help modal.
//! - [`error`] — error / alarm text.
//! - [`sidebar_section`] — the sidebar's muted `spaces` / `agents`
//!   section headers (phux-foz.9).
//! - [`agent_idle`] / [`agent_working`] / [`agent_blocked`] /
//!   [`agent_done`] — agent lifecycle state colors in the sidebar's
//!   agents section (phux-foz.9).
//!
//! [`accent`]: Theme::accent
//! [`chord`]: Theme::chord
//! [`action`]: Theme::action
//! [`dim`]: Theme::dim
//! [`border`]: Theme::border
//! [`title`]: Theme::title
//! [`section_header`]: Theme::section_header
//! [`error`]: Theme::error
//! [`sidebar_section`]: Theme::sidebar_section
//! [`agent_idle`]: Theme::agent_idle
//! [`agent_working`]: Theme::agent_working
//! [`agent_blocked`]: Theme::agent_blocked
//! [`agent_done`]: Theme::agent_done
//!
//! ## Overrides
//!
//! [`Theme::from_cfg`] reads `[theme]` from `phux_config` — a free-form
//! `slot -> color-string` map ([`phux_config::ThemeCfg`]). Recognized
//! slot keys override the default; an unknown key is ignored and an
//! unparseable color string falls back to the slot's default (both
//! logged at `warn`).

use std::str::FromStr;

use ratatui::style::Color;

/// Named color slots for chrome + overlay painting.
///
/// Construct the default with [`Theme::default`] or layer config
/// overrides with [`Theme::from_cfg`]. Each field is a ratatui [`Color`]
/// so consumers under `render/` can drop it straight into a [`Style`].
///
/// [`Style`]: ratatui::style::Style
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Theme {
    /// Modal titles (help / prompt border title text).
    pub accent: Color,
    /// Keybinding chords in the help table.
    pub chord: Color,
    /// Action labels. Distinct slot so a theme can diverge chord vs
    /// action coloring without a code change; defaults to the terminal
    /// foreground (no explicit color) like the action column does today.
    pub action: Color,
    /// De-emphasized text: footer hints, the "no bindings" notice.
    pub dim: Color,
    /// Modal borders.
    pub border: Color,
    /// Window / section titles where a theme wants them distinct from
    /// `accent`. Defaults to the same value as `accent`.
    pub title: Color,
    /// Section headings inside the help modal.
    pub section_header: Color,
    /// Error / alarm text.
    pub error: Color,
    /// Modal interior background — the "panel" fill behind a floating
    /// modal's body. Defaults to `Reset` (inherit the terminal background)
    /// so the box reads as transparent unless a theme opts into a tint.
    pub surface: Color,
    /// Drop-shadow color painted one cell below + right of a floating
    /// modal, giving it depth over the live panes. A subtle dark by
    /// default so it reads as a shadow on most terminals.
    pub shadow: Color,
    /// Foreground of selection chrome: the copy-mode status strip (and
    /// future selected list rows).
    pub selection_fg: Color,
    /// Background of selection chrome: the copy-mode status strip (and
    /// future selected list rows).
    pub selection_bg: Color,
    /// Attention chrome (phux-foz.1): the sidebar tab marker and the
    /// status-bar hint painted when an agent in a pane is waiting on a
    /// human answer (ADR-0035 `AgentEvent::Asked`).
    pub attention: Color,
    /// Sidebar section headers (phux-foz.9): the muted lowercase
    /// `spaces` / `agents` headings of the herdr-shaped sidebar.
    pub sidebar_section: Color,
    /// Agent lifecycle coloring (phux-foz.9): an `idle` agent row's
    /// glyph + state text in the sidebar's agents section.
    pub agent_idle: Color,
    /// Agent lifecycle coloring (phux-foz.9): a `working` agent row.
    pub agent_working: Color,
    /// Agent lifecycle coloring (phux-foz.9): a `blocked` agent row
    /// (waiting on a human).
    pub agent_blocked: Color,
    /// Agent lifecycle coloring (phux-foz.9): a `done` agent row.
    pub agent_done: Color,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            accent: Color::Rgb(190, 242, 100),
            chord: Color::Rgb(134, 239, 172),
            // `Reset` = terminal default foreground; the action column
            // was unstyled (plain `Span::raw`) before this module.
            action: Color::Reset,
            dim: Color::DarkGray,
            border: Color::Rgb(82, 82, 91),
            title: Color::Rgb(190, 242, 100),
            section_header: Color::Yellow,
            error: Color::Red,
            // Reset = no fill (inherit terminal bg); opt-in via config.
            surface: Color::Reset,
            // A near-black slate that reads as a shadow on dark and light
            // terminals alike.
            shadow: Color::Rgb(28, 28, 38),
            // Reproduces the copy-mode strip's prior look (bright white on
            // dark gray) now that it routes through the theme.
            selection_fg: Color::White,
            selection_bg: Color::Indexed(240),
            // Amber: reads as "needs you" without colliding with `error`
            // red or the lime `accent`.
            attention: Color::Rgb(251, 191, 36),
            // phux-foz.9: sidebar section headers read as muted chrome,
            // same register as `dim`.
            sidebar_section: Color::DarkGray,
            // phux-foz.9: agent lifecycle colors. Idle is a muted slate
            // ("nothing needs you"), working the green of live progress,
            // blocked the attention amber, done a settled blue.
            agent_idle: Color::Rgb(148, 163, 184),
            agent_working: Color::Rgb(134, 239, 172),
            agent_blocked: Color::Rgb(251, 191, 36),
            agent_done: Color::Rgb(96, 165, 250),
        }
    }
}

impl Theme {
    /// Build a theme from the default, layering `[theme]` config
    /// overrides on top.
    ///
    /// Each recognized slot key in `cfg.slots` whose value parses as a
    /// color replaces the default for that slot. Unknown keys are
    /// ignored (warn); unparseable color strings keep the default
    /// (warn). Parsing accepts everything ratatui's [`Color`] `FromStr`
    /// accepts: named colors (`"cyan"`), hex (`"#cdd6f4"`), and ANSI
    /// indices (`"12"`).
    #[must_use]
    pub fn from_cfg(cfg: &phux_config::ThemeCfg) -> Self {
        let mut theme = Self::default();
        for (key, spec) in &cfg.slots {
            let Some(slot) = theme.slot_mut(key) else {
                tracing::warn!(slot = key, "unknown theme slot; ignoring");
                continue;
            };
            match parse_color(spec) {
                Some(color) => *slot = color,
                None => {
                    tracing::warn!(
                        slot = key,
                        color = spec,
                        "unparseable theme color; keeping default"
                    );
                }
            }
        }
        theme
    }

    /// Mutable handle to the slot named `key`, or `None` if `key` is not
    /// a recognized slot. Slot names match the field names.
    fn slot_mut(&mut self, key: &str) -> Option<&mut Color> {
        match key {
            "accent" => Some(&mut self.accent),
            "chord" => Some(&mut self.chord),
            "action" => Some(&mut self.action),
            "dim" => Some(&mut self.dim),
            "border" => Some(&mut self.border),
            "title" => Some(&mut self.title),
            "section_header" => Some(&mut self.section_header),
            "error" => Some(&mut self.error),
            "surface" => Some(&mut self.surface),
            "shadow" => Some(&mut self.shadow),
            "selection_fg" => Some(&mut self.selection_fg),
            "selection_bg" => Some(&mut self.selection_bg),
            "attention" => Some(&mut self.attention),
            "sidebar_section" => Some(&mut self.sidebar_section),
            "agent_idle" => Some(&mut self.agent_idle),
            "agent_working" => Some(&mut self.agent_working),
            "agent_blocked" => Some(&mut self.agent_blocked),
            "agent_done" => Some(&mut self.agent_done),
            _ => None,
        }
    }
}

/// Parse a color string into a ratatui [`Color`]. `None` when ratatui
/// can't interpret it (caller keeps the slot default).
fn parse_color(spec: &str) -> Option<Color> {
    Color::from_str(spec).ok()
}

#[cfg(test)]
#[allow(clippy::expect_used, reason = "tests")]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn cfg(pairs: &[(&str, &str)]) -> phux_config::ThemeCfg {
        let mut slots = BTreeMap::new();
        for (k, v) in pairs {
            slots.insert((*k).to_owned(), (*v).to_owned());
        }
        phux_config::ThemeCfg { slots }
    }

    #[test]
    fn default_slots_match_shipped_colors() {
        let t = Theme::default();
        assert_eq!(t.accent, Color::Rgb(190, 242, 100));
        assert_eq!(t.chord, Color::Rgb(134, 239, 172));
        assert_eq!(t.action, Color::Reset);
        assert_eq!(t.dim, Color::DarkGray);
        assert_eq!(t.border, Color::Rgb(82, 82, 91));
        assert_eq!(t.title, Color::Rgb(190, 242, 100));
        assert_eq!(t.section_header, Color::Yellow);
        assert_eq!(t.error, Color::Red);
        // Design tokens added for floating-modal depth + selection chrome.
        assert_eq!(t.surface, Color::Reset);
        assert_eq!(t.shadow, Color::Rgb(28, 28, 38));
        assert_eq!(t.selection_fg, Color::White);
        assert_eq!(t.selection_bg, Color::Indexed(240));
        // phux-foz.1: attention chrome for the agent-asked (ADR-0035) badge.
        assert_eq!(t.attention, Color::Rgb(251, 191, 36));
        // phux-foz.9: sidebar section headers + agent lifecycle colors.
        assert_eq!(t.sidebar_section, Color::DarkGray);
        assert_eq!(t.agent_idle, Color::Rgb(148, 163, 184));
        assert_eq!(t.agent_working, Color::Rgb(134, 239, 172));
        assert_eq!(t.agent_blocked, Color::Rgb(251, 191, 36));
        assert_eq!(t.agent_done, Color::Rgb(96, 165, 250));
    }

    /// phux-foz.9: every sidebar/agent slot is config-overridable like the
    /// rest — unknown-slot warnings would otherwise silently eat them.
    #[test]
    fn sidebar_and_agent_slots_are_overridable() {
        let t = Theme::from_cfg(&cfg(&[
            ("sidebar_section", "#6c7086"),
            ("agent_idle", "white"),
            ("agent_working", "green"),
            ("agent_blocked", "red"),
            ("agent_done", "blue"),
        ]));
        assert_eq!(t.sidebar_section, Color::Rgb(0x6c, 0x70, 0x86));
        assert_eq!(t.agent_idle, Color::White);
        assert_eq!(t.agent_working, Color::Green);
        assert_eq!(t.agent_blocked, Color::Red);
        assert_eq!(t.agent_done, Color::Blue);
        assert_eq!(t.accent, Theme::default().accent);
    }

    #[test]
    fn attention_slot_is_overridable() {
        let t = Theme::from_cfg(&cfg(&[("attention", "#f38ba8")]));
        assert_eq!(t.attention, Color::Rgb(0xf3, 0x8b, 0xa8));
        assert_eq!(t.accent, Theme::default().accent);
    }

    #[test]
    fn surface_and_selection_slots_are_overridable() {
        let t = Theme::from_cfg(&cfg(&[
            ("surface", "#1e1e2e"),
            ("shadow", "#000000"),
            ("selection_bg", "blue"),
            ("selection_fg", "15"),
        ]));
        assert_eq!(t.surface, Color::Rgb(0x1e, 0x1e, 0x2e));
        assert_eq!(t.shadow, Color::Rgb(0, 0, 0));
        assert_eq!(t.selection_bg, Color::Blue);
        assert_eq!(t.selection_fg, Color::Indexed(15));
        // Untouched slots keep their defaults.
        assert_eq!(t.accent, Color::Rgb(190, 242, 100));
    }

    #[test]
    fn from_cfg_empty_is_default() {
        let t = Theme::from_cfg(&phux_config::ThemeCfg::default());
        assert_eq!(t, Theme::default());
    }

    #[test]
    fn named_color_override_applies() {
        let t = Theme::from_cfg(&cfg(&[("accent", "magenta")]));
        assert_eq!(t.accent, Color::Magenta);
        // Untouched slots keep their default.
        assert_eq!(t.chord, Color::Rgb(134, 239, 172));
    }

    #[test]
    fn hex_color_override_applies() {
        let t = Theme::from_cfg(&cfg(&[("section_header", "#cdd6f4")]));
        assert_eq!(t.section_header, Color::Rgb(0xcd, 0xd6, 0xf4));
    }

    #[test]
    fn indexed_color_override_applies() {
        let t = Theme::from_cfg(&cfg(&[("chord", "12")]));
        assert_eq!(t.chord, Color::Indexed(12));
    }

    #[test]
    fn unknown_slot_is_ignored() {
        let t = Theme::from_cfg(&cfg(&[("not_a_slot", "red")]));
        assert_eq!(t, Theme::default());
    }

    #[test]
    fn unparseable_color_keeps_default() {
        let t = Theme::from_cfg(&cfg(&[("accent", "definitely-not-a-color")]));
        assert_eq!(t.accent, Theme::default().accent);
    }

    #[test]
    fn multiple_overrides_apply_independently() {
        let t = Theme::from_cfg(&cfg(&[
            ("accent", "blue"),
            ("error", "yellow"),
            ("dim", "white"),
        ]));
        assert_eq!(t.accent, Color::Blue);
        assert_eq!(t.error, Color::Yellow);
        assert_eq!(t.dim, Color::White);
        assert_eq!(t.section_header, Color::Yellow); // default kept
    }
}
