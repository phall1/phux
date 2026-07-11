//! Agent integration template parsing (phux-ark7, [ADR-0042]).
//!
//! An *integration template* (`integrations/<id>.toml`) is a checked-in,
//! documented package describing a terminal-native agent phux can launch,
//! detect, and supervise. It is a **different** file format from the plugin
//! manifest (`phux-plugin.toml`, see [`crate::plugin`]): a plugin *ships*
//! templates under its `integrations/` directory, and the launch executor
//! resolves a named template's `[launch]` command into a child-process argv
//! it spawns as a pane's program.
//!
//! Only the fields the launcher needs are modeled here (`id`,
//! `display_name`, `kind`, and `[launch]`); every other key a template
//! carries — `[detect]`, `[link]`, `[session_identity]`,
//! `[agent_identity]`, `capabilities`, ... — is ignored, so this stays a
//! thin, forward-compatible view over a richer package format (unknown keys
//! are **not** rejected).
//!
//! [ADR-0042]: ../../ADR/0042-launch-executor.md

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Placeholder for the owning plugin's root directory in a template's
/// `[launch] command`.
///
/// The launch executor expands it to the absolute plugin root before
/// spawning, so the argv (e.g. a wrapper-script path) resolves from any
/// working directory. Expansion is a plain string substitution into the
/// affected argv element — never a shell evaluation — so a value can carry
/// the placeholder without opening a shell-injection surface.
pub const PLUGIN_ROOT_PLACEHOLDER: &str = "${PHUX_PLUGIN_ROOT}";

/// A parsed agent integration template (`integrations/<id>.toml`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IntegrationTemplate {
    /// Stable integration id (the `phux launch <id>` name).
    pub id: String,
    /// Human-readable display name, when declared.
    pub display_name: Option<String>,
    /// Open-vocabulary kind slug (e.g. `terminal-agent`), when declared.
    pub kind: Option<String>,
    /// The `[launch]` command, when the template declares one. A template
    /// with no `[launch]` section is parseable but not launchable.
    pub launch: Option<IntegrationLaunch>,
    /// Canonical path the template was loaded from.
    pub template_path: PathBuf,
}

/// The `[launch]` section of an integration template: the argv the launch
/// executor spawns, plus where to run it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IntegrationLaunch {
    /// Command argv; `command[0]` is the program. Non-empty by validation.
    /// May contain [`PLUGIN_ROOT_PLACEHOLDER`] elements, expanded at
    /// resolution time by [`expand_launch_argv`].
    pub command: Vec<String>,
    /// Directory the launched program runs in.
    pub working_directory: LaunchWorkingDirectory,
}

/// Where a launched integration's program runs.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum LaunchWorkingDirectory {
    /// Run in the directory `phux launch` was invoked from — the user's
    /// workspace. The default: an agent should run where the human is, so a
    /// template that omits `working_directory` lands the agent in the
    /// caller's project rather than the plugin's tree.
    #[default]
    Workspace,
    /// Run in the owning plugin's root directory. Use this only when the
    /// launched program genuinely belongs to the plugin tree; agents that
    /// operate on the user's code want `workspace`.
    #[serde(rename = "plugin-root")]
    PluginRoot,
}

/// Error raised while reading or validating an integration template.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum IntegrationError {
    /// I/O failure while reading the template.
    #[error("integration template io: {0}")]
    Io(#[from] std::io::Error),
    /// TOML parse failure.
    #[error("{}: {message}", path.display())]
    Parse {
        /// Template path.
        path: PathBuf,
        /// Parse message.
        message: String,
    },
    /// Schema validation failure after TOML parsing.
    #[error("{0}")]
    Invalid(String),
}

#[derive(Debug, Deserialize)]
struct RawTemplate {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    display_name: Option<String>,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    launch: Option<RawLaunch>,
}

#[derive(Debug, Deserialize)]
struct RawLaunch {
    #[serde(default)]
    command: Vec<String>,
    #[serde(default)]
    working_directory: LaunchWorkingDirectory,
}

/// Load and validate an integration template from `path`.
///
/// # Errors
///
/// Returns an error if the file cannot be read, cannot be parsed as TOML,
/// or violates the template schema (missing `id`, or a `[launch]` section
/// whose `command` is empty or whose program is blank).
pub fn load_integration_template(path: &Path) -> Result<IntegrationTemplate, IntegrationError> {
    let text = std::fs::read_to_string(path)?;
    parse_integration_template(&text, path)
}

/// Parse and validate an integration template from in-memory `text`,
/// attributing errors to `path`.
///
/// # Errors
///
/// See [`load_integration_template`].
pub fn parse_integration_template(
    text: &str,
    path: &Path,
) -> Result<IntegrationTemplate, IntegrationError> {
    let raw: RawTemplate = toml::from_str(text).map_err(|err| IntegrationError::Parse {
        path: path.to_path_buf(),
        message: err.message().to_owned(),
    })?;

    let id = raw
        .id
        .map(|id| id.trim().to_owned())
        .filter(|id| !id.is_empty())
        .ok_or_else(|| {
            IntegrationError::Invalid(format!(
                "{}: integration template is missing a non-empty `id`",
                path.display()
            ))
        })?;

    let launch = raw
        .launch
        .map(|raw_launch| build_launch(&id, path, raw_launch))
        .transpose()?;

    Ok(IntegrationTemplate {
        id,
        display_name: raw.display_name.as_deref().and_then(trim_optional),
        kind: raw.kind.as_deref().and_then(trim_optional),
        launch,
        template_path: path.to_path_buf(),
    })
}

fn build_launch(
    id: &str,
    path: &Path,
    raw: RawLaunch,
) -> Result<IntegrationLaunch, IntegrationError> {
    if raw.command.is_empty() {
        return Err(IntegrationError::Invalid(format!(
            "{}: integration {id:?} `[launch] command` must be a non-empty argv",
            path.display()
        )));
    }
    if raw.command[0].trim().is_empty() {
        return Err(IntegrationError::Invalid(format!(
            "{}: integration {id:?} `[launch] command[0]` (the program) must not be blank",
            path.display()
        )));
    }
    Ok(IntegrationLaunch {
        command: raw.command,
        working_directory: raw.working_directory,
    })
}

fn trim_optional(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_owned())
    }
}

/// Expand a launch `command` into a spawnable argv: every
/// [`PLUGIN_ROOT_PLACEHOLDER`] occurrence is replaced with `plugin_root`,
/// then `extra_args` are appended verbatim.
///
/// This is a pure, per-element string substitution — no shell, no globbing,
/// no word-splitting — so a template value that embeds the placeholder
/// stays a single argv element and untrusted content cannot inject extra
/// arguments or commands.
#[must_use]
pub fn expand_launch_argv(
    command: &[String],
    plugin_root: &Path,
    extra_args: &[String],
) -> Vec<String> {
    let root = plugin_root.display().to_string();
    let mut argv: Vec<String> = command
        .iter()
        .map(|part| part.replace(PLUGIN_ROOT_PLACEHOLDER, &root))
        .collect();
    argv.extend(extra_args.iter().cloned());
    argv
}

#[cfg(test)]
mod tests {
    use super::*;

    const CLAUDE: &str = r#"
schema_version = 1
id = "claude-code"
display_name = "Claude Code"
kind = "terminal-agent"
first_party = true
capabilities = ["terminal-control"]

[detect]
mode = "opt-in"
command = "claude"

[agent_identity]
name = "claude"
kind = "claude"

[launch]
command = ["sh", "${PHUX_PLUGIN_ROOT}/scripts/phux-agent-wrap.sh", "--name", "claude", "--kind", "claude", "--", "claude"]
working_directory = "workspace"
"#;

    fn parse(text: &str) -> Result<IntegrationTemplate, IntegrationError> {
        parse_integration_template(text, Path::new("claude-code.toml"))
    }

    #[test]
    fn parses_launch_and_ignores_unmodeled_keys() {
        let template = parse(CLAUDE).expect("valid template parses");
        assert_eq!(template.id, "claude-code");
        assert_eq!(template.display_name.as_deref(), Some("Claude Code"));
        assert_eq!(template.kind.as_deref(), Some("terminal-agent"));
        let launch = template.launch.expect("launch present");
        assert_eq!(launch.command[0], "sh");
        assert_eq!(launch.command.last().unwrap(), "claude");
        assert_eq!(launch.working_directory, LaunchWorkingDirectory::Workspace);
    }

    #[test]
    fn working_directory_defaults_to_workspace_when_absent() {
        let template = parse(
            r#"
id = "bare"
[launch]
command = ["sh", "-c", "true"]
"#,
        )
        .expect("valid");
        assert_eq!(
            template.launch.unwrap().working_directory,
            LaunchWorkingDirectory::Workspace
        );
    }

    #[test]
    fn plugin_root_working_directory_parses() {
        let template = parse(
            r#"
id = "bare"
[launch]
command = ["sh"]
working_directory = "plugin-root"
"#,
        )
        .expect("valid");
        assert_eq!(
            template.launch.unwrap().working_directory,
            LaunchWorkingDirectory::PluginRoot
        );
    }

    #[test]
    fn template_without_launch_is_valid_but_not_launchable() {
        let template = parse(
            r#"
id = "detect-only"
display_name = "Detect Only"
"#,
        )
        .expect("valid");
        assert!(template.launch.is_none());
    }

    #[test]
    fn missing_id_is_rejected() {
        let err = parse(
            r#"
display_name = "No Id"
[launch]
command = ["sh"]
"#,
        )
        .expect_err("missing id");
        assert!(matches!(err, IntegrationError::Invalid(_)));
    }

    #[test]
    fn empty_launch_command_is_rejected() {
        let err = parse(
            r#"
id = "empty"
[launch]
command = []
"#,
        )
        .expect_err("empty command");
        assert!(matches!(err, IntegrationError::Invalid(_)));
    }

    #[test]
    fn blank_program_is_rejected() {
        let err = parse(
            r#"
id = "blank"
[launch]
command = ["  ", "arg"]
"#,
        )
        .expect_err("blank program");
        assert!(matches!(err, IntegrationError::Invalid(_)));
    }

    #[test]
    fn expand_substitutes_plugin_root_and_appends_extra_args() {
        let command = vec![
            "sh".to_owned(),
            "${PHUX_PLUGIN_ROOT}/scripts/wrap.sh".to_owned(),
            "--".to_owned(),
            "codex".to_owned(),
        ];
        let argv = expand_launch_argv(
            &command,
            Path::new("/opt/plugins/agent-tools"),
            &["--resume".to_owned()],
        );
        assert_eq!(
            argv,
            vec![
                "sh",
                "/opt/plugins/agent-tools/scripts/wrap.sh",
                "--",
                "codex",
                "--resume",
            ]
        );
    }

    /// The placeholder expansion is a per-element replace, so a value that
    /// contains shell metacharacters (or the placeholder mid-string) stays
    /// exactly one argv element — it can never split into extra arguments.
    #[test]
    fn expansion_is_injection_safe_per_element() {
        let command = vec![
            "sh".to_owned(),
            "${PHUX_PLUGIN_ROOT}/a b; rm -rf ~".to_owned(),
        ];
        let argv = expand_launch_argv(&command, Path::new("/root"), &[]);
        assert_eq!(argv.len(), 2);
        assert_eq!(argv[1], "/root/a b; rm -rf ~");
    }
}
