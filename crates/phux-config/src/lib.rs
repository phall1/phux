//! phux-config: TOML config + status-bar widget contract.
//!
//! This crate owns the typed schema for `~/.config/phux/config.toml`
//! (see `DESIGN.md` §4). Higher-level crates load a [`Config`] via
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
    Action, Config, DefaultsCfg, ExperimentalCfg, HookEntry, KeybindingsCfg, StatusCfg, ThemeCfg,
    Widget, WidgetSpec,
};
pub use widget::{
    SessionNameWidget, StatusBar, StatusWidget, TimeWidget, WidgetCells, WidgetContext,
    WidgetError, WidgetFactory, WidgetRegistry, row_to_string,
};

use std::path::Path;

/// Parse a TOML config from a string.
///
/// `path` is used only for error reporting — it is embedded in
/// [`ConfigError::Parse`] so messages display the source file.
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
