use std::path::Path;

use phux_config::plugin;

#[test]
fn checked_in_continuum_manifest_loads() -> Result<(), Box<dyn std::error::Error>> {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("examples/plugins/continuum/phux-plugin.toml");

    let loaded = plugin::load_plugin_manifest(&manifest)?;

    assert_eq!(loaded.id, "com.phux.demo.continuum");
    assert_eq!(loaded.actions[0].id, "autosave");
    assert_eq!(loaded.actions[1].id, "restore-latest");
    assert_eq!(loaded.events[0].id, "idle-autosave");
    assert_eq!(loaded.events[1].on, "session.changed");
    assert_eq!(loaded.workspaces[0].id, "continuum");
    assert_eq!(loaded.workspaces[0].actions, ["autosave", "restore-latest"]);
    assert_eq!(
        loaded.workspaces[0].events,
        ["idle-autosave", "session-autosave"]
    );
    Ok(())
}
