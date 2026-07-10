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
    /// Configured prefix chord.
    pub prefix: &'a str,
    /// The TUI's windows in display order (active one flagged), consumed
    /// by the `windows` widget. Empty ⇒ no window bar. TUI-side data fed
    /// into the widget pipeline via [`Self::as_widget`].
    pub windows: &'a [WindowInfo],
    /// phux-foz.4: the focused pane's live working directory (`""` when
    /// unknown), consumed by the `cwd` widget. Injected by the painter
    /// from driver-fed state, like `windows`.
    pub cwd: &'a str,
    /// phux-foz.4: the focused pane's last known command exit code
    /// (OSC-133 `command_finished`), consumed by the `exit` widget.
    pub last_exit: Option<i32>,
}

impl<'a> StatusBarContext<'a> {
    /// Convert to the lower-level [`WidgetContext`] expected by
    /// [`StatusBar::render`].
    #[must_use]
    pub const fn as_widget(&self) -> WidgetContext<'a> {
        WidgetContext {
            now: self.now,
            session_name: self.session_name,
            prefix: self.prefix,
            windows: self.windows,
            cwd: self.cwd,
            last_exit: self.last_exit,
        }
    }
}

/// Build a [`StatusBarContext`] for one render pass.
///
/// Kept so callers in `attach/` can construct one without depending on
/// `phux-config`'s internals directly. Window data is injected by the
/// painter (see [`StatusBarPainter::paint`]); standalone callers pass an
/// empty slice. The focused-pane data feeds (`cwd`, `last_exit`) are
/// painter-owned too and injected the same way.
#[must_use]
pub const fn make_context(session_name: &str, now: SystemTime) -> StatusBarContext<'_> {
    StatusBarContext {
        now,
        session_name,
        prefix: "C-a",
        windows: &[],
        cwd: "",
        last_exit: None,
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

/// ADR-0033: emit the supervisory badge right-aligned on `row_index`, as a
/// reverse-video + bold chip atop the already-painted widget row. ASCII-only
/// (no emojis, per repo convention), so the char count is the cell width.
fn paint_supervisory_overlay<W: Write>(
    out: &mut W,
    badge: &str,
    row_index: u16,
    cols: u16,
) -> io::Result<()> {
    if cols == 0 || badge.is_empty() {
        return Ok(());
    }
    let visible: String = badge.chars().take(cols as usize).collect();
    let width = u16::try_from(visible.chars().count()).unwrap_or(cols);
    let start_col = cols.saturating_sub(width);
    let one_based_row = row_index.saturating_add(1);
    let one_based_col = start_col.saturating_add(1);
    // CUP to the chip's left edge, reverse+bold, text, hard reset.
    write!(
        out,
        "\x1b[{one_based_row};{one_based_col}H\x1b[7;1m{visible}\x1b[0m"
    )?;
    out.flush()
}

/// phux-foz.1: emit the agent-attention hint as a chip immediately left of
/// the supervisory badge (`right_offset` cells in from the right edge; `0`
/// when no badge is present). Same reverse+bold treatment as the ADR-0033
/// badge, but the foreground rides the theme's `attention` slot — under
/// reverse video it reads as the chip's fill color.
fn paint_attention_overlay<W: Write>(
    out: &mut W,
    hint: &str,
    row_index: u16,
    cols: u16,
    right_offset: u16,
    color: Color,
) -> io::Result<()> {
    let avail = cols.saturating_sub(right_offset);
    if avail == 0 || hint.is_empty() {
        return Ok(());
    }
    let visible: String = hint.chars().take(avail as usize).collect();
    let width = u16::try_from(visible.chars().count()).unwrap_or(avail);
    let start_col = avail.saturating_sub(width);
    let one_based_row = row_index.saturating_add(1);
    let one_based_col = start_col.saturating_add(1);
    write!(out, "\x1b[{one_based_row};{one_based_col}H\x1b[7;1m")?;
    crate::render::sgr::write_sgr_color(out, color, true)?;
    write!(out, "{visible}\x1b[0m")?;
    out.flush()
}

/// ADR-0033 / phux-foz.1: overlay a badge into a composed bar buffer (the
/// `phux snapshot --rendered` path), right-aligned `right_offset` cells in
/// from the right edge, so the dense-cell snapshot matches the live VT paint.
fn overlay_badge_into_buffer(
    buffer: &mut Buffer,
    badge: &str,
    cols: u16,
    right_offset: u16,
    style: Style,
) {
    let avail = cols.saturating_sub(right_offset);
    if avail == 0 || badge.is_empty() {
        return;
    }
    let visible: Vec<char> = badge.chars().take(avail as usize).collect();
    let width = u16::try_from(visible.len()).unwrap_or(avail);
    let start = avail.saturating_sub(width);
    let mut tmp = [0u8; 4];
    for (i, ch) in visible.iter().enumerate() {
        let x = start.saturating_add(u16::try_from(i).unwrap_or(0));
        if x >= avail {
            break;
        }
        let cell = &mut buffer[(x, 0)];
        cell.set_symbol(ch.encode_utf8(&mut tmp));
        cell.set_style(style);
    }
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
    /// ADR-0033: when `Some`, a supervisory badge (e.g. `[ FROZEN ]`,
    /// `[ WHEEL:you ]`) is overlaid right-aligned on the bar row for the
    /// focused pane. Set by the driver from inbound `TerminalControl` state; a
    /// change invalidates the cache so the row repaints (and erases a cleared
    /// badge). Painted over the composed widget row, not replacing it.
    supervisory: Option<String>,
    /// phux-foz.1: when `Some`, the agent-attention hint (e.g. `[ ASK ]`)
    /// is overlaid immediately left of the supervisory badge. Set by the
    /// driver whenever a pane's ADR-0035 asked flag flips; same cache
    /// semantics as `supervisory`.
    attention: Option<String>,
    /// phux-foz.1: chip foreground for the attention hint, from the theme's
    /// `attention` slot (the painter never hardcodes it). Under the chip's
    /// reverse video the foreground reads as the fill color.
    attention_fg: Color,
    prefix: String,
    /// phux-foz.4: the focused pane's live working directory, fed by the
    /// driver from `cwd_changed` events (via the pane slots) and injected
    /// into the render context like `windows`. `None` ⇒ unknown (the
    /// `cwd` widget renders nothing).
    focused_cwd: Option<String>,
    /// phux-foz.4: the focused pane's last known command exit code, fed
    /// by the driver from `command_finished` events. `None` ⇒ unknown.
    last_exit: Option<i32>,
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
            .field("supervisory", &self.supervisory)
            .field("attention", &self.attention)
            .field("attention_fg", &self.attention_fg)
            .field("prefix", &self.prefix)
            .field("focused_cwd", &self.focused_cwd)
            .field("last_exit", &self.last_exit)
            .finish()
    }
}

impl StatusBarPainter {
    /// Build a painter from an already-composed [`StatusBar`].
    #[must_use]
    pub fn new(bar: StatusBar, position: Position) -> Self {
        Self {
            bar,
            position,
            last_row: None,
            last_viewport: None,
            windows: Vec::new(),
            error: None,
            supervisory: None,
            attention: None,
            attention_fg: Color::Reset,
            prefix: "C-a".to_owned(),
            focused_cwd: None,
            last_exit: None,
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
            supervisory: None,
            attention: None,
            attention_fg: Color::Reset,
            prefix: "C-a".to_owned(),
            focused_cwd: None,
            last_exit: None,
        }
    }

    /// Set the configured prefix chord exposed to prefix-aware widgets.
    pub fn set_prefix(&mut self, prefix: impl Into<String>) {
        let prefix = prefix.into();
        if self.prefix != prefix {
            self.prefix = prefix;
            self.invalidate();
        }
    }

    /// Update the window list rendered by the `windows` widget. A change
    /// forces the next paint to redraw (the list isn't part of the
    /// widget-row cache key — the widget reads it from the context).
    /// Returns `true` if the list actually changed (so a caller with no
    /// other paint trigger can gate a repaint on it).
    pub fn set_windows(&mut self, windows: Vec<WindowInfo>) -> bool {
        if self.windows == windows {
            return false;
        }
        self.windows = windows;
        self.invalidate();
        true
    }

    /// phux-foz.4: set (or clear, with `None`) the focused pane's live
    /// working directory rendered by the `cwd` widget. Returns `true` if
    /// the value actually changed; a change invalidates the row cache.
    pub fn set_focused_cwd(&mut self, cwd: Option<String>) -> bool {
        if self.focused_cwd == cwd {
            return false;
        }
        self.focused_cwd = cwd;
        self.invalidate();
        true
    }

    /// phux-foz.4: set (or clear, with `None`) the focused pane's last
    /// command exit code rendered by the `exit` widget. Returns `true` if
    /// the value actually changed; a change invalidates the row cache.
    pub fn set_last_exit(&mut self, last_exit: Option<i32>) -> bool {
        if self.last_exit == last_exit {
            return false;
        }
        self.last_exit = last_exit;
        self.invalidate();
        true
    }

    /// ADR-0033: set (or clear, with `None`) the supervisory badge overlaid on
    /// the bar for the focused pane. Returns `true` if the badge actually
    /// changed (so the caller can gate a repaint on it). A change invalidates
    /// the cache so the row repaints — which also erases a badge that just
    /// cleared.
    ///
    /// No-op while an error line is showing: the badge rides the normal bar and
    /// is suppressed under the error strip, so storing it (and invalidating the
    /// error-line cache) would only force a spurious error-strip re-emit.
    pub fn set_supervisory(&mut self, badge: Option<String>) -> bool {
        if self.error.is_some() || self.supervisory == badge {
            return false;
        }
        self.supervisory = badge;
        self.invalidate();
        true
    }

    /// phux-foz.1: set (or clear, with `None`) the agent-attention hint
    /// overlaid left of the supervisory badge. Returns `true` if the hint
    /// actually changed; same error-line suppression and cache semantics as
    /// [`Self::set_supervisory`].
    pub fn set_attention(&mut self, hint: Option<String>) -> bool {
        if self.error.is_some() || self.attention == hint {
            return false;
        }
        self.attention = hint;
        self.invalidate();
        true
    }

    /// phux-foz.1: set the attention chip's foreground from the theme's
    /// `attention` slot. The driver calls this once at attach; the painter
    /// itself never hardcodes the color.
    pub fn set_attention_color(&mut self, color: Color) {
        if self.attention_fg != color {
            self.attention_fg = color;
            self.invalidate();
        }
    }

    /// Cells the attention chip is shifted in from the right edge: the
    /// supervisory badge's width plus a 1-cell gap, or `0` when no badge is
    /// showing. Shared by the live paint and the snapshot compose so both
    /// place the chip identically.
    fn attention_offset(&self) -> u16 {
        self.supervisory.as_ref().map_or(0, |badge| {
            u16::try_from(badge.chars().count())
                .unwrap_or(u16::MAX)
                .saturating_add(1)
        })
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
        // The supervisory badge rides the normal bar row, so it only paints
        // when there is a bar to host it. An empty configured bar with no
        // windows stays a no-op (the badge is suppressed rather than ghosting
        // over un-erased pane content on a row the bar never blanks).
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
            prefix: &self.prefix,
            windows: &self.windows,
            cwd: self.focused_cwd.as_deref().unwrap_or(""),
            last_exit: self.last_exit,
            ..*ctx
        };
        // Delegate to the ratatui-backed renderer. We pre-composed
        // `new_row` for cache-keying; the renderer recomposes — cheap
        // (same inputs, deterministic) and keeps `render_status_bar`
        // usable standalone in tests.
        render_status_bar(out, &self.bar, &ctx, row_index, cols)?;
        // ADR-0033: overlay the supervisory badge atop the freshly-painted
        // widget row (right-aligned). Emitted after the row so it wins; the
        // full-row repaint above erases any stale/cleared badge first.
        if let Some(badge) = &self.supervisory {
            paint_supervisory_overlay(out, badge, row_index, cols)?;
        }
        // phux-foz.1: the attention hint chips in immediately left of the
        // badge (or at the right edge when no badge is up). Same repaint
        // discipline: the full-row repaint erased any cleared hint.
        if let Some(hint) = &self.attention {
            paint_attention_overlay(
                out,
                hint,
                row_index,
                cols,
                self.attention_offset(),
                self.attention_fg,
            )?;
        }
        self.last_row = Some((cols, new_row));
        self.last_viewport = Some((cols, rows));
        Ok(())
    }

    /// Compose the status row into a fresh `cols × 1` ratatui [`Buffer`]
    /// without emitting VT or touching the paint cache (`phux-l5xa`).
    ///
    /// Returns `(buffer, row_index)` where `row_index` is the bar's row in a
    /// `rows`-high viewport, or `None` when nothing would paint (zero dims,
    /// or an empty bar with no windows and no error). Mirrors the
    /// composition in [`Self::paint`] / [`Self::paint_error_line`] so the
    /// `phux snapshot --rendered` frame shows the same bar the live VT paint
    /// would — read as dense cells, with no emulator re-parse.
    pub(crate) fn compose_buffer(
        &self,
        cols: u16,
        rows: u16,
        ctx: &StatusBarContext<'_>,
    ) -> Option<(Buffer, u16)> {
        if cols == 0 || rows == 0 {
            return None;
        }
        let row_index: u16 = match self.position {
            Position::Bottom => rows.saturating_sub(1),
            Position::Top => 0,
        };
        if let Some(message) = &self.error {
            let mut buffer = Buffer::empty(Rect::new(0, 0, cols, 1));
            let style = Style::default().add_modifier(Modifier::REVERSED | Modifier::BOLD);
            let mut tmp = [0u8; 4];
            let mut x: u16 = 0;
            for ch in message.chars() {
                if x >= cols {
                    break;
                }
                let cell = &mut buffer[(x, 0)];
                cell.set_symbol(ch.encode_utf8(&mut tmp));
                cell.set_style(style);
                x = x.saturating_add(1);
            }
            // Extend the reverse-video strip across the rest of the row so it
            // spans the full width, matching `paint_error_line`.
            while x < cols {
                let cell = &mut buffer[(x, 0)];
                cell.set_symbol(" ");
                cell.set_style(style);
                x = x.saturating_add(1);
            }
            return Some((buffer, row_index));
        }
        // Match `paint`: the badge only composes onto a non-empty bar row.
        if self.bar.is_empty() && self.windows.is_empty() {
            return None;
        }
        let ctx = StatusBarContext {
            prefix: &self.prefix,
            windows: &self.windows,
            cwd: self.focused_cwd.as_deref().unwrap_or(""),
            last_exit: self.last_exit,
            ..*ctx
        };
        let row = self.bar.render(&ctx.as_widget(), cols);
        let mut buffer = Buffer::empty(Rect::new(0, 0, cols, 1));
        fill_buffer(&mut buffer, &row, cols);
        // ADR-0033: overlay the supervisory badge into the snapshot buffer so
        // `phux snapshot --rendered` shows the same chip the live paint draws.
        if let Some(badge) = &self.supervisory {
            overlay_badge_into_buffer(
                &mut buffer,
                badge,
                cols,
                0,
                Style::default().add_modifier(Modifier::REVERSED | Modifier::BOLD),
            );
        }
        // phux-foz.1: the attention hint composes left of the badge, themed
        // like the live paint.
        if let Some(hint) = &self.attention {
            overlay_badge_into_buffer(
                &mut buffer,
                hint,
                cols,
                self.attention_offset(),
                Style::default()
                    .fg(self.attention_fg)
                    .add_modifier(Modifier::REVERSED | Modifier::BOLD),
            );
        }
        Some((buffer, row_index))
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

    /// phux-r82.6: the async data feeds behind the bar's `exec` widgets.
    /// The driver spawns one bounded interval runner per feed; an
    /// error-line painter (empty bar) has none.
    #[must_use]
    pub fn exec_feeds(&self) -> Vec<phux_config::widget::ExecFeed> {
        self.bar.exec_feeds()
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
        make_context(session, UNIX_EPOCH)
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
                attention: false,
                branch: None,
            },
            WindowInfo {
                name: "vim".to_owned(),
                active: false,
                zoomed: false,
                attention: false,
                branch: None,
            },
        ];
        let ctx = StatusBarContext {
            windows: &windows,
            ..make_context("", UNIX_EPOCH)
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
        let ctx = make_context("", UNIX_EPOCH);
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
            attention: false,
            branch: None,
        }]);
        let mut buf = Vec::new();
        p.paint(&mut buf, 40, 10, &ctx_default("")).unwrap();
        let s = strip_csi(&String::from_utf8(buf).unwrap());
        assert!(
            s.contains("0:a"),
            "painter should render the strip; got {s:?}"
        );
    }

    /// phux-foz.1: the attention hint paints as a right-aligned chip on the
    /// bar row, colored by the theme-fed `attention_fg` (reverse video makes
    /// the fg the chip fill).
    #[test]
    fn painter_paints_attention_hint_right_aligned() {
        let mut p = StatusBarPainter::new(windows_bar(), Position::Bottom);
        p.set_windows(vec![WindowInfo {
            name: "a".to_owned(),
            active: true,
            zoomed: false,
            attention: false,
            branch: None,
        }]);
        p.set_attention_color(Color::Rgb(251, 191, 36));
        assert!(p.set_attention(Some("[ ASK ]".to_owned())));
        let mut buf = Vec::new();
        p.paint(&mut buf, 40, 10, &ctx_default("")).unwrap();
        let s = String::from_utf8(buf).unwrap();
        // Right-aligned: "[ ASK ]" is 7 cells wide in a 40-col bar on the
        // bottom row (row 10) => CUP col 34.
        assert!(
            s.contains("\x1b[10;34H"),
            "attention chip must right-align; got {s:?}"
        );
        assert!(
            s.contains("\x1b[38;2;251;191;36m"),
            "chip must carry the themed attention color; got {s:?}"
        );
        assert!(strip_csi(&s).contains("[ ASK ]"), "chip text; got {s:?}");
    }

    /// phux-foz.1: with a supervisory badge up, the attention chip shifts
    /// left of it (badge width + 1-cell gap) instead of overpainting it.
    #[test]
    fn attention_hint_sits_left_of_the_supervisory_badge() {
        let mut p = StatusBarPainter::new(windows_bar(), Position::Bottom);
        p.set_windows(vec![WindowInfo {
            name: "a".to_owned(),
            active: true,
            zoomed: false,
            attention: false,
            branch: None,
        }]);
        assert!(p.set_supervisory(Some("[ FROZEN ]".to_owned())));
        assert!(p.set_attention(Some("[ ASK ]".to_owned())));
        let mut buf = Vec::new();
        p.paint(&mut buf, 40, 10, &ctx_default("")).unwrap();
        let s = String::from_utf8(buf).unwrap();
        // Badge: 10 cells at cols 31..40. Chip: offset 11 from the right
        // edge => right-aligned at col 40-11-7+1 = 23.
        assert!(
            s.contains("\x1b[10;31H"),
            "badge keeps the right edge; got {s:?}"
        );
        assert!(
            s.contains("\x1b[10;23H"),
            "chip must shift left of the badge; got {s:?}"
        );
        let visible = strip_csi(&s);
        assert!(visible.contains("[ ASK ]") && visible.contains("[ FROZEN ]"));
    }

    /// phux-foz.1: clearing the hint reports the change and the repainted
    /// row no longer carries it.
    #[test]
    fn cleared_attention_hint_stops_painting() {
        let mut p = StatusBarPainter::new(windows_bar(), Position::Bottom);
        p.set_windows(vec![WindowInfo {
            name: "a".to_owned(),
            active: true,
            zoomed: false,
            attention: false,
            branch: None,
        }]);
        assert!(p.set_attention(Some("[ ASK ]".to_owned())));
        assert!(
            !p.set_attention(Some("[ ASK ]".to_owned())),
            "unchanged hint must report no change"
        );
        let mut buf = Vec::new();
        p.paint(&mut buf, 40, 10, &ctx_default("")).unwrap();
        assert!(p.set_attention(None), "clearing must report a change");
        let mut buf = Vec::new();
        p.paint(&mut buf, 40, 10, &ctx_default("")).unwrap();
        let visible = strip_csi(&String::from_utf8(buf).unwrap());
        assert!(
            !visible.contains("ASK"),
            "cleared hint must not repaint; got {visible:?}"
        );
    }

    /// phux-foz.4: the painter-owned focused-pane cwd feeds the `cwd`
    /// widget; setting it invalidates the cache and the widget renders
    /// the (home-uncollapsed here) directory.
    #[test]
    fn painter_renders_focused_cwd_through_cwd_widget() {
        let cfg = StatusCfg {
            left: vec![spec("cwd", &[])],
            ..Default::default()
        };
        let mut p = StatusBarPainter::new(build_bar(&cfg), Position::Bottom);
        let mut buf = Vec::new();
        p.paint(&mut buf, 40, 24, &ctx_default("")).unwrap();
        assert!(
            !strip_csi(&String::from_utf8_lossy(&buf)).contains("/tmp"),
            "unknown cwd renders nothing"
        );
        assert!(p.set_focused_cwd(Some("/tmp/project".to_owned())));
        assert!(
            !p.set_focused_cwd(Some("/tmp/project".to_owned())),
            "unchanged cwd reports no change"
        );
        let mut buf = Vec::new();
        p.paint(&mut buf, 40, 24, &ctx_default("")).unwrap();
        let visible = strip_csi(&String::from_utf8_lossy(&buf));
        assert!(
            visible.contains("/tmp/project"),
            "cwd must render; got {visible:?}"
        );
    }

    /// phux-foz.4: the painter-owned last-exit feeds the `exit` widget.
    /// Clearing it (a code-less `command_finished`) blanks the widget again.
    #[test]
    fn painter_renders_last_exit_through_exit_widget() {
        let cfg = StatusCfg {
            right: vec![spec(
                "exit",
                &[("format", toml::Value::String("rc={code}".into()))],
            )],
            ..Default::default()
        };
        let mut p = StatusBarPainter::new(build_bar(&cfg), Position::Bottom);
        assert!(p.set_last_exit(Some(127)));
        let mut buf = Vec::new();
        p.paint(&mut buf, 40, 24, &ctx_default("")).unwrap();
        let visible = strip_csi(&String::from_utf8_lossy(&buf));
        assert!(
            visible.contains("rc=127"),
            "exit code must render; got {visible:?}"
        );
        assert!(p.set_last_exit(None), "clearing reports a change");
        let mut buf = Vec::new();
        p.paint(&mut buf, 40, 24, &ctx_default("")).unwrap();
        let visible = strip_csi(&String::from_utf8_lossy(&buf));
        assert!(
            !visible.contains("rc="),
            "cleared exit must blank the widget; got {visible:?}"
        );
    }

    /// phux-r82.6: the painter exposes its bar's exec feeds so the driver
    /// can spawn runners; pushing output through a feed shows up on the
    /// next paint (the async-refresh-into-cached-state contract).
    #[test]
    fn painter_exec_feed_output_lands_on_the_bar() {
        let cfg = StatusCfg {
            left: vec![spec(
                "exec",
                &[("command", toml::Value::String("battery.sh".into()))],
            )],
            ..Default::default()
        };
        let mut p = StatusBarPainter::new(build_bar(&cfg), Position::Bottom);
        let feeds = p.exec_feeds();
        assert_eq!(feeds.len(), 1, "one exec widget => one feed");

        let mut buf = Vec::new();
        p.paint(&mut buf, 40, 24, &ctx_default("")).unwrap();
        assert!(
            !strip_csi(&String::from_utf8_lossy(&buf)).contains("BAT"),
            "no output before the first run"
        );

        feeds[0].apply_output("BAT 87%\n");
        let mut buf = Vec::new();
        p.paint(&mut buf, 40, 24, &ctx_default("")).unwrap();
        let visible = strip_csi(&String::from_utf8_lossy(&buf));
        assert!(
            visible.contains("BAT 87%"),
            "cached exec output must render; got {visible:?}"
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
    fn painter_threads_configured_prefix_to_help_hints_widget() {
        let cfg = StatusCfg {
            center: vec![spec("help-hints", &[])],
            ..Default::default()
        };
        let mut painter = StatusBarPainter::new(build_bar(&cfg), Position::Bottom);
        painter.set_prefix("C-b");

        let mut buf = Vec::new();
        painter.paint(&mut buf, 80, 24, &ctx_default("")).unwrap();
        let visible = strip_csi(&String::from_utf8(buf).unwrap());

        assert!(
            visible.contains("C-b ? help"),
            "configured prefix should reach hints widget: {visible:?}"
        );
        assert!(
            !visible.contains("C-a ? help"),
            "default prefix must not leak after rebind: {visible:?}"
        );
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
