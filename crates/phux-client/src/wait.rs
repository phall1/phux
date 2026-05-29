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

/// Tracks whether the viewport has held still long enough to count as
/// "settled" for [`Condition::Idle`]. Pulled out of the poll loop so the
/// dwell logic is unit-testable with an injected clock.
#[derive(Debug)]
struct IdleTracker {
    last: Option<Vec<String>>,
    stable_since: Instant,
}

impl IdleTracker {
    const fn new(now: Instant) -> Self {
        Self {
            last: None,
            stable_since: now,
        }
    }

    /// Record the latest `lines` observed at `now`; return `true` once they
    /// have been unchanged for at least `dwell`. The first observation, and
    /// any observation that differs from the previous one, resets the dwell
    /// clock and returns `false`.
    fn observe(&mut self, lines: &[String], now: Instant, dwell: Duration) -> bool {
        if self.last.as_deref() == Some(lines) {
            now.duration_since(self.stable_since) >= dwell
        } else {
            self.stable_since = now;
            self.last = Some(lines.to_vec());
            false
        }
    }
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
    let mut idle = IdleTracker::new(start);

    loop {
        let screen = get_screen(socket, terminal_id.clone()).await?;
        polls = polls.saturating_add(1);

        let met = match condition {
            Condition::Contains(needle) => screen
                .lines
                .iter()
                .any(|line| line.contains(needle.as_str())),
            Condition::Idle(dwell) => idle.observe(&screen.lines, Instant::now(), *dwell),
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

    fn lines(rows: &[&str]) -> Vec<String> {
        rows.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn contains_matches_any_line() {
        let s = screen(&["building", "step 2", "all DONE here"]);
        assert!(s.lines.iter().any(|l| l.contains("DONE")));
        assert!(!s.lines.iter().any(|l| l.contains("MISSING")));
    }

    #[test]
    fn idle_first_observation_never_settles() {
        let t0 = Instant::now();
        let mut idle = IdleTracker::new(t0);
        // Even with a generous gap, the first read just records a baseline.
        assert!(!idle.observe(
            &lines(&["a"]),
            t0 + Duration::from_secs(10),
            Duration::from_millis(50)
        ));
    }

    #[test]
    fn idle_settles_after_dwell_without_change() {
        let t0 = Instant::now();
        let mut idle = IdleTracker::new(t0);
        let dwell = Duration::from_millis(100);
        assert!(!idle.observe(&lines(&["a"]), t0, dwell)); // baseline
        // Unchanged but not yet dwelled.
        assert!(!idle.observe(&lines(&["a"]), t0 + Duration::from_millis(60), dwell));
        // Unchanged and past the dwell.
        assert!(idle.observe(&lines(&["a"]), t0 + Duration::from_millis(160), dwell));
    }

    #[test]
    fn idle_resets_dwell_on_any_change() {
        let t0 = Instant::now();
        let mut idle = IdleTracker::new(t0);
        let dwell = Duration::from_millis(100);
        assert!(!idle.observe(&lines(&["a"]), t0, dwell));
        // Content changes well after the dwell would have elapsed — must NOT
        // settle, and must restart the clock from this moment.
        assert!(!idle.observe(&lines(&["b"]), t0 + Duration::from_millis(500), dwell));
        // 60ms after the change: still not settled.
        assert!(!idle.observe(&lines(&["b"]), t0 + Duration::from_millis(560), dwell));
        // 110ms after the change: settled.
        assert!(idle.observe(&lines(&["b"]), t0 + Duration::from_millis(610), dwell));
    }
}
