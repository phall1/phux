use std::path::Path;

use tempfile::TempDir;

use phux_config::{Config, parse_str, plugin};

fn write_manifest(dir: &TempDir, body: &str) -> Result<std::path::PathBuf, std::io::Error> {
    let path = dir.path().join("phux-plugin.toml");
    std::fs::write(&path, body)?;
    Ok(path)
}

#[test]
fn config_accepts_plugin_manifest_entries() -> Result<(), Box<dyn std::error::Error>> {
    let input = r#"
[[plugins]]
manifest = "/tmp/phux-plugin.toml"
enabled = true
"#;

    let cfg: Config = parse_str(input, Path::new("config.toml"))?;

    assert_eq!(cfg.plugins.len(), 1);
    assert_eq!(cfg.plugins[0].manifest, Path::new("/tmp/phux-plugin.toml"));
    assert!(cfg.plugins[0].enabled);
    Ok(())
}

#[test]
fn plugin_manifest_loads_actions_events_and_panes() -> Result<(), Box<dyn std::error::Error>> {
    let dir = TempDir::new()?;
    let manifest = write_manifest(
        &dir,
        r#"
id = "example.agent-tools"
name = "Agent Tools"
version = "0.1.0"
min_phux_version = "0.0.2"
description = "Agent workflow helpers"
platforms = ["linux", "macos"]

[[actions]]
id = "summarize"
title = "Summarize pane"
contexts = ["pane"]
command = ["python3", "summarize.py"]

[[events]]
on = "pane.idle"
command = ["sh", "-c", "printf idle"]

[[panes]]
id = "board"
title = "Agent Board"
placement = "split"
command = ["agent-board"]
"#,
    )?;

    let loaded = plugin::load_plugin_manifest(&manifest)?;

    assert_eq!(loaded.id, "example.agent-tools");
    assert_eq!(loaded.plugin_root, dir.path().canonicalize()?);
    assert_eq!(loaded.actions[0].id, "summarize");
    assert_eq!(loaded.events[0].on, "pane.idle");
    assert_eq!(
        loaded.panes[0].placement,
        plugin::PluginPanePlacement::Split
    );
    Ok(())
}

#[test]
fn plugin_manifest_rejects_duplicate_action_ids() -> Result<(), Box<dyn std::error::Error>> {
    let dir = TempDir::new()?;
    let manifest = write_manifest(
        &dir,
        r#"
id = "example.dup"
name = "Duplicate"
version = "0.1.0"
min_phux_version = "0.0.2"

[[actions]]
id = "run"
title = "Run"
command = ["true"]

[[actions]]
id = "run"
title = "Run again"
command = ["true"]
"#,
    )?;

    let Err(err) = plugin::load_plugin_manifest(&manifest) else {
        return Err("duplicate action manifest loaded successfully".into());
    };
    assert!(
        err.to_string().contains("duplicate plugin action id"),
        "error should name duplicate action id; got {err}"
    );
    Ok(())
}

#[test]
fn plugin_manifest_rejects_oversized_files() -> Result<(), Box<dyn std::error::Error>> {
    let dir = TempDir::new()?;
    let manifest = dir.path().join("phux-plugin.toml");
    std::fs::write(&manifest, "x".repeat(1024 * 1024 + 1))?;

    let Err(err) = plugin::load_plugin_manifest(&manifest) else {
        return Err("oversized manifest loaded successfully".into());
    };
    assert!(
        err.to_string().contains("exceeds"),
        "error should name size limit; got {err}"
    );
    Ok(())
}

#[test]
#[cfg(unix)]
fn plugin_manifest_parse_errors_use_supplied_symlink_path() -> Result<(), Box<dyn std::error::Error>>
{
    let dir = TempDir::new()?;
    let target_dir = dir.path().join("private-target");
    std::fs::create_dir_all(&target_dir)?;
    let target = target_dir.join("phux-plugin.toml");
    std::fs::write(&target, "not valid = [")?;
    let link = dir.path().join("public-link.toml");
    std::os::unix::fs::symlink(&target, &link)?;

    let Err(err) = plugin::load_plugin_manifest(&link) else {
        return Err("malformed symlinked manifest loaded successfully".into());
    };
    let message = err.to_string();
    assert!(
        message.contains("public-link.toml"),
        "parse error should report caller-facing symlink path; got {message}"
    );
    assert!(
        !message.contains("private-target"),
        "parse error should not leak canonical target path; got {message}"
    );
    Ok(())
}
