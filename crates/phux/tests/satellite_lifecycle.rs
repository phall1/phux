#![allow(clippy::expect_used, reason = "tests")]
#![allow(clippy::unwrap_used, reason = "tests")]

use std::process::Command;

use tempfile::TempDir;

const PHUX: &str = env!("CARGO_BIN_EXE_phux");
const BANNER_FRAGMENT: &str = "pre-alpha";

fn run_with_xdg(args: &[&str], xdg_config_home: &std::path::Path) -> (i32, String, String) {
    let out = Command::new(PHUX)
        .env("XDG_CONFIG_HOME", xdg_config_home)
        .args(args)
        .output()
        .expect("run phux binary");
    (
        out.status.code().expect("phux exited via code, not signal"),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn add_list_update_remove_json_is_machine_readable() {
    let tmp = TempDir::new().expect("tempdir");
    let xdg = tmp.path().join("xdg");

    let (code, stdout, stderr) = run_with_xdg(
        &["satellite", "add", "devbox", "ssh://devbox", "--json"],
        &xdg,
    );
    assert_eq!(
        code, 0,
        "`satellite add --json` should exit 0; stderr={stderr}"
    );
    assert!(
        !stdout.contains(BANNER_FRAGMENT) && !stderr.contains(BANNER_FRAGMENT),
        "`satellite add --json` must be banner-free; stdout={stdout:?} stderr={stderr:?}"
    );
    let value: serde_json::Value = serde_json::from_str(&stdout).expect("add stdout is JSON");
    assert_eq!(value["schema_version"], 1);
    assert_eq!(value["satellite"]["name"], "devbox");
    assert_eq!(value["satellite"]["endpoint"], "ssh://devbox");
    assert_eq!(value["satellite"]["enabled"], true);

    let (code, stdout, stderr) = run_with_xdg(&["satellite", "list", "--json"], &xdg);
    assert_eq!(
        code, 0,
        "`satellite list --json` should exit 0; stderr={stderr}"
    );
    let value: serde_json::Value = serde_json::from_str(&stdout).expect("list stdout is JSON");
    assert_eq!(value["satellites"][0]["name"], "devbox");

    let (code, stdout, stderr) = run_with_xdg(
        &[
            "satellite",
            "add",
            "devbox",
            "quic://devbox.example:8788",
            "--disabled",
            "--json",
        ],
        &xdg,
    );
    assert_eq!(
        code, 0,
        "`satellite add` should update existing entries; stderr={stderr}"
    );
    let value: serde_json::Value = serde_json::from_str(&stdout).expect("update stdout is JSON");
    assert_eq!(value["satellite"]["endpoint"], "quic://devbox.example:8788");
    assert_eq!(value["satellite"]["enabled"], false);

    let config = std::fs::read_to_string(xdg.join("phux").join("config.toml"))
        .expect("read config after update");
    assert_eq!(config.matches("[[satellites]]").count(), 1);
    assert!(config.contains(r#"endpoint = "quic://devbox.example:8788""#));
    assert!(config.contains("enabled = false"));

    let (code, stdout, stderr) = run_with_xdg(&["satellite", "remove", "devbox", "--json"], &xdg);
    assert_eq!(
        code, 0,
        "`satellite remove --json` should exit 0; stderr={stderr}"
    );
    let value: serde_json::Value = serde_json::from_str(&stdout).expect("remove stdout is JSON");
    assert_eq!(value["removed"]["name"], "devbox");

    let (code, stdout, stderr) = run_with_xdg(&["satellite", "list", "--json"], &xdg);
    assert_eq!(
        code, 0,
        "`satellite list --json` should exit 0 after removal; stderr={stderr}"
    );
    let value: serde_json::Value =
        serde_json::from_str(&stdout).expect("empty list stdout is JSON");
    assert_eq!(value["satellites"].as_array().expect("satellites").len(), 0);
}

#[test]
fn duplicate_configured_satellite_names_are_rejected() {
    let tmp = TempDir::new().expect("tempdir");
    let xdg = tmp.path().join("xdg");
    let config_dir = xdg.join("phux");
    std::fs::create_dir_all(&config_dir).expect("create config dir");
    std::fs::write(
        config_dir.join("config.toml"),
        r#"
[[satellites]]
name = "devbox"
endpoint = "ssh://devbox-a"

[[satellites]]
name = "devbox"
endpoint = "ssh://devbox-b"
"#,
    )
    .expect("write config");

    let (code, stdout, stderr) = run_with_xdg(&["satellite", "list", "--json"], &xdg);

    assert_ne!(code, 0, "duplicate satellite names should be refused");
    assert!(stdout.is_empty());
    assert!(stderr.contains(r#"duplicate satellite name "devbox""#));
}

#[test]
fn invalid_endpoint_fails_without_stdout() {
    let tmp = TempDir::new().expect("tempdir");

    let (code, stdout, stderr) = run_with_xdg(
        &["satellite", "add", "devbox", "devbox", "--json"],
        tmp.path(),
    );

    assert_ne!(code, 0, "invalid endpoint should fail");
    assert!(stdout.is_empty());
    assert!(stderr.contains("endpoint must be a URI"));
}

#[test]
#[cfg(unix)]
fn lifecycle_refuses_to_overwrite_symlinked_config() {
    let tmp = TempDir::new().expect("tempdir");
    let xdg = tmp.path().join("xdg");
    let config_dir = xdg.join("phux");
    std::fs::create_dir_all(&config_dir).expect("create config dir");
    let victim = tmp.path().join("victim.toml");
    std::fs::write(&victim, "do-not-touch").expect("write victim");
    std::os::unix::fs::symlink(&victim, config_dir.join("config.toml")).expect("symlink config");

    let (code, stdout, stderr) = run_with_xdg(
        &["satellite", "add", "devbox", "ssh://devbox", "--json"],
        &xdg,
    );

    assert_ne!(code, 0, "symlinked config should be refused");
    assert!(stdout.is_empty());
    assert!(stderr.contains("must not be a symlink"));
    assert_eq!(
        std::fs::read_to_string(victim).expect("read victim"),
        "do-not-touch"
    );
}
