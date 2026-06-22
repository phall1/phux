use std::path::{Path, PathBuf};
use std::process::ExitCode;

use phux_config::loader as config_loader;
use phux_config::plugin::{self, PluginManifest};

use crate::commands::ConfigAction;

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
        ConfigAction::Show { default } => {
            // `--default` echoes the embedded defaults verbatim, comments
            // and all — the annotated source of truth. Plain `show`
            // renders the effective merged document (defaults + the user's
            // overrides) as canonical TOML.
            if *default {
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
        ConfigAction::Plugins { json } => run_config_plugins(*json),
    }
}

fn run_config_plugins(json: bool) -> ExitCode {
    let path = config_loader::config_path();
    let cfg = match config_loader::load_from(&path) {
        Ok(cfg) => cfg,
        Err(err) => {
            eprintln!("phux: {err}");
            return ExitCode::FAILURE;
        }
    };
    let mut loaded = Vec::new();
    for entry in cfg.plugins {
        let manifest_path = resolve_manifest_path(&entry.manifest, &path);
        let manifest = match plugin::load_plugin_manifest(&manifest_path) {
            Ok(manifest) => manifest,
            Err(err) => {
                eprintln!("phux: could not load {}: {err}", manifest_path.display());
                return ExitCode::FAILURE;
            }
        };
        loaded.push((entry.enabled, manifest));
    }
    if json {
        return print_plugins_json(&loaded);
    }
    for (enabled, manifest) in loaded {
        let state = if enabled { "enabled" } else { "disabled" };
        println!("{} {} ({state})", manifest.id, manifest.version);
    }
    ExitCode::SUCCESS
}

fn resolve_manifest_path(manifest: &Path, config_path: &Path) -> PathBuf {
    if manifest.is_absolute() {
        return manifest.to_path_buf();
    }
    config_path
        .parent()
        .map_or_else(|| manifest.to_path_buf(), |parent| parent.join(manifest))
}

fn print_plugins_json(plugins: &[(bool, PluginManifest)]) -> ExitCode {
    let plugins: Vec<_> = plugins
        .iter()
        .map(|(enabled, manifest)| {
            serde_json::json!({
                "id": manifest.id,
                "name": manifest.name,
                "version": manifest.version,
                "min_phux_version": manifest.min_phux_version,
                "description": manifest.description,
                "manifest_path": manifest.manifest_path,
                "plugin_root": manifest.plugin_root,
                "enabled": enabled,
                "platforms": manifest.platforms,
                "build": manifest.build,
                "actions": manifest.actions,
                "events": manifest.events,
                "panes": manifest.panes,
            })
        })
        .collect();
    let doc = serde_json::json!({
        "schema_version": 1,
        "plugins": plugins,
    });
    match serde_json::to_string_pretty(&doc) {
        Ok(rendered) => {
            println!("{rendered}");
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("phux: could not render plugins JSON: {err}");
            ExitCode::FAILURE
        }
    }
}
