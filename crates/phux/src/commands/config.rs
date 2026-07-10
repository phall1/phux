mod config_json;

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use phux_config::loader as config_loader;
use phux_config::plugin::{self, PluginManifest};

use super::config_action::ConfigAction;
use config_json::{print_agents_json, print_plugins_json};

/// `phux config <action>` (phux-ijp). Entirely client-local: inspects
/// and scaffolds the on-disk config without contacting a server.
pub(crate) fn run_config(action: &ConfigAction) -> ExitCode {
    match action {
        ConfigAction::Path => {
            println!("{}", config_loader::config_path().display());
            ExitCode::SUCCESS
        }
        ConfigAction::Init { force } => {
            let path = config_loader::config_path();
            match phux_config::scaffold::write_reference_config(&path, *force) {
                Ok(phux_config::scaffold::ScaffoldOutcome::Wrote(p)) => {
                    println!("wrote {}", p.display());
                    ExitCode::SUCCESS
                }
                Ok(phux_config::scaffold::ScaffoldOutcome::Skipped(p)) => {
                    eprintln!(
                        "phux: {} already exists; refusing to overwrite (use --force)",
                        p.display()
                    );
                    ExitCode::FAILURE
                }
                Err(err) => {
                    eprintln!("phux: could not write config: {err}");
                    ExitCode::FAILURE
                }
            }
        }
        ConfigAction::Show {
            default,
            layers,
            json,
        } => run_config_show(*default, *layers, *json),
        ConfigAction::Plugins { json } => run_config_plugins(*json),
        ConfigAction::Agents { json } => run_config_agents(*json),
        ConfigAction::Run {
            plugin,
            action,
            timeout,
            cwd,
            json,
        } => run_config_action(plugin, action, *timeout, cwd.clone(), *json),
    }
}

/// `phux config show [--default | --layers [--json]]`.
///
/// `--default` echoes the embedded defaults verbatim, comments and all
/// — the annotated source of truth. Plain `show` renders the effective
/// merged document (defaults + `extends` layers + the user's overrides,
/// ADR-0039) as canonical TOML. `--layers` renders provenance instead:
/// which layer of the stack set each effective key, and for `-append`
/// arrays, which layer contributed each element.
fn run_config_show(default: bool, layers: bool, json: bool) -> ExitCode {
    if default {
        print!("{}", phux_config::DEFAULT_CONFIG_TOML);
        return ExitCode::SUCCESS;
    }
    let path = config_loader::config_path();
    let user_input = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(err) => {
            eprintln!("phux: could not read {}: {err}", path.display());
            return ExitCode::FAILURE;
        }
    };
    if layers {
        let provenance = match phux_config::merged_config_with_provenance(&user_input, &path) {
            Ok((_, provenance)) => provenance,
            Err(err) => {
                eprintln!("phux: {err}");
                return ExitCode::FAILURE;
            }
        };
        if json {
            return config_json::print_layers_json(&path, &provenance);
        }
        return print_layers_human(&provenance);
    }
    let merged = match phux_config::merged_config_table(&user_input, &path) {
        Ok(table) => table,
        Err(err) => {
            eprintln!("phux: {err}");
            return ExitCode::FAILURE;
        }
    };
    match toml::to_string(&merged) {
        Ok(rendered) => {
            print!("{rendered}");
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("phux: could not render config: {err}");
            ExitCode::FAILURE
        }
    }
}

/// Render the provenance view: the layer stack, then one row per
/// effective leaf key (arrays expand to one row per element) tagged
/// with the 1-based index and short name of the owning layer.
fn print_layers_human(provenance: &phux_config::ConfigProvenance) -> ExitCode {
    println!("layers (merge order; later layers win):");
    for (i, layer) in provenance.layers.iter().enumerate() {
        let label = match layer {
            phux_config::LayerSource::Defaults => "defaults (embedded)".to_owned(),
            phux_config::LayerSource::Extended(p) => p.display().to_string(),
            phux_config::LayerSource::User(p) => format!("{} (user)", p.display()),
        };
        println!("  [{}] {label}", i + 1);
    }
    println!();
    println!("keys:");
    let mut rows: Vec<(String, usize)> = Vec::new();
    for (key, origin) in &provenance.keys {
        match origin.elements.as_deref() {
            Some(elements) if !elements.is_empty() => {
                for (i, layer) in elements.iter().enumerate() {
                    rows.push((format!("{key}[{i}]"), *layer));
                }
            }
            _ => rows.push((key.clone(), origin.layer)),
        }
    }
    let width = rows.iter().map(|(key, _)| key.len()).max().unwrap_or(0);
    for (key, layer) in rows {
        let label = provenance
            .layers
            .get(layer)
            .map_or_else(|| "?".to_owned(), layer_short_label);
        println!("  {key:<width$}  <- [{}] {label}", layer + 1);
    }
    ExitCode::SUCCESS
}

/// Short per-row tag for a layer: `defaults`, the layer file's name,
/// or `user`. The 1-based index printed beside it disambiguates layers
/// whose file names collide.
fn layer_short_label(layer: &phux_config::LayerSource) -> String {
    match layer {
        phux_config::LayerSource::Defaults => "defaults".to_owned(),
        phux_config::LayerSource::Extended(p) => p.file_name().map_or_else(
            || p.display().to_string(),
            |n| n.to_string_lossy().into_owned(),
        ),
        phux_config::LayerSource::User(_) => "user".to_owned(),
    }
}

pub(super) struct LoadedPlugin {
    pub(super) enabled: bool,
    pub(super) manifest: PluginManifest,
}

fn load_configured_plugins() -> Result<Vec<LoadedPlugin>, String> {
    let path = config_loader::config_path();
    let cfg = match config_loader::load_from(&path) {
        Ok(cfg) => cfg,
        Err(err) => return Err(err.to_string()),
    };
    let mut loaded = Vec::new();
    for entry in cfg.plugins {
        let manifest_path = resolve_manifest_path(&entry.manifest, &path);
        let manifest = match plugin::load_plugin_manifest(&manifest_path) {
            Ok(manifest) => manifest,
            Err(err) => {
                return Err(format!("could not load {}: {err}", manifest_path.display()));
            }
        };
        loaded.push(LoadedPlugin {
            enabled: entry.enabled,
            manifest,
        });
    }
    Ok(loaded)
}

fn run_config_plugins(json: bool) -> ExitCode {
    let loaded = match load_configured_plugins() {
        Ok(loaded) => loaded,
        Err(err) => {
            eprintln!("phux: {err}");
            return ExitCode::FAILURE;
        }
    };
    if json {
        return print_plugins_json(&loaded);
    }
    for plugin in loaded {
        let state = if plugin.enabled {
            "enabled"
        } else {
            "disabled"
        };
        let manifest = plugin.manifest;
        println!("{} {} ({state})", manifest.id, manifest.version);
    }
    ExitCode::SUCCESS
}

fn run_config_agents(json: bool) -> ExitCode {
    let loaded = match load_configured_plugins() {
        Ok(loaded) => loaded,
        Err(err) => {
            eprintln!("phux: {err}");
            return ExitCode::FAILURE;
        }
    };
    if json {
        return print_agents_json(&loaded);
    }
    for plugin in loaded {
        let plugin_state = if plugin.enabled {
            "enabled"
        } else {
            "disabled"
        };
        for agent in plugin.manifest.agents {
            println!(
                "{}:{} {} {:?} {:?} ({plugin_state})",
                plugin.manifest.id, agent.id, agent.label, agent.state, agent.attention
            );
        }
    }
    ExitCode::SUCCESS
}

fn run_config_action(
    plugin: &str,
    action: &str,
    timeout: Option<u64>,
    cwd: Option<PathBuf>,
    json: bool,
) -> ExitCode {
    let path = config_loader::config_path();
    let timeout = timeout.map(Duration::from_secs);
    let request = phux_plugin::PluginActionRequest {
        plugin_id: plugin.to_owned(),
        action_id: action.to_owned(),
        timeout,
        cwd,
    };
    let rt = match crate::commands::cli_runtime() {
        Ok(rt) => rt,
        Err(code) => return code,
    };
    match rt.block_on(phux_plugin::run_configured_action(&path, &request)) {
        Ok(output) => print_action_output(&output, json),
        Err(err) => {
            eprintln!("phux: {err}");
            ExitCode::FAILURE
        }
    }
}

fn print_action_output(output: &phux_plugin::PluginActionOutput, json: bool) -> ExitCode {
    if json {
        return match serde_json::to_string_pretty(output) {
            Ok(rendered) => {
                println!("{rendered}");
                action_exit_code(output)
            }
            Err(err) => {
                eprintln!("phux: could not render plugin action JSON: {err}");
                ExitCode::FAILURE
            }
        };
    }
    print!("{}", output.stdout);
    eprint!("{}", output.stderr);
    action_exit_code(output)
}

fn action_exit_code(output: &phux_plugin::PluginActionOutput) -> ExitCode {
    match output.outcome {
        phux_plugin::PluginActionOutcome::Completed => output
            .exit_code
            .and_then(|code| u8::try_from(code).ok())
            .map_or(ExitCode::FAILURE, ExitCode::from),
        phux_plugin::PluginActionOutcome::TimedOut => ExitCode::from(125),
    }
}

fn resolve_manifest_path(manifest: &Path, config_path: &Path) -> PathBuf {
    if manifest.is_absolute() {
        return manifest.to_path_buf();
    }
    config_path
        .parent()
        .map_or_else(|| manifest.to_path_buf(), |parent| parent.join(manifest))
}
