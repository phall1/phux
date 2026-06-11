//! Status-bar chrome layer (phux-5ke.2).
//!
//! Replaces the hand-painted cell positioning from
//! `attach::status_bar::StatusBarPainter` with a ratatui-based composer
//! for the reserved bottom (or top) row of the outer terminal. The
//! composer lives in the chrome layer, the only place that imports
//! `ratatui`; the pane-interior substrate is in the `phux-client-core`
//! crate, which has no `ratatui` dependency (ADR-0020).
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
//! Defaults to [`Position::Bottom`] per `docs/consumers/tui.md` §8.5. `Position::Top`
//! is reserved for a future config knob — no TOML key surfaces it today.

use std::io::{self, Write};
use std::time::{Duration, SystemTime};

use std::str::FromStr;

use phux_config::widget::{Cell as WidgetCell, CellStyle, StatusBar, WidgetContext, WindowInfo};
use ratatui::buffer::{Buffer, Cell as RatatuiCell};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};

/// Where the status bar lives in the outer terminal. Defaults to
/// [`Self::Bottom`] per `docs/consumers/tui.md` §8.5.
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
    /// The TUI's windows in display order (active one flagged), consumed
    /// by the `windows` widget. Empty ⇒ no window bar. TUI-side data fed
    /// into the widget pipeline via [`Self::as_widget`].
    pub windows: &'a [WindowInfo],
}

impl<'a> StatusBarContext<'a> {
    /// Convert to the lower-level [`WidgetContext`] expected by
    /// [`StatusBar::render`].
    #[must_use]
    pub const fn as_widget(&self) -> WidgetContext<'a> {
        WidgetContext {
            now: self.now,
            session_name: self.session_name,
            windows: self.windows,
        }
    }
}

/// Build a [`StatusBarContext`] for one render pass.
///
/// Kept so callers in `attach/` can construct one without depending on
/// `phux-config`'s internals directly. Window data is injected by the
/// painter (see [`StatusBarPainter::paint`]); standalone callers pass an
/// empty slice.
#[must_use]
pub const fn make_context(session_name: &str, now: SystemTime) -> StatusBarContext<'_> {
    StatusBarContext {
        now,
        session_name,
        windows: &[],
    }
}

/// Translate a phux-config [`CellStyle`] (plain data) into a ratatui
/// [`Style`]. Color strings are parsed at this boundary (ADR-0020); an
/// unparseable color degrades to the terminal default with a warning
/// rather than failing the paint.
fn to_ratatui_style(style: &CellStyle) -> Style {
    let mut s = Style::default();
    if let Some(fg) = parse_color(style.fg.as_deref()) {
        s = s.fg(fg);
    }
    if let Some(bg) = parse_color(style.bg.as_deref()) {
        s = s.bg(bg);
    }
    let mut m = Modifier::empty();
    m.set(Modifier::BOLD, style.bold);
    m.set(Modifier::DIM, style.dim);
    m.set(Modifier::ITALIC, style.italic);
    m.set(Modifier::UNDERLINED, style.underline);
    m.set(Modifier::REVERSED, style.reverse);
    s.add_modifier(m)
}

/// Parse a color string (`"red"`, `"#cdd6f4"`, `"12"`) into a ratatui
/// [`Color`]. `None`/unparseable ⇒ `None` (terminal default).
fn parse_color(spec: Option<&str>) -> Option<Color> {
    let s = spec?;
    Color::from_str(s).map_or_else(
        |_| {
            tracing::warn!(color = s, "unrecognized status-bar color; using default");
            None
        },
        Some,
    )
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
    // phux-4li.17: paint when there are configured widgets OR a window
    // bar to draw. An empty bar with no windows is still a no-op.
    if cols == 0 || (bar.is_empty() && ctx.windows.is_empty()) {
        return Ok(());
    }

    // 1. Compose widget cells (left/center/right slot policy lives in
    //    phux-config; we just consume the resulting strip).
    let row = bar.render(&ctx.as_widget(), cols);

    // 2. Lay into a ratatui Buffer of shape `cols × 1`, carrying each
    //    cell's style across the boundary. The Buffer's coordinate space
    //    is (0,0)..(cols,1); we never read it back.
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
        // phux-ahv.3: carry per-cell style (fg/bg/attrs) across the
        // ratatui boundary; `write_buffer` emits it as SGR.
        if let Some(style) = &cell.style {
            target.set_style(to_ratatui_style(style));
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
    let mut prev_styled = false;
    for x in 0..cols {
        let cell = &buffer[(x, 0)];
        // phux-ahv.3: per-cell SGR (shared with the overlay painter).
        crate::render::sgr::emit_cell_sgr(out, cell, &mut prev_styled)?;
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
    /// phux-ahv.3: the window list fed to the `windows` widget. Updated
    /// by the driver from the `Workspace` and injected into the render
    /// context inside [`Self::paint`]; a change invalidates the cache.
    windows: Vec<WindowInfo>,
    /// phux-9vf: when `Some`, the painter ignores `bar`/`windows` and
    /// paints this fixed error line instead. Set by the attach path when
    /// the on-disk config fails to load or build, so the user sees a
    /// visible reason the bar and keybindings are degraded rather than a
    /// silently empty row.
    error: Option<String>,
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
            .field("windows.len", &self.windows.len())
            .field("error", &self.error)
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
            windows: Vec::new(),
            error: None,
        }
    }

    /// phux-9vf: build a painter that shows a fixed error line instead of
    /// the configured widgets.
    ///
    /// The attach path reaches for this when the on-disk config fails to
    /// load or build: rather than silently dropping to an empty bar (and
    /// no keybindings) with only a `tracing::warn` the user never sees,
    /// the bar row shows the parse error and points at `phux config show`
    /// for the full diagnostic. The painter built this way is never
    /// "empty" and always reports a poll interval, so the error stays on
    /// screen across repaints.
    #[must_use]
    pub fn error_line(message: impl Into<String>) -> Self {
        Self {
            bar: StatusBar::empty(),
            position: Position::default(),
            last_row: None,
            last_viewport: None,
            windows: Vec::new(),
            error: Some(message.into()),
        }
    }

    /// Update the window list rendered by the `windows` widget. A change
    /// forces the next paint to redraw (the list isn't part of the
    /// widget-row cache key — the widget reads it from the context).
    pub fn set_windows(&mut self, windows: Vec<WindowInfo>) {
        if self.windows != windows {
            self.windows = windows;
            self.invalidate();
        }
    }

    /// True if the underlying bar has no widgets configured.
    ///
    /// phux-9vf: an error-line painter is never empty — the fixed
    /// diagnostic must always reserve and paint its row so the user sees
    /// why their chrome is degraded.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.error.is_none() && self.bar.is_empty()
    }

    /// Tightest poll interval among the bar's widgets. `None` ⇒ no
    /// time-based repaint is needed. v0 returns a conservative
    /// `Some(1s)` when the bar isn't empty so the `time` widget
    /// refreshes at its declared cadence.
    ///
    /// phux-9vf: an error-line painter reports the same `Some(1s)` so the
    /// driver's `status_tick` arm keeps repainting it and the diagnostic
    /// survives pane output stomping the bottom row.
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
        if cols == 0 || rows == 0 {
            return Ok(());
        }
        // phux-9vf: an error-line painter bypasses the widget pipeline and
        // paints the fixed diagnostic. It takes priority over the normal
        // "empty bar with no windows is a no-op" short-circuit below.
        if self.error.is_some() {
            return self.paint_error_line(out, cols, rows);
        }
        if self.bar.is_empty() && self.windows.is_empty() {
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
        // The window list is owned by the painter (the driver sets it
        // from the Workspace); inject it into the render context so
        // callers don't have to thread it through every paint path.
        let ctx = StatusBarContext {
            windows: &self.windows,
            ..*ctx
        };
        // Delegate to the ratatui-backed renderer. We pre-composed
        // `new_row` for cache-keying; the renderer recomposes — cheap
        // (same inputs, deterministic) and keeps `render_status_bar`
        // usable standalone in tests.
        render_status_bar(out, &self.bar, &ctx, row_index, cols)?;
        self.last_row = Some((cols, new_row));
        self.last_viewport = Some((cols, rows));
        Ok(())
    }

    /// phux-9vf: paint the fixed error diagnostic onto the bar row.
    ///
    /// Bypasses the widget composer entirely: the message is laid into a
    /// reverse-video row (so it reads as an alarm strip rather than blending
    /// into normal chrome) and truncated to `cols`. Cached on `last_row` /
    /// `last_viewport` like the normal path so repeated paints with
    /// unchanged dims are no-ops; a resize forces a repaint.
    fn paint_error_line<W: Write>(&mut self, out: &mut W, cols: u16, rows: u16) -> io::Result<()> {
        // Callers gate on `self.error.is_some()`; an empty string is a
        // valid (if unusual) diagnostic, so default to "" rather than
        // returning early.
        let message = self.error.clone().unwrap_or_default();
        // The error row carries no widget cells; we key the cache solely on
        // viewport dims (the message is fixed for this painter's lifetime).
        let viewport_changed = self.last_viewport != Some((cols, rows));
        if !viewport_changed && self.last_row.is_some() {
            return Ok(());
        }
        let row_index: u16 = match self.position {
            Position::Bottom => rows.saturating_sub(1),
            Position::Top => 0,
        };
        let mut buffer = Buffer::empty(Rect::new(0, 0, cols, 1));
        let style = Style::default().add_modifier(Modifier::REVERSED | Modifier::BOLD);
        let mut x: u16 = 0;
        for ch in message.chars() {
            if x >= cols {
                break;
            }
            let mut tmp = [0u8; 4];
            let cell = &mut buffer[(x, 0)];
            cell.set_symbol(ch.encode_utf8(&mut tmp));
            cell.set_style(style);
            x = x.saturating_add(1);
        }
        // Extend the reverse-video field across the rest of the row so the
        // alarm strip spans the full width, not just the message.
        while x < cols {
            let cell = &mut buffer[(x, 0)];
            cell.set_symbol(" ");
            cell.set_style(style);
            x = x.saturating_add(1);
        }
        write_buffer(out, &buffer, row_index, cols)?;
        // Mark the cache populated so the dims-only key short-circuits the
        // next repaint; the stored row is empty (we don't compose widgets).
        self.last_row = Some((cols, Vec::new()));
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
            windows: &[],
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
    fn error_line_painter_is_not_empty_and_polls() {
        // phux-9vf: an error-line painter must report non-empty + a poll
        // interval so the driver reserves the row and keeps repainting the
        // diagnostic (otherwise pane output stomps it and it never returns).
        let p = StatusBarPainter::error_line("config error: boom (run: phux config show)");
        assert!(!p.is_empty(), "error-line painter must not be empty");
        assert_eq!(p.min_poll_interval(), Some(Duration::from_secs(1)));
    }

    #[test]
    fn error_line_painter_renders_message_on_bar_row() {
        // phux-9vf: the fixed diagnostic paints onto the bar row even though
        // no widgets are configured (the normal empty-bar short-circuit
        // would otherwise emit nothing).
        let mut p =
            StatusBarPainter::error_line("config error: dup [status] (run: phux config show)");
        let mut buf = Vec::new();
        p.paint(&mut buf, 80, 24, &ctx_default("")).unwrap();
        let s = String::from_utf8_lossy(&buf);
        assert!(s.contains("\x1b[24;1H"), "no CUP-to-bar-row: {s:?}");
        // The painter emits one SGR-wrapped cell per glyph, so the message
        // is not a contiguous substring of the raw VT. Strip the CSI escapes
        // to recover the printable text and assert on that.
        let printable = strip_csi(&s);
        assert!(
            printable.contains("config error"),
            "missing error text: {printable:?} (raw {s:?})"
        );
        assert!(
            printable.contains("phux config show"),
            "missing call-to-action: {printable:?} (raw {s:?})"
        );
    }

    #[test]
    fn error_line_painter_repaint_is_idempotent_on_unchanged_dims() {
        let mut p = StatusBarPainter::error_line("config error: boom");
        let mut buf = Vec::new();
        p.paint(&mut buf, 40, 24, &ctx_default("")).unwrap();
        let first_len = buf.len();
        assert!(first_len > 0, "first paint must emit the error row");
        p.paint(&mut buf, 40, 24, &ctx_default("")).unwrap();
        assert_eq!(buf.len(), first_len, "unchanged dims must be a no-op");
    }

    #[test]
    fn error_line_painter_repaints_after_invalidate() {
        // The driver invalidates the bar after pane output overwrites the
        // bottom row; the diagnostic must then repaint.
        let mut p = StatusBarPainter::error_line("config error: boom");
        let mut buf = Vec::new();
        p.paint(&mut buf, 40, 24, &ctx_default("")).unwrap();
        let first_len = buf.len();
        p.invalidate();
        p.paint(&mut buf, 40, 24, &ctx_default("")).unwrap();
        assert!(buf.len() > first_len, "invalidate must force a repaint");
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

    fn windows_bar() -> StatusBar {
        let cfg = StatusCfg {
            left: vec![spec("windows", &[])],
            ..StatusCfg::default()
        };
        build_bar(&cfg)
    }

    /// Strip CSI escape sequences so a text assertion isn't defeated by
    /// the per-cell SGR that styled tabs interleave between glyphs.
    fn strip_csi(s: &str) -> String {
        let mut out = String::new();
        let mut chars = s.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '\x1b' && chars.peek() == Some(&'[') {
                chars.next();
                for n in chars.by_ref() {
                    if ('@'..='~').contains(&n) {
                        break;
                    }
                }
            } else if c != '\x1b' {
                out.push(c);
            }
        }
        out
    }

    #[test]
    fn windows_widget_renders_tab_strip() {
        let bar = windows_bar();
        let windows = [
            WindowInfo {
                name: "bash".to_owned(),
                active: true,
                zoomed: false,
            },
            WindowInfo {
                name: "vim".to_owned(),
                active: false,
                zoomed: false,
            },
        ];
        let ctx = StatusBarContext {
            now: UNIX_EPOCH,
            session_name: "",
            windows: &windows,
        };
        let mut buf = Vec::new();
        render_status_bar(&mut buf, &bar, &ctx, 0, 40).unwrap();
        let s = String::from_utf8(buf).unwrap();
        // The active tab carries an SGR (default preset = bold+reverse),
        // so glyphs interleave with escapes — strip CSI before the text
        // assertion.
        assert!(
            s.contains("\x1b[1"),
            "expected bold SGR for the active tab; got {s:?}"
        );
        let visible = strip_csi(&s);
        assert!(visible.contains("0:bash"), "first tab; got {visible:?}");
        assert!(visible.contains("1:vim"), "second tab; got {visible:?}");
    }

    #[test]
    fn empty_bar_and_no_windows_is_noop() {
        let bar = build_bar(&StatusCfg::default());
        let ctx = StatusBarContext {
            now: UNIX_EPOCH,
            session_name: "",
            windows: &[],
        };
        let mut buf = Vec::new();
        render_status_bar(&mut buf, &bar, &ctx, 0, 40).unwrap();
        assert!(buf.is_empty(), "empty bar + no windows must not paint");
    }

    #[test]
    fn painter_set_windows_paints_tab_strip() {
        // A painter whose bar has the `windows` widget renders the strip
        // from its injected window list; a changed list forces a repaint.
        let mut p = StatusBarPainter::new(windows_bar(), Position::Bottom);
        p.set_windows(vec![WindowInfo {
            name: "a".to_owned(),
            active: true,
            zoomed: false,
        }]);
        let mut buf = Vec::new();
        p.paint(&mut buf, 40, 10, &ctx_default("")).unwrap();
        let s = strip_csi(&String::from_utf8(buf).unwrap());
        assert!(
            s.contains("0:a"),
            "painter should render the strip; got {s:?}"
        );
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
