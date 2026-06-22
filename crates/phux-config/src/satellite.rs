//! Satellite registry schema.

use serde::{Deserialize, Serialize};

/// A satellite declared in `config.toml`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SatelliteConfigEntry {
    /// Hub-local satellite name, used in `TerminalId::Satellite.host`.
    pub name: String,

    /// Transport endpoint URI for the satellite server.
    pub endpoint: String,

    /// Whether this satellite is active for hub routing.
    #[serde(default = "default_true")]
    pub enabled: bool,
}

const fn default_true() -> bool {
    true
}
