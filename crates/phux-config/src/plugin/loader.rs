use std::path::Path;

use serde::Deserialize;

use super::source::load_manifest_source;
use super::validate::{
    non_empty, normalize_command, normalize_id, reject_duplicate_ids, trim_optional,
};
use super::{
    PluginAgentAttention, PluginAgentState, PluginManifest, PluginManifestAction,
    PluginManifestAgent, PluginManifestBuild, PluginManifestError, PluginManifestEvent,
    PluginManifestPane, PluginPanePlacement, PluginPlatform,
};

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
    agents: Vec<RawPluginManifestAgent>,
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
struct RawPluginManifestAgent {
    id: String,
    label: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    state: PluginAgentState,
    #[serde(default)]
    attention: PluginAgentAttention,
    #[serde(default)]
    contexts: Vec<String>,
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
    let source = load_manifest_source(path)?;
    let manifest_path = source.canonical_path;
    let plugin_root = manifest_path
        .parent()
        .ok_or_else(|| PluginManifestError::Invalid("manifest path has no parent".to_owned()))?
        .to_path_buf();
    let raw: RawPluginManifest =
        toml::from_str(&source.input).map_err(|err| PluginManifestError::Parse {
            path: source.display_path,
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
    let agents = raw
        .agents
        .into_iter()
        .map(normalize_agent)
        .collect::<Result<Vec<_>, _>>()?;
    reject_duplicate_ids(agents.iter().map(|agent| agent.id.as_str()), "plugin agent")?;
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
        agents,
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

fn normalize_agent(
    raw: RawPluginManifestAgent,
) -> Result<PluginManifestAgent, PluginManifestError> {
    let contexts = raw
        .contexts
        .into_iter()
        .map(|context| non_empty(&context, "plugin agent context"))
        .collect::<Result<Vec<_>, _>>()?;

    Ok(PluginManifestAgent {
        id: normalize_id(&raw.id, false, "plugin agent id")?,
        label: non_empty(&raw.label, "plugin agent label")?,
        description: raw.description.as_deref().and_then(trim_optional),
        state: raw.state,
        attention: raw.attention,
        contexts,
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
