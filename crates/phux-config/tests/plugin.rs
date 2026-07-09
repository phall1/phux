use std::path::Path;

use tempfile::TempDir;

use phux_config::{Config, parse_str, plugin};

fn write_manifest(dir: &TempDir, body: &str) -> Result<std::path::PathBuf, std::io::Error> {
    let path = dir.path().join("phux-plugin.toml");
    std::fs::write(&path, body)?;
    Ok(path)
}

#[test]
fn checked_in_provider_showcase_manifest_loads() -> Result<(), Box<dyn std::error::Error>> {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("examples/plugins/provider-showcase/phux-plugin.toml");

    let loaded = plugin::load_plugin_manifest(&manifest)?;

    assert_eq!(loaded.id, "com.phux.demo.provider-showcase");
    assert_eq!(loaded.events[0].id, "idle");
    assert_eq!(loaded.panes[0].id, "board");
    assert_eq!(loaded.links[0].id, "ticket");
    assert_eq!(loaded.workspaces[0].id, "ops-bench");
    assert_eq!(loaded.workspaces[0].panes[0].pane, "board");
    Ok(())
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

[[agents]]
id = "codex"
label = "Codex"
state = "blocked"
attention = "high"
contexts = ["workspace"]

[[events]]
id = "idle"
title = "Pane idle"
on = "pane.idle"
command = ["sh", "-c", "printf idle"]

[[panes]]
id = "board"
title = "Agent Board"
placement = "split"
command = ["agent-board"]

[[links]]
id = "ticket"
title = "Open ticket"
contexts = ["pane"]
schemes = ["https"]
patterns = ["https://linear.app/*"]
command = ["open", "{url}"]

[[workspaces]]
id = "agent-bench"
title = "Agent Bench"
description = "Restore and supervise the agent bench"
contexts = ["workspace"]
agents = ["codex"]
actions = ["summarize"]
events = ["idle"]

[[workspaces.panes]]
id = "board-role"
pane = "board"
role = "board"
"#,
    )?;

    let loaded = plugin::load_plugin_manifest(&manifest)?;

    assert_eq!(loaded.id, "example.agent-tools");
    assert_eq!(loaded.plugin_root, dir.path().canonicalize()?);
    assert_eq!(loaded.actions[0].id, "summarize");
    assert_eq!(loaded.agents[0].id, "codex");
    assert_eq!(loaded.agents[0].state, plugin::PluginAgentState::Blocked);
    assert_eq!(
        loaded.agents[0].attention,
        plugin::PluginAgentAttention::High
    );
    assert_eq!(loaded.events[0].id, "idle");
    assert_eq!(loaded.events[0].on, "pane.idle");
    assert_eq!(
        loaded.panes[0].placement,
        plugin::PluginPanePlacement::Split
    );
    assert_eq!(loaded.links[0].id, "ticket");
    assert_eq!(loaded.links[0].schemes, ["https"]);
    assert_eq!(loaded.workspaces[0].id, "agent-bench");
    assert_eq!(loaded.workspaces[0].title, "Agent Bench");
    assert_eq!(loaded.workspaces[0].agents, ["codex"]);
    assert_eq!(loaded.workspaces[0].actions, ["summarize"]);
    assert_eq!(loaded.workspaces[0].events, ["idle"]);
    assert_eq!(loaded.workspaces[0].panes[0].id, "board-role");
    assert_eq!(loaded.workspaces[0].panes[0].pane, "board");
    assert_eq!(loaded.workspaces[0].panes[0].role, "board");
    Ok(())
}

#[test]
fn plugin_manifest_rejects_workspace_unknown_pane_ref() -> Result<(), Box<dyn std::error::Error>> {
    let dir = TempDir::new()?;
    let manifest = write_manifest(
        &dir,
        r#"
id = "example.bad-workspace"
name = "Bad Workspace"
version = "0.1.0"
min_phux_version = "0.0.2"

[[workspaces]]
id = "bench"
title = "Bench"

[[workspaces.panes]]
id = "missing"
pane = "not-declared"
role = "lead"
"#,
    )?;

    let Err(err) = plugin::load_plugin_manifest(&manifest) else {
        return Err("workspace with unknown pane ref loaded successfully".into());
    };
    assert!(
        err.to_string()
            .contains("workspace bench references unknown pane 'not-declared'"),
        "error should name missing workspace pane reference; got {err}"
    );
    Ok(())
}

#[test]
fn plugin_manifest_defaults_agent_state_to_unknown() -> Result<(), Box<dyn std::error::Error>> {
    let dir = TempDir::new()?;
    let manifest = write_manifest(
        &dir,
        r#"
id = "example.agent-state"
name = "Agent State"
version = "0.1.0"
min_phux_version = "0.0.2"

[[agents]]
id = "background-worker"
label = "Background Worker"
"#,
    )?;

    let loaded = plugin::load_plugin_manifest(&manifest)?;

    assert_eq!(loaded.agents[0].state, plugin::PluginAgentState::Unknown);
    assert_eq!(
        loaded.agents[0].attention,
        plugin::PluginAgentAttention::Normal
    );
    Ok(())
}

#[test]
fn plugin_manifest_rejects_duplicate_agent_ids() -> Result<(), Box<dyn std::error::Error>> {
    let dir = TempDir::new()?;
    let manifest = write_manifest(
        &dir,
        r#"
id = "example.dup-agents"
name = "Duplicate Agents"
version = "0.1.0"
min_phux_version = "0.0.2"

[[agents]]
id = "codex"
label = "Codex"

[[agents]]
id = "codex"
label = "Codex again"
"#,
    )?;

    let Err(err) = plugin::load_plugin_manifest(&manifest) else {
        return Err("duplicate agent manifest loaded successfully".into());
    };
    assert!(
        err.to_string().contains("duplicate plugin agent id"),
        "error should name duplicate agent id; got {err}"
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
fn plugin_manifest_rejects_duplicate_provider_ids() -> Result<(), Box<dyn std::error::Error>> {
    let dir = TempDir::new()?;
    let manifest = write_manifest(
        &dir,
        r#"
id = "example.dup-providers"
name = "Duplicate Providers"
version = "0.1.0"
min_phux_version = "0.0.2"

[[events]]
id = "idle"
title = "Idle"
on = "pane.idle"
command = ["true"]

[[events]]
id = "idle"
title = "Idle again"
on = "pane.idle"
command = ["true"]
"#,
    )?;

    let Err(err) = plugin::load_plugin_manifest(&manifest) else {
        return Err("duplicate event provider manifest loaded successfully".into());
    };
    assert!(
        err.to_string().contains("duplicate plugin event id"),
        "error should name duplicate event provider id; got {err}"
    );
    Ok(())
}

#[test]
fn plugin_manifest_rejects_malformed_link_provider_ids() -> Result<(), Box<dyn std::error::Error>> {
    let dir = TempDir::new()?;
    let manifest = write_manifest(
        &dir,
        r#"
id = "example.bad-link"
name = "Bad Link"
version = "0.1.0"
min_phux_version = "0.0.2"

[[links]]
id = "bad link"
title = "Bad"
schemes = ["https"]
command = ["true"]
"#,
    )?;

    let Err(err) = plugin::load_plugin_manifest(&manifest) else {
        return Err("malformed link provider manifest loaded successfully".into());
    };
    assert!(
        err.to_string().contains("invalid plugin link handler id"),
        "error should name malformed link provider id; got {err}"
    );
    Ok(())
}

#[test]
fn plugin_manifest_rejects_link_provider_without_matchers() -> Result<(), Box<dyn std::error::Error>>
{
    let dir = TempDir::new()?;
    let manifest = write_manifest(
        &dir,
        r#"
id = "example.no-link-matchers"
name = "No Link Matchers"
version = "0.1.0"
min_phux_version = "0.0.2"

[[links]]
id = "ticket"
title = "Ticket"
command = ["true"]
"#,
    )?;

    let Err(err) = plugin::load_plugin_manifest(&manifest) else {
        return Err("link provider without matchers loaded successfully".into());
    };
    assert!(
        err.to_string()
            .contains("requires at least one scheme or pattern"),
        "error should name missing link provider matcher; got {err}"
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

#[test]
fn plugin_action_keys_field_parses_and_defaults_to_none() -> Result<(), Box<dyn std::error::Error>>
{
    let dir = TempDir::new()?;
    let manifest = write_manifest(
        &dir,
        r#"
id = "example.keys"
name = "Keys"
version = "0.1.0"
min_phux_version = "0.0.2"

[[actions]]
id = "bound"
title = "Bound action"
command = ["true"]
keys = "g"

[[actions]]
id = "unbound"
title = "Unbound action"
command = ["true"]

[[actions]]
id = "blank"
title = "Blank keys action"
command = ["true"]
keys = "   "
"#,
    )?;

    let loaded = plugin::load_plugin_manifest(&manifest)?;

    assert_eq!(loaded.actions[0].keys.as_deref(), Some("g"));
    assert_eq!(loaded.actions[1].keys, None, "keys defaults to None");
    assert_eq!(
        loaded.actions[2].keys, None,
        "whitespace-only keys normalizes to None"
    );
    Ok(())
}

#[test]
fn load_enabled_manifests_skips_disabled_and_broken_plugins()
-> Result<(), Box<dyn std::error::Error>> {
    let dir = TempDir::new()?;
    let config_path = dir.path().join("config.toml");
    // Three manifests: one healthy + enabled, one healthy + disabled, one
    // missing entirely. Only the first must load; the rest are skipped
    // without failing the batch.
    let good = dir.path().join("good.toml");
    std::fs::write(
        &good,
        r#"
id = "example.good"
name = "Good"
version = "0.1.0"
min_phux_version = "0.0.2"

[[actions]]
id = "act"
title = "Act"
command = ["true"]
"#,
    )?;
    let off = dir.path().join("off.toml");
    std::fs::write(
        &off,
        r#"
id = "example.off"
name = "Off"
version = "0.1.0"
min_phux_version = "0.0.2"
"#,
    )?;

    let entries = vec![
        plugin::PluginConfigEntry {
            // Relative path: resolves against the config file's directory.
            manifest: std::path::PathBuf::from("good.toml"),
            enabled: true,
        },
        plugin::PluginConfigEntry {
            manifest: off,
            enabled: false,
        },
        plugin::PluginConfigEntry {
            manifest: dir.path().join("missing.toml"),
            enabled: true,
        },
    ];

    let manifests = plugin::load_enabled_manifests(&config_path, &entries);

    assert_eq!(manifests.len(), 1, "only the enabled, healthy manifest");
    assert_eq!(manifests[0].id, "example.good");
    Ok(())
}
