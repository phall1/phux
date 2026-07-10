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

// ---------------------------------------------------------------------------
// phux-r82.6: [[widgets]] status-bar contributions
// ---------------------------------------------------------------------------

#[test]
fn plugin_manifest_loads_widget_contributions() -> Result<(), Box<dyn std::error::Error>> {
    let dir = TempDir::new()?;
    let manifest = write_manifest(
        &dir,
        r#"
id = "example.battery"
name = "Battery"
version = "0.1.0"
min_phux_version = "0.0.2"

[[widgets]]
id = "battery"
slot = "right"
kind = "exec"
command = "battery.sh"
interval = "30s"

[[widgets]]
id = "branch"
kind = "exec"
command = ["git-branch-widget"]
"#,
    )?;

    let loaded = plugin::load_plugin_manifest(&manifest)?;

    assert_eq!(loaded.widgets.len(), 2);
    assert_eq!(loaded.widgets[0].id, "battery");
    assert_eq!(loaded.widgets[0].kind, "exec");
    assert_eq!(
        loaded.widgets[0].slot,
        plugin::PluginWidgetSlot::Right,
        "explicit slot"
    );
    assert_eq!(
        loaded.widgets[0].opts.get("interval"),
        Some(&toml::Value::String("30s".to_owned())),
        "kind-specific options ride the flattened opts map"
    );
    assert_eq!(
        loaded.widgets[1].slot,
        plugin::PluginWidgetSlot::Right,
        "slot defaults to right"
    );
    Ok(())
}

#[test]
fn plugin_manifest_rejects_duplicate_widget_ids() -> Result<(), Box<dyn std::error::Error>> {
    let dir = TempDir::new()?;
    let manifest = write_manifest(
        &dir,
        r#"
id = "example.dup"
name = "Dup"
version = "0.1.0"
min_phux_version = "0.0.2"

[[widgets]]
id = "w"
kind = "exec"
command = "a"

[[widgets]]
id = "w"
kind = "exec"
command = "b"
"#,
    )?;
    assert!(plugin::load_plugin_manifest(&manifest).is_err());
    Ok(())
}

#[test]
fn merge_widget_contributions_appends_after_user_widgets_and_drops_invalid()
-> Result<(), Box<dyn std::error::Error>> {
    use phux_config::widget::WidgetRegistry;
    use phux_config::{StatusCfg, Widget};

    let dir = TempDir::new()?;
    let manifest = write_manifest(
        &dir,
        r#"
id = "example.mixed"
name = "Mixed"
version = "0.1.0"
min_phux_version = "0.0.2"

[[widgets]]
id = "ok"
slot = "left"
kind = "exec"
command = "ok.sh"

[[widgets]]
id = "bad-kind"
kind = "no-such-widget"

[[widgets]]
id = "bad-opts"
kind = "exec"
interval = "30s"
"#,
    )?;
    let loaded = plugin::load_plugin_manifest(&manifest)?;

    let mut status = StatusCfg {
        left: vec![Widget::Bare("session-name".to_owned())],
        ..StatusCfg::default()
    };
    plugin::merge_widget_contributions(
        &mut status,
        std::slice::from_ref(&loaded),
        &WidgetRegistry::with_builtins(),
    );

    // The valid contribution appended AFTER the user's widget; the unknown
    // kind and the command-less exec were both dropped.
    assert_eq!(status.left.len(), 2);
    assert!(matches!(&status.left[0], Widget::Bare(k) if k == "session-name"));
    match &status.left[1] {
        Widget::Spec(spec) => assert_eq!(spec.kind, "exec"),
        other @ Widget::Bare(_) => panic!("expected contributed spec, got {other:?}"),
    }
    assert!(status.center.is_empty() && status.right.is_empty());
    Ok(())
}

/// The `min_phux_version` gate (phux-r82.2): a manifest whose floor is at
/// or below the current phux version loads; equality is the boundary case.
#[test]
fn manifest_at_current_phux_version_loads() -> Result<(), Box<dyn std::error::Error>> {
    let dir = TempDir::new()?;
    let manifest = write_manifest(
        &dir,
        &format!(
            r#"
id = "example.current"
name = "Current"
version = "0.1.0"
min_phux_version = "{}"
"#,
            plugin::CURRENT_PHUX_VERSION
        ),
    )?;

    let loaded = plugin::load_plugin_manifest(&manifest)?;

    assert_eq!(loaded.id, "example.current");
    assert_eq!(loaded.min_phux_version, plugin::CURRENT_PHUX_VERSION);
    Ok(())
}

/// A manifest demanding a future phux is rejected at load time with an
/// error naming the plugin, its floor, and the running version — so both
/// `phux plugin link` (which loads the manifest) and every load-time
/// consumer see the same clear refusal.
#[test]
fn manifest_requiring_future_phux_is_rejected() -> Result<(), Box<dyn std::error::Error>> {
    let dir = TempDir::new()?;
    let manifest = write_manifest(
        &dir,
        r#"
id = "example.future"
name = "Future"
version = "0.1.0"
min_phux_version = "99.0.0"
"#,
    )?;

    let err = plugin::load_plugin_manifest(&manifest).expect_err("future floor must be rejected");

    let message = err.to_string();
    assert!(message.contains("example.future"), "{message}");
    assert!(message.contains("99.0.0"), "{message}");
    assert!(message.contains(plugin::CURRENT_PHUX_VERSION), "{message}");
    Ok(())
}

/// A `min_phux_version` that is not a dotted numeric version is a schema
/// error, not a silent pass.
#[test]
fn manifest_with_malformed_min_phux_version_is_rejected() -> Result<(), Box<dyn std::error::Error>>
{
    let dir = TempDir::new()?;
    let manifest = write_manifest(
        &dir,
        r#"
id = "example.badfloor"
name = "Bad Floor"
version = "0.1.0"
min_phux_version = "latest"
"#,
    )?;

    let err =
        plugin::load_plugin_manifest(&manifest).expect_err("malformed floor must be rejected");

    let message = err.to_string();
    assert!(message.contains("malformed min_phux_version"), "{message}");
    Ok(())
}

/// The best-effort batch loader skips (never propagates) a plugin gated
/// out by `min_phux_version`, so one too-new plugin cannot take down the
/// TUI or server consuming the healthy ones.
#[test]
fn load_enabled_manifests_skips_version_gated_plugin() -> Result<(), Box<dyn std::error::Error>> {
    let dir = TempDir::new()?;
    let config_path = dir.path().join("config.toml");
    let good = dir.path().join("good.toml");
    std::fs::write(
        &good,
        r#"
id = "example.good-floor"
name = "Good"
version = "0.1.0"
min_phux_version = "0.0.1"
"#,
    )?;
    let future = dir.path().join("future.toml");
    std::fs::write(
        &future,
        r#"
id = "example.future-floor"
name = "Future"
version = "0.1.0"
min_phux_version = "99.0.0"
"#,
    )?;

    let entries = vec![
        plugin::PluginConfigEntry {
            manifest: good,
            enabled: true,
        },
        plugin::PluginConfigEntry {
            manifest: future,
            enabled: true,
        },
    ];

    let manifests = plugin::load_enabled_manifests(&config_path, &entries);

    assert_eq!(manifests.len(), 1, "the gated plugin is skipped");
    assert_eq!(manifests[0].id, "example.good-floor");
    Ok(())
}
