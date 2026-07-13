//! Which agent binary is running in a pane (ADR-0046 §A).
//!
//! Identity comes from the kernel, not from the title. The title is a
//! string the program chose to print — a shell that `echo`s "claude" would
//! fool it. The foreground process group of the pane's own PTY is what the
//! user is actually typing at.
//!
//! The wrinkle is that agent CLIs ship in two shapes: a native binary
//! (`argv[0] = "claude"`) and a script under a runtime (`node
//! .../@anthropic-ai/claude-code/cli.js`). So we unwrap runtime wrappers,
//! and we match on two tiers — see [`foreground_agent`].

use super::rules::RuleSet;
use crate::proc_query;

/// Interpreters that merely *host* an agent: the interesting name is
/// further along argv, not at `argv[0]`.
const RUNTIME_WRAPPERS: [&str; 14] = [
    "node", "nodejs", "bun", "deno", "python", "python3", "sh", "bash", "zsh", "fish", "env",
    "npx", "uv", "uvx",
];

/// Suffixes stripped from a script name before matching (`cli.js` -> `cli`).
const SCRIPT_SUFFIXES: [&str; 5] = [".js", ".mjs", ".cjs", ".py", ".ts"];

/// The agent kind running in the foreground of this PTY, if any.
///
/// 1. Ask the kernel which process group owns the tty.
/// 2. Read that process's argv.
/// 3. Resolve a kind from argv, in two tiers:
///
/// **Tier 1 — basename.** The basename of `argv[0]`, with any script suffix
/// stripped, matched against the manifests' `binaries`. When `argv[0]` is a
/// [runtime wrapper](RUNTIME_WRAPPERS), every later argument's basename is
/// tried too. This catches the native `claude` binary.
///
/// **Tier 2 — program-path components.** For arguments that are unambiguously
/// a *program path* — `argv[0]` when it contains a `/`, and any wrapper
/// argument whose basename carries a script suffix — each path component is
/// matched too. This catches `node .../claude-code/cli.js`, whose basename
/// (`cli`) is far too generic to list as a binary name, and the
/// version-pinned native install (`.../share/claude/versions/2.1.207`),
/// whose basename is a version number.
///
/// Tier 2 is deliberately NOT applied to arbitrary arguments. `sh -c 'cd
/// ~/.claude/foo && make'` must not identify as an agent just because a
/// user's *data* path contains the word — so a plain string argument is
/// never split into path components. Only the program's own path is.
///
/// First hit wins; `None` when nothing matches (the caller then publishes
/// nothing, and retracts any record it previously wrote).
pub(crate) fn foreground_agent(master_fd: Option<i32>, rules: &RuleSet) -> Option<String> {
    let pgid = proc_query::foreground_pgid(master_fd?)?;
    let argv = proc_query::process_argv(pgid)?;
    kind_from_argv(&argv, rules)
}

/// The rule-matching core of [`foreground_agent`], split out so it is a pure
/// function of `(argv, rules)` and can be exhaustively table-tested without
/// a live process.
pub(crate) fn kind_from_argv(argv: &[String], rules: &RuleSet) -> Option<String> {
    let first = argv.first()?;

    if let Some(kind) = match_program(first, rules) {
        return Some(kind);
    }

    // `argv[0]` is only a host: keep looking.
    if !is_runtime_wrapper(first) {
        return None;
    }
    for arg in argv.iter().skip(1) {
        // Flags never name the program.
        if arg.starts_with('-') {
            continue;
        }
        if let Some(kind) = match_program(arg, rules) {
            return Some(kind);
        }
    }
    None
}

/// Match one *program-shaped* argument against the rule set: tier 1 on its
/// basename, then tier 2 on its path components when it is unambiguously a
/// program path.
fn match_program(arg: &str, rules: &RuleSet) -> Option<String> {
    let base = strip_script_suffix(basename(arg));
    if let Some(kind) = rules.kind_for_binary(base) {
        return Some(kind.to_owned());
    }
    if !is_program_path(arg) {
        return None;
    }
    arg.split('/')
        .filter(|part| !part.is_empty())
        .find_map(|part| rules.kind_for_binary(strip_script_suffix(part)))
        .map(str::to_owned)
}

/// Whether `arg` is unambiguously the path of a program (as opposed to a
/// data path, a flag, or a shell command string). Requires a `/` — a bare
/// name is handled by tier 1 — and no whitespace, which a `sh -c` command
/// string would carry.
fn is_program_path(arg: &str) -> bool {
    arg.contains('/') && !arg.chars().any(char::is_whitespace)
}

/// The trailing path component of `arg`.
fn basename(arg: &str) -> &str {
    arg.rsplit('/').next().unwrap_or(arg)
}

/// Strip a known script suffix, if present.
fn strip_script_suffix(name: &str) -> &str {
    for suffix in SCRIPT_SUFFIXES {
        if let Some(stem) = name.strip_suffix(suffix) {
            return stem;
        }
    }
    name
}

/// Whether `arg` names an interpreter rather than an agent.
fn is_runtime_wrapper(arg: &str) -> bool {
    // A login shell arrives as `-zsh`; strip the leading dash before
    // comparing, or an interactive shell would never be recognized as a
    // wrapper and we would stop scanning at argv[0].
    let name = basename(arg).trim_start_matches('-');
    RUNTIME_WRAPPERS.contains(&name)
}

#[cfg(test)]
#[allow(clippy::expect_used, reason = "tests")]
mod tests {
    use super::kind_from_argv;
    use crate::agent_detect::rules::{ManifestSpec, RuleSet};

    fn rules() -> RuleSet {
        let spec: ManifestSpec = toml::from_str(
            r#"
kind = "claude"
binaries = ["claude", "claude-code"]
"#,
        )
        .expect("manifest parses");
        let mut set = RuleSet::default();
        set.install(spec).expect("compiles");
        set
    }

    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| (*s).to_owned()).collect()
    }

    fn kind(parts: &[&str]) -> Option<String> {
        kind_from_argv(&argv(parts), &rules())
    }

    #[test]
    fn native_binary_matches_on_argv0_basename() {
        assert_eq!(kind(&["claude"]).as_deref(), Some("claude"));
        assert_eq!(kind(&["/usr/local/bin/claude"]).as_deref(), Some("claude"));
    }

    #[test]
    fn version_pinned_native_install_matches_on_a_path_component() {
        // The real shape of a `claude` install: the bin entry is a symlink to
        // a version-numbered file, so the BASENAME is "2.1.207".
        assert_eq!(
            kind(&["/home/u/.local/share/claude/versions/2.1.207"]).as_deref(),
            Some("claude")
        );
    }

    #[test]
    fn node_hosted_install_matches_through_the_wrapper_on_a_path_component() {
        // The npm shape. The basename is `cli`, which is far too generic to
        // ever list as a binary name; the package directory is the signal.
        assert_eq!(
            kind(&[
                "node",
                "/home/u/.npm/lib/node_modules/@anthropic-ai/claude-code/cli.js",
            ])
            .as_deref(),
            Some("claude")
        );
    }

    #[test]
    fn wrapper_flags_are_skipped() {
        assert_eq!(
            kind(&["node", "--enable-source-maps", "/opt/claude-code/cli.js"]).as_deref(),
            Some("claude")
        );
    }

    #[test]
    fn login_shell_is_recognized_as_a_wrapper_and_yields_nothing() {
        assert_eq!(kind(&["-zsh"]), None);
        assert_eq!(kind(&["/bin/zsh"]), None);
    }

    /// The regression this design exists to prevent: a shell command string
    /// that merely *mentions* an agent-shaped path must NOT identify as that
    /// agent. A bogus agent row in the sidebar is a real bug, not a
    /// harmless one.
    #[test]
    fn shell_command_string_naming_a_data_path_does_not_identify() {
        assert_eq!(kind(&["sh", "-c", "cd /home/u/claude/notes && make"]), None);
        assert_eq!(kind(&["bash", "-c", "grep -r claude ."]), None);
    }

    /// A data path handed to a non-wrapper program is never even considered.
    #[test]
    fn a_non_wrapper_program_is_not_unwrapped() {
        assert_eq!(kind(&["vim", "/home/u/claude/notes.md"]), None);
        assert_eq!(kind(&["cat", "/opt/claude-code/cli.js"]), None);
    }

    #[test]
    fn unrelated_programs_do_not_identify() {
        assert_eq!(kind(&["htop"]), None);
        assert_eq!(kind(&["node", "/opt/other/server.js"]), None);
        assert_eq!(kind(&[]), None);
    }

    #[test]
    fn matching_is_case_insensitive() {
        assert_eq!(kind(&["CLAUDE"]).as_deref(), Some("claude"));
    }
}
