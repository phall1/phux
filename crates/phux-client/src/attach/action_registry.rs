//! Canonical action registry (phux-ahv.8).
//!
//! The command palette and the help overlay both need a human-facing
//! catalogue of the actions the dispatcher can run. That catalogue must
//! not drift from what [`run_action`](super::input_dispatch) actually
//! handles — a palette entry for an action the dispatcher ignores is a
//! dead command, and an action the dispatcher handles but the palette
//! omits is undiscoverable.
//!
//! ## Drift prevention
//!
//! There is one source of truth for the set of action *names*:
//! [`ACTION_NAMES`](super::input_dispatch::ACTION_NAMES), owned next to
//! `run_action`. This module's [`REGISTRY`] supplies the *presentation*
//! (description + the default [`ResolvedAction`] the palette commits) for
//! each of those names. A unit test
//! (`registry_covers_every_dispatched_action`) asserts the two sets are
//! equal in both directions, so adding an arm to `run_action` without
//! registering it — or vice versa — fails CI. Adding a new action is
//! therefore a three-touch change that the compiler and the test funnel
//! together: the `run_action` match arm, the `ACTION_NAMES` entry, and the
//! [`REGISTRY`] row.
//!
//! Palette items resolve their *bound chord* at build time from the live
//! [`KeybindingsCfg`] snapshot, so the displayed shortcut always reflects
//! the user's actual config (or `"unbound"`).

use std::collections::BTreeMap;

use phux_config::keybind::ResolvedAction;
use phux_config::{Action, KeybindingsCfg};

use crate::render::overlay::select_list::SelectItem;

/// A registry row: an action the palette can offer.
#[derive(Debug, Clone, Copy)]
pub struct ActionSpec {
    /// Canonical action name (matches a `run_action` arm and an
    /// [`super::input_dispatch::ACTION_NAMES`] entry).
    pub name: &'static str,
    /// One-line human description shown in the palette.
    pub description: &'static str,
    /// Inline `(key, value)` args the palette-committed
    /// [`ResolvedAction`] should carry. Empty for bare actions; e.g.
    /// `split-pane` carries `direction = "vertical"` so the palette
    /// commits a concrete, runnable action rather than a half-specified
    /// one that would bell.
    pub args: &'static [(&'static str, ArgValue)],
}

/// A statically-expressible argument value for a registry row.
///
/// `ResolvedAction::args` is a `BTreeMap<String, toml::Value>`, but
/// `toml::Value` isn't `const`-constructible, so the registry expresses
/// args with this small enum and converts at build time.
#[derive(Debug, Clone, Copy)]
pub enum ArgValue {
    /// A string-valued arg, e.g. `direction = "vertical"`.
    Str(&'static str),
    /// An integer-valued arg, e.g. `amount = 5`.
    Int(i64),
}

impl ArgValue {
    fn to_toml(self) -> toml::Value {
        match self {
            Self::Str(s) => toml::Value::String(s.to_owned()),
            Self::Int(n) => toml::Value::Integer(n),
        }
    }
}

impl ActionSpec {
    /// The [`ResolvedAction`] this spec commits when chosen from the
    /// palette — the same shape a keybinding produces, so it flows
    /// through `run_action` identically.
    #[must_use]
    pub fn resolved_action(&self) -> ResolvedAction {
        let mut args = BTreeMap::new();
        for (k, v) in self.args {
            args.insert((*k).to_owned(), v.to_toml());
        }
        ResolvedAction {
            action: self.name.to_owned(),
            args,
        }
    }
}

/// The canonical, in-tree catalogue of palette-offerable actions.
///
/// Every name here MUST be handled by a `run_action` arm and listed in
/// [`super::input_dispatch::ACTION_NAMES`] (enforced by a unit test).
///
/// Notes on inclusions/exclusions:
/// - `select-window` is parameterized by `index`, which the palette has
///   no UI to collect; the `<leader> w` window picker is the right
///   surface for "jump to window N", so it is omitted here.
/// - `rename-window` with no `name` arg opens the interactive prompt, so
///   the palette offers the bare form (prompt-driven).
/// - `command-palette` is omitted — opening the palette from the palette
///   is noise.
pub const REGISTRY: &[ActionSpec] = &[
    ActionSpec {
        name: "split-pane",
        description: "Split the focused pane side-by-side (vertical divider)",
        args: &[("direction", ArgValue::Str("vertical"))],
    },
    ActionSpec {
        name: "kill-pane",
        description: "Close the focused pane",
        args: &[],
    },
    ActionSpec {
        name: "new-window",
        description: "Open a new window",
        args: &[],
    },
    ActionSpec {
        name: "kill-window",
        description: "Close the active window and all its panes",
        args: &[],
    },
    ActionSpec {
        name: "next-window",
        description: "Switch to the next window",
        args: &[],
    },
    ActionSpec {
        name: "previous-window",
        description: "Switch to the previous window",
        args: &[],
    },
    ActionSpec {
        name: "window-picker",
        description: "Pick a window from a filterable list",
        args: &[],
    },
    ActionSpec {
        name: "session-picker",
        description: "Pick a session from a filterable list",
        args: &[],
    },
    ActionSpec {
        name: "new-session",
        description: "Create a new session and switch to it",
        args: &[],
    },
    ActionSpec {
        name: "rename-window",
        description: "Rename the active window (interactive prompt)",
        args: &[],
    },
    ActionSpec {
        name: "rename-session",
        description: "Rename the current session (interactive prompt)",
        args: &[],
    },
    ActionSpec {
        name: "focus-direction",
        description: "Move focus to the pane on the left",
        args: &[("direction", ArgValue::Str("left"))],
    },
    ActionSpec {
        name: "resize-pane",
        description: "Grow the focused pane to the left",
        args: &[
            ("direction", ArgValue::Str("left")),
            ("amount", ArgValue::Int(5)),
        ],
    },
    ActionSpec {
        name: "next-pane",
        description: "Cycle focus to the next pane",
        args: &[],
    },
    ActionSpec {
        name: "previous-pane",
        description: "Cycle focus to the previous pane",
        args: &[],
    },
    ActionSpec {
        name: "toggle-zoom",
        description: "Zoom the focused pane to fill the window (toggle)",
        args: &[],
    },
    ActionSpec {
        name: "show-help",
        description: "Show the keybindings help overlay",
        args: &[],
    },
    ActionSpec {
        name: "detach",
        description: "Detach this client from the session",
        args: &[],
    },
];

/// Build the palette's [`SelectItem`] rows from the [`REGISTRY`],
/// annotating each with its currently-bound chord (or `"unbound"`).
///
/// `keybindings` is the live config snapshot; `None` (config failed to
/// load) yields every row as `"unbound"`. The committed action is the
/// registry's [`ActionSpec::resolved_action`], so choosing a palette row
/// runs exactly what a keybinding would.
#[must_use]
pub fn palette_items(keybindings: Option<&KeybindingsCfg>) -> Vec<SelectItem> {
    REGISTRY
        .iter()
        .map(|spec| {
            let resolved = spec.resolved_action();
            let chord = keybindings.map_or_else(
                || "unbound".to_owned(),
                |kb| bound_chord(kb, &resolved).unwrap_or_else(|| "unbound".to_owned()),
            );
            SelectItem::new(spec.description, resolved).secondary(chord)
        })
        .collect()
}

/// Find the chord a user has bound to `target`, formatted as the literal
/// keystrokes to type.
///
/// Prefix-table entries are shown with the leader prefixed (e.g.
/// `"C-a |"`); global entries are shown as-is. The prefix table is
/// scanned before globals.
///
/// A registry action like `split-pane` may be bound under several chords
/// that differ only in args (`|` = vertical, `-` = horizontal). We prefer
/// the binding whose args exactly match the registry row, so the palette
/// shows the chord that runs *this* row; we fall back to a name-only
/// match when no exact-args binding exists. `None` when nothing maps to
/// the action name at all.
#[must_use]
fn bound_chord(cfg: &KeybindingsCfg, target: &ResolvedAction) -> Option<String> {
    // First pass: an exact (name + args) match.
    if let Some(chord) = scan(cfg, target, true) {
        return Some(chord);
    }
    // Fallback: any binding with the same action name.
    scan(cfg, target, false)
}

/// Scan the prefix table then globals for a binding to `target`'s action.
/// With `exact`, the binding's args must also equal `target.args`.
fn scan(cfg: &KeybindingsCfg, target: &ResolvedAction, exact: bool) -> Option<String> {
    for (chord, action) in &cfg.prefix_table {
        if binding_matches(action, target, exact) {
            return Some(format!("{} {chord}", cfg.prefix));
        }
    }
    for (chord, action) in &cfg.global {
        if binding_matches(action, target, exact) {
            return Some(chord.clone());
        }
    }
    None
}

/// `true` when `action` names `target.action` (and, when `exact`, its
/// resolved args equal `target.args`).
fn binding_matches(action: &Action, target: &ResolvedAction, exact: bool) -> bool {
    let resolved = ResolvedAction::from(action);
    resolved.action == target.action && (!exact || resolved.args == target.args)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn registry_covers_every_dispatched_action() {
        // `command-palette` and `select-window` are dispatched but
        // intentionally excluded from the palette (no palette UI / opens
        // self), so they are exempt from the registry-side check.
        // `switch-session` is likewise dispatched-only: it requires a
        // `name` arg supplied by the session picker, so a bare palette
        // row would have no target to act on.
        const PALETTE_EXEMPT: &[&str] = &[
            "command-palette",
            "select-window",
            "switch-session",
            "copy-mode",
        ];

        // The two source-of-truth sets must be identical: the registry's
        // presentation rows and the dispatcher's handled-action names.
        let dispatched: BTreeSet<&str> = super::super::input_dispatch::ACTION_NAMES
            .iter()
            .copied()
            .collect();
        let registered: BTreeSet<&str> = REGISTRY.iter().map(|s| s.name).collect();

        // Every registered action is dispatched.
        for name in &registered {
            assert!(
                dispatched.contains(name),
                "registry lists `{name}` but run_action has no arm (or ACTION_NAMES omits it)",
            );
        }
        // Every dispatched action is registered, modulo the documented
        // exemptions.
        for name in &dispatched {
            if PALETTE_EXEMPT.contains(name) {
                continue;
            }
            assert!(
                registered.contains(name),
                "run_action handles `{name}` but the palette registry omits it \
                 (add an ActionSpec or document it in PALETTE_EXEMPT)",
            );
        }
    }

    #[test]
    fn resolved_action_carries_registry_args() {
        let split = REGISTRY
            .iter()
            .find(|s| s.name == "split-pane")
            .expect("split-pane registered");
        let ra = split.resolved_action();
        assert_eq!(ra.action, "split-pane");
        assert_eq!(
            ra.args.get("direction"),
            Some(&toml::Value::String("vertical".to_owned()))
        );
    }

    #[test]
    fn palette_items_show_unbound_when_no_config() {
        let items = palette_items(None);
        assert!(
            items
                .iter()
                .all(|i| i.secondary.as_deref() == Some("unbound")),
            "no config ⇒ every row unbound",
        );
    }
}
