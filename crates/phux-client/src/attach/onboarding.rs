//! First-run onboarding hint (phux-foz.6).
//!
//! On attach, when nothing exists at the canonical config path
//! ([`phux_config::loader::config_path`]), the driver shows a dismissible
//! notice pointing at `phux config init` and the `C-a ?` help binding. The
//! notice reuses [`ToastOverlay`] — it is exactly the "one-shot,
//! any-key-dismisses" modal that overlay was built for — so this module owns
//! only the *trigger rule* and the *hint content*, both plain data the
//! driver feeds into the existing overlay stack.
//!
//! Trigger rule (documented in `docs/consumers/tui.md` §4.0.1):
//!
//! * Decided **once per `phux attach` invocation**, by a single existence
//!   check at attach time. Session switches within the invocation do not
//!   re-show it, and creating a config mid-attach does not retract it —
//!   the next attach simply won't show it.
//! * **Any key dismisses it** for the rest of that invocation (the
//!   keystroke is consumed by the modal, like every other overlay).
//! * It **never shows when anything exists at the config path** — a config
//!   file (even one that fails to parse) or a stray directory. Presence,
//!   not validity, is the test: an existing-but-broken config means the
//!   user has already found the config system, and `phux config init`
//!   refuses to overwrite, so the hint's advice would be wrong there.
//! * Nothing is persisted. No state file, no "seen" flag: while no config
//!   exists, every attach shows the hint once; creating one silences it
//!   permanently.
//!
//! The hint hardcodes the `C-a ?` chord deliberately: it only ever shows
//! when no config file exists, which is precisely when the embedded
//! defaults (prefix `C-a`, `?` = `show-help`) are guaranteed to apply.

use std::path::Path;

/// Title of the onboarding hint modal.
pub(super) const ONBOARDING_TITLE: &str = "Welcome to phux";

/// The trigger rule: show the hint iff we can positively determine that
/// nothing exists at `config_path`.
///
/// Uses [`Path::try_exists`] so an *undetermined* check (e.g. a
/// permission error on the config directory) suppresses the hint rather
/// than showing it — the invariant is "never appear when a config file
/// exists", so doubt resolves to not showing.
pub(super) fn should_show(config_path: &Path) -> bool {
    matches!(config_path.try_exists(), Ok(false))
}

/// The hint body handed to [`ToastOverlay`].
///
/// Mentions the two discovery affordances the bead names: `phux config
/// init` (scaffolds a commented starter config at `config_path`) and
/// `C-a ?` (the default help binding — guaranteed active, since the hint
/// only shows when no config overrides the defaults).
pub(super) fn hint_lines(config_path: &Path) -> Vec<String> {
    vec![
        "No config file found - phux is running on its built-in defaults.".to_owned(),
        "That works fine; when you want to customize it:".to_owned(),
        String::new(),
        "  phux config init   write a commented starter config to".to_owned(),
        format!("                     {}", config_path.display()),
        "  C-a ?              show the keybindings help".to_owned(),
        String::new(),
        "This hint appears on attach only while no config file exists.".to_owned(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shows_when_nothing_exists_at_the_config_path() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("phux").join("config.toml");
        assert!(
            should_show(&path),
            "missing file (and missing parent dir) must trigger the hint"
        );
    }

    #[test]
    fn never_shows_when_a_config_file_exists() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("config.toml");
        std::fs::write(&path, "[defaults]\n").expect("write config");
        assert!(
            !should_show(&path),
            "existing config must suppress the hint"
        );
    }

    #[test]
    fn never_shows_for_an_unparsable_config_file() {
        // Presence, not validity: a broken config still means the user has
        // found the config system, and `config init` refuses to overwrite.
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("config.toml");
        std::fs::write(&path, "this is [[ not toml").expect("write config");
        assert!(
            !should_show(&path),
            "an existing-but-invalid config must still suppress the hint"
        );
    }

    #[test]
    fn never_shows_when_a_directory_squats_the_config_path() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("config.toml");
        std::fs::create_dir(&path).expect("mkdir");
        assert!(
            !should_show(&path),
            "anything existing at the path suppresses the hint"
        );
    }

    #[test]
    fn empty_config_file_still_counts_as_existing() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("config.toml");
        std::fs::write(&path, "").expect("write config");
        assert!(!should_show(&path), "zero-byte config still suppresses");
    }

    #[test]
    fn hint_names_config_init_and_the_help_chord() {
        let lines = hint_lines(Path::new("/home/u/.config/phux/config.toml"));
        let body = lines.join("\n");
        assert!(
            body.contains("phux config init"),
            "must point at init:\n{body}"
        );
        assert!(
            body.contains("C-a ?"),
            "must point at the help chord:\n{body}"
        );
        assert!(
            body.contains("/home/u/.config/phux/config.toml"),
            "must show where the config will land:\n{body}"
        );
    }
}
