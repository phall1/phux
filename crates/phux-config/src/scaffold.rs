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

/// Header prepended by [`distro_reference_config`], ahead of the active
/// `extends` line. `{distro}` is substituted with the layer path.
const DISTRO_SCAFFOLD_HEADER: &str = "\
# phux configuration, scaffolded on top of a starter distribution.
#
# The `extends` line below layers the distro at
#   {distro}
# between phux's built-in defaults and this file (defaults <- distro <-
# you; see docs/CONFIG.md \"Layered configs\"). Every key you leave unset
# tracks the distro, and every key the distro leaves unset tracks the
# shipped default — so updates to either keep reaching you. Uncomment a
# line below and edit it to override that one setting.
#
# Run `phux config show` to see the effective merged config.

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
    out.push_str(&commented_default_body());
    out
}

/// Build the distro-flavored starter config: a header naming the distro,
/// one **active** `extends` line pointing at `distro`, then the same
/// comment-projection body as [`reference_config`].
///
/// The `extends` line is the only live statement in the file — the rest
/// stays inert, so the distro and the shipped defaults remain
/// authoritative until the user uncomments an override. `distro` should
/// be absolute (see `crate::distro::resolve_distro`): the scaffolded
/// file lives in the user's config directory, not next to the distro.
#[must_use]
#[allow(
    clippy::literal_string_with_formatting_args,
    reason = "`{distro}` is this scaffold's own template placeholder, not a std format arg"
)]
pub fn distro_reference_config(distro: &Path) -> String {
    let distro_display = distro.display().to_string();
    let mut out = DISTRO_SCAFFOLD_HEADER.replace("{distro}", &distro_display);
    out.push_str("extends = [");
    out.push_str(&toml_basic_string(&distro_display));
    out.push_str("]\n\n");
    out.push_str(&commented_default_body());
    out
}

/// Quote `s` as a TOML basic string (escaping `"`, `\`, and control
/// characters), so arbitrary filesystem paths survive the scaffold.
fn toml_basic_string(s: &str) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if u32::from(c) < 0x20 => {
                // Control characters in a path are pathological, but a
                // scaffold must never emit unparseable TOML.
                let _ = write!(out, "\\u{:04X}", u32::from(c));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// The comment-projection of [`DEFAULT_CONFIG_TOML`]'s body shared by
/// both scaffold flavors (see [`reference_config`] for the rules).
fn commented_default_body() -> String {
    let mut out = String::new();
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
    write_scaffold(path, &reference_config(), force)
}

/// Write an already-rendered scaffold (e.g. [`distro_reference_config`])
/// to `path`, creating parent directories, with the same
/// refuse-to-clobber contract as [`write_reference_config`].
///
/// # Errors
///
/// Returns the underlying [`io::Error`] if creating the parent directory
/// or writing the file fails.
pub fn write_scaffold(path: &Path, contents: &str, force: bool) -> io::Result<ScaffoldOutcome> {
    if path.exists() && !force {
        return Ok(ScaffoldOutcome::Skipped(path.to_path_buf()));
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, contents)?;
    Ok(ScaffoldOutcome::Wrote(path.to_path_buf()))
}
