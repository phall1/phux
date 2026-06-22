//! Declarative plugin manifest parsing for phux config consumers.

mod loader;
mod source;
mod validate;

use std::path::PathBuf;

pub use loader::load_plugin_manifest;
use serde::{Deserialize, Serialize};

/// A plugin declared in `config.toml`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PluginConfigEntry {
    /// Path to a `phux-plugin.toml` manifest.
    pub manifest: PathBuf,
    /// Whether this plugin is active for consumers that execute plugins.
    #[serde(default = "default_true")]
    pub enabled: bool,
}

/// Parsed `phux-plugin.toml` manifest.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PluginManifest {
    /// Globally unique plugin id.
    pub id: String,
    /// Human-readable plugin name.
    pub name: String,
    /// Plugin package version.
    pub version: String,
    /// Oldest phux version the manifest targets.
    pub min_phux_version: String,
    /// Optional human-readable summary.
    pub description: Option<String>,
    /// Canonical manifest path.
    pub manifest_path: PathBuf,
    /// Directory containing the manifest.
    pub plugin_root: PathBuf,
    /// Supported platforms, when declared.
    pub platforms: Option<Vec<PluginPlatform>>,
    /// Build commands declared by the plugin.
    pub build: Vec<PluginManifestBuild>,
    /// Action entrypoints declared by the plugin.
    pub actions: Vec<PluginManifestAction>,
    /// Event hook entrypoints declared by the plugin.
    pub events: Vec<PluginManifestEvent>,
    /// Pane entrypoints declared by the plugin.
    pub panes: Vec<PluginManifestPane>,
}

/// Platform names accepted in plugin manifests.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "kebab-case")]
pub enum PluginPlatform {
    /// Linux.
    Linux,
    /// macOS.
    Macos,
    /// Windows.
    Windows,
}

/// Build command declared in a plugin manifest.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PluginManifestBuild {
    /// Optional platform override for this build step.
    pub platforms: Option<Vec<PluginPlatform>>,
    /// Command argv to execute.
    pub command: Vec<String>,
}

/// Action entrypoint declared in a plugin manifest.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PluginManifestAction {
    /// Plugin-local action id.
    pub id: String,
    /// Human-readable action title.
    pub title: String,
    /// Optional human-readable description.
    pub description: Option<String>,
    /// Context names where this action is relevant.
    pub contexts: Vec<String>,
    /// Optional platform override for this action.
    pub platforms: Option<Vec<PluginPlatform>>,
    /// Command argv to execute.
    pub command: Vec<String>,
}

/// Event hook entrypoint declared in a plugin manifest.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PluginManifestEvent {
    /// Event name this hook observes.
    pub on: String,
    /// Optional platform override for this hook.
    pub platforms: Option<Vec<PluginPlatform>>,
    /// Command argv to execute.
    pub command: Vec<String>,
}

/// Pane entrypoint declared in a plugin manifest.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PluginManifestPane {
    /// Plugin-local pane id.
    pub id: String,
    /// Human-readable pane title.
    pub title: String,
    /// Optional human-readable description.
    pub description: Option<String>,
    /// Optional platform override for this pane.
    pub platforms: Option<Vec<PluginPlatform>>,
    /// Where a future runtime host should place the pane.
    pub placement: PluginPanePlacement,
    /// Command argv to execute.
    pub command: Vec<String>,
}

/// Placement requested by a plugin pane entrypoint.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum PluginPanePlacement {
    /// Temporary overlay over the focused pane.
    #[default]
    Overlay,
    /// Split next to the focused pane.
    Split,
    /// New window/tab.
    Tab,
    /// Zoomed pane view.
    Zoomed,
}

/// Error raised while reading or validating a plugin manifest.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum PluginManifestError {
    /// I/O failure while reading the manifest.
    #[error("plugin manifest io: {0}")]
    Io(#[from] std::io::Error),
    /// TOML parse failure.
    #[error("{}: {message}", path.display())]
    Parse {
        /// Manifest path.
        path: PathBuf,
        /// Parse message.
        message: String,
    },
    /// Schema validation failure after TOML parsing.
    #[error("{0}")]
    Invalid(String),
}

const fn default_true() -> bool {
    true
}
