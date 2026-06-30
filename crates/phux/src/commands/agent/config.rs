use phux_config::plugin::PluginManifestAgent;

use super::model::PluginAgent;

pub(super) fn configured_agents() -> Vec<PluginAgent> {
    let path = phux_config::loader::config_path();
    let Ok(cfg) = phux_config::loader::load_from(&path) else {
        return Vec::new();
    };
    let mut agents = Vec::new();
    for entry in cfg.plugins {
        let manifest_path = if entry.manifest.is_absolute() {
            entry.manifest
        } else {
            path.parent().map_or_else(
                || entry.manifest.clone(),
                |parent| parent.join(&entry.manifest),
            )
        };
        let Ok(manifest) = phux_config::plugin::load_plugin_manifest(&manifest_path) else {
            continue;
        };
        if entry.enabled {
            agents.extend(manifest.agents.iter().map(plugin_agent));
        }
    }
    agents
}

fn plugin_agent(agent: &PluginManifestAgent) -> PluginAgent {
    PluginAgent {
        id: agent.id.clone(),
        label: agent.label.clone(),
        state: agent.state,
        attention: agent.attention,
    }
}
