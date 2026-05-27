//! Status-bar render integration for the attach loop.
//!
//! Owns one composed [`StatusBar`] per attached client. Knows how to
//! paint the bar to the *bottom* row of the outer terminal as VT
//! (cursor-positioning + a single row's text), and how to keep its
//! own dirty bit so callers don't redraw on every pane frame.
//!
//! Placement: bottom row only, per `DESIGN.md` §8.5 ("One row, bottom
//! of the outer terminal"). `DESIGN.md` does not (yet) offer a `top`
//! config knob; the [`Position`] enum is `pub` so a future config
//! switch can plug in without breaking callers.
//!
//! Refresh cadence: per `DESIGN.md` §8.4, widgets with `poll_interval`
//! set their own cadence; widgets without one repaint only on events.
//! The status bar exposes [`StatusBarPainter::min_poll_interval`] so
//! the attach loop can arm a single tokio sleep per consumed cadence
//! instead of redrawing the whole screen on every widget tick.
//!
//! The painter writes into any [`Write`] — the attach loop hands it
//! locked stdout; tests hand it a `Vec<u8>` and assert on the bytes.
//!
//! The painter does **not** ask the pane renderer to stop short of the
//! last row. We accept that the status bar overwrites the bottom row
//! of the pane mirror — same compromise tmux makes — because clamping
//! the pane to `rows - 1` rows from the client side would race with
//! the server's authoritative pane sizing and is out of scope for
//! `phux-nz4.5` (the server-sizing path is touched by `phux-vp0.4`,
//! `phux-4hp` in parallel). The bar paints *after* the pane render
//! each frame so the visible result is correct.

use std::io::{self, Write};
use std::time::{Duration, SystemTime};

use phux_config::widget::{Cell as WidgetCell, StatusBar, WidgetContext};

/// Where the status bar lives in the outer terminal. Defaults to
/// [`Self::Bottom`] per `DESIGN.md` §8.5.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Position {
    /// One row at the very bottom of the outer terminal.
    #[default]
    Bottom,
    /// One row at the very top of the outer terminal. Reserved for a
    /// future config knob; no TOML key surfaces it today.
    Top,
}

/// VT painter for a composed [`StatusBar`].
///
/// Holds a single composed bar and the bookkeeping needed to paint it
/// without flickering: last-rendered cell-strip (so we can skip work
/// when nothing changed), last viewport dims (so a resize forces a
/// repaint), and the position selection.
pub struct StatusBarPainter {
    bar: StatusBar,
    position: Position,
    /// Last painted strip, keyed by viewport width. `None` ⇒ never
    /// painted (or width changed); next call paints unconditionally.
    last_row: Option<(u16, Vec<WidgetCell>)>,
    /// Last (cols, rows) we painted into. Different dims invalidate
    /// `last_row` and force a fresh paint.
    last_viewport: Option<(u16, u16)>,
}

impl std::fmt::Debug for StatusBarPainter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StatusBarPainter")
            .field("bar", &self.bar)
            .field("position", &self.position)
            .field(
                "last_row.len",
                &self.last_row.as_ref().map(|(_, r)| r.len()),
            )
            .field("last_viewport", &self.last_viewport)
            .finish()
    }
}

impl StatusBarPainter {
    /// Build a painter from an already-composed [`StatusBar`].
    #[must_use]
    pub const fn new(bar: StatusBar, position: Position) -> Self {
        Self {
            bar,
            position,
            last_row: None,
            last_viewport: None,
        }
    }

    /// True if the underlying bar has no widgets configured. Callers
    /// can short-circuit reservation logic on this.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.bar.is_empty()
    }

    /// Tightest poll interval among the widgets in this bar, or `None`
    /// if no widget needs a time-based repaint. The attach loop can
    /// arm a single `tokio::time::sleep` at this cadence rather than
    /// redrawing the whole screen on every widget tick.
    ///
    /// At the [`StatusBar`] composer level we don't currently expose
    /// per-widget intervals (the trait method is on the underlying
    /// `StatusWidget`); for v0 we return a conservative `Some(1s)`
    /// when the bar isn't empty so the `time` widget refreshes at its
    /// declared cadence. Future work can plumb the actual minimum
    /// through.
    #[must_use]
    pub fn min_poll_interval(&self) -> Option<Duration> {
        if self.is_empty() {
            None
        } else {
            Some(Duration::from_secs(1))
        }
    }

    /// Paint the status bar onto `out` for a viewport of `cols × rows`.
    ///
    /// Cheap to call repeatedly: if the rendered row is byte-identical
    /// to the previous one *and* the viewport dims are unchanged, this
    /// is a no-op (no bytes written, no cursor move). A dimension
    /// change forces a fresh paint.
    ///
    /// # Errors
    ///
    /// Forwards any [`io::Error`] from `out`.
    pub fn paint<W: Write>(
        &mut self,
        out: &mut W,
        cols: u16,
        rows: u16,
        ctx: &WidgetContext<'_>,
    ) -> io::Result<()> {
        if cols == 0 || rows == 0 || self.bar.is_empty() {
            return Ok(());
        }
        let new_row = self.bar.render(ctx, cols);
        let viewport_changed = self.last_viewport != Some((cols, rows));
        let row_changed = match &self.last_row {
            Some((w, prev)) => *w != cols || prev != &new_row,
            None => true,
        };
        if !viewport_changed && !row_changed {
            return Ok(());
        }
        let row_index: u16 = match self.position {
            Position::Bottom => rows.saturating_sub(1),
            Position::Top => 0,
        };
        write_row(out, row_index, &new_row)?;
        self.last_row = Some((cols, new_row));
        self.last_viewport = Some((cols, rows));
        Ok(())
    }

    /// Force the next [`Self::paint`] to redraw unconditionally —
    /// e.g. after a SIGWINCH or after the pane renderer wrote the
    /// bottom row.
    pub fn invalidate(&mut self) {
        self.last_row = None;
        self.last_viewport = None;
    }
}

/// Build a [`WidgetContext`] suitable for rendering the status bar.
///
/// Kept here so the attach loop can construct one without depending
/// on `phux-config`'s internals directly.
#[must_use]
pub const fn make_context(session_name: &str, now: SystemTime) -> WidgetContext<'_> {
    WidgetContext { now, session_name }
}

/// Write a single status-bar row to `out` at the supplied row index,
/// starting at column 0. Hides the cursor for the duration of the
/// paint and restores SGR at the start and end of the row.
///
/// Blank cells render as ASCII space; non-blank cells render their
/// first grapheme codepoint followed by any combining codepoints.
/// Styling is intentionally minimal — the widget contract today only
/// carries text, no SGR. When the widget cell shape grows colors
/// (`phux-config` follow-up tracked there), this is the one place to
/// emit them.
fn write_row<W: Write>(out: &mut W, row_index: u16, row: &[WidgetCell]) -> io::Result<()> {
    // Position cursor at the start of the status row.
    let one_based_row = row_index.saturating_add(1);
    write!(out, "\x1b[?25l\x1b[{one_based_row};1H\x1b[0m")?;
    let mut buf = [0u8; 4];
    for cell in row {
        if cell.text.is_empty() {
            out.write_all(b" ")?;
        } else {
            for ch in &cell.text {
                out.write_all(ch.encode_utf8(&mut buf).as_bytes())?;
            }
        }
    }
    // Reset SGR after the row so the next paint doesn't inherit our
    // (currently empty) attributes. Cursor stays hidden — the pane
    // renderer restores it on its next pass.
    out.write_all(b"\x1b[0m")?;
    out.flush()
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;
    use phux_config::widget::WidgetRegistry;
    use phux_config::{StatusCfg, Widget, WidgetSpec};
    use std::time::UNIX_EPOCH;

    fn ctx_default(session: &str) -> WidgetContext<'_> {
        WidgetContext {
            now: UNIX_EPOCH,
            session_name: session,
        }
    }

    fn spec(kind: &str, opts: &[(&str, toml::Value)]) -> Widget {
        Widget::Spec(WidgetSpec {
            kind: kind.to_owned(),
            opts: opts
                .iter()
                .map(|(k, v)| ((*k).to_owned(), v.clone()))
                .collect(),
        })
    }

    fn build_bar(cfg: &StatusCfg) -> StatusBar {
        let reg = WidgetRegistry::with_builtins();
        StatusBar::build(cfg, &reg).unwrap()
    }

    #[test]
    fn empty_bar_writes_nothing() {
        let cfg = StatusCfg::default();
        let mut p = StatusBarPainter::new(build_bar(&cfg), Position::Bottom);
        assert!(p.is_empty());
        let mut buf = Vec::new();
        p.paint(&mut buf, 80, 24, &ctx_default("")).unwrap();
        assert!(buf.is_empty());
    }

    #[test]
    fn bottom_position_targets_last_row() {
        let cfg = StatusCfg {
            left: vec![Widget::Bare("session-name".into())],
            ..Default::default()
        };
        let mut p = StatusBarPainter::new(build_bar(&cfg), Position::Bottom);
        let mut buf = Vec::new();
        p.paint(&mut buf, 10, 24, &ctx_default("hi")).unwrap();
        let s = String::from_utf8_lossy(&buf);
        // Row 24 (last of 24-row viewport).
        assert!(s.contains("\x1b[24;1H"), "no CUP-to-row-24: {s:?}");
        assert!(s.contains("hi"), "missing widget text: {s:?}");
    }

    #[test]
    fn top_position_targets_row_1() {
        let cfg = StatusCfg {
            left: vec![Widget::Bare("session-name".into())],
            ..Default::default()
        };
        let mut p = StatusBarPainter::new(build_bar(&cfg), Position::Top);
        let mut buf = Vec::new();
        p.paint(&mut buf, 10, 24, &ctx_default("hi")).unwrap();
        let s = String::from_utf8_lossy(&buf);
        assert!(s.contains("\x1b[1;1H"), "no CUP-to-row-1: {s:?}");
    }

    #[test]
    fn paint_is_idempotent_on_unchanged_row() {
        let cfg = StatusCfg {
            left: vec![Widget::Bare("session-name".into())],
            ..Default::default()
        };
        let mut p = StatusBarPainter::new(build_bar(&cfg), Position::Bottom);
        let mut buf = Vec::new();
        p.paint(&mut buf, 10, 24, &ctx_default("x")).unwrap();
        let first_len = buf.len();
        // Second paint with same dims + same ctx must add nothing.
        p.paint(&mut buf, 10, 24, &ctx_default("x")).unwrap();
        assert_eq!(buf.len(), first_len);
    }

    #[test]
    fn paint_redraws_on_viewport_change() {
        let cfg = StatusCfg {
            left: vec![Widget::Bare("session-name".into())],
            ..Default::default()
        };
        let mut p = StatusBarPainter::new(build_bar(&cfg), Position::Bottom);
        let mut buf = Vec::new();
        p.paint(&mut buf, 10, 24, &ctx_default("x")).unwrap();
        let first_len = buf.len();
        // Change width — must repaint.
        p.paint(&mut buf, 20, 24, &ctx_default("x")).unwrap();
        assert!(buf.len() > first_len);
    }

    #[test]
    fn paint_redraws_on_context_change() {
        let cfg = StatusCfg {
            left: vec![Widget::Bare("session-name".into())],
            ..Default::default()
        };
        let mut p = StatusBarPainter::new(build_bar(&cfg), Position::Bottom);
        let mut buf = Vec::new();
        p.paint(&mut buf, 10, 24, &ctx_default("a")).unwrap();
        let first_len = buf.len();
        p.paint(&mut buf, 10, 24, &ctx_default("b")).unwrap();
        assert!(buf.len() > first_len);
    }

    #[test]
    fn zero_dims_skip_paint() {
        let cfg = StatusCfg {
            left: vec![Widget::Bare("session-name".into())],
            ..Default::default()
        };
        let mut p = StatusBarPainter::new(build_bar(&cfg), Position::Bottom);
        let mut buf = Vec::new();
        p.paint(&mut buf, 0, 24, &ctx_default("x")).unwrap();
        p.paint(&mut buf, 80, 0, &ctx_default("x")).unwrap();
        assert!(buf.is_empty());
    }

    #[test]
    fn time_and_session_both_appear_when_configured() {
        // Integration scenario from the nz4.5 acceptance criteria.
        let cfg = StatusCfg {
            left: vec![Widget::Bare("session-name".into())],
            right: vec![spec(
                "time",
                &[("format", toml::Value::String("LITERAL".into()))],
            )],
            ..Default::default()
        };
        let mut p = StatusBarPainter::new(build_bar(&cfg), Position::Bottom);
        let mut buf = Vec::new();
        p.paint(&mut buf, 30, 24, &ctx_default("main")).unwrap();
        let s = String::from_utf8_lossy(&buf);
        assert!(s.contains("main"), "session widget missing: {s:?}");
        assert!(s.contains("LITERAL"), "time widget missing: {s:?}");
    }

    #[test]
    fn min_poll_interval_some_when_non_empty() {
        let cfg = StatusCfg {
            left: vec![Widget::Bare("session-name".into())],
            ..Default::default()
        };
        let p = StatusBarPainter::new(build_bar(&cfg), Position::Bottom);
        assert_eq!(p.min_poll_interval(), Some(Duration::from_secs(1)));
    }

    #[test]
    fn min_poll_interval_none_when_empty() {
        let cfg = StatusCfg::default();
        let p = StatusBarPainter::new(build_bar(&cfg), Position::Bottom);
        assert_eq!(p.min_poll_interval(), None);
    }

    #[test]
    fn invalidate_forces_repaint() {
        let cfg = StatusCfg {
            left: vec![Widget::Bare("session-name".into())],
            ..Default::default()
        };
        let mut p = StatusBarPainter::new(build_bar(&cfg), Position::Bottom);
        let mut buf = Vec::new();
        p.paint(&mut buf, 10, 24, &ctx_default("x")).unwrap();
        let first_len = buf.len();
        p.invalidate();
        p.paint(&mut buf, 10, 24, &ctx_default("x")).unwrap();
        assert!(buf.len() > first_len);
    }

    #[test]
    fn make_context_helper_exposes_session_and_now() {
        let now = UNIX_EPOCH;
        let c = make_context("alpha", now);
        assert_eq!(c.session_name, "alpha");
        assert_eq!(c.now, now);
    }
}
