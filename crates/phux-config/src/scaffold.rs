//! Config scaffolding: materialize a commented starter `config.toml`.
//!
//! # Why a *projection*, not a copy
//!
//! phux ships its defaults as the embedded, prose-annotated
//! [`DEFAULT_CONFIG_TOML`] base layer; a user's `config.toml` is merged
//! *on top* of it (see [`crate::merged_config_table`]). Writing the
//! defaults out as active values would freeze them at scaffold time — a
//! later phux that changes a default would no longer reach anyone who
//! ran `phux config init`.
//!
//! So [`reference_config`] emits a **comment-projection** of the
//! embedded defaults: identical prose, but every active assignment and
//! table header is commented out. The result is inert — it parses to an
//! empty overlay, so the live defaults stay authoritative — while still
//! documenting every option *with its real default visible* next to it.
//! Uncommenting a line is the only way the file changes behavior.
//!
//! This keeps a single source of truth: [`DEFAULT_CONFIG_TOML`]. The
//! starter file is generated from it, never hand-maintained alongside.
//!
//! [`DEFAULT_CONFIG_TOML`]: crate::DEFAULT_CONFIG_TOML

use std::path::{Path, PathBuf};
use std::{fs, io};

use crate::DEFAULT_CONFIG_TOML;

/// Header prepended to the scaffolded `config.toml`, replacing the
/// embedded default's "this ships with the binary" preamble (which is
/// meaningless in a user's config dir).
const SCAFFOLD_HEADER: &str = "\
# phux configuration.
#
# This is YOUR override file. phux ships its defaults compiled into the
# binary; everything below is the shipped default, commented out. While a
# line stays commented, phux uses the built-in default (so upgrades that
# change a default still reach you). Uncomment a line and edit it to
# override that one setting — anything you leave commented keeps tracking
# the default.
#
# Run `phux config show --default` to see the live annotated defaults, or
# `phux config show` to see your effective config after overrides.

";

/// Outcome of a [`write_reference_config`] call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScaffoldOutcome {
    /// The starter config was written to this path.
    Wrote(PathBuf),
    /// A file already existed at this path and `force` was `false`, so
    /// nothing was written.
    Skipped(PathBuf),
}

/// Build the commented starter config: a fixed user-facing header
/// followed by a comment-projection of [`DEFAULT_CONFIG_TOML`]'s body.
///
/// The embedded default's own header block (its leading run of comment
/// and blank lines) is dropped; projection begins at the first
/// structural line (a table header or assignment). From there, blank
/// lines and lines that are already comments pass through verbatim;
/// every other line is prefixed with `# ` so the document is fully
/// inert.
#[must_use]
pub fn reference_config() -> String {
    let mut out = String::from(SCAFFOLD_HEADER);
    let mut in_body = false;
    for line in DEFAULT_CONFIG_TOML.lines() {
        if !in_body {
            // Skip the embedded default's preamble: leading comments and
            // blanks, up to the first real TOML line.
            let trimmed = line.trim_start();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            in_body = true;
        }
        // Comment out structural lines (assignments, table headers);
        // blank lines and existing prose comments pass through as-is.
        let trimmed = line.trim_start();
        if !(trimmed.is_empty() || trimmed.starts_with('#')) {
            out.push_str("# ");
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// Write [`reference_config`] to `path`, creating parent directories.
///
/// Refuses to clobber: if `path` already exists and `force` is `false`,
/// returns [`ScaffoldOutcome::Skipped`] and touches nothing. With
/// `force`, an existing file is overwritten.
///
/// # Errors
///
/// Returns the underlying [`io::Error`] if creating the parent directory
/// or writing the file fails.
pub fn write_reference_config(path: &Path, force: bool) -> io::Result<ScaffoldOutcome> {
    if path.exists() && !force {
        return Ok(ScaffoldOutcome::Skipped(path.to_path_buf()));
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, reference_config())?;
    Ok(ScaffoldOutcome::Wrote(path.to_path_buf()))
}
