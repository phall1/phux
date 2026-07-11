//! The bundled herdr starter distribution (phux-r82.9).
//!
//! Pins the checked-in `distros/herdr/herdr.toml` package end-to-end:
//! it resolves through `extends`, its curated values land in the typed
//! config, its `[[plugins-append]]` manifests absolutize against the
//! layer directory and point at real files, and a user config layered
//! on top overrides it per key while still composing with its appends.

#![allow(clippy::expect_used, reason = "tests")]

use std::path::{Path, PathBuf};

use phux_config::{Action, Config, parse_with_defaults};

/// Absolute path to the checked-in herdr layer.
fn herdr_layer() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("distros/herdr/herdr.toml")
        .canonicalize()
        .expect("distros/herdr/herdr.toml exists in the repo")
}

/// Parse a user config body that extends the bundled herdr layer.
fn parse_with_herdr(user_body: &str) -> Config {
    let user = format!("extends = [\"{}\"]\n{user_body}", herdr_layer().display());
    // The user config path deliberately lives far from the distro to
    // prove nothing resolves against the config directory by accident.
    parse_with_defaults(&user, Path::new("/nonexistent-config-dir/config.toml"))
        .expect("herdr stack parses")
}

#[test]
fn herdr_curated_values_land_in_the_typed_config() {
    let cfg = parse_with_herdr("");

    // Which-key-first: snappier popup, still enabled.
    assert!(cfg.keybindings.which_key);
    assert_eq!(cfg.keybindings.which_key_delay_ms, 400);

    // Curated prefix-table additions.
    assert_eq!(
        cfg.keybindings.prefix_table.get("Space"),
        Some(&Action::Bare("command-palette".to_owned()))
    );
    assert!(cfg.keybindings.prefix_table.contains_key("|"));
    assert!(cfg.keybindings.prefix_table.contains_key("-"));
    assert_eq!(
        cfg.keybindings.prefix_table.get("Tab"),
        Some(&Action::Bare("next-window".to_owned()))
    );
    // Shipped defaults survive underneath (tables merge per chord).
    assert!(
        cfg.keybindings.prefix_table.contains_key("c"),
        "shipped new-window binding must survive the herdr layer"
    );

    // Session naming opinion.
    assert_eq!(cfg.defaults.session_name_template, "${cwd-basename}");

    // Theme slots.
    assert_eq!(
        cfg.theme.slots.get("accent").map(String::as_str),
        Some("#7aa2f7")
    );
    assert_eq!(
        cfg.theme.slots.get("attention").map(String::as_str),
        Some("#ff9e64")
    );

    // Status lineup is owned by the distro (plain assignment).
    assert_eq!(cfg.status.left.len(), 1);
    assert_eq!(cfg.status.center.len(), 1);
    assert_eq!(cfg.status.right.len(), 2);
}

#[test]
fn herdr_plugin_manifests_absolutize_and_exist_on_disk() {
    let cfg = parse_with_herdr("");

    let manifests: Vec<_> = cfg.plugins.iter().map(|p| p.manifest.clone()).collect();
    assert_eq!(manifests.len(), 2, "continuum + agent-tools: {manifests:?}");
    assert!(
        manifests[0].ends_with("examples/plugins/continuum/phux-plugin.toml"),
        "{manifests:?}"
    );
    assert!(
        manifests[1].ends_with("examples/plugins/agent-tools/phux-plugin.toml"),
        "{manifests:?}"
    );
    for manifest in &manifests {
        assert!(
            manifest.is_absolute(),
            "layer-relative manifest must be rewritten to absolute: {manifest:?}"
        );
        assert!(
            !manifest
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir)),
            "rewritten manifest must be lexically normalized (no `..`): {manifest:?}"
        );
        assert!(
            manifest.exists(),
            "herdr wires a manifest that is not in the repo: {manifest:?}"
        );
        assert!(
            phux_config::plugin::load_plugin_manifest(manifest).is_ok(),
            "wired manifest must load: {manifest:?}"
        );
    }
}

#[test]
fn user_overrides_win_over_herdr_and_appends_compose() {
    let cfg = parse_with_herdr(
        r#"
[keybindings]
which-key-delay-ms = 800

[keybindings.prefix-table]
"Space" = "show-help"

[theme]
accent = "magenta"

[[plugins-append]]
manifest = "/home/me/extra/phux-plugin.toml"
"#,
    );

    // User leaf beats the distro leaf.
    assert_eq!(cfg.keybindings.which_key_delay_ms, 800);
    assert_eq!(
        cfg.keybindings.prefix_table.get("Space"),
        Some(&Action::Bare("show-help".to_owned()))
    );
    // Theme merges per slot: the user's accent wins, herdr's other
    // slots survive.
    assert_eq!(
        cfg.theme.slots.get("accent").map(String::as_str),
        Some("magenta")
    );
    assert_eq!(
        cfg.theme.slots.get("chord").map(String::as_str),
        Some("#9ece6a")
    );
    // The user's append stacks on the distro's two plugins.
    assert_eq!(cfg.plugins.len(), 3);
    assert_eq!(
        cfg.plugins[2].manifest,
        Path::new("/home/me/extra/phux-plugin.toml")
    );
}

#[test]
fn user_plain_plugins_assignment_drops_the_distro_set() {
    // The documented escape hatch: replacement wins over inherited
    // appends, so a user can opt out of the herdr plugin set entirely.
    let cfg = parse_with_herdr("plugins = []\n");
    assert!(cfg.plugins.is_empty(), "{:?}", cfg.plugins);
}
