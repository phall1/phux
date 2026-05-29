//! Poll-floor wait primitive (phux-cfd, ADR-0022 §4).
//!
//! `wait` is the floor of the event surface: rather than a wire
//! subscription, it polls the side-effect-free [`get_screen`] read until a
//! condition holds. This always works — no shell integration, no new wire
//! frames — and because [`get_screen`] never attaches or resizes, polling
//! is safe against a pane another client is using.
//!
//! Conditions are evaluated **client-side** against each fresh
//! [`ScreenState`]. That is deliberate (ADR-0022 §4, "no one-way doors"):
//! the matchable set grows here as ordinary code, never as a frozen wire
//! enum. Server-pushed events (`command_finished`, `bell`, …) are a future
//! *additive* acceleration of this same contract, not a replacement.
//!
//! [`get_screen`]: crate::snapshot::get_screen

use std::path::Path;
use std::time::Duration;

use phux_core::screen::ScreenState;
use phux_protocol::ids::TerminalId;
use tokio::time::Instant;

use crate::attach::AttachError;
use crate::snapshot::get_screen;

/// Default gap between polls. Below human settle perception, well above
/// the per-poll round-trip cost on a local UDS.
pub const DEFAULT_POLL_INTERVAL: Duration = Duration::from_millis(150);

/// Default dwell for [`Condition::Idle`] when the caller gives no explicit
/// duration: the pane must hold still this long to count as settled.
pub const DEFAULT_IDLE_DWELL: Duration = Duration::from_millis(500);

/// What a poll loop waits for. Evaluated CLI-side against each fresh
/// screen, so new variants are additive (ADR-0022 §4).
#[derive(Debug, Clone)]
pub enum Condition {
    /// Met once any viewport line contains this substring. (Regex is an
    /// additive future refinement of the same `--until` flag — substring
    /// is the dependency-free floor.)
    Contains(String),
    /// Met once the viewport text holds still for this long — the pane has
    /// "settled" (output stopped, prompt likely back).
    Idle(Duration),
}

/// Why [`poll_until`] returned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaitOutcome {
    /// The [`Condition`] was satisfied.
    Met,
    /// The overall timeout elapsed before the condition held.
    TimedOut,
}

/// The result of a poll loop: why it stopped, the last screen observed,
/// and how many reads it took (useful for `--json` and diagnostics).
#[derive(Debug, Clone)]
pub struct WaitResult {
    /// Why polling stopped.
    pub outcome: WaitOutcome,
    /// The most recent screen read.
    pub screen: ScreenState,
    /// Number of [`get_screen`] reads performed.
    pub polls: u32,
}

/// Poll `terminal_id` until `condition` holds or `timeout` elapses.
///
/// Reads the screen every `interval` via the side-effect-free
/// [`get_screen`]. Returns [`WaitOutcome::TimedOut`] (not an error) when
/// the deadline passes first, so callers map the two outcomes to their own
/// exit codes. `timeout = None` waits indefinitely.
///
/// # Errors
///
/// Propagates [`AttachError`] from the underlying screen read (no server,
/// transport failure, unknown terminal).
pub async fn poll_until(
    socket: &Path,
    terminal_id: TerminalId,
    condition: &Condition,
    timeout: Option<Duration>,
    interval: Duration,
) -> Result<WaitResult, AttachError> {
    let start = Instant::now();
    let mut polls: u32 = 0;
    // Idle bookkeeping: the last content we saw and when it last changed.
    let mut last_lines: Option<Vec<String>> = None;
    let mut stable_since = start;

    loop {
        let screen = get_screen(socket, terminal_id.clone()).await?;
        polls = polls.saturating_add(1);

        let met = match condition {
            Condition::Contains(needle) => screen
                .lines
                .iter()
                .any(|line| line.contains(needle.as_str())),
            Condition::Idle(dwell) => {
                if last_lines.as_ref() == Some(&screen.lines) {
                    stable_since.elapsed() >= *dwell
                } else {
                    // Content changed (or first read): reset the dwell clock.
                    stable_since = Instant::now();
                    last_lines = Some(screen.lines.clone());
                    false
                }
            }
        };
        if met {
            return Ok(WaitResult {
                outcome: WaitOutcome::Met,
                screen,
                polls,
            });
        }
        if timeout.is_some_and(|t| start.elapsed() >= t) {
            return Ok(WaitResult {
                outcome: WaitOutcome::TimedOut,
                screen,
                polls,
            });
        }
        tokio::time::sleep(interval).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen(lines: &[&str]) -> ScreenState {
        ScreenState {
            schema_version: phux_core::screen::SCHEMA_VERSION,
            pane: 1,
            cols: 80,
            rows: u16::try_from(lines.len()).unwrap_or(0),
            cursor: None,
            lines: lines.iter().map(|s| (*s).to_owned()).collect(),
        }
    }

    #[test]
    fn contains_matches_any_line() {
        let cond = Condition::Contains("DONE".to_owned());
        let s = screen(&["building", "step 2", "all DONE here"]);
        let met = matches!(&cond, Condition::Contains(n) if s.lines.iter().any(|l| l.contains(n)));
        assert!(met);
    }

    #[test]
    fn contains_misses_when_absent() {
        let cond = Condition::Contains("DONE".to_owned());
        let s = screen(&["building", "step 2"]);
        let met = matches!(&cond, Condition::Contains(n) if s.lines.iter().any(|l| l.contains(n)));
        assert!(!met);
    }
}
