//! Status-bar chrome layer (phux-5ke.2).
//!
//! Replaces the hand-painted cell positioning from
//! `attach::status_bar::StatusBarPainter` with a ratatui-based composer
//! for the reserved bottom (or top) row of the outer terminal. The
//! composer is the *only* place under `phux-client` that imports
//! `ratatui` (CI guard: `scripts/check-ratatui-boundary.sh`).
//!
//! Pipeline:
//!
//! 1. Higher layer ([`phux_config::widget::StatusBar`]) composes the
//!    widget row into a `Vec<WidgetCell>` of caller-supplied width.
//! 2. [`render_status_bar`] copies those cells into a ratatui
//!    [`ratatui::buffer::Buffer`] of shape `cols × 1`. Layout splits
//!    are available via ratatui's [`ratatui::layout::Layout`] if a
//!    consumer ever wants per-segment styling — today we mirror the
//!    composer's output 1:1.
//! 3. [`render_status_bar`] emits raw VT bytes (CUP + per-cell SGR +
//!    grapheme) to the writer. We do **not** route through crossterm;
//!    the rest of `phux-client` writes raw VT to stdout and the
//!    boundary stays clean.
//!
//! ### SGR + cursor invariants (per ADR-0020 §Decision)
//!
//! - The painter emits a hard SGR reset (`\x1b[0m`) after the row so
//!   subsequent paints don't inherit our attributes.
//! - The painter does **not** restore the cursor itself — the caller
//!   (`crate::attach::paint::paint_bar_after_pane`) restores the
//!   focused pane's logical cursor after the bar paints (this matches
//!   the post-34bfc07 paint order).
//!
//! ### Placement
//!
//! Defaults to [`Position::Bottom`] per `DESIGN.md` §8.5. `Position::Top`
//! is reserved for a future config knob — no TOML key surfaces it today.

use std::io::{self, Write};
use std::time::{Duration, SystemTime};

use phux_config::widget::{Cell as WidgetCell, StatusBar, WidgetContext};
use ratatui::buffer::{Buffer, Cell as RatatuiCell};
use ratatui::layout::Rect;

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

/// Inputs the chrome composer needs to paint one frame of the bar.
///
/// Mirrors what `phux_config::widget::WidgetContext` carries today
/// (clock + session name). Held as a struct here so the caller can
/// pass one borrow across the ratatui boundary without juggling
/// individual scalars.
#[derive(Debug, Clone, Copy)]
pub struct StatusBarContext<'a> {
    /// Wall-clock time the bar is rendering at.
    pub now: SystemTime,
    /// Current session name (`""` if not in a session).
    pub session_name: &'a str,
}

impl<'a> StatusBarContext<'a> {
    /// Convert to the lower-level [`WidgetContext`] expected by
    /// [`StatusBar::render`].
    #[must_use]
    pub const fn as_widget(&self) -> WidgetContext<'a> {
        WidgetContext {
            now: self.now,
            session_name: self.session_name,
        }
    }
}

/// Build a [`StatusBarContext`] for one render pass. Kept so callers
/// in `attach/` can construct one without depending on
/// `phux-config`'s internals directly.
#[must_use]
pub const fn make_context(session_name: &str, now: SystemTime) -> StatusBarContext<'_> {
    StatusBarContext { now, session_name }
}

/// Render the composed status row at `row_index` for a viewport of
/// `cols` columns and emit raw VT bytes to `out`.
///
/// `bar` is the already-composed widget pipeline — we ask it for a
/// `Vec<WidgetCell>` of the right width and copy that into a ratatui
/// [`Buffer`] of shape `cols × 1`. The buffer is then walked
/// left-to-right and emitted as CUP + per-cell glyphs. A hard SGR
/// reset (`\x1b[0m`) closes the row per the invariant in the module
/// header.
///
/// # Errors
///
/// Forwards any [`io::Error`] from `out`.
pub fn render_status_bar<W: Write>(
    out: &mut W,
    bar: &StatusBar,
    ctx: &StatusBarContext<'_>,
    row_index: u16,
    cols: u16,
) -> io::Result<()> {
    if cols == 0 || bar.is_empty() {
        return Ok(());
    }

    // 1. Compose widget cells (left/center/right slot policy lives in
    //    phux-config; we just consume the resulting strip).
    let row = bar.render(&ctx.as_widget(), cols);

    // 2. Lay into a ratatui Buffer of shape `cols × 1`. Using a
    //    Buffer (rather than emitting straight from `row`) keeps the
    //    door open for per-segment ratatui styling without churning
    //    the wire-side composer. The Buffer's coordinate space is
    //    (0,0)..(cols,1); we never read it back.
    let mut buffer = Buffer::empty(Rect::new(0, 0, cols, 1));
    fill_buffer(&mut buffer, &row, cols);

    // 3. Emit the buffer to VT. Cursor hide for the duration of the
    //    paint; SGR reset on entry and exit so we don't inherit nor
    //    bequeath attributes. Cursor restore is the caller's job —
    //    see module header.
    write_buffer(out, &buffer, row_index, cols)
}

/// Copy a [`StatusBar`] composer row into a ratatui [`Buffer`].
///
/// Blank widget cells map to a literal ASCII space; non-blank cells
/// concatenate their grapheme codepoints into the buffer cell's
/// symbol. Styling is intentionally minimal: the widget contract
/// today only carries text. When the widget cell shape grows colors
/// (tracked in `phux-config`), grow it here too.
fn fill_buffer(buffer: &mut Buffer, row: &[WidgetCell], cols: u16) {
    let mut tmp = [0u8; 4];
    for (col, cell) in row.iter().enumerate().take(usize::from(cols)) {
        // `col < cols (u16)` from the `.take(usize::from(cols))` bound, so
        // the narrowing back to `u16` is provably lossless.
        let Ok(x) = u16::try_from(col) else {
            break;
        };
        let target: &mut RatatuiCell = &mut buffer[(x, 0)];
        if cell.text.is_empty() {
            target.set_symbol(" ");
        } else {
            // Base codepoint + any combining marks. We build the
            // grapheme into a small stack string and hand it to
            // set_symbol in one go (ratatui's Cell stores symbols as
            // CompactString so the heap stays cold for ASCII).
            let mut s = String::with_capacity(cell.text.len());
            for ch in &cell.text {
                s.push_str(ch.encode_utf8(&mut tmp));
            }
            target.set_symbol(&s);
        }
    }
}

/// Walk a ratatui [`Buffer`] left-to-right at `y=0` and emit raw VT
/// bytes for the row. Encoding: hide cursor, CUP to `(row_index, 1)`,
/// SGR reset, per-cell symbol, SGR reset, flush. The painter does
/// NOT show the cursor again — the caller restores it at the focused
/// pane's logical position.
fn write_buffer<W: Write>(
    out: &mut W,
    buffer: &Buffer,
    row_index: u16,
    cols: u16,
) -> io::Result<()> {
    let one_based_row = row_index.saturating_add(1);
    // CUP to the bar row + SGR reset. We deliberately do NOT hide the
    // cursor here: the bar paint completes in sub-ms on a modern
    // terminal, and the old `?25l`-without-guaranteed-`?25h` pattern
    // stranded the cursor invisible when the caller had no last_cursor
    // to restore (fresh attach, libghostty snapshot before first PTY
    // output). Caller still positions the cursor at the focused pane
    // after this returns.
    write!(out, "\x1b[{one_based_row};1H\x1b[0m")?;
    for x in 0..cols {
        let cell = &buffer[(x, 0)];
        let sym = cell.symbol();
        if sym.is_empty() {
            out.write_all(b" ")?;
        } else {
            out.write_all(sym.as_bytes())?;
        }
    }
    // SGR reset on exit so the next paint inherits no attributes from us.
    out.write_all(b"\x1b[0m")?;
    out.flush()
}

/// VT painter for a composed [`StatusBar`].
///
/// Thin stateful wrapper over [`render_status_bar`]: caches the last
/// rendered widget row so repeated paints with unchanged inputs are
/// no-ops, and tracks viewport dims so a resize invalidates the
/// cache. The cache lives here (not in `render_status_bar`) because
/// the function-level renderer is stateless and reusable.
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

    /// True if the underlying bar has no widgets configured.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.bar.is_empty()
    }

    /// Tightest poll interval among the bar's widgets. `None` ⇒ no
    /// time-based repaint is needed. v0 returns a conservative
    /// `Some(1s)` when the bar isn't empty so the `time` widget
    /// refreshes at its declared cadence.
    #[must_use]
    pub fn min_poll_interval(&self) -> Option<Duration> {
        if self.is_empty() {
            None
        } else {
            Some(Duration::from_secs(1))
        }
    }

    /// Paint the bar onto `out` for a viewport of `cols × rows`.
    ///
    /// Cheap to call repeatedly: identical widget output + unchanged
    /// dims is a no-op (zero bytes written). Dimension changes force
    /// a fresh paint.
    ///
    /// # Errors
    ///
    /// Forwards any [`io::Error`] from `out`.
    pub fn paint<W: Write>(
        &mut self,
        out: &mut W,
        cols: u16,
        rows: u16,
        ctx: &StatusBarContext<'_>,
    ) -> io::Result<()> {
        if cols == 0 || rows == 0 || self.bar.is_empty() {
            return Ok(());
        }
        let new_row = self.bar.render(&ctx.as_widget(), cols);
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
        // Delegate to the ratatui-backed renderer. We pre-composed
        // `new_row` for cache-keying; the renderer recomposes — cheap
        // (same inputs, deterministic) and keeps `render_status_bar`
        // usable standalone in tests.
        render_status_bar(out, &self.bar, ctx, row_index, cols)?;
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

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;
    use phux_config::widget::WidgetRegistry;
    use phux_config::{StatusCfg, Widget, WidgetSpec};
    use std::time::UNIX_EPOCH;

    fn ctx_default(session: &str) -> StatusBarContext<'_> {
        StatusBarContext {
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

    #[test]
    fn render_status_bar_function_emits_cup_and_text() {
        // Direct test of the stateless function form.
        let cfg = StatusCfg {
            left: vec![Widget::Bare("session-name".into())],
            ..Default::default()
        };
        let bar = build_bar(&cfg);
        let mut buf = Vec::new();
        render_status_bar(&mut buf, &bar, &ctx_default("hello"), 23, 20).unwrap();
        let s = String::from_utf8_lossy(&buf);
        // 23 → 24 (1-based).
        assert!(s.contains("\x1b[24;1H"), "no CUP-to-row-24: {s:?}");
        assert!(s.contains("hello"), "missing text: {s:?}");
        assert!(s.ends_with("\x1b[0m"), "missing SGR reset tail: {s:?}");
    }

    #[test]
    fn render_status_bar_empty_bar_is_noop() {
        let cfg = StatusCfg::default();
        let bar = build_bar(&cfg);
        let mut buf = Vec::new();
        render_status_bar(&mut buf, &bar, &ctx_default(""), 0, 80).unwrap();
        assert!(buf.is_empty());
    }

    #[test]
    fn render_status_bar_zero_cols_is_noop() {
        let cfg = StatusCfg {
            left: vec![Widget::Bare("session-name".into())],
            ..Default::default()
        };
        let bar = build_bar(&cfg);
        let mut buf = Vec::new();
        render_status_bar(&mut buf, &bar, &ctx_default("x"), 0, 0).unwrap();
        assert!(buf.is_empty());
    }
}
