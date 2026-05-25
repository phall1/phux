//! Config loader: resolves the on-disk config path and parses it.
//!
//! Resolution rules (see `DESIGN.md` §4.1):
//! * Prefer `$XDG_CONFIG_HOME/phux/config.toml` when `XDG_CONFIG_HOME` is set.
//! * Otherwise use `$HOME/.config/phux/config.toml`.
//! * If neither environment variable is set, fall back to the current
//!   working directory — same behavior as most user-config crates.
//!
//! A missing config file is *not* an error: callers get
//! [`Config::default`] and a `tracing::debug!` line. Any other I/O error
//! propagates as [`ConfigError::Io`].

use std::path::{Path, PathBuf};
use std::{fs, io};

use crate::{Config, ConfigError, parse_str};

/// Resolve the canonical config path: `$XDG_CONFIG_HOME/phux/config.toml`
/// (with `~/.config/phux/config.toml` fallback when `XDG_CONFIG_HOME` is
/// unset).
///
/// This is pure path math — it performs no I/O and does not check whether
/// the returned path exists.
#[must_use]
pub fn config_path() -> PathBuf {
    if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME") {
        let mut p = PathBuf::from(xdg);
        p.push("phux");
        p.push("config.toml");
        return p;
    }

    let base = std::env::var_os("HOME").map_or_else(PathBuf::new, PathBuf::from);
    let mut p = base;
    p.push(".config");
    p.push("phux");
    p.push("config.toml");
    p
}

/// Load the config from the canonical [`config_path`].
///
/// Missing-file is treated as "no overrides": returns `Ok(Config::default())`
/// after logging at `debug`. Any other read failure bubbles up as
/// [`ConfigError::Io`]; malformed TOML bubbles up as [`ConfigError::Parse`].
///
/// # Errors
///
/// See [`load_from`].
pub fn load() -> Result<Config, ConfigError> {
    load_from(&config_path())
}

/// Load the config from a specific path. Useful for tests and for a future
/// `phux --config <path>` CLI flag.
///
/// Missing-file (`io::ErrorKind::NotFound`) returns `Ok(Config::default())`
/// and emits a `tracing::debug!` event.
///
/// # Errors
///
/// * [`ConfigError::Io`] if reading `path` fails for any reason other than
///   "not found".
/// * [`ConfigError::Parse`] if `path` exists but does not parse / validate
///   against the schema.
pub fn load_from(path: &Path) -> Result<Config, ConfigError> {
    match fs::read_to_string(path) {
        Ok(contents) => parse_str(&contents, path),
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            tracing::debug!(
                path = %path.display(),
                "phux config not present; using embedded defaults"
            );
            Ok(Config::default())
        }
        Err(err) => Err(ConfigError::Io(err)),
    }
}
