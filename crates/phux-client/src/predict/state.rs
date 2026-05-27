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
use unicode_width::UnicodeWidthChar;

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
    /// Column of the cell, 0-indexed. For cursor-motion predictions
    /// ([`PredictionKind::CursorLeft`] / [`PredictionKind::CursorRight`])
    /// this is the *target* column after the motion — the predict-side
    /// cursor has been moved there already and reconcile confirms when
    /// the authoritative cursor catches up to it.
    pub col: u16,
    /// What the prediction wants the cell to display. For backspace
    /// predictions this is a single space (`' '`) — visually "erase".
    /// For cursor-motion predictions this is a placeholder space; the
    /// overlay paints no cell for them.
    pub ch: char,
    /// Cell width of [`Self::ch`], in columns. `1` for ASCII and most
    /// scripts, `2` for CJK / wide emoji (phux-9gw.1.4). Cursor-motion
    /// and `Newline` predictions store `0` — they paint no cell.
    pub width: u8,
    /// Kind of prediction, for reconciliation diagnostics + the
    /// per-class confirm/contradict bookkeeping.
    pub kind: PredictionKind,
}

/// What the prediction was modelling. The per-cell reconcile path branches
/// on this to decide what counts as "confirmed" vs "contradicted" — see
/// the sibling `reconcile` module for the match table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PredictionKind {
    /// A printable byte: forward cursor, paint the char. Reconcile
    /// confirms when the authoritative cell at `(row, col)` has `ch` as
    /// its base grapheme.
    Insert,
    /// A backspace at end-of-line: cursor back one column, blank the cell.
    /// Reconcile confirms when the authoritative cell at `(row, col)` is
    /// blank (no grapheme or a single space).
    BackspaceEol,
    /// An Enter keystroke after the user typed onto the current row:
    /// cursor jumps to `(row+1, 0)`. Paints no overlay cell — the
    /// prediction is purely a cursor-motion estimate so subsequent
    /// inserts on the next row anchor correctly. Reconcile confirms
    /// when the authoritative cursor has advanced past `pred.row`.
    Newline,
    /// Left arrow over a known cell on the current line (phux-9gw.1.3).
    /// Paints no overlay; reconcile confirms when the authoritative
    /// cursor matches `(pred.row, pred.col)`.
    CursorLeft,
    /// Right arrow over a known cell on the current line (phux-9gw.1.3).
    /// Paints no overlay; reconcile confirms when the authoritative
    /// cursor matches `(pred.row, pred.col)`.
    CursorRight,
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

    /// Drop the prediction at the front of the queue. Used by the
    /// per-cell reconcile to consume confirmed predictions one at a
    /// time. Returns the dropped prediction so callers can log.
    pub(crate) fn pop_front(&mut self) -> Option<Prediction> {
        self.pending.pop_front()
    }

    /// Peek at the prediction at the front of the queue.
    pub(crate) fn front(&self) -> Option<&Prediction> {
        self.pending.front()
    }

    /// Try to predict the visual effect of `event`. Returns the outcome
    /// so the caller can record it (and, in tests, assert on the
    /// classification decision).
    ///
    /// Safe classes (see module docs):
    ///
    /// - Single-Unicode-scalar `text` payload (any printable code point
    ///   with cell width 1 or 2 per `unicode-width`), no Ctrl / Alt /
    ///   Super modifier active. SHIFT is fine (it's part of producing the
    ///   character).
    /// - `PhysicalKey::Backspace`, no modifier, when the cursor is past
    ///   column 0 and below the last column.
    /// - `PhysicalKey::Enter`, no modifier, past column 0 and on a row
    ///   that is not the last viewport row.
    ///
    /// Cursor-motion arrow predictions ([`PhysicalKey::ArrowLeft`] /
    /// [`PhysicalKey::ArrowRight`]) require a peek at the cell grid so
    /// the predict layer knows the width of the grapheme being stepped
    /// over; they are routed through [`Self::predict_key_with_grid`].
    /// This entry point skips arrows.
    pub fn predict_key(&mut self, event: &KeyEvent) -> PredictionOutcome {
        self.predict_key_with_grid(event, |_, _| None)
    }

    /// Same as [`Self::predict_key`] but with a read closure into the
    /// authoritative cell grid. Used by the attach driver, which has
    /// the libghostty `Terminal` + `TerminalRenderer` on hand and can
    /// supply a per-cell grapheme lookup via `read_grapheme_at`.
    ///
    /// The closure is invoked at most once per call, only when a
    /// cursor-motion arrow needs to know the width of the grapheme it
    /// would step over.
    pub fn predict_key_with_grid<F>(
        &mut self,
        event: &KeyEvent,
        mut read_cell: F,
    ) -> PredictionOutcome
    where
        F: FnMut(u16, u16) -> Option<char>,
    {
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
        if event.key == PhysicalKey::Enter {
            return self.predict_enter();
        }
        if event.key == PhysicalKey::ArrowLeft {
            return self.predict_arrow_left(&mut read_cell);
        }
        if event.key == PhysicalKey::ArrowRight {
            return self.predict_arrow_right(&mut read_cell);
        }

        // Printable single-char insert.
        let Some(text) = event.text.as_deref() else {
            return PredictionOutcome::Skipped;
        };
        let mut chars = text.chars();
        let (Some(ch), None) = (chars.next(), chars.next()) else {
            // Multi-codepoint grapheme (ZWJ sequence, combining marks).
            // Deferred — phux-9gw.1.4 ships single-scalar only.
            return PredictionOutcome::Skipped;
        };
        if !is_safe_predictable(ch) {
            return PredictionOutcome::Skipped;
        }
        let Some(width) = grapheme_width(ch) else {
            return PredictionOutcome::Skipped;
        };
        self.predict_insert(ch, width)
    }

    /// Predict an Enter keystroke as a cursor jump to `(row+1, 0)`.
    ///
    /// Conservative gate: only predict when (a) the cursor is past
    /// column 0 *and* there is already an `Insert` prediction queued on
    /// the current row, or (b) the cursor is past column 0 outright.
    /// Condition (a) is a strong proxy for "the user just typed text on
    /// this line, so the next Enter is line submission" — the bulk of
    /// the latency-hiding win. Without (a) we still predict, because
    /// the cell-match reconcile will drop the Newline cheaply on
    /// contradiction.
    ///
    /// Refuses to predict at the last row (would need to model scroll)
    /// or at column 0 (would need to know whether the shell discards a
    /// bare Enter or echoes a fresh prompt).
    fn predict_enter(&mut self) -> PredictionOutcome {
        if self.cursor_col == 0 {
            return PredictionOutcome::Skipped;
        }
        if self.rows == 0 || self.cursor_row.saturating_add(1) >= self.rows {
            return PredictionOutcome::Skipped;
        }
        let pred_row = self.cursor_row;
        self.pending.push_back(Prediction {
            row: pred_row,
            col: self.cursor_col,
            ch: '\n',
            width: 0,
            kind: PredictionKind::Newline,
        });
        // Advance the cursor estimate so subsequent inserts queue on the
        // next row at column 0.
        self.cursor_row = self.cursor_row.saturating_add(1);
        self.cursor_col = 0;
        PredictionOutcome::Predicted
    }

    fn predict_insert(&mut self, ch: char, width: u8) -> PredictionOutcome {
        if self.cols == 0 || self.rows == 0 || self.cursor_row >= self.rows {
            return PredictionOutcome::Skipped;
        }
        // Conservative: refuse to predict at or past the rightmost column.
        // The server may wrap, may scroll, may stay (DECAWM off) — we
        // don't know which from here. For width-2 graphemes we also need
        // the *next* column to fit. The next reconcile will resync.
        let advance = u16::from(width);
        if advance == 0 {
            return PredictionOutcome::Skipped;
        }
        let end_col = self.cursor_col.saturating_add(advance);
        if end_col >= self.cols {
            return PredictionOutcome::Skipped;
        }
        self.pending.push_back(Prediction {
            row: self.cursor_row,
            col: self.cursor_col,
            ch,
            width,
            kind: PredictionKind::Insert,
        });
        self.cursor_col = end_col;
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
            width: 1,
            kind: PredictionKind::BackspaceEol,
        });
        self.cursor_col = new_col;
        PredictionOutcome::Predicted
    }

    /// Predict a left-arrow keystroke (phux-9gw.1.3).
    ///
    /// Reads the cell immediately to the left of the predict cursor via
    /// `read_cell`. If that cell has a known grapheme we retreat by its
    /// cell width; if the cell is blank we refuse to predict — we have
    /// no anchor to know whether the cursor would land on the prompt,
    /// on a wide grapheme's tail, or on blank text. Refuses at column 0.
    fn predict_arrow_left<F>(&mut self, read_cell: &mut F) -> PredictionOutcome
    where
        F: FnMut(u16, u16) -> Option<char>,
    {
        if self.cursor_col == 0 {
            return PredictionOutcome::Skipped;
        }
        if self.rows == 0 || self.cursor_row >= self.rows {
            return PredictionOutcome::Skipped;
        }
        let probe_col = self.cursor_col - 1;
        let Some(ch) = read_cell(self.cursor_row, probe_col) else {
            return PredictionOutcome::Skipped;
        };
        let Some(width) = grapheme_width(ch) else {
            return PredictionOutcome::Skipped;
        };
        let advance = u16::from(width);
        if advance == 0 {
            return PredictionOutcome::Skipped;
        }
        // Width-2 case: the grapheme's base cell is at `probe_col - 1`
        // and the tail occupies `probe_col`. We want to land the cursor
        // on the base. Refuse if that would go below 0.
        let new_col = if advance == 1 {
            probe_col
        } else if self.cursor_col >= advance {
            self.cursor_col - advance
        } else {
            return PredictionOutcome::Skipped;
        };
        self.pending.push_back(Prediction {
            row: self.cursor_row,
            col: new_col,
            ch: ' ',
            width: 0,
            kind: PredictionKind::CursorLeft,
        });
        self.cursor_col = new_col;
        PredictionOutcome::Predicted
    }

    /// Predict a right-arrow keystroke (phux-9gw.1.3).
    ///
    /// Reads the cell at the predict cursor. If that cell has a known
    /// grapheme we advance by its cell width; blank → refuse. Refuses
    /// past the right edge.
    fn predict_arrow_right<F>(&mut self, read_cell: &mut F) -> PredictionOutcome
    where
        F: FnMut(u16, u16) -> Option<char>,
    {
        if self.cols == 0 || self.cursor_col >= self.cols {
            return PredictionOutcome::Skipped;
        }
        if self.rows == 0 || self.cursor_row >= self.rows {
            return PredictionOutcome::Skipped;
        }
        let Some(ch) = read_cell(self.cursor_row, self.cursor_col) else {
            return PredictionOutcome::Skipped;
        };
        let Some(width) = grapheme_width(ch) else {
            return PredictionOutcome::Skipped;
        };
        let advance = u16::from(width);
        if advance == 0 {
            return PredictionOutcome::Skipped;
        }
        let new_col = self.cursor_col.saturating_add(advance);
        // Refuse to land at or past the rightmost column — same
        // wrap-conservatism as `predict_insert`.
        if new_col >= self.cols {
            return PredictionOutcome::Skipped;
        }
        self.pending.push_back(Prediction {
            row: self.cursor_row,
            col: new_col,
            ch: ' ',
            width: 0,
            kind: PredictionKind::CursorRight,
        });
        self.cursor_col = new_col;
        PredictionOutcome::Predicted
    }
}

/// Reject control codes (anything below 0x20 except space, plus DEL).
/// Everything above U+007E is delegated to [`grapheme_width`] — the
/// `unicode-width` lookup correctly classifies combining marks as
/// width 0 (skipped) and CJK / wide emoji as width 2.
const fn is_safe_predictable(ch: char) -> bool {
    let c = ch as u32;
    // ASCII control range (0x00..=0x1F) and DEL (0x7F) are not safe to
    // predict; the shell may or may not echo them depending on stty
    // settings, and even space-as-character is fine because c == 0x20.
    if c < 0x20 {
        return false;
    }
    if c == 0x7F {
        return false;
    }
    true
}

/// Cell width of a single Unicode scalar value, in terminal columns.
/// Returns `None` for non-printable code points (`UnicodeWidthChar`
/// returns `None` for control codes).
///
/// Width is capped at 2 — `unicode-width` never returns more than 2
/// for a single scalar, but the explicit cap defends against a future
/// crate-rev change.
fn grapheme_width(ch: char) -> Option<u8> {
    let w = UnicodeWidthChar::width(ch)?;
    if w == 0 {
        // Combining marks, zero-width joiners, variation selectors —
        // these need a base grapheme cluster, which is a multi-scalar
        // case we defer (phux-9gw.1.4 ships single-scalar only).
        return None;
    }
    Some(u8::try_from(w.min(2)).unwrap_or(1))
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
    fn enter_at_col_zero_is_not_predicted() {
        let mut s = PredictionState::new(PredictiveConfig::enabled(), 80, 24);
        let ev = key_named(PhysicalKey::Enter, ModSet::empty());
        assert_eq!(s.predict_key(&ev), PredictionOutcome::Skipped);
        assert_eq!(s.pending_len(), 0);
    }

    #[test]
    fn enter_after_insert_predicts_newline_and_advances_row() {
        let mut s = PredictionState::new(PredictiveConfig::enabled(), 80, 24);
        for ch in ["h", "i"] {
            assert_eq!(s.predict_key(&key_text(ch)), PredictionOutcome::Predicted);
        }
        assert_eq!(s.cursor(), (0, 2));
        let enter = key_named(PhysicalKey::Enter, ModSet::empty());
        assert_eq!(s.predict_key(&enter), PredictionOutcome::Predicted);
        assert_eq!(s.cursor(), (1, 0));
        assert_eq!(s.pending_len(), 3);
        let last = s.pending().last().expect("three predictions");
        assert_eq!(last.kind, PredictionKind::Newline);
        assert_eq!(last.row, 0);
        assert_eq!(last.col, 2);
    }

    #[test]
    fn enter_on_last_row_is_not_predicted() {
        let mut s = PredictionState::new(PredictiveConfig::enabled(), 80, 24);
        // Park the cursor on the last row, past col 0.
        s.set_cursor(23, 5);
        let enter = key_named(PhysicalKey::Enter, ModSet::empty());
        assert_eq!(s.predict_key(&enter), PredictionOutcome::Skipped);
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
    fn single_scalar_latin1_is_predicted_width_one() {
        // U+00E9 — single Unicode scalar, cell width 1 per
        // `unicode-width`. phux-9gw.1.4: predict it as a single-cell insert.
        let mut s = PredictionState::new(PredictiveConfig::enabled(), 80, 24);
        let ev = key_text("é");
        assert_eq!(s.predict_key(&ev), PredictionOutcome::Predicted);
        let p = s.pending().next().expect("one prediction");
        assert_eq!(p.ch, 'é');
        assert_eq!(p.width, 1);
        assert_eq!(s.cursor(), (0, 1));
    }

    #[test]
    fn cjk_codepoint_is_predicted_width_two() {
        // U+4E2D 中 — CJK Unified Ideograph, cell width 2.
        let mut s = PredictionState::new(PredictiveConfig::enabled(), 80, 24);
        let ev = key_text("中");
        assert_eq!(s.predict_key(&ev), PredictionOutcome::Predicted);
        let p = s.pending().next().expect("one prediction");
        assert_eq!(p.ch, '中');
        assert_eq!(p.width, 2);
        // Width-2 grapheme advances the cursor by two columns.
        assert_eq!(s.cursor(), (0, 2));
    }

    #[test]
    fn cjk_at_right_edge_minus_one_is_not_predicted() {
        // Width-2 needs two free cells. Park the predict cursor one
        // before the last column on a 10-wide viewport: cols 0..9, last
        // col is 9; cursor at 8 → end_col would be 10, which is past
        // the rightmost column we permit (insert refuses at `>= cols`).
        let mut s = PredictionState::new(PredictiveConfig::enabled(), 10, 24);
        s.set_cursor(0, 8);
        let ev = key_text("中");
        assert_eq!(s.predict_key(&ev), PredictionOutcome::Skipped);
    }

    #[test]
    fn combining_mark_alone_is_not_predicted() {
        // U+0301 COMBINING ACUTE ACCENT — width 0, no base grapheme on
        // its own. We defer multi-scalar graphemes; reject this case.
        let mut s = PredictionState::new(PredictiveConfig::enabled(), 80, 24);
        let ev = key_text("\u{0301}");
        assert_eq!(s.predict_key(&ev), PredictionOutcome::Skipped);
    }

    #[test]
    fn multi_scalar_grapheme_is_not_predicted() {
        // "👨\u{200D}💻" — ZWJ-joined emoji sequence. Two scalars in
        // `text` → we reject (single-scalar only for now).
        let mut s = PredictionState::new(PredictiveConfig::enabled(), 80, 24);
        let ev = key_text("e\u{0301}");
        assert_eq!(s.predict_key(&ev), PredictionOutcome::Skipped);
    }

    #[test]
    fn arrow_left_over_known_cell_advances_predict_cursor() {
        let mut s = PredictionState::new(PredictiveConfig::enabled(), 80, 24);
        s.set_cursor(0, 5);
        // Cell at (0, 4) is 'a' (width 1).
        let ev = key_named(PhysicalKey::ArrowLeft, ModSet::empty());
        let outcome =
            s.predict_key_with_grid(&ev, |r, c| if (r, c) == (0, 4) { Some('a') } else { None });
        assert_eq!(outcome, PredictionOutcome::Predicted);
        assert_eq!(s.cursor(), (0, 4));
        let p = s.pending().next().expect("one prediction");
        assert_eq!(p.kind, PredictionKind::CursorLeft);
        assert_eq!(p.col, 4);
        assert_eq!(p.row, 0);
    }

    #[test]
    fn arrow_left_over_blank_cell_is_not_predicted() {
        let mut s = PredictionState::new(PredictiveConfig::enabled(), 80, 24);
        s.set_cursor(0, 5);
        let ev = key_named(PhysicalKey::ArrowLeft, ModSet::empty());
        let outcome = s.predict_key_with_grid(&ev, |_, _| None);
        assert_eq!(outcome, PredictionOutcome::Skipped);
        assert_eq!(s.pending_len(), 0);
    }

    #[test]
    fn arrow_left_at_col_zero_is_not_predicted() {
        let mut s = PredictionState::new(PredictiveConfig::enabled(), 80, 24);
        let ev = key_named(PhysicalKey::ArrowLeft, ModSet::empty());
        let outcome = s.predict_key_with_grid(&ev, |_, _| Some('a'));
        assert_eq!(outcome, PredictionOutcome::Skipped);
    }

    #[test]
    fn arrow_right_over_known_cell_advances_predict_cursor() {
        let mut s = PredictionState::new(PredictiveConfig::enabled(), 80, 24);
        s.set_cursor(0, 3);
        let ev = key_named(PhysicalKey::ArrowRight, ModSet::empty());
        let outcome =
            s.predict_key_with_grid(&ev, |r, c| if (r, c) == (0, 3) { Some('x') } else { None });
        assert_eq!(outcome, PredictionOutcome::Predicted);
        assert_eq!(s.cursor(), (0, 4));
        let p = s.pending().next().expect("one prediction");
        assert_eq!(p.kind, PredictionKind::CursorRight);
        assert_eq!(p.col, 4);
    }

    #[test]
    fn arrow_right_over_blank_cell_is_not_predicted() {
        let mut s = PredictionState::new(PredictiveConfig::enabled(), 80, 24);
        s.set_cursor(0, 3);
        let ev = key_named(PhysicalKey::ArrowRight, ModSet::empty());
        let outcome = s.predict_key_with_grid(&ev, |_, _| None);
        assert_eq!(outcome, PredictionOutcome::Skipped);
    }

    #[test]
    fn arrow_right_at_right_edge_is_not_predicted() {
        let mut s = PredictionState::new(PredictiveConfig::enabled(), 10, 24);
        s.set_cursor(0, 9);
        let ev = key_named(PhysicalKey::ArrowRight, ModSet::empty());
        // Cursor at col 9 (== cols-1) with cells 0..9 valid; advancing
        // by 1 would land at 10 (past the right edge) — refuse.
        let outcome = s.predict_key_with_grid(&ev, |_, _| Some('x'));
        assert_eq!(outcome, PredictionOutcome::Skipped);
    }

    #[test]
    fn arrow_right_over_wide_at_right_edge_minus_one_is_not_predicted() {
        // Width-2 advance needs two cells to the right. Cursor at col 7
        // with cols=10: end_col would be 9 (allowed for plain insert
        // but the wide-tail would land at col 8 — that's still within
        // bounds). The real refusal case is cursor at col 8: end_col
        // would be 10 (past the right edge for a width-2 grapheme).
        let mut s = PredictionState::new(PredictiveConfig::enabled(), 10, 24);
        s.set_cursor(0, 8);
        let ev = key_named(PhysicalKey::ArrowRight, ModSet::empty());
        let outcome = s.predict_key_with_grid(&ev, |_, _| Some('中'));
        assert_eq!(outcome, PredictionOutcome::Skipped);
    }

    #[test]
    fn arrow_right_over_wide_grapheme_advances_by_two() {
        let mut s = PredictionState::new(PredictiveConfig::enabled(), 80, 24);
        s.set_cursor(0, 3);
        let ev = key_named(PhysicalKey::ArrowRight, ModSet::empty());
        let outcome =
            s.predict_key_with_grid(&ev, |r, c| if (r, c) == (0, 3) { Some('中') } else { None });
        assert_eq!(outcome, PredictionOutcome::Predicted);
        assert_eq!(s.cursor(), (0, 5));
        let p = s.pending().next().expect("one prediction");
        assert_eq!(p.col, 5);
    }

    #[test]
    fn arrow_via_predict_key_default_grid_is_skipped() {
        // The plain `predict_key` entry point routes arrows through a
        // grid closure that always returns `None` — arrows are only
        // useful when the driver passes the live grid via
        // `predict_key_with_grid`. Verify the conservative default
        // even when the cursor is past col 0.
        let mut s = PredictionState::new(PredictiveConfig::enabled(), 80, 24);
        s.set_cursor(0, 5);
        let ev = key_named(PhysicalKey::ArrowLeft, ModSet::empty());
        assert_eq!(s.predict_key(&ev), PredictionOutcome::Skipped);
        let ev = key_named(PhysicalKey::ArrowRight, ModSet::empty());
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
