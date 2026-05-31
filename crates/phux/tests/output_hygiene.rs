//! Binary-level output-hygiene tests for `phux` (cli-ergonomics).
//!
//! These drive the REAL `phux` binary (handed to us by cargo at
//! `env!("CARGO_BIN_EXE_phux")`) but need NO running server, so unlike
//! `run_wait_e2e.rs` they are cheap and run in the default pool. They pin
//! the contracts an agent or shell script depends on:
//!
//!   * A one-shot verb prints NO build banner — stderr stays clean.
//!   * A `--json` path puts ONLY JSON on stdout (or nothing, on error),
//!     never the banner; errors go to stderr with a nonzero exit.
//!   * `--version` reports on stdout, banner-free.
//!
//! Every verb that contacts a server is pointed at a socket path that
//! does not exist, so the server is never auto-spawned: `ls` does not
//! auto-start one, and the selector verbs see a connect error first.

#![allow(clippy::expect_used, reason = "tests")]
#![allow(clippy::unwrap_used, reason = "tests")]

use std::process::Command;

/// Path to the freshly-built `phux` binary, injected by cargo.
const PHUX: &str = env!("CARGO_BIN_EXE_phux");

/// A socket path guaranteed not to exist, so no verb finds (or spawns) a
/// server. Unique per process to avoid any cross-run collision.
fn dead_socket() -> String {
    format!("/tmp/phux-no-such-server-{}.sock", std::process::id())
}

/// Run `phux <args...>` and return `(exit_code, stdout, stderr)`.
fn run(args: &[&str]) -> (i32, String, String) {
    let out = Command::new(PHUX)
        .args(args)
        .output()
        .expect("run phux binary");
    (
        out.status.code().expect("phux exited via code, not signal"),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// The pre-alpha build banner that used to print on EVERY invocation. No
/// one-shot verb may emit it (it pollutes stderr for scripts/agents).
const BANNER_FRAGMENT: &str = "pre-alpha";

#[test]
fn version_is_clean_stdout_with_no_banner() {
    let (code, stdout, stderr) = run(&["--version"]);
    assert_eq!(code, 0, "--version should exit 0; stderr={stderr}");
    assert!(
        stdout.contains(env!("CARGO_PKG_VERSION")),
        "--version stdout should carry the version; got {stdout:?}"
    );
    assert!(
        !stdout.contains(BANNER_FRAGMENT) && !stderr.contains(BANNER_FRAGMENT),
        "--version must not print the pre-alpha banner; stdout={stdout:?} stderr={stderr:?}"
    );
}

#[test]
fn help_does_not_print_banner() {
    let (code, stdout, stderr) = run(&["--help"]);
    assert_eq!(code, 0, "--help should exit 0");
    assert!(
        !stdout.contains(BANNER_FRAGMENT) && !stderr.contains(BANNER_FRAGMENT),
        "--help must not print the pre-alpha banner; stdout={stdout:?} stderr={stderr:?}"
    );
}

#[test]
fn ls_json_no_server_is_silent_stdout_and_banner_free() {
    let sock = dead_socket();
    let (code, stdout, stderr) = run(&["ls", "--json", "--socket", &sock]);
    assert_ne!(code, 0, "`ls --json` with no server should exit nonzero");
    assert!(
        stdout.is_empty(),
        "`ls --json` with no server must leave stdout empty (no banner, no partial JSON); got {stdout:?}"
    );
    assert!(
        !stderr.contains(BANNER_FRAGMENT),
        "`ls --json` must not print the banner to stderr; got {stderr:?}"
    );
    assert!(
        stderr.contains("no server"),
        "the error should explain there is no server; got {stderr:?}"
    );
}

#[test]
fn ls_plain_no_server_is_banner_free() {
    let sock = dead_socket();
    let (code, _stdout, stderr) = run(&["ls", "--socket", &sock]);
    assert_ne!(
        code, 0,
        "`ls` with no server should exit nonzero (like tmux)"
    );
    assert!(
        !stderr.contains(BANNER_FRAGMENT),
        "`ls` must not print the banner; got {stderr:?}"
    );
}

#[test]
fn snapshot_json_no_server_is_silent_stdout_and_banner_free() {
    let sock = dead_socket();
    let (code, stdout, stderr) = run(&["snapshot", "--json", "work", "--socket", &sock]);
    assert_ne!(
        code, 0,
        "`snapshot --json` with no server should exit nonzero"
    );
    assert!(
        stdout.is_empty(),
        "`snapshot --json` with no server must leave stdout empty; got {stdout:?}"
    );
    assert!(
        !stderr.contains(BANNER_FRAGMENT),
        "`snapshot --json` must not print the banner; got {stderr:?}"
    );
}

#[test]
fn config_path_is_clean_stdout_with_no_banner() {
    // `config` never contacts a server, so it always runs. Its stdout must
    // be the path alone — no banner above it.
    let (code, stdout, stderr) = run(&["config", "path"]);
    assert_eq!(code, 0, "`config path` should exit 0; stderr={stderr}");
    assert!(
        !stdout.contains(BANNER_FRAGMENT) && !stderr.contains(BANNER_FRAGMENT),
        "`config path` must not print the banner; stdout={stdout:?} stderr={stderr:?}"
    );
    assert!(
        stdout.lines().count() == 1 && !stdout.trim().is_empty(),
        "`config path` stdout should be exactly the path on one line; got {stdout:?}"
    );
}
