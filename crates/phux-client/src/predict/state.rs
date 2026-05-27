//! Prediction state — the queue of in-flight predictions and the policy
//! that decides which keystrokes are safe to predict.
//!
//! The prediction state is *cursor-anchored*, not *cell-anchored*: each
//! prediction records the (row, col) at which it expects to paint, plus
//! the character to paint there. The state machine maintains a small
//! cursor estimate that walks forward by one column per printable
//! prediction and backward by one column per backspace prediction, so
//! consecutive predictions stack into a horizontal run.
//!
//! See the module-level docs in [`super`] for the rationale on which key
//! classes are predicted and why the visual decoration is underline.

use std::collections::VecDeque;

use phux_protocol::input::key::{KeyEvent, ModSet, PhysicalKey};

/// Per-client knob for predictive echo.
///
/// Wire to [`crate::attach::run_with_predict`]. Default is `enabled: false`
/// — predictive echo is off until field-proven. Future config keys
/// (timeout, decoration choice, RTT-adaptive predict policy) belong here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PredictiveConfig {
    /// Whether to apply predictive local echo at all. `false` ⇒ the
    /// attach loop bypasses the prediction layer entirely.
    pub enabled: bool,
}

impl PredictiveConfig {
    /// Convenience constructor — predictive echo on.
    #[must_use]
    pub const fn enabled() -> Self {
        Self { enabled: true }
    }

    /// Convenience constructor — predictive echo off (the default).
    #[must_use]
    pub const fn disabled() -> Self {
        Self { enabled: false }
    }
}

/// One in-flight prediction: a single-cell visual edit guessed from a
/// keystroke that has been sent upstream but not yet confirmed by a
/// `TerminalOutput` frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prediction {
    /// Row of the cell, 0-indexed from the top of the viewport.
    pub row: u16,
    /// Column of the cell, 0-indexed.
    pub col: u16,
    /// What the prediction wants the cell to display. For backspace
    /// predictions this is a single space (`' '`) — visually "erase".
    pub ch: char,
    /// Kind of prediction, for reconciliation diagnostics + the future
    /// per-class confirm/contradict bookkeeping.
    pub kind: PredictionKind,
}

/// What the prediction was modelling. Carried for diagnostics; the
/// reconcile path is class-agnostic in v0 (it drops everything on any
/// server output).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PredictionKind {
    /// A printable ASCII byte: forward cursor, paint the char.
    Insert,
    /// A backspace at end-of-line: cursor back one column, blank the cell.
    BackspaceEol,
}

/// What [`PredictionState::predict_key`] decided about a key event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PredictionOutcome {
    /// The key was predicted; the new prediction is at `queue.back()`.
    Predicted,
    /// The key was outside the safe set — no prediction was made, but the
    /// key still travels upstream as normal. This is the "fall through"
    /// path that keeps the slow-but-correct behaviour for everything we
    /// don't yet feel safe predicting.
    Skipped,
    /// Predictive echo is disabled by config; the caller should treat this
    /// like `Skipped`.
    Disabled,
}

/// The client-side prediction state.
///
/// The state is updated in two places:
///
/// - [`Self::predict_key`] is called on the keystroke path, before the
///   `INPUT_KEY` frame is sent. It either enqueues a prediction (and
///   advances the cursor estimate) or returns [`PredictionOutcome::Skipped`].
/// - [`super::reconcile_terminal_output`] is called on the server-frame
///   path when a `TerminalOutput` arrives. It drains the queue and resyncs
///   the cursor estimate from authoritative state.
#[derive(Debug, Default)]
pub struct PredictionState {
    cfg: PredictiveConfig,
    /// FIFO of pending predictions. Ordered by issue time so the overlay
    /// can paint them left-to-right.
    pending: VecDeque<Prediction>,
    /// Best estimate of where the server's cursor will be after applying
    /// every pending prediction. Updated immediately by `predict_key`;
    /// re-synced from authoritative state by `reconcile_*`.
    cursor_row: u16,
    cursor_col: u16,
    /// Viewport size, in cells. The cursor estimate is clamped to
    /// `0..cols` and `0..rows` — we conservatively skip predicting at the
    /// last column to avoid guessing wrap behaviour.
    cols: u16,
    rows: u16,
}

impl PredictionState {
    /// New state with predictive echo configured per `cfg` and an
    /// initial viewport of `cols × rows`.
    #[must_use]
    pub const fn new(cfg: PredictiveConfig, cols: u16, rows: u16) -> Self {
        Self {
            cfg,
            pending: VecDeque::new(),
            cursor_row: 0,
            cursor_col: 0,
            cols,
            rows,
        }
    }

    /// Whether predictive echo is currently on.
    #[must_use]
    pub const fn is_enabled(&self) -> bool {
        self.cfg.enabled
    }

    /// Update the cached viewport. Called on SIGWINCH / snapshot frames
    /// so the safe-prediction guard (skip last column, clamp rows)
    /// tracks the actual size.
    ///
    /// Resizing clears the prediction queue: the predicted cells'
    /// coordinates were anchored to the previous viewport and we'd
    /// rather drop them than risk painting at a stale position.
    pub fn set_viewport(&mut self, cols: u16, rows: u16) {
        self.cols = cols;
        self.rows = rows;
        self.pending.clear();
        self.cursor_row = self.cursor_row.min(rows.saturating_sub(1));
        self.cursor_col = self.cursor_col.min(cols.saturating_sub(1));
    }

    /// Re-anchor the cursor estimate from authoritative state. Called
    /// from the reconcile path after a `TerminalOutput` has been
    /// applied to the libghostty Terminal — the renderer's `RenderState`
    /// has the post-apply cursor, and we copy it here.
    pub fn set_cursor(&mut self, row: u16, col: u16) {
        self.cursor_row = row.min(self.rows.saturating_sub(1));
        self.cursor_col = col.min(self.cols.saturating_sub(1));
    }

    /// Number of predictions waiting for confirmation.
    #[must_use]
    pub fn pending_len(&self) -> usize {
        self.pending.len()
    }

    /// Current cursor estimate `(row, col)`. Updated by `predict_key`
    /// and `set_cursor`; exposed for diagnostics and tests.
    #[must_use]
    pub const fn cursor(&self) -> (u16, u16) {
        (self.cursor_row, self.cursor_col)
    }

    /// Current viewport `(cols, rows)`. Exposed for diagnostics and
    /// tests; production callers do not need this.
    #[must_use]
    pub const fn viewport(&self) -> (u16, u16) {
        (self.cols, self.rows)
    }

    /// Read-only access to the pending queue (used by the overlay).
    pub fn pending(&self) -> impl Iterator<Item = &Prediction> {
        self.pending.iter()
    }

    /// Drop every pending prediction. Called by the reconcile path.
    pub fn clear(&mut self) {
        self.pending.clear();
    }

    /// Try to predict the visual effect of `event`. Returns the outcome
    /// so the caller can record it (and, in tests, assert on the
    /// classification decision).
    ///
    /// Safe classes (see module docs):
    ///
    /// - Printable ASCII single-character `text` payload, no Ctrl / Alt /
    ///   Super modifier active. SHIFT is fine (it's part of producing the
    ///   character).
    /// - `PhysicalKey::Backspace`, no modifier, when the cursor is past
    ///   column 0 and below the last column.
    pub fn predict_key(&mut self, event: &KeyEvent) -> PredictionOutcome {
        if !self.cfg.enabled {
            return PredictionOutcome::Disabled;
        }
        // Reject any non-Press action — repeats and releases produce
        // their own server-side echo path we don't model yet.
        if !matches!(event.action, phux_protocol::input::key::KeyAction::Press) {
            return PredictionOutcome::Skipped;
        }
        // Reject keystrokes with any "command-y" modifier active. SHIFT
        // is OK because it's already baked into `text` for letters.
        let blocking_mods = ModSet::CTRL | ModSet::ALT | ModSet::SUPER;
        if event.mods.intersects(blocking_mods) {
            return PredictionOutcome::Skipped;
        }

        if event.key == PhysicalKey::Backspace {
            return self.predict_backspace_eol();
        }

        // Printable single-char insert.
        let Some(text) = event.text.as_deref() else {
            return PredictionOutcome::Skipped;
        };
        let mut chars = text.chars();
        let (Some(ch), None) = (chars.next(), chars.next()) else {
            return PredictionOutcome::Skipped;
        };
        if !is_safe_printable(ch) {
            return PredictionOutcome::Skipped;
        }
        self.predict_insert(ch)
    }

    fn predict_insert(&mut self, ch: char) -> PredictionOutcome {
        // Conservative: refuse to predict at the rightmost column. The
        // server may wrap, may scroll, may stay (DECAWM off) — we don't
        // know which from here. The next reconcile will resync.
        if self.cols == 0 || self.cursor_col + 1 >= self.cols {
            return PredictionOutcome::Skipped;
        }
        if self.rows == 0 || self.cursor_row >= self.rows {
            return PredictionOutcome::Skipped;
        }
        self.pending.push_back(Prediction {
            row: self.cursor_row,
            col: self.cursor_col,
            ch,
            kind: PredictionKind::Insert,
        });
        self.cursor_col = self.cursor_col.saturating_add(1);
        PredictionOutcome::Predicted
    }

    fn predict_backspace_eol(&mut self) -> PredictionOutcome {
        // Backspace at column 0 is "move to end of previous line" on most
        // shells (or no-op). We refuse to guess either way.
        if self.cursor_col == 0 {
            return PredictionOutcome::Skipped;
        }
        if self.rows == 0 || self.cursor_row >= self.rows {
            return PredictionOutcome::Skipped;
        }
        let new_col = self.cursor_col - 1;
        self.pending.push_back(Prediction {
            row: self.cursor_row,
            col: new_col,
            ch: ' ',
            kind: PredictionKind::BackspaceEol,
        });
        self.cursor_col = new_col;
        PredictionOutcome::Predicted
    }
}

/// Printable ASCII (excluding DEL and space-as-control). Space (`0x20`)
/// is included — predicting "user typed a space" is a common case.
const fn is_safe_printable(ch: char) -> bool {
    let c = ch as u32;
    c >= 0x20 && c <= 0x7E
}

#[cfg(test)]
#[allow(clippy::expect_used, reason = "tests")]
mod tests {
    use super::*;
    use phux_protocol::input::key::KeyAction;

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

    fn key_named(k: PhysicalKey, mods: ModSet) -> KeyEvent {
        KeyEvent {
            action: KeyAction::Press,
            key: k,
            mods,
            consumed_mods: ModSet::empty(),
            composing: false,
            text: None,
            unshifted_codepoint: None,
        }
    }

    #[test]
    fn disabled_config_skips_all_predictions() {
        let mut s = PredictionState::new(PredictiveConfig::disabled(), 80, 24);
        let r = s.predict_key(&key_text("a"));
        assert_eq!(r, PredictionOutcome::Disabled);
        assert_eq!(s.pending_len(), 0);
    }

    #[test]
    fn printable_ascii_is_predicted_and_advances_cursor() {
        let mut s = PredictionState::new(PredictiveConfig::enabled(), 80, 24);
        let r = s.predict_key(&key_text("a"));
        assert_eq!(r, PredictionOutcome::Predicted);
        assert_eq!(s.pending_len(), 1);
        let p = s.pending().next().expect("one prediction");
        assert_eq!(p.ch, 'a');
        assert_eq!(p.col, 0);
        assert_eq!(p.row, 0);
        assert_eq!(p.kind, PredictionKind::Insert);
        assert_eq!(s.cursor_col, 1);
    }

    #[test]
    fn multiple_keystrokes_stack_horizontally() {
        let mut s = PredictionState::new(PredictiveConfig::enabled(), 80, 24);
        for ch in ["h", "e", "l", "l", "o"] {
            assert_eq!(s.predict_key(&key_text(ch)), PredictionOutcome::Predicted);
        }
        let cols: Vec<u16> = s.pending().map(|p| p.col).collect();
        assert_eq!(cols, vec![0, 1, 2, 3, 4]);
        assert_eq!(s.cursor_col, 5);
    }

    #[test]
    fn ctrl_modifier_is_not_predicted() {
        let mut s = PredictionState::new(PredictiveConfig::enabled(), 80, 24);
        let mut ev = key_text("a");
        ev.mods = ModSet::CTRL;
        assert_eq!(s.predict_key(&ev), PredictionOutcome::Skipped);
        assert_eq!(s.pending_len(), 0);
    }

    #[test]
    fn alt_modifier_is_not_predicted() {
        let mut s = PredictionState::new(PredictiveConfig::enabled(), 80, 24);
        let mut ev = key_text("a");
        ev.mods = ModSet::ALT;
        assert_eq!(s.predict_key(&ev), PredictionOutcome::Skipped);
    }

    #[test]
    fn non_press_action_is_not_predicted() {
        let mut s = PredictionState::new(PredictiveConfig::enabled(), 80, 24);
        let mut ev = key_text("a");
        ev.action = KeyAction::Release;
        assert_eq!(s.predict_key(&ev), PredictionOutcome::Skipped);
    }

    #[test]
    fn arrow_key_is_not_predicted() {
        let mut s = PredictionState::new(PredictiveConfig::enabled(), 80, 24);
        let ev = key_named(PhysicalKey::ArrowLeft, ModSet::empty());
        assert_eq!(s.predict_key(&ev), PredictionOutcome::Skipped);
    }

    #[test]
    fn enter_is_not_predicted() {
        let mut s = PredictionState::new(PredictiveConfig::enabled(), 80, 24);
        let ev = key_named(PhysicalKey::Enter, ModSet::empty());
        assert_eq!(s.predict_key(&ev), PredictionOutcome::Skipped);
    }

    #[test]
    fn tab_is_not_predicted() {
        let mut s = PredictionState::new(PredictiveConfig::enabled(), 80, 24);
        let ev = key_named(PhysicalKey::Tab, ModSet::empty());
        assert_eq!(s.predict_key(&ev), PredictionOutcome::Skipped);
    }

    #[test]
    fn backspace_at_col_zero_is_not_predicted() {
        let mut s = PredictionState::new(PredictiveConfig::enabled(), 80, 24);
        let ev = key_named(PhysicalKey::Backspace, ModSet::empty());
        assert_eq!(s.predict_key(&ev), PredictionOutcome::Skipped);
    }

    #[test]
    fn backspace_after_insert_is_predicted_and_decrements_cursor() {
        let mut s = PredictionState::new(PredictiveConfig::enabled(), 80, 24);
        assert_eq!(s.predict_key(&key_text("a")), PredictionOutcome::Predicted);
        assert_eq!(s.cursor_col, 1);
        let bs = key_named(PhysicalKey::Backspace, ModSet::empty());
        assert_eq!(s.predict_key(&bs), PredictionOutcome::Predicted);
        assert_eq!(s.cursor_col, 0);
        assert_eq!(s.pending_len(), 2);
        let last = s.pending().last().expect("two predictions");
        assert_eq!(last.kind, PredictionKind::BackspaceEol);
        assert_eq!(last.col, 0);
        assert_eq!(last.ch, ' ');
    }

    #[test]
    fn rightmost_column_insert_is_not_predicted() {
        let mut s = PredictionState::new(PredictiveConfig::enabled(), 5, 24);
        // Push to col 4 (last column on a 5-wide viewport).
        for _ in 0..4 {
            s.predict_key(&key_text("x"));
        }
        assert_eq!(s.cursor_col, 4);
        // 5th press would land at col 4 (the last); refuse.
        assert_eq!(s.predict_key(&key_text("x")), PredictionOutcome::Skipped);
    }

    #[test]
    fn set_viewport_clears_predictions() {
        let mut s = PredictionState::new(PredictiveConfig::enabled(), 80, 24);
        s.predict_key(&key_text("a"));
        s.predict_key(&key_text("b"));
        assert_eq!(s.pending_len(), 2);
        s.set_viewport(100, 30);
        assert_eq!(s.pending_len(), 0);
        assert_eq!(s.cols, 100);
        assert_eq!(s.rows, 30);
    }

    #[test]
    fn set_cursor_clamps_to_viewport() {
        let mut s = PredictionState::new(PredictiveConfig::enabled(), 80, 24);
        s.set_cursor(50, 200);
        assert_eq!(s.cursor_row, 23);
        assert_eq!(s.cursor_col, 79);
    }

    #[test]
    fn shift_modifier_is_fine_for_uppercase() {
        let mut s = PredictionState::new(PredictiveConfig::enabled(), 80, 24);
        let mut ev = key_text("A");
        ev.mods = ModSet::SHIFT;
        assert_eq!(s.predict_key(&ev), PredictionOutcome::Predicted);
    }

    #[test]
    fn multibyte_text_is_not_predicted_v0() {
        // U+00E9 — single codepoint but we conservatively only predict
        // ASCII for v0. Widen later.
        let mut s = PredictionState::new(PredictiveConfig::enabled(), 80, 24);
        let ev = key_text("é");
        assert_eq!(s.predict_key(&ev), PredictionOutcome::Skipped);
    }

    #[test]
    fn empty_text_is_not_predicted() {
        let mut s = PredictionState::new(PredictiveConfig::enabled(), 80, 24);
        let mut ev = key_text("a");
        ev.text = None;
        assert_eq!(s.predict_key(&ev), PredictionOutcome::Skipped);
    }

    #[test]
    fn del_byte_is_not_predicted_as_printable() {
        let mut s = PredictionState::new(PredictiveConfig::enabled(), 80, 24);
        let ev = key_text("\x7f");
        assert_eq!(s.predict_key(&ev), PredictionOutcome::Skipped);
    }
}
