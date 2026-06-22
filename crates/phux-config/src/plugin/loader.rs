use std::collections::BTreeSet;
use std::path::Path;

use serde::Deserialize;

use super::{
    PluginManifest, PluginManifestAction, PluginManifestBuild, PluginManifestError,
    PluginManifestEvent, PluginManifestPane, PluginPanePlacement, PluginPlatform,
};

const PLUGIN_ID_MAX_CHARS: usize = 120;
const ENTRY_ID_MAX_CHARS: usize = 120;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPluginManifest {
    id: String,
    name: String,
    version: String,
    min_phux_version: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    platforms: Option<Vec<PluginPlatform>>,
    #[serde(default)]
    build: Vec<RawPluginManifestBuild>,
    #[serde(default)]
    actions: Vec<RawPluginManifestAction>,
    #[serde(default)]
    events: Vec<RawPluginManifestEvent>,
    #[serde(default)]
    panes: Vec<RawPluginManifestPane>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPluginManifestBuild {
    #[serde(default)]
    platforms: Option<Vec<PluginPlatform>>,
    command: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPluginManifestAction {
    id: String,
    title: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    contexts: Vec<String>,
    #[serde(default)]
    platforms: Option<Vec<PluginPlatform>>,
    command: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPluginManifestEvent {
    on: String,
    #[serde(default)]
    platforms: Option<Vec<PluginPlatform>>,
    command: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPluginManifestPane {
    id: String,
    title: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    platforms: Option<Vec<PluginPlatform>>,
    #[serde(default)]
    placement: PluginPanePlacement,
    command: Vec<String>,
}

/// Load and validate a `phux-plugin.toml` manifest.
///
/// # Errors
///
/// Returns an error if the file cannot be read, cannot be parsed as TOML,
/// or violates the plugin manifest schema.
pub fn load_plugin_manifest(path: &Path) -> Result<PluginManifest, PluginManifestError> {
    let manifest_path = if path.is_dir() {
        path.join("phux-plugin.toml")
    } else {
        path.to_path_buf()
    }
    .canonicalize()?;
    let plugin_root = manifest_path
        .parent()
        .ok_or_else(|| PluginManifestError::Invalid("manifest path has no parent".to_owned()))?
        .to_path_buf();
    let input = std::fs::read_to_string(&manifest_path)?;
    let raw: RawPluginManifest =
        toml::from_str(&input).map_err(|err| PluginManifestError::Parse {
            path: manifest_path.clone(),
            message: err.message().to_owned(),
        })?;

    let id = normalize_id(&raw.id, true, "plugin id")?;
    let name = non_empty(&raw.name, "plugin name")?;
    let version = non_empty(&raw.version, "plugin version")?;
    let min_phux_version = non_empty(&raw.min_phux_version, "plugin min_phux_version")?;
    let description = raw.description.as_deref().and_then(trim_optional);

    let build = raw
        .build
        .into_iter()
        .map(normalize_build)
        .collect::<Result<Vec<_>, _>>()?;
    let actions = raw
        .actions
        .into_iter()
        .map(normalize_action)
        .collect::<Result<Vec<_>, _>>()?;
    reject_duplicate_ids(
        actions.iter().map(|action| action.id.as_str()),
        "plugin action",
    )?;
    let events = raw
        .events
        .into_iter()
        .map(normalize_event)
        .collect::<Result<Vec<_>, _>>()?;
    let panes = raw
        .panes
        .into_iter()
        .map(normalize_pane)
        .collect::<Result<Vec<_>, _>>()?;
    reject_duplicate_ids(panes.iter().map(|pane| pane.id.as_str()), "plugin pane")?;

    Ok(PluginManifest {
        id,
        name,
        version,
        min_phux_version,
        description,
        manifest_path,
        plugin_root,
        platforms: raw.platforms,
        build,
        actions,
        events,
        panes,
    })
}

fn normalize_build(
    raw: RawPluginManifestBuild,
) -> Result<PluginManifestBuild, PluginManifestError> {
    let command = normalize_command(&raw.command)?;

    Ok(PluginManifestBuild {
        platforms: raw.platforms,
        command,
    })
}

fn normalize_action(
    raw: RawPluginManifestAction,
) -> Result<PluginManifestAction, PluginManifestError> {
    let contexts = raw
        .contexts
        .iter()
        .map(|context| non_empty(context, "plugin action context"))
        .collect::<Result<Vec<_>, _>>()?;
    let command = normalize_command(&raw.command)?;

    Ok(PluginManifestAction {
        id: normalize_id(&raw.id, false, "plugin action id")?,
        title: non_empty(&raw.title, "plugin action title")?,
        description: raw.description.as_deref().and_then(trim_optional),
        contexts,
        platforms: raw.platforms,
        command,
    })
}

fn normalize_event(
    raw: RawPluginManifestEvent,
) -> Result<PluginManifestEvent, PluginManifestError> {
    let command = normalize_command(&raw.command)?;

    Ok(PluginManifestEvent {
        on: non_empty(&raw.on, "plugin event name")?,
        platforms: raw.platforms,
        command,
    })
}

fn normalize_pane(raw: RawPluginManifestPane) -> Result<PluginManifestPane, PluginManifestError> {
    let command = normalize_command(&raw.command)?;

    Ok(PluginManifestPane {
        id: normalize_id(&raw.id, false, "plugin pane id")?,
        title: non_empty(&raw.title, "plugin pane title")?,
        description: raw.description.as_deref().and_then(trim_optional),
        platforms: raw.platforms,
        placement: raw.placement,
        command,
    })
}

fn normalize_command(command: &[String]) -> Result<Vec<String>, PluginManifestError> {
    if command.is_empty() {
        return Err(PluginManifestError::Invalid(
            "plugin command must not be empty".to_owned(),
        ));
    }
    command
        .iter()
        .map(|part| non_empty(part, "plugin command part"))
        .collect()
}

fn normalize_id(value: &str, allow_dot: bool, label: &str) -> Result<String, PluginManifestError> {
    let value = value.trim();
    let valid = !value.is_empty()
        && value.len() <= PLUGIN_ID_MAX_CHARS
        && value.chars().all(|ch| {
            ch.is_ascii_alphanumeric()
                || ch == '_'
                || ch == '-'
                || ch == ':'
                || (allow_dot && ch == '.')
        });
    if valid {
        Ok(value.to_owned())
    } else {
        Err(PluginManifestError::Invalid(format!("invalid {label}")))
    }
}

fn non_empty(value: &str, label: &str) -> Result<String, PluginManifestError> {
    let value = value.trim().to_owned();
    if value.is_empty() {
        Err(PluginManifestError::Invalid(format!("{label} is required")))
    } else {
        Ok(value)
    }
}

fn trim_optional(value: &str) -> Option<String> {
    let value = value.trim().to_owned();
    (!value.is_empty()).then_some(value)
}

fn reject_duplicate_ids<'a>(
    ids: impl IntoIterator<Item = &'a str>,
    label: &str,
) -> Result<(), PluginManifestError> {
    let mut seen = BTreeSet::new();
    for id in ids {
        if id.len() > ENTRY_ID_MAX_CHARS || !seen.insert(id) {
            return Err(PluginManifestError::Invalid(format!(
                "duplicate {label} id '{id}'"
            )));
        }
    }
    Ok(())
}
