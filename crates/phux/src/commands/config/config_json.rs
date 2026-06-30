use std::process::ExitCode;

use phux_config::plugin::PluginManifestAgent;

use super::LoadedPlugin;

pub(super) fn print_plugins_json(plugins: &[LoadedPlugin]) -> ExitCode {
    let plugins: Vec<_> = plugins.iter().map(plugin_json).collect();
    print_json(
        &serde_json::json!({
            "schema_version": 1,
            "plugins": plugins,
        }),
        "plugins",
    )
}

pub(super) fn print_agents_json(plugins: &[LoadedPlugin]) -> ExitCode {
    let agents: Vec<_> = plugins
        .iter()
        .flat_map(|plugin| {
            plugin
                .manifest
                .agents
                .iter()
                .map(|agent| agent_json(plugin, agent))
        })
        .collect();
    print_json(
        &serde_json::json!({
            "schema_version": 1,
            "agents": agents,
        }),
        "agents",
    )
}

fn plugin_json(plugin: &LoadedPlugin) -> serde_json::Value {
    let manifest = &plugin.manifest;
    serde_json::json!({
        "id": manifest.id,
        "name": manifest.name,
        "version": manifest.version,
        "min_phux_version": manifest.min_phux_version,
        "description": manifest.description,
        "manifest_path": manifest.manifest_path,
        "plugin_root": manifest.plugin_root,
        "enabled": plugin.enabled,
        "platforms": manifest.platforms,
        "build": manifest.build,
        "agents": manifest.agents,
        "actions": manifest.actions,
        "events": manifest.events,
        "panes": manifest.panes,
        "links": manifest.links,
        "workspaces": manifest.workspaces,
    })
}

fn agent_json(plugin: &LoadedPlugin, agent: &PluginManifestAgent) -> serde_json::Value {
    serde_json::json!({
        "plugin_id": plugin.manifest.id,
        "plugin_enabled": plugin.enabled,
        "id": agent.id,
        "label": agent.label,
        "description": agent.description,
        "state": agent.state,
        "attention": agent.attention,
        "contexts": agent.contexts,
    })
}

fn print_json(value: &serde_json::Value, label: &str) -> ExitCode {
    match serde_json::to_string_pretty(value) {
        Ok(rendered) => {
            println!("{rendered}");
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("phux: could not render {label} JSON: {err}");
            ExitCode::FAILURE
        }
    }
}
