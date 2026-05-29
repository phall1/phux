//! phux-config: TOML config + status-bar widget contract.
//!
//! This crate owns the typed schema for `~/.config/phux/config.toml`
//! (see `docs/consumers/tui.md` §4). Higher-level crates load a [`Config`] via
//! [`parse_str`] and consume the typed view; widget rendering, keybind
//! resolution, and hook dispatch all read from this schema.
//!
//! Parse errors carry `line:col` locations derived from the TOML byte
//! span so end-user diagnostics can point at the offending token.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod error;
mod schema;

// Wave 5 modules — each owned by its respective subtask:
pub mod keybind; // phux-nz4.3
pub mod loader; // phux-nz4.2
pub mod widget; // phux-nz4.4 (note: schema::Widget is the TOML enum; widget::Widget is the trait)

pub use error::{ConfigError, byte_offset_to_line_col};
pub use schema::{
    Action, Config, CwdInheritance, DefaultsCfg, ExperimentalCfg, HookEntry, KeybindingsCfg,
    ParamAction, StatusCfg, ThemeCfg, Widget, WidgetSpec,
};
pub use widget::{
    SessionNameWidget, StatusBar, StatusWidget, TimeWidget, WidgetCells, WidgetContext,
    WidgetError, WidgetFactory, WidgetRegistry, row_to_string,
};

use std::path::Path;

/// Default phux configuration, shipped with the binary.
///
/// The loader layers the user's on-disk config on top of this — each
/// leaf the user sets wins; everything else is inherited from here.
/// Pure parsing via [`parse_str`] does NOT apply this; only
/// [`parse_with_defaults`] (used by [`loader::load_from`]) does.
///
/// Embedded at compile time. The doctest below pins that it parses.
///
/// ```
/// let cfg = phux_config::parse_str(
///     phux_config::DEFAULT_CONFIG_TOML,
///     std::path::Path::new("default.toml"),
/// ).expect("embedded defaults must parse");
/// assert_eq!(cfg.keybindings.prefix, "C-a");
/// ```
pub const DEFAULT_CONFIG_TOML: &str = include_str!("default.toml");

/// Parse a TOML config from a string.
///
/// `path` is used only for error reporting — it is embedded in
/// [`ConfigError::Parse`] so messages display the source file.
///
/// This does NOT apply [`DEFAULT_CONFIG_TOML`]; callers wanting the
/// user-facing "shipped defaults + user overrides" behavior should use
/// [`parse_with_defaults`] (or [`loader::load_from`], which routes
/// through it).
///
/// # Errors
///
/// Returns [`ConfigError::Parse`] if the input is not valid TOML or
/// does not deserialize into the schema (including unknown fields,
/// which are rejected by `serde(deny_unknown_fields)`).
pub fn parse_str(input: &str, path: &Path) -> Result<Config, ConfigError> {
    match toml::from_str::<Config>(input) {
        Ok(cfg) => Ok(cfg),
        Err(e) => {
            let (line, col) = e
                .span()
                .map_or((1, 1), |range| byte_offset_to_line_col(input, range.start));
            Err(ConfigError::Parse {
                path: path.to_path_buf(),
                line,
                col,
                message: e.message().to_owned(),
            })
        }
    }
}

/// Parse `user_input` and layer it over [`DEFAULT_CONFIG_TOML`].
///
/// Merge semantics: every leaf the user sets wins. Tables merge
/// recursively (so the user can add one binding without restating the
/// whole `prefix-table`). Arrays do NOT merge element-wise — they
/// overwrite, because there is no per-element identity for widget
/// lists / hook lists.
///
/// `path` is used only for error reporting on the user input.
///
/// # Errors
///
/// Returns [`ConfigError::Parse`] if either the embedded defaults or
/// the user input fail to parse as TOML, or if the merged document
/// fails to deserialize into the schema.
pub fn parse_with_defaults(user_input: &str, path: &Path) -> Result<Config, ConfigError> {
    let default_table: toml::Table = toml::from_str(DEFAULT_CONFIG_TOML).map_err(|e| {
        let (line, col) = e.span().map_or((1, 1), |r| {
            byte_offset_to_line_col(DEFAULT_CONFIG_TOML, r.start)
        });
        ConfigError::Parse {
            path: Path::new("<embedded default.toml>").to_path_buf(),
            line,
            col,
            message: e.message().to_owned(),
        }
    })?;
    let user_table: toml::Table = toml::from_str(user_input).map_err(|e| {
        let (line, col) = e
            .span()
            .map_or((1, 1), |r| byte_offset_to_line_col(user_input, r.start));
        ConfigError::Parse {
            path: path.to_path_buf(),
            line,
            col,
            message: e.message().to_owned(),
        }
    })?;
    let merged = merge_tables(default_table, user_table);
    toml::Value::Table(merged).try_into().map_err(|e| {
        let (line, col) = e
            .span()
            .map_or((1, 1), |r| byte_offset_to_line_col(user_input, r.start));
        ConfigError::Parse {
            path: path.to_path_buf(),
            line,
            col,
            message: e.message().to_owned(),
        }
    })
}

/// Recursively merge `overlay` into `base`. Tables merge per-key;
/// any other Value type (including arrays) replaces wholesale.
fn merge_tables(mut base: toml::Table, overlay: toml::Table) -> toml::Table {
    for (k, ov) in overlay {
        match (base.remove(&k), ov) {
            (Some(toml::Value::Table(b)), toml::Value::Table(o)) => {
                base.insert(k, toml::Value::Table(merge_tables(b, o)));
            }
            (_, v) => {
                base.insert(k, v);
            }
        }
    }
    base
}

/// Render a [`DefaultsCfg::session_name_template`] into a concrete
/// session name for an auto-created session (phux-4li.1).
///
/// Substitutes the `${cwd-basename}` placeholder with the final path
/// component of `cwd`. Session names double as selector tokens
/// (`name:N.M`, see `docs/consumers/tui.md` §3), and `:` is the
/// session→window delimiter, so any `:` in the basename is replaced with
/// `_` to keep an auto-name from colliding with the selector grammar.
/// (`.` is left intact — it only delimits inside the `name:N.M` tail, so
/// a dotted directory name like `my.project` parses cleanly as a bare
/// session name.) Unknown placeholders pass through verbatim.
///
/// May return an empty string when the template renders empty (e.g. the
/// template is exactly `${cwd-basename}` and `cwd` is `/`, which has no
/// final component); the caller decides the fallback.
#[must_use]
pub fn render_session_name_template(template: &str, cwd: &Path) -> String {
    let basename = cwd
        .file_name()
        .map(|os| os.to_string_lossy().replace(':', "_"))
        .unwrap_or_default();
    template.replace("${cwd-basename}", &basename)
}

#[cfg(test)]
mod session_name_tests {
    use super::render_session_name_template;
    use std::path::Path;

    #[test]
    fn literal_template_passes_through_unchanged() {
        // The shipped default is the literal "default" — no placeholder,
        // so behavior is unchanged unless the user opts into a template.
        assert_eq!(
            render_session_name_template("default", Path::new("/Users/phall/workspace/phux")),
            "default"
        );
    }

    #[test]
    fn cwd_basename_substitutes_the_final_component() {
        assert_eq!(
            render_session_name_template(
                "phux-${cwd-basename}",
                Path::new("/Users/phall/workspace/phux")
            ),
            "phux-phux"
        );
        assert_eq!(
            render_session_name_template("${cwd-basename}", Path::new("/home/me/notes")),
            "notes"
        );
    }

    #[test]
    fn colon_in_basename_is_sanitized_to_underscore() {
        // ':' would otherwise read as the session→window selector
        // delimiter.
        assert_eq!(
            render_session_name_template("${cwd-basename}", Path::new("/tmp/a:b")),
            "a_b"
        );
    }

    #[test]
    fn dot_in_basename_is_preserved() {
        assert_eq!(
            render_session_name_template("${cwd-basename}", Path::new("/tmp/my.project")),
            "my.project"
        );
    }

    #[test]
    fn root_cwd_renders_empty_basename() {
        // No final component — caller falls back to a default name.
        assert_eq!(
            render_session_name_template("${cwd-basename}", Path::new("/")),
            ""
        );
    }

    #[test]
    fn unknown_placeholder_passes_through_verbatim() {
        assert_eq!(
            render_session_name_template("${unknown}", Path::new("/tmp/x")),
            "${unknown}"
        );
    }
}
