#![allow(clippy::expect_used, reason = "tests")]

use std::path::{Path, PathBuf};
use std::process::Command;

const PHUX: &str = env!("CARGO_BIN_EXE_phux");
const PLUGIN_ID: &str = "com.phux.demo.agent-tools";
const ACTION_ID: &str = "inspect";

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("canonical repo root")
}

fn demo_xdg() -> PathBuf {
    repo_root().join("examples/plugins/agent-tools/config")
}

fn run_demo(args: &[&str]) -> (i32, String, String) {
    let out = Command::new(PHUX)
        .env("XDG_CONFIG_HOME", demo_xdg())
        .args(args)
        .output()
        .expect("run phux binary");
    (
        out.status.code().expect("phux exited via code"),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn checked_in_plugin_demo_lists_validates_and_runs() {
    let (code, stdout, stderr) = run_demo(&["config", "plugins"]);
    assert_eq!(code, 0, "config plugins should succeed; stderr={stderr}");
    assert_eq!(stdout.trim(), format!("{PLUGIN_ID} 0.1.0 (enabled)"));

    let (code, stdout, stderr) = run_demo(&["config", "plugins", "--json"]);
    assert_eq!(
        code, 0,
        "config plugins --json validates the manifest; stderr={stderr}"
    );
    let plugins: serde_json::Value = serde_json::from_str(&stdout).expect("plugins stdout is JSON");
    assert_eq!(plugins["plugins"][0]["id"], PLUGIN_ID);
    assert_eq!(plugins["plugins"][0]["actions"][0]["id"], ACTION_ID);
    assert_eq!(plugins["plugins"][0]["enabled"], true);

    let (code, stdout, stderr) = run_demo(&["config", "run", PLUGIN_ID, ACTION_ID]);
    assert_eq!(code, 0, "config run should succeed; stderr={stderr}");
    assert!(stdout.contains("core=stable terminal/session host"));
    assert!(stdout.contains("plugin=agentic workflow package"));
    assert!(stdout.contains(&format!("plugin_id={PLUGIN_ID}")));
    assert!(stdout.contains(&format!("action_id={ACTION_ID}")));

    let (code, stdout, stderr) = run_demo(&["config", "run", PLUGIN_ID, ACTION_ID, "--json"]);
    assert_eq!(code, 0, "config run --json should succeed; stderr={stderr}");
    let output: serde_json::Value = serde_json::from_str(&stdout).expect("run stdout is JSON");
    assert_eq!(output["plugin_id"], PLUGIN_ID);
    assert_eq!(output["action_id"], ACTION_ID);
    assert_eq!(output["outcome"], "completed");
    assert_eq!(output["exit_code"], 0);
    assert!(
        output["stdout"]
            .as_str()
            .expect("stdout field")
            .contains("phux plugin demo")
    );
}
