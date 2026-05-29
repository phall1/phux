//! `phux run` — run a command in a pane and capture its exit code, output,
//! and duration (phux-ab8, ADR-0022 §3).
//!
//! The exit code is the load-bearing value, and it cannot come from a grid
//! walk: libghostty records OSC-133 *semantic marks* per cell but does not
//! retain the `OSC 133;D;<code>` exit status. So `run` uses the portable,
//! shell-integration-free floor — it appends a **sentinel** to the command
//! that prints the real `$?` in a uniquely-tagged form, then polls the
//! side-effect-free screen read until the sentinel appears and parses the
//! code out.
//!
//! The sentinel is crafted so the shell's *echo* of the typed input never
//! false-matches the *output*: the typed form carries a literal `%d`
//! (printf's format directive), while the printed form carries digits.
//! Scanning for a parseable integer between the tags therefore matches
//! only the real output line.
//!
//! `run` assumes a POSIX shell (sh/bash/zsh): it relies on `;`, `$?`, and
//! `printf`. Fish and other non-POSIX shells are out of scope for v0.

use std::path::Path;
use std::time::Duration;

use phux_core::screen::ScreenState;
use phux_protocol::wire::frame::AttachTarget;
use serde::Serialize;
use tokio::time::Instant;

use crate::attach::AttachError;
use crate::send_keys;
use crate::snapshot::get_screen;
use crate::wait::DEFAULT_POLL_INTERVAL;

/// A completed command's result — the agent-facing contract for `run`
/// (ADR-0022 §3). `exit_code == n` for a child that did `_exit(n)`.
#[derive(Debug, Clone, Serialize)]
pub struct RunResult {
    /// The command line as submitted (without the sentinel).
    pub command: String,
    /// The child's exit code, parsed from the sentinel.
    pub exit_code: i32,
    /// Captured stdout/stderr as it appeared on screen, between the
    /// command echo and the sentinel. See `truncated`.
    pub output: String,
    /// Wall-clock from submit to sentinel-seen, in milliseconds. Includes
    /// poll latency, so it is an upper bound on the child's own runtime,
    /// not a precise measurement.
    pub duration_ms: u64,
    /// `true` when the command echo had scrolled out of the viewport, so
    /// `output` is best-effort visible context rather than a clean capture.
    /// Full capture needs scrollback (phux-o1v).
    pub truncated: bool,
}

/// Why [`run`] returned.
#[derive(Debug, Clone)]
pub enum RunOutcome {
    /// The sentinel was seen; the command finished.
    Completed(RunResult),
    /// The timeout elapsed before the sentinel appeared. Carries the last
    /// screen so the caller can show what the command was doing.
    TimedOut {
        /// The command line as submitted.
        command: String,
        /// Wall-clock waited before giving up, in milliseconds.
        duration_ms: u64,
        /// The last screen observed.
        screen: ScreenState,
    },
}

/// The sentinel's stable prefix for `nonce`. Output form: `<prefix><code>=END`.
fn sentinel_prefix(nonce: &str) -> String {
    format!("PHUXRC{nonce}=")
}

/// The literal *typed* form of the sentinel (carries printf's `%d`), used
/// to locate the command-echo line so it is excluded from `output`.
fn sentinel_template(nonce: &str) -> String {
    format!("{}%d=END", sentinel_prefix(nonce))
}

/// Build the shell line to submit: the user command, then a `printf` that
/// emits the tagged exit code on its own fresh line.
fn command_line(cmd: &str, nonce: &str) -> String {
    // Leading `\n` guarantees the sentinel starts a fresh row (so it never
    // wraps onto the tail of the command's last output line).
    format!("{cmd}; printf '\\n{}%d=END\\n' $?", sentinel_prefix(nonce))
}

/// Scan `lines` for the *output* sentinel and parse its exit code,
/// returning `(line_index, code)`. The command-echo line carries a literal
/// `%d` between the tags, so its parse fails and it is skipped.
fn parse_sentinel(lines: &[String], nonce: &str) -> Option<(usize, i32)> {
    let prefix = sentinel_prefix(nonce);
    for (i, line) in lines.iter().enumerate() {
        let Some(after) = line.split(prefix.as_str()).nth(1) else {
            continue;
        };
        if let Some(code_str) = after.split("=END").next()
            && let Ok(code) = code_str.parse::<i32>()
        {
            return Some((i, code));
        }
    }
    None
}

/// Extract the command's output from the viewport given the sentinel's
/// row, returning `(output, truncated)`. When the command echo is visible,
/// `output` is exactly the rows between it and the sentinel; otherwise the
/// echo scrolled off and we return best-effort visible context.
fn extract_output(lines: &[String], sentinel_idx: usize, nonce: &str) -> (String, bool) {
    let template = sentinel_template(nonce);
    let echo_idx = lines.iter().position(|l| l.contains(template.as_str()));
    match echo_idx {
        Some(echo) if echo < sentinel_idx => {
            let body = lines[echo + 1..sentinel_idx].join("\n");
            (body.trim_end().to_owned(), false)
        }
        _ => {
            let body = lines[..sentinel_idx].join("\n");
            (body.trim_end().to_owned(), true)
        }
    }
}

/// Run `cmd` in the focused pane of `target`, capturing its exit code.
///
/// Submits the command (plus a sentinel) via the input path — which means
/// it attaches transiently, like `send-keys` — then polls the
/// side-effect-free screen read until the sentinel appears or `timeout`
/// elapses.
///
/// # Errors
///
/// Propagates [`AttachError`] from the input send or the screen reads.
pub async fn run(
    socket: &Path,
    target: AttachTarget,
    cmd: &str,
    nonce: &str,
    timeout: Option<Duration>,
) -> Result<RunOutcome, AttachError> {
    let line = command_line(cmd, nonce);
    let start = Instant::now();
    // Submit the command; learn the exact pane it landed in so we poll the
    // same one we wrote to.
    let pane = send_keys::send(socket, target, &[line, "Enter".to_owned()]).await?;

    loop {
        let screen = get_screen(socket, pane.clone()).await?;
        if let Some((idx, code)) = parse_sentinel(&screen.lines, nonce) {
            let (output, truncated) = extract_output(&screen.lines, idx, nonce);
            return Ok(RunOutcome::Completed(RunResult {
                command: cmd.to_owned(),
                exit_code: code,
                output,
                duration_ms: duration_ms(start),
                truncated,
            }));
        }
        if timeout.is_some_and(|t| start.elapsed() >= t) {
            return Ok(RunOutcome::TimedOut {
                command: cmd.to_owned(),
                duration_ms: duration_ms(start),
                screen,
            });
        }
        tokio::time::sleep(DEFAULT_POLL_INTERVAL).await;
    }
}

/// Milliseconds elapsed since `start`, saturating into `u64`.
fn duration_ms(start: Instant) -> u64 {
    u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_line_appends_sentinel_printf() {
        let line = command_line("ls -la", "42");
        assert_eq!(line, "ls -la; printf '\\nPHUXRC42=%d=END\\n' $?");
    }

    #[test]
    fn parses_exit_code_from_output_line_only() {
        // The echo line carries the literal %d; the output line carries 0.
        let lines = vec![
            "❯ false; printf '\\nPHUXRC42=%d=END\\n' $?".to_owned(),
            "PHUXRC42=1=END".to_owned(),
        ];
        assert_eq!(parse_sentinel(&lines, "42"), Some((1, 1)));
    }

    #[test]
    fn ignores_echo_line_when_output_absent() {
        // Only the echo is visible (command still running): no parse.
        let lines = vec!["❯ sleep 5; printf '\\nPHUXRC42=%d=END\\n' $?".to_owned()];
        assert_eq!(parse_sentinel(&lines, "42"), None);
    }

    #[test]
    fn extracts_output_between_echo_and_sentinel() {
        let lines = vec![
            "❯ echo hi; printf '\\nPHUXRC7=%d=END\\n' $?".to_owned(),
            "hi".to_owned(),
            "PHUXRC7=0=END".to_owned(),
        ];
        let (idx, code) = parse_sentinel(&lines, "7").unwrap();
        let (output, truncated) = extract_output(&lines, idx, "7");
        assert_eq!(code, 0);
        assert_eq!(output, "hi");
        assert!(!truncated);
    }

    #[test]
    fn flags_truncated_when_echo_scrolled_off() {
        let lines = vec![
            "line that scrolled".to_owned(),
            "more output".to_owned(),
            "PHUXRC7=0=END".to_owned(),
        ];
        let (idx, _) = parse_sentinel(&lines, "7").unwrap();
        let (_output, truncated) = extract_output(&lines, idx, "7");
        assert!(truncated);
    }
}
