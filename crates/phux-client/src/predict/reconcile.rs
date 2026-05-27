//! Reconciliation — drain predictions when authoritative state arrives.
//!
//! See [`super`] for the rationale on why v0 reconciliation is a wholesale
//! drain rather than a per-character match game. The short version: the
//! renderer is about to repaint the affected rows from authoritative
//! libghostty state on its next pass, so any prediction sitting on top
//! of stdout is going to be overwritten regardless.
//!
//! The interesting policy is the cursor resync: after a `TerminalOutput`
//! has been applied to the libghostty Terminal, we copy the post-apply
//! cursor coordinates into the [`PredictionState`] so the next
//! prediction queues at the right anchor.

use super::state::PredictionState;

/// Drain pending predictions and re-anchor the cursor estimate.
///
/// `cursor_row` / `cursor_col` come from `RenderState::cursor_viewport`
/// after the latest `TerminalOutput` has been applied. The caller — the
/// attach driver — has that information from the same render path it
/// uses to redraw, so threading it through is free.
///
/// Returns the number of predictions that were dropped. Callers can
/// log this for diagnostics; the test suite asserts on it to prove
/// the rollback path runs.
pub fn reconcile_terminal_output(
    state: &mut PredictionState,
    cursor_row: u16,
    cursor_col: u16,
) -> usize {
    let dropped = state.pending_len();
    state.clear();
    state.set_cursor(cursor_row, cursor_col);
    dropped
}

#[cfg(test)]
#[allow(clippy::expect_used, reason = "tests")]
mod tests {
    use super::*;
    use crate::predict::state::{PredictionState, PredictiveConfig};
    use phux_protocol::input::key::{KeyAction, KeyEvent, ModSet, PhysicalKey};

    fn key_text(s: &str) -> KeyEvent {
        KeyEvent {
            action: KeyAction::Press,
            key: PhysicalKey::A,
            mods: ModSet::empty(),
            consumed_mods: ModSet::empty(),
            composing: false,
            text: Some(s.to_owned()),
            unshifted_codepoint: s.chars().next().map(u32::from),
        }
    }

    /// Confirm path: server output arrives with the cursor sitting where
    /// the prediction said it would. We still drop the prediction (v0
    /// semantics) — the test asserts the queue empties and the cursor
    /// estimate now matches authoritative state.
    #[test]
    fn confirm_path_drops_prediction_and_updates_cursor() {
        let mut s = PredictionState::new(PredictiveConfig::enabled(), 80, 24);
        s.predict_key(&key_text("a"));
        assert_eq!(s.pending_len(), 1);
        // Server confirms: cursor advanced to col 1.
        let dropped = reconcile_terminal_output(&mut s, 0, 1);
        assert_eq!(dropped, 1);
        assert_eq!(s.pending_len(), 0);
        assert_eq!(s.cursor(), (0, 1));
    }

    /// Contradict path: server output places the cursor somewhere the
    /// predictions did not anticipate (e.g. the program did its own
    /// cursor motion). Predictions are dropped, the cursor estimate
    /// snaps to the authoritative value.
    #[test]
    fn contradict_path_rolls_back_cleanly() {
        let mut s = PredictionState::new(PredictiveConfig::enabled(), 80, 24);
        for ch in ["a", "b", "c"] {
            s.predict_key(&key_text(ch));
        }
        assert_eq!(s.pending_len(), 3);
        assert_eq!(s.cursor(), (0, 3));
        // Server says: cursor jumped to (5, 10). All predictions wrong.
        let dropped = reconcile_terminal_output(&mut s, 5, 10);
        assert_eq!(dropped, 3);
        assert_eq!(s.pending_len(), 0);
        assert_eq!(s.cursor(), (5, 10));
    }

    /// Slow link: many predictions queued, then a single reconcile drains
    /// them all. Demonstrates the cumulative-ack semantics — one server
    /// frame is enough to clear the entire backlog.
    #[test]
    fn slow_link_many_predictions_drain_at_once() {
        let mut s = PredictionState::new(PredictiveConfig::enabled(), 80, 24);
        // Type 20 characters before any echo arrives.
        for _ in 0..20 {
            s.predict_key(&key_text("x"));
        }
        assert_eq!(s.pending_len(), 20);
        assert_eq!(s.cursor(), (0, 20));
        // Server has now caught up: cursor at col 20.
        let dropped = reconcile_terminal_output(&mut s, 0, 20);
        assert_eq!(dropped, 20);
        assert_eq!(s.pending_len(), 0);
        assert_eq!(s.cursor(), (0, 20));
    }

    /// Reconcile is a no-op on an empty queue. The cursor still resyncs
    /// (the renderer might have done its own redraw for reasons other
    /// than our predictions — bell, refocus, etc).
    #[test]
    fn empty_queue_is_a_noop_but_resyncs_cursor() {
        let mut s = PredictionState::new(PredictiveConfig::enabled(), 80, 24);
        let dropped = reconcile_terminal_output(&mut s, 7, 4);
        assert_eq!(dropped, 0);
        assert_eq!(s.cursor(), (7, 4));
    }
}
