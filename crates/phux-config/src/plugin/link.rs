use serde::Deserialize;

use super::validate::{non_empty, normalize_command, normalize_id};
use super::{PluginManifestError, PluginManifestLinkHandler, PluginPlatform};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawPluginManifestLinkHandler {
    id: String,
    title: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    contexts: Vec<String>,
    #[serde(default)]
    schemes: Vec<String>,
    #[serde(default)]
    patterns: Vec<String>,
    #[serde(default)]
    platforms: Option<Vec<PluginPlatform>>,
    command: Vec<String>,
}

pub(super) fn normalize_link_handler(
    raw: RawPluginManifestLinkHandler,
) -> Result<PluginManifestLinkHandler, PluginManifestError> {
    let contexts = raw
        .contexts
        .iter()
        .map(|context| non_empty(context, "plugin link handler context"))
        .collect::<Result<Vec<_>, _>>()?;
    let schemes = raw
        .schemes
        .iter()
        .map(|scheme| non_empty(scheme, "plugin link handler scheme"))
        .collect::<Result<Vec<_>, _>>()?;
    let patterns = raw
        .patterns
        .iter()
        .map(|pattern| non_empty(pattern, "plugin link handler pattern"))
        .collect::<Result<Vec<_>, _>>()?;
    if schemes.is_empty() && patterns.is_empty() {
        return Err(PluginManifestError::Invalid(
            "plugin link handler requires at least one scheme or pattern".to_owned(),
        ));
    }
    let command = normalize_command(&raw.command)?;

    Ok(PluginManifestLinkHandler {
        id: normalize_id(&raw.id, false, "plugin link handler id")?,
        title: non_empty(&raw.title, "plugin link handler title")?,
        description: raw
            .description
            .as_deref()
            .and_then(super::validate::trim_optional),
        contexts,
        schemes,
        patterns,
        platforms: raw.platforms,
        command,
    })
}
