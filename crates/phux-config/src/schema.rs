//! Typed schema for `config.toml`.
//!
//! Field and section names track `docs/consumers/tui.md` §4 verbatim. The TOML side
//! uses kebab-case (`history-limit`); the Rust side uses `snake_case`
//! and `#[serde(rename = ...)]` bridges the two.
//!
//! `Eq` is intentionally not derived: several structs carry
//! `toml::Value` (which is not `Eq` because of `f64`).

#![allow(clippy::derive_partial_eq_without_eq)]

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Top-level config. See `docs/consumers/tui.md` §4.2.
///
/// Sections are all optional; an empty config file parses to
/// [`Config::default`].
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Server-wide defaults — shell, history, refresh rate, log filter.
    #[serde(default)]
    pub defaults: DefaultsCfg,

    /// Prefix + prefix-table + global keybindings.
    #[serde(default)]
    pub keybindings: KeybindingsCfg,

    /// Status-bar slot composition.
    #[serde(default)]
    pub status: StatusCfg,

    /// Event hooks (`[[hooks.<name>]]`).
    ///
    /// Keyed by hook name (e.g. `pane-exit`, `after-new-pane`); each
    /// entry is the array-of-tables under that name.
    #[serde(default)]
    pub hooks: BTreeMap<String, Vec<HookEntry>>,

    /// Color slots (theme). Free-form key/value of color strings.
    #[serde(default)]
    pub theme: ThemeCfg,

    /// Experimental knobs gated behind `[experimental]`.
    ///
    /// Everything under this section is subject to change without notice.
    /// See `docs/consumers/tui.md` §4.2 for the user-facing caveat.
    #[serde(default)]
    pub experimental: ExperimentalCfg,
}

// ---------------------------------------------------------------------------
// [defaults]
// ---------------------------------------------------------------------------

/// `[defaults]` table. See `docs/consumers/tui.md` §12 for shipped values.
///
/// This struct is intentionally NOT `#[non_exhaustive]`: it is constructed
/// only via `Default` + struct-update syntax (`..DefaultsCfg::default()`)
/// in tests and via serde everywhere else, so adding fields is
/// source-compatible for all in-tree consumers.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct DefaultsCfg {
    /// Shell to spawn in new panes. `None` ⇒ honor `$SHELL` (fallback
    /// `/bin/sh`) at runtime.
    #[serde(default)]
    pub shell: Option<String>,

    /// Lines of scrollback retained per pane.
    ///
    /// The TOML key is `history-limit` — the tmux-shaped name kept since
    /// `phux-config` first shipped. `scrollback-lines` was proposed in
    /// phux-4li.1 but consciously folded into this field rather than
    /// duplicated: they describe the same per-Terminal scrollback cap.
    #[serde(default = "default_history_limit", rename = "history-limit")]
    pub history_limit: u32,

    /// Cap on pane re-render rate, in Hz.
    #[serde(default = "default_refresh_rate", rename = "refresh-rate")]
    pub refresh_rate: u32,

    /// `tracing` filter directive (overridden by `$PHUX_LOG`).
    #[serde(default, rename = "log-filter")]
    pub log_filter: Option<String>,

    /// Mouse handling enabled at server level.
    #[serde(default = "default_true")]
    pub mouse: bool,

    /// How a freshly-spawned pane chooses its working directory.
    ///
    /// Default: [`CwdInheritance::InheritFocused`], matching tmux. See
    /// the enum docs for the full set.
    ///
    /// TODO(phux-4li.1 follow-up): wiring [`CwdInheritance::InheritFocused`]
    /// requires the server to track per-Terminal PTY working directory
    /// (OSC 7 from the shell or a kernel-side query). This ticket lands
    /// only the config knob; the server-side mechanism is a follow-up.
    #[serde(default, rename = "cwd-inheritance")]
    pub cwd_inheritance: CwdInheritance,

    /// Command to spawn when `phux` auto-creates a session on attach.
    ///
    /// `None` (default) ⇒ honor [`DefaultsCfg::shell`] (which in turn
    /// honors `$SHELL`). Set explicitly to launch e.g. a TUI dashboard
    /// or a specific REPL as the initial program.
    #[serde(default, rename = "spawn-on-attach")]
    pub spawn_on_attach: Option<String>,

    /// Naming template for auto-created sessions.
    ///
    /// Default: `"default"`. Supports `${cwd-basename}` substitution
    /// (resolved at session-creation time using the client's working
    /// directory). Other placeholders may be added later; unknown
    /// placeholders are passed through verbatim.
    #[serde(
        default = "default_session_name_template",
        rename = "session-name-template"
    )]
    pub session_name_template: String,
}

impl Default for DefaultsCfg {
    fn default() -> Self {
        Self {
            shell: None,
            history_limit: default_history_limit(),
            refresh_rate: default_refresh_rate(),
            log_filter: None,
            mouse: true,
            cwd_inheritance: CwdInheritance::default(),
            spawn_on_attach: None,
            session_name_template: default_session_name_template(),
        }
    }
}

const fn default_history_limit() -> u32 {
    50_000
}
const fn default_refresh_rate() -> u32 {
    60
}
const fn default_true() -> bool {
    true
}
fn default_session_name_template() -> String {
    "default".to_owned()
}

/// How a newly-spawned pane chooses its working directory.
///
/// Selected by the `defaults.cwd-inheritance` TOML key. Values use
/// kebab-case on the wire and `PascalCase` in Rust.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "kebab-case")]
pub enum CwdInheritance {
    /// Inherit the focused pane's current working directory. Default —
    /// matches tmux's default split behavior. Requires server-side PTY
    /// working-dir tracking (TODO; see [`DefaultsCfg::cwd_inheritance`]).
    #[default]
    InheritFocused,
    /// Always spawn in `$HOME`.
    Home,
    /// Spawn in the directory the session was created in.
    SessionRoot,
    /// Remember the last CWD per window and reuse it for new panes in
    /// that window.
    LastCwdPerWindow,
}

// ---------------------------------------------------------------------------
// [keybindings]
// ---------------------------------------------------------------------------

/// `[keybindings]` table: prefix key, prefix-table, and global table.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct KeybindingsCfg {
    /// Prefix key chord (e.g. `"C-a"`). Defaults to `C-a` — chosen for
    /// portability across host terminals (some emulators silently
    /// swallow `C-Space` before it reaches the client). Chord syntax
    /// per `crate::keybind`: modifier letters (`C` / `M` / `A` / `S`)
    /// joined to the key by `-`.
    #[serde(default = "default_prefix")]
    pub prefix: String,

    /// Bindings that fire after the prefix.
    #[serde(default, rename = "prefix-table")]
    pub prefix_table: BTreeMap<String, Action>,

    /// Bindings that fire any time (typically `super`/`hyper` chords).
    #[serde(default)]
    pub global: BTreeMap<String, Action>,
}

impl Default for KeybindingsCfg {
    fn default() -> Self {
        Self {
            prefix: default_prefix(),
            prefix_table: BTreeMap::new(),
            global: BTreeMap::new(),
        }
    }
}

fn default_prefix() -> String {
    "C-a".to_owned()
}

/// An action attached to a binding, hook, or status slot.
///
/// Per `docs/consumers/tui.md` §4.2, this is either a bare string (no parameters,
/// e.g. `"kill-pane"`) or an inline table whose `action` field names
/// the action and whose remaining fields supply parameters.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum Action {
    /// `"detach"` — bare action name, no parameters.
    Bare(String),
    /// `{ action = "new-pane", direction = "vertical" }` — parameterized.
    Parameterized(ParamAction),
}

/// Parameterized action: `action` plus arbitrary `kind`-specific args.
///
/// `args` collects every remaining key in the inline table. This mirrors
/// the design's "we don't centrally enumerate action parameters in the
/// loader" stance — schema validation per action lives in the
/// dispatcher, not here.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ParamAction {
    /// The action name (e.g. `new-pane`, `run`, `focus-pane`).
    ///
    /// `docs/consumers/tui.md` uses both `action = "..."` (in keybindings, §4.2)
    /// and `kind = "..."` (in hooks, §9) to name the action. We accept
    /// either spelling on input and canonicalize to `action` on output.
    #[serde(alias = "kind")]
    pub action: String,
    /// Remaining inline-table fields, passed through as TOML values.
    #[serde(flatten)]
    pub args: BTreeMap<String, toml::Value>,
}

// ---------------------------------------------------------------------------
// [status]
// ---------------------------------------------------------------------------

/// `[status]` table: three slots, each a list of widgets.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct StatusCfg {
    /// Left slot. Default: empty (callers may substitute defaults).
    #[serde(default)]
    pub left: Vec<Widget>,
    /// Center slot.
    #[serde(default)]
    pub center: Vec<Widget>,
    /// Right slot.
    #[serde(default)]
    pub right: Vec<Widget>,
}

/// A status-bar widget.
///
/// Per `docs/consumers/tui.md` §8.1, this is either a bare string (`"session"` is
/// shorthand for `{ kind = "session" }`) or an inline table with `kind`
/// plus widget-specific options.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum Widget {
    /// `"session"` — shorthand for `{ kind = "session" }`.
    Bare(String),
    /// Full widget spec: `kind` plus arbitrary options.
    Spec(WidgetSpec),
}

/// Long-form widget spec.
///
/// `opts` carries every field except `kind` so that future widget
/// parameters don't require schema churn here; the renderer in
/// `phux-client` validates per-`kind`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WidgetSpec {
    /// Widget kind (`clock`, `session`, `exec`, ...). See
    /// `docs/consumers/tui.md` §8.3 for the built-in catalog.
    pub kind: String,
    /// Remaining inline-table fields.
    #[serde(flatten)]
    pub opts: BTreeMap<String, toml::Value>,
}

// ---------------------------------------------------------------------------
// [[hooks.<name>]]
// ---------------------------------------------------------------------------

/// One entry under `[[hooks.<name>]]`: a `when` predicate plus an
/// `action` to run on match. See `docs/consumers/tui.md` §9.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct HookEntry {
    /// Match clauses (`exit-code = 0`, `cwd-startswith = "..."`, etc.).
    /// First-match-wins per hook event.
    #[serde(default)]
    pub when: BTreeMap<String, toml::Value>,
    /// Action to fire on match — same shape as a keybind action.
    pub action: Action,
}

// ---------------------------------------------------------------------------
// [experimental]
// ---------------------------------------------------------------------------

/// `[experimental]` table — opt-in flags for unstable features.
///
/// Anything here may be renamed, repurposed, or removed without a
/// `SemVer` bump. Set explicitly only if you accept that contract.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ExperimentalCfg {
    /// Engage Mosh-class predictive local echo in `phux attach`.
    ///
    /// When `true`, the attach loop dispatches to
    /// `phux_client::attach::run_with_predict` with a
    /// `PredictiveConfig { enabled: true, .. }`. See `phux-9gw.1` for
    /// the algorithm and `crates/phux-client/src/predict/` for the
    /// implementation.
    #[serde(default, rename = "predictive-echo")]
    pub predictive_echo: bool,
}

// ---------------------------------------------------------------------------
// [theme]
// ---------------------------------------------------------------------------

/// `[theme]` table. Free-form `slot -> color-string` map; the renderer
/// owns interpretation. We deliberately do not type-check colors here.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(transparent)]
pub struct ThemeCfg {
    /// Slot → color string (e.g. `"fg" -> "#cdd6f4"`).
    pub slots: BTreeMap<String, String>,
}
