//! Integration tests for the config schema.
//!
//! Covers:
//! 1. The canonical `DESIGN.md` §4.2 example round-trips
//!    (`parse → serialize → reparse` is equal under `PartialEq`).
//! 2. A syntactically-malformed input produces a `ConfigError::Parse`
//!    with the expected `line:col`, and we snapshot its `Display`.
//! 3. Missing optional sections fall back to defaults.
//! 4. Unknown fields are rejected (`deny_unknown_fields`).

use std::path::PathBuf;

use phux_config::{Config, ConfigError, DefaultsCfg, parse_str};

/// The canonical example from `DESIGN.md` §4.2.
const CANONICAL: &str = r##"
[defaults]
shell          = "/bin/zsh"
history-limit  = 50000
refresh-rate   = 60

[keybindings]
prefix = "ctrl+space"

# Bindings under the prefix.
[keybindings.prefix-table]
"c"        = { action = "new-pane", direction = "horizontal" }
"v"        = { action = "new-pane", direction = "vertical" }
"x"        = "kill-pane"
"n"        = "new-window"
"tab"      = "next-window"
"h"        = { action = "focus-pane", direction = "left" }
"j"        = { action = "focus-pane", direction = "down" }
"k"        = { action = "focus-pane", direction = "up" }
"l"        = { action = "focus-pane", direction = "right" }
"d"        = "detach"
"shift+r"  = "rename-window"

# Global table: bindings that fire without a prefix.
[keybindings.global]

[status]
left   = ["session"]
center = ["windows"]
right  = [{ kind = "clock", format = "%H:%M" }]

[[hooks.pane-exit]]
when   = { exit-code = 0 }
action = "noop"

[[hooks.pane-exit]]
when   = { exit-code = "*" }
action = { kind = "notify", text = "pane {pane} exited with {exit-code}" }

[theme]
fg = "#cdd6f4"
bg = "#1e1e2e"
"##;

fn path() -> PathBuf {
    PathBuf::from("config.toml")
}

#[test]
fn canonical_example_round_trips() {
    let parsed: Config = parse_str(CANONICAL, &path()).expect("canonical parses");

    // Re-serialize and re-parse; the two `Config` values must compare equal.
    let reserialized = toml::to_string(&parsed).expect("re-serialize");
    let reparsed: Config =
        parse_str(&reserialized, &path()).expect("reparse of re-serialized config");

    assert_eq!(parsed, reparsed, "round trip should be identity");

    // Spot-check a couple of fields so a regression doesn't silently
    // pass via two-way equality of broken values.
    assert_eq!(parsed.keybindings.prefix, "ctrl+space");
    assert_eq!(parsed.defaults.shell.as_deref(), Some("/bin/zsh"));
    assert_eq!(parsed.defaults.history_limit, 50_000);
    assert_eq!(parsed.hooks.get("pane-exit").map(Vec::len), Some(2));
    assert_eq!(
        parsed.theme.slots.get("fg").map(String::as_str),
        Some("#cdd6f4")
    );
}

#[test]
fn missing_sections_use_defaults() {
    // Only [defaults] present, and only one field within it. Everything
    // else must populate from `Default`.
    let input = r#"
[defaults]
shell = "/bin/bash"
"#;
    let cfg = parse_str(input, &path()).expect("partial config parses");

    let want_defaults = DefaultsCfg {
        shell: Some("/bin/bash".to_owned()),
        ..DefaultsCfg::default()
    };
    assert_eq!(cfg.defaults, want_defaults);
    assert_eq!(cfg.keybindings.prefix, "ctrl+a"); // schema default
    assert!(cfg.keybindings.prefix_table.is_empty());
    assert!(cfg.status.left.is_empty());
    assert!(cfg.hooks.is_empty());
    assert!(cfg.theme.slots.is_empty());
}

#[test]
fn empty_input_is_full_defaults() {
    let cfg = parse_str("", &path()).expect("empty parses");
    assert_eq!(cfg, Config::default());
}

#[test]
fn unknown_field_at_top_level_is_rejected() {
    let input = r#"
not-a-real-section = "oops"
"#;
    let err = parse_str(input, &path()).expect_err("unknown field rejected");
    assert!(matches!(err, ConfigError::Parse { .. }));
}

#[test]
fn unknown_field_in_defaults_is_rejected() {
    let input = r#"
[defaults]
shell = "/bin/zsh"
histroy-limit = 50000  # typo: histroy
"#;
    let err = parse_str(input, &path()).expect_err("typo rejected by deny_unknown_fields");
    let ConfigError::Parse { message, .. } = err else {
        panic!("expected Parse variant");
    };
    assert!(
        message.contains("histroy-limit") || message.contains("unknown"),
        "message should mention the unknown field: {message}"
    );
}

#[test]
fn malformed_input_reports_line_col_and_snapshots() {
    // Unclosed string in the middle of the prefix-table. The offending
    // token sits on the line with the bad value.
    //
    // Line layout (1-indexed):
    //   1: (empty leading newline)
    //   2: [keybindings.prefix-table]
    //   3: "c" = "kill-pane
    //   4: "x" = "kill-pane"
    let input = "\n[keybindings.prefix-table]\n\"c\" = \"kill-pane\n\"x\" = \"kill-pane\"\n";

    let err =
        parse_str(input, &PathBuf::from("config.toml")).expect_err("malformed input should error");

    let ConfigError::Parse { line, col, .. } = &err else {
        panic!("expected Parse variant, got {err:?}");
    };

    // The error must point inside the broken line (line 3) — not at the
    // start of the file. We assert the line and a generous col window
    // so the test isn't brittle against `toml = "0.8"` minor bumps.
    assert_eq!(*line, 3, "error should point at the broken line");
    assert!(*col >= 1, "col must be 1-indexed");

    // Snapshot the Display form. Normalize the column to a placeholder
    // because exact column depends on `toml`'s internal pointer choice
    // (start of token vs. error position) and is allowed to drift.
    let rendered = format!("{err}");
    let normalized = normalize_col(&rendered);
    insta::assert_snapshot!("malformed_parse_error", normalized);
}

// ---------------------------------------------------------------------------
// [experimental] predictive-echo  (phux-9gw.1.2)
// ---------------------------------------------------------------------------

#[test]
fn experimental_predictive_echo_true_parses() {
    let input = r"
[experimental]
predictive-echo = true
";
    let cfg = parse_str(input, &path()).expect("[experimental] section parses");
    assert!(
        cfg.experimental.predictive_echo,
        "predictive-echo = true should land as true in the typed view"
    );
}

#[test]
fn experimental_predictive_echo_defaults_false_when_absent() {
    // No [experimental] section at all: the field must default to false.
    let cfg = parse_str("", &path()).expect("empty parses");
    assert!(
        !cfg.experimental.predictive_echo,
        "absent [experimental] section must leave predictive-echo at its false default"
    );

    // Empty [experimental] table is also valid and yields the same default.
    let cfg2 = parse_str("[experimental]\n", &path()).expect("empty section parses");
    assert!(!cfg2.experimental.predictive_echo);
}

#[test]
fn experimental_predictive_echo_malformed_value_reports_key() {
    // Bool field given an integer: the error must reach the user with
    // enough context to find the key.
    let input = r"
[experimental]
predictive-echo = 1
";
    let err = parse_str(input, &path()).expect_err("integer is not a bool");
    let ConfigError::Parse { message, line, .. } = err else {
        panic!("expected ConfigError::Parse for malformed value");
    };
    assert!(
        message.contains("bool") || message.contains("boolean"),
        "error should mention the expected type; got: {message}"
    );
    // The offending value sits on line 3 (leading newline + section line + value line).
    assert_eq!(line, 3, "error should point at the broken value line");
}

/// Replace the `:COL:` in `path:LINE:COL: message` with `:<col>:` so
/// the snapshot is stable across `toml` crate minor versions.
fn normalize_col(s: &str) -> String {
    // Format is `path: line:col: message`. Find the second colon
    // after the line number and rewrite up to the next colon.
    let Some(first_colon) = s.find(':') else {
        return s.to_owned();
    };
    let after_path = &s[first_colon + 1..];
    let Some(line_end) = after_path.find(':') else {
        return s.to_owned();
    };
    let rest = &after_path[line_end + 1..];
    let Some(col_end) = rest.find(':') else {
        return s.to_owned();
    };
    let mut out = String::with_capacity(s.len());
    out.push_str(&s[..=first_colon]);
    out.push_str(&after_path[..=line_end]);
    out.push_str("<col>");
    out.push_str(&rest[col_end..]);
    out
}
