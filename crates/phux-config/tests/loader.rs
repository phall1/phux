//! Integration tests for `phux_config::loader`.
//!
//! Covers:
//! 1. Missing-file → `Config::default()` (no error).
//! 2. Present-file → parsed `Config`.
//! 3. Non-`NotFound` I/O errors propagate as `ConfigError::Io`.
//! 4. `config_path()` resolution honors `XDG_CONFIG_HOME`.
//! 5. `config_path()` falls back to `$HOME/.config/phux/config.toml`.
//!
//! Tests 4 and 5 mutate process-global environment variables and are
//! serialized via a `Mutex` so they don't race against each other or
//! anything else that reads env (the other tests in this file don't
//! touch env).

use std::path::PathBuf;
use std::sync::Mutex;

use phux_config::{ConfigError, loader};
use tempfile::TempDir;

/// Guards every test that mutates `XDG_CONFIG_HOME` / `HOME`. `env::set_var`
/// is process-global and unsynchronized; without this, parallel test
/// execution can observe torn state.
static ENV_LOCK: Mutex<()> = Mutex::new(());

/// Scoped env guard: snapshots a variable on construction and restores it
/// on drop. `set_var` / `remove_var` are `unsafe` under the 2024 edition;
/// this concentrates that unsafety in one well-audited place.
struct EnvGuard {
    key: &'static str,
    prev: Option<std::ffi::OsString>,
}

impl EnvGuard {
    fn set(key: &'static str, value: &str) -> Self {
        let prev = std::env::var_os(key);
        // SAFETY: process-global env mutation. All env-touching tests in
        // this file acquire `ENV_LOCK` before constructing an `EnvGuard`,
        // so no other thread races us. Restored on drop.
        unsafe {
            std::env::set_var(key, value);
        }
        Self { key, prev }
    }

    fn unset(key: &'static str) -> Self {
        let prev = std::env::var_os(key);
        // SAFETY: same as `set` — gated by `ENV_LOCK`.
        unsafe {
            std::env::remove_var(key);
        }
        Self { key, prev }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        // SAFETY: gated by `ENV_LOCK` for the lifetime of the test.
        unsafe {
            match &self.prev {
                Some(v) => std::env::set_var(self.key, v),
                None => std::env::remove_var(self.key),
            }
        }
    }
}

#[test]
fn missing_file_returns_default() {
    let tmp = TempDir::new().expect("tempdir");
    let path = tmp.path().join("definitely-not-here.toml");
    assert!(!path.exists(), "precondition: file does not exist");

    let cfg = loader::load_from(&path).expect("missing file is not an error");
    // Missing file ⇒ shipped defaults are applied. The embedded
    // `default.toml` populates a real prefix-table + a status bar so
    // out-of-the-box phux is usable without user config.
    assert_eq!(cfg.keybindings.prefix, "C-a");
    assert!(
        !cfg.keybindings.prefix_table.is_empty(),
        "embedded defaults must populate prefix-table"
    );
    assert_eq!(
        cfg.keybindings.prefix_table.get("d"),
        Some(&phux_config::Action::Bare("detach".to_owned()))
    );
    assert!(
        !cfg.status.left.is_empty() || !cfg.status.right.is_empty(),
        "embedded defaults must populate the status bar"
    );
}

#[test]
fn present_file_parses() {
    let tmp = TempDir::new().expect("tempdir");
    let path = tmp.path().join("config.toml");
    std::fs::write(
        &path,
        r#"
[defaults]
shell = "/bin/zsh"
history-limit = 1234
"#,
    )
    .expect("write config");

    let cfg = loader::load_from(&path).expect("valid config parses");
    assert_eq!(cfg.defaults.shell.as_deref(), Some("/bin/zsh"));
    assert_eq!(cfg.defaults.history_limit, 1234);
    // Untouched section uses default.
    assert_eq!(cfg.keybindings.prefix, "C-a");
}

#[test]
fn io_error_propagates() {
    // Pointing at a directory triggers `EISDIR` on Unix when we try to
    // `read_to_string`. That's a real I/O error — not `NotFound` — and
    // must surface as `ConfigError::Io`, not be swallowed into defaults.
    let tmp = TempDir::new().expect("tempdir");
    let dir_path = tmp.path().to_path_buf();
    assert!(dir_path.is_dir(), "precondition: path is a directory");

    let err = loader::load_from(&dir_path).expect_err("reading a dir must fail");
    match err {
        ConfigError::Io(_) => {}
        other => panic!("expected ConfigError::Io, got {other:?}"),
    }
}

#[test]
fn xdg_path_resolution() {
    let _guard = ENV_LOCK.lock().expect("env lock");
    let _xdg = EnvGuard::set("XDG_CONFIG_HOME", "/tmp/whatever");
    // HOME must not be allowed to shadow XDG when both are set.
    let _home = EnvGuard::set("HOME", "/tmp/should-be-ignored");

    let got = loader::config_path();
    assert_eq!(got, PathBuf::from("/tmp/whatever/phux/config.toml"));
}

#[test]
fn home_fallback() {
    let _guard = ENV_LOCK.lock().expect("env lock");
    let _xdg = EnvGuard::unset("XDG_CONFIG_HOME");
    let _home = EnvGuard::set("HOME", "/tmp/x");

    let got = loader::config_path();
    assert_eq!(got, PathBuf::from("/tmp/x/.config/phux/config.toml"));
}
