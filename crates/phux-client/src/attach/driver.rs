//! The attach loop driver: connect, HELLO + ATTACH, then `tokio::select!`
//! over the server, stdin, SIGWINCH, and the detach chord until the server
//! sends `DETACHED` or the user requests detach.
//!
//! The driver owns:
//!
//! * the [`super::connection::Connection`] (UDS transport),
//! * stdout via a [`RawModeGuard`] that flips the outer terminal into raw
//!   mode + alt screen on construction and restores it on drop (panic-safe
//!   per ADR-0003's "no hung outer terminals" requirement),
//! * a stdin reader,
//! * a SIGWINCH listener (currently a no-op; once `VIEWPORT_RESIZE` lands
//!   in phux-4hp it will start sending resize frames upstream),
//! * a local `libghostty_vt::Terminal` + [`TerminalRenderer`] for the focused
//!   pane (under ADR-0013 the client is bytes-in / `vt_write` / dirty-row
//!   redraw — see `research/2026-05-25-libghostty-renderstate.md`).

#![allow(
    clippy::result_large_err,
    reason = "AttachError carries an io::Error which is naturally large; the variants are mutually exclusive and we never carry the result in a hot loop."
)]

use std::collections::HashMap;
use std::io::{self, IsTerminal, Write};
use std::os::fd::AsFd;
use std::path::Path;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use libghostty_vt::terminal::Mode;
use libghostty_vt::{Terminal as GhosttyTerminal, TerminalOptions};
use phux_protocol::PROTOCOL_VERSION;
use phux_protocol::caps::{ClientCapabilities, Layer, LayerSet, OutputMode, detect_color_support};
use phux_protocol::ids::{ClientId, GroupId, TerminalId};
use phux_protocol::wire::frame::{AttachTarget, FrameKind, Scope, TerminalLifecycle, ViewportInfo};
use rustix::termios::{LocalModes, OptionalActions, Termios};
use tokio::io::AsyncReadExt;
use tokio::signal::unix::{SignalKind, signal};
use tracing::Instrument as _;

use super::actions::{PendingSplit, PendingWindow};
use super::connection::{Connection, Dial};
use super::exec_widgets::spawn_exec_feed_runners;
use super::input::StdinParser;
use super::input_dispatch::{
    DispatchCtx, ReattachTarget, dispatch_input_events, encode_layout_or_log,
};
use super::paint::{
    SidebarEdge, SidebarReservation, content_rect, paint_bar_after_pane, paint_full_frame,
};
use super::plugin_actions::{self, PluginActionEntry, PluginRunResult};
use super::render::{SelectionRect, TerminalRenderer, write_cup, write_reset};
use super::server_frame::{AgentMetaIndex, handle_server_frame};
use crate::agent_meta::{AgentRecord, TERMINAL_AGENT_KEY};
use crate::layout::Workspace;
use crate::predict::{Overlay, PredictionState, PredictiveConfig};
use crate::render::chrome::sidebar::SidebarPainter;
use crate::render::chrome::status_bar::{Position, StatusBarPainter};
use crate::render::overlay::OverlayState;
use phux_config::SidebarPosition;

/// One pane's mirror: the libghostty Terminal that ingests
/// `TERMINAL_OUTPUT` and the renderer that paints it to the outer
/// terminal. Grown from "one of these per attach" (single-pane v0) to
/// "one of these per leaf in the layout tree" by phux-4li.4. The driver
/// keeps a `PaneMap` of these keyed by [`TerminalId`].
pub(super) struct PaneSlot {
    /// libghostty mirror for this pane.
    pub terminal: GhosttyTerminal<'static, 'static>,
    /// Cached render scaffolding. One per pane so libghostty's iterators
    /// stay warm across frames (the renderer's `last_cursor` is also
    /// per-pane, so each pane's predictive-echo anchor is independent).
    pub renderer: TerminalRenderer<'static>,
    /// ADR-0033 supervisory lifecycle for this pane, driven by inbound
    /// `TerminalControl` events: `Running` until a `Freeze` (SIGSTOP) flips it
    /// to `Frozen`. Read at paint time to render the "FROZEN" chrome badge.
    pub lifecycle: TerminalLifecycle,
    /// ADR-0033 input-lease holder for this pane (the wire `ClientId` that has
    /// "the wheel"), or `None` when the pane is `Open`. Compared against the
    /// driver's own `ClientId` to render "you" vs another client.
    pub input_holder: Option<ClientId>,
    /// phux-foz.1: `true` when an agent in this pane is waiting on a human
    /// answer. Set by an inbound ADR-0035 `AgentEvent::Asked`; cleared when
    /// the user sends key/paste input to the pane (see
    /// [`clear_attention_on_input`]). Read at chrome-paint time for the
    /// window tab marker and the status-bar attention hint.
    pub attention: bool,
    /// Start of the current DEC synchronized-output transaction (`?2026h`).
    pub sync_output_since: Option<tokio::time::Instant>,
    /// Whether mirror state changed during the transaction.
    pub sync_output_dirty: bool,
    /// phux-foz.4: the pane's working directory as the server last
    /// announced it — seeded from the `ATTACHED` snapshot's
    /// `TerminalInfo.cwd` (the spawn cwd) and refined by `cwd_changed`
    /// events. `None` until either lands. Projected into the status-bar
    /// `cwd` widget when this pane is focused.
    pub cwd: Option<String>,
    /// phux-foz.4: exit code of the last command that finished in this
    /// pane (`command_finished.exit_code`, OSC-133 `D` mark). `None`
    /// before the first command finishes or when the shell reported no
    /// code. Projected into the status-bar `exit` widget when focused.
    pub last_exit: Option<i32>,
}

impl std::fmt::Debug for PaneSlot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PaneSlot").finish_non_exhaustive()
    }
}

impl PaneSlot {
    /// Allocate a fresh slot at a known pane size.
    ///
    /// The client must size libghostty before feeding VT bytes: wrapping,
    /// clipping, absolute cursor movement, and several style spans are all
    /// width-sensitive during `vt_write`, so a later resize cannot reliably
    /// recover from a placeholder geometry.
    pub(super) fn new_with_size(cols: u16, rows: u16) -> Result<Self, AttachError> {
        let mut terminal = GhosttyTerminal::new(TerminalOptions {
            cols: cols.max(1),
            rows: rows.max(1),
            max_scrollback: 10_000,
        })?;
        phux_protocol::kitty_replay::configure_terminal_for_kitty_graphics(&mut terminal)?;
        terminal.resize(
            cols.max(1),
            rows.max(1),
            super::paint::FALLBACK_CELL_PX.0,
            super::paint::FALLBACK_CELL_PX.1,
        )?;
        Ok(Self {
            terminal,
            renderer: TerminalRenderer::new()?,
            // Panes start running and un-leased; a late-arriving
            // TerminalControl (after the pane's first snapshot/output) updates
            // these — a pane that exists before its control state is benign.
            lifecycle: TerminalLifecycle::Running,
            input_holder: None,
            attention: false,
            sync_output_since: None,
            sync_output_dirty: false,
            cwd: None,
            last_exit: None,
        })
    }

    /// Allocate a fresh slot with a conservative placeholder size.
    /// Prefer [`Self::new_with_size`] whenever the attach snapshot,
    /// viewport, or layout already tells us the pane's real dimensions.
    pub(super) fn new() -> Result<Self, AttachError> {
        Self::new_with_size(80, 24)
    }

    /// Refresh synchronized-output bookkeeping after a VT write. Returns
    /// `true` while painting must remain suppressed.
    pub(super) fn update_sync_output(&mut self, now: tokio::time::Instant) -> bool {
        let active = self.terminal.mode(Mode::SYNC_OUTPUT).unwrap_or(false);
        if active {
            self.sync_output_since.get_or_insert(now);
            self.sync_output_dirty = true;
        } else {
            self.sync_output_since = None;
            self.sync_output_dirty = false;
        }
        active
    }
}

/// ADR-0033: compose the status-bar supervisory badge for the focused pane,
/// or `None` when it is running and un-leased (so no badge paints). Reads the
/// per-pane lifecycle + input-lease holder tracked from `TerminalControl`
/// events; the holder renders as "you" when it matches this client's own id,
/// else as the other client's numeric id. No emojis (plain ASCII chrome).
fn supervisory_badge(
    panes: &HashMap<TerminalId, PaneSlot>,
    focused_pane: Option<&TerminalId>,
    own_client_id: Option<ClientId>,
) -> Option<String> {
    let slot = panes.get(focused_pane?)?;
    let frozen = matches!(slot.lifecycle, TerminalLifecycle::Frozen);
    format_supervisory_badge(frozen, slot.input_holder, own_client_id)
}

/// Pure badge formatter (split out from [`supervisory_badge`] so the
/// state→string mapping is testable without a libghostty-backed `PaneSlot`).
/// `None` ⇒ no badge (running and un-leased).
fn format_supervisory_badge(
    frozen: bool,
    input_holder: Option<ClientId>,
    own_client_id: Option<ClientId>,
) -> Option<String> {
    let wheel = input_holder.map(|holder| {
        if Some(holder) == own_client_id {
            "WHEEL:you".to_owned()
        } else {
            format!("WHEEL:c{}", holder.get())
        }
    });
    match (frozen, wheel) {
        (false, None) => None,
        (true, None) => Some("[ FROZEN ]".to_owned()),
        (false, Some(w)) => Some(format!("[ {w} ]")),
        (true, Some(w)) => Some(format!("[ FROZEN {w} ]")),
    }
}

/// phux-foz.1: compose the status-bar attention hint, or `None` when no pane
/// is waiting on a human answer. Counts every pane with the ADR-0035 asked
/// flag set (across ALL windows, not just the active one — the hint's job is
/// to surface a question the user cannot currently see).
fn attention_hint(panes: &HashMap<TerminalId, PaneSlot>) -> Option<String> {
    format_attention_hint(panes.values().filter(|slot| slot.attention).count())
}

/// Pure hint formatter (split out from [`attention_hint`] so the count→string
/// mapping is testable without a libghostty-backed `PaneSlot`). `None` ⇒ no
/// hint (nothing is asking). Plain ASCII chrome, matching the ADR-0033
/// supervisory badge convention.
fn format_attention_hint(asking: usize) -> Option<String> {
    match asking {
        0 => None,
        1 => Some("[ ASK ]".to_owned()),
        n => Some(format!("[ ASK x{n} ]")),
    }
}

/// Refresh the window strip AND the supervisory badge together (ADR-0033),
/// plus the phux-foz.1 attention hint.
///
/// All three feed one status-bar paint, so they must stay in lockstep: a site
/// that refreshed the window list on a focus/layout change but forgot the
/// badge would silently desync them. This single chokepoint makes that
/// impossible — every focus/layout-change site calls it instead of
/// hand-rolling the trio.
///
/// Returns `true` when any painter input actually changed, so a caller that
/// paints nothing else (the `chrome_dirty` event path) can gate its repaint
/// on it instead of repainting the full frame for state the user already
/// sees.
#[allow(
    clippy::too_many_arguments,
    reason = "arg list mirrors the driver's chrome state; the ADR-0040 agent index made it 8"
)]
fn refresh_window_chrome(
    status_bar: Option<&mut StatusBarPainter>,
    sidebar_painter: &mut SidebarPainter,
    workspace: &Workspace,
    panes: &HashMap<TerminalId, PaneSlot>,
    focused_pane: Option<&TerminalId>,
    zoomed: Option<&TerminalId>,
    own_client_id: Option<ClientId>,
    // ADR-0040: structured `phux.agent/v1` records; a window whose focused
    // leaf carries one is labelled from it instead of the OSC title.
    agent_meta: &HashMap<TerminalId, AgentRecord>,
) -> bool {
    let windows = window_infos(workspace, panes, zoomed, agent_meta);
    let mut changed = false;
    if let Some(sb) = status_bar {
        changed |= sb.set_windows(windows.clone());
        changed |= sb.set_supervisory(supervisory_badge(panes, focused_pane, own_client_id));
        changed |= sb.set_attention(attention_hint(panes));
        // phux-foz.4: project the focused pane's data feeds into the bar so
        // the `cwd` / `exit` widgets track focus changes and inbound
        // `cwd_changed` / `command_finished` events through this same
        // chokepoint. Unfocused (or unknown) folds to None => the widgets
        // render nothing.
        let focused = focused_pane.and_then(|id| panes.get(id));
        changed |= sb.set_focused_cwd(focused.and_then(|slot| slot.cwd.clone()));
        changed |= sb.set_last_exit(focused.and_then(|slot| slot.last_exit));
    }
    changed |= sidebar_painter.set_windows(windows);
    changed
}

/// Re-anchor the predictive-echo layer to a (newly) focused pane (phux-7ry0).
///
/// Predictions are pane-local, but the layer carries a single cursor anchor +
/// viewport. On a focus change it still holds the *previous* pane's bounds and
/// cursor; left stale, the first keystroke into the new pane echoes at the old
/// pane's coordinates — and because the pane-grid bounds clamp the next
/// outer-absolute resync, that lands mid-screen (the ghost echo after a split).
///
/// Reset the viewport to the new pane's grid and the cursor to its
/// authoritative pane-local position (or `(0, 0)` for a freshly spawned pane
/// that has not rendered yet), dropping any predictions anchored to the old
/// pane. Called from every focus-change site: click-to-focus, keybinding pane
/// navigation, and split (the new pane becomes focused).
pub(super) fn reanchor_predict_to_pane(
    predict: &mut PredictionState,
    panes: &HashMap<TerminalId, PaneSlot>,
    fid: &TerminalId,
) {
    let Some(slot) = panes.get(fid) else {
        // No slot yet — suspend until the pane's first snapshot syncs the
        // cursor and re-arms prediction.
        predict.suspend();
        return;
    };
    let cols = slot.terminal.cols().unwrap_or(0);
    let rows = slot.terminal.rows().unwrap_or(0);
    if cols > 0 && rows > 0 {
        // `set_viewport` drops the pending queue + the prompt-boundary anchor.
        predict.set_viewport(cols, rows);
    } else {
        predict.clear();
    }
    // Anchor on the pane's authoritative pane-local cursor; the overlay
    // re-adds the pane origin when painting. If the pane has not rendered yet
    // (a freshly split pane), there is NO real cursor — suspend rather than
    // anchor at (0, 0), or a quick keystroke echoes a ghost glyph at the
    // screen's top-left that the pane never overwrites (phux-7ry0 follow-up).
    match slot.renderer.last_cursor_local() {
        Some((row, col)) => predict.set_cursor(row, col),
        None => predict.suspend(),
    }
}

/// Window before a parser-pending bare ESC is interpreted as the Escape
/// key, anchored to when the ESC became pending (see `esc_deadline` in
/// `main_loop`). The client reads stdin from the *outer* terminal, which
/// writes a key's full `ESC [`/`ESC O` sequence in one burst — a split
/// only happens at a read-buffer boundary — so a short window suffices to
/// disambiguate. It must stay short: a modal-editor user pays this window
/// on EVERY bare Escape, and the inner application (vim's `ttimeoutlen`,
/// readline's `keyseq-timeout`) then stacks its own on top. tmux installs
/// ship `escape-time 0..10` for the same reason; 10ms keeps Escape under
/// the perception floor while still absorbing split sequences.
const ESC_FLUSH_IDLE: Duration = Duration::from_millis(10);

/// phux-jhv8: upper bound on how many already-queued frames one `recv`
/// wake-up drains before painting. A back-to-back output burst (nvim
/// startup) is a few dozen frames; the cap only guards against a server
/// that streams without pause starving the stdin/signal `select!` arms.
const FRAME_COALESCE_CAP: usize = 1024;

/// Safety valve for an application that enters DEC synchronized output and
/// never leaves it. Normal TUI transactions last milliseconds.
const SYNC_OUTPUT_WATCHDOG: Duration = Duration::from_secs(1);

/// The terminal a frame would repaint under normal handling, if any — the
/// `vt_write` + render pair a coalesced burst can defer to a later same-pane
/// frame (phux-jhv8). Output and snapshot frames carry pane content; every
/// other frame (layout, lifecycle, control) paints through its own path or
/// not at all, so it never defers (returns `None`).
const fn frame_paint_target(frame: &FrameKind) -> Option<&TerminalId> {
    match frame {
        FrameKind::TerminalOutput { terminal_id, .. }
        | FrameKind::TerminalSnapshot { terminal_id, .. } => Some(terminal_id),
        _ => None,
    }
}

/// Per-frame paint-deferral mask for a coalesced burst (phux-jhv8).
///
/// `targets[i]` is the pane frame `i` would repaint (`None` for control
/// frames). The result is `true` at `i` iff some later frame repaints the
/// *same* pane — meaning frame `i`'s paint is redundant and can be skipped
/// (its `vt_write` still applies). Each pane's LAST frame is therefore never
/// deferred, so every touched pane settles exactly once and none is left
/// stale; control frames (`None`) never defer.
fn coalesce_defer_flags(targets: &[Option<TerminalId>]) -> Vec<bool> {
    (0..targets.len())
        .map(|i| {
            targets[i].as_ref().is_some_and(|pane| {
                targets[i + 1..]
                    .iter()
                    .any(|later| later.as_ref() == Some(pane))
            })
        })
        .collect()
}

/// Whether a frame actually skips its paint in a coalesced burst.
///
/// `deferred_by_coalesce` is the per-pane last-wins mask from
/// [`coalesce_defer_flags`]. A frame defers iff that mask says so AND it is not
/// a [`FrameKind::TerminalSnapshot`]: a snapshot is authoritative full-screen
/// state and must always paint, never be superseded by a later same-pane
/// incremental `TerminalOutput` (whose partial paint assumes a screen the
/// snapshot never actually drew — the attach/reattach/split "mangled screen"
/// bug). Ordinary output frames still coalesce. The headless ingest path passes
/// `defer_paint = true` to `handle_server_frame` directly and never routes
/// through here, so its no-VT-emit invariant is unaffected.
const fn frame_defers_paint(deferred_by_coalesce: bool, frame: &FrameKind) -> bool {
    deferred_by_coalesce && !matches!(frame, FrameKind::TerminalSnapshot { .. })
}

/// Paint the active overlay layer (called only when an overlay is active).
///
/// Copy-mode is **not** a modal overlay: it repaints the focused pane with its
/// selection reverse-videoed — the live content is otherwise untouched, so
/// nothing on screen swaps — plus a status line. Every other overlay is modal:
/// clear the screen and paint its own surface. The branch is chosen by
/// [`OverlayState::copy_selection`].
#[allow(
    clippy::too_many_arguments,
    reason = "mirrors paint_full_frame's paint context plus the overlay state"
)]
fn paint_active_overlay<W: super::RenderSink>(
    out: &mut W,
    overlays: &OverlayState,
    workspace: &Workspace,
    panes: &mut HashMap<TerminalId, PaneSlot>,
    focused: Option<&TerminalId>,
    // phux-x2hm: the driver's pane-zoom state. The base-frame repaints below
    // render through `Workspace::render_window` so the zoomed pane fills the
    // window; the copy-mode branch keeps using the REAL active window because
    // copy mode operates on the focused pane regardless of zoom.
    zoomed: Option<&TerminalId>,
    viewport_dims: (u16, u16),
    status_bar: Option<&mut StatusBarPainter>,
    // phux-4h5a: the sidebar reservation, so base-frame repaints under an
    // overlay keep panes inset (no reflow flicker when a modal opens). The
    // strip painter isn't threaded here — overlays are transient and the
    // driver re-invalidates + repaints the strip on dismiss — so the base
    // repaint passes `None` for the painter; the reservation alone keeps the
    // tiling consistent. `None` reservation (default) is byte-identical.
    sidebar: Option<SidebarReservation>,
    session_name: &str,
    theme: &crate::render::Theme,
) {
    if let Some(sel) = overlays.copy_selection() {
        let (Some(ls), Some(fid)) = (workspace.active_window(), focused) else {
            return;
        };
        // Set the selection on the focused renderer for this one paint, repaint
        // the (zoom-honoring) base frame — the renderer inverts the selected
        // cells with their own styles — then clear it so ordinary renders are
        // unaffected. `ls` only gated the early-return on focus; the actual
        // paint goes through the zoomed view so the base matches the screen.
        let _ = ls;
        let base = workspace.render_window(zoomed);
        if let Some(slot) = panes.get_mut(fid) {
            slot.renderer.set_selection(Some(sel));
        }
        if let Some(base) = base.as_deref() {
            paint_full_frame(
                out,
                base,
                panes,
                focused,
                viewport_dims,
                status_bar,
                sidebar,
                None,
                session_name,
            );
        }
        if let Some(slot) = panes.get_mut(fid) {
            slot.renderer.set_selection(None);
        }
        let _ = paint_copy_mode_status(out, sel, viewport_dims, theme);
    } else if let Some(clip) = overlays.active_bounds(viewport_dims) {
        // Floating modal (help / prompt / command palette / pickers): keep
        // the live panes visible by repainting the base frame, then emit
        // only the modal's bounded region on top. No `\x1b[2J` — the panes
        // surround the box instead of vanishing behind a full-screen clear.
        if let Some(ls) = workspace.render_window(zoomed).as_deref() {
            paint_full_frame(
                out,
                ls,
                panes,
                focused,
                viewport_dims,
                status_bar,
                sidebar,
                None,
                session_name,
            );
        }
        let _ = overlays.paint_clipped(out, viewport_dims, clip, theme.shadow);
    } else {
        // Full-screen overlay (no bounded region): clear + paint.
        let _ = out.write_all(b"\x1b[2J\x1b[H");
        let _ = overlays.paint(out, viewport_dims);
    }
}

/// Emit the copy-mode status strip over the bottom viewport row, then hide the
/// hardware cursor (the reverse-video selection is the position indicator).
fn paint_copy_mode_status<W: Write>(
    out: &mut W,
    sel: SelectionRect,
    viewport_dims: (u16, u16),
    theme: &crate::render::Theme,
) -> io::Result<()> {
    let (cols, rows) = viewport_dims;
    if rows == 0 || cols == 0 {
        return Ok(());
    }
    let cell_count =
        u32::from(sel.end_row - sel.start_row + 1) * u32::from(sel.end_col - sel.start_col + 1);
    let status =
        format!(" copy-mode | {cell_count} cell(s) | arrows/PgUp/PgDn scroll | Enter copy | Esc ");
    write_cup(out, rows - 1, 0)?;
    // Selection strip from the theme (`selection_bg`/`selection_fg`). `\x1b[K`
    // fills the rest of the row with the strip bg; then reset + hide the cursor.
    out.write_all(b"\x1b[0m")?;
    crate::render::write_sgr_color(out, theme.selection_bg, false)?;
    crate::render::write_sgr_color(out, theme.selection_fg, true)?;
    let visible: String = status.chars().take(cols as usize).collect();
    out.write_all(visible.as_bytes())?;
    out.write_all(b"\x1b[K\x1b[0m\x1b[?25l")?;
    out.flush()
}

/// Errors the attach loop can surface to its caller.
///
/// Most variants wrap a richer underlying cause; the driver is careful to
/// fail fast rather than silently dropping protocol violations.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum AttachError {
    /// Local I/O error — UDS connect, socket read/write, stdin/stdout, or
    /// terminal ioctl.
    #[error("attach loop io error: {0}")]
    Io(#[source] io::Error),

    /// A remote transport could not be established: QUIC handshake, TLS
    /// certificate verification (a fingerprint that did not match the pin), or
    /// a refused/oversized auth preamble. Distinguished from local [`Self::Io`]
    /// so the CLI can point at the address, the pin, and the token rather than a
    /// missing socket file.
    #[error("transport connect error: {0}")]
    Connect(String),

    /// The server closed the connection without sending `DETACHED`.
    /// Distinguished from a clean detach so the CLI can surface "server
    /// went away" vs "you detached".
    #[error("connection closed by server before DETACHED")]
    Disconnected,

    /// The server sent something we cannot interpret — undecodable frame,
    /// or a valid frame we don't expect at this point in the lifecycle.
    #[error("protocol error: {0}")]
    Protocol(String),

    /// Could not put the outer terminal into the expected state.
    #[error("terminal control error: {0}")]
    Terminal(String),

    /// Stdin is not a terminal. The attach loop needs a TTY because raw
    /// mode and alt-screen toggling require one. We bail early instead of
    /// silently no-op'ing.
    #[error("stdin is not a terminal; attach requires an interactive TTY")]
    NotATty,

    /// A libghostty operation failed on the client's local Terminal.
    #[error("libghostty: {0}")]
    Ghostty(#[from] libghostty_vt::Error),

    /// The server replied with a structured `ERROR` frame instead of
    /// `ATTACHED`. The session may not exist, the protocol version may
    /// have been rejected, or some other ATTACH-time server policy
    /// refused the request. The CLI surfaces this as actionable text.
    #[error("server refused attach: {0}")]
    Refused(String),
}

impl From<io::Error> for AttachError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<super::render::RenderError> for AttachError {
    fn from(value: super::render::RenderError) -> Self {
        match value {
            super::render::RenderError::Io(e) => Self::Io(e),
            super::render::RenderError::Ghostty(e) => Self::Ghostty(e),
            super::render::RenderError::KittyReplay(e) => Self::Protocol(e.to_string()),
        }
    }
}

/// Public entry point: run an attach loop against `socket`, targeting
/// `target`. Blocks until the server sends `DETACHED` or the user
/// detaches.
///
/// The function is `async` because it relies on tokio; embedders must
/// drive it on a tokio runtime. Per ADR-0003 the canonical runtime is
/// `tokio::runtime::Builder::new_current_thread` — the returned future
/// is intentionally `!Send` because libghostty's `Terminal` is `!Send`
/// and lives on the attach task's stack across `await` points. The
/// single-threaded runtime never moves the future between threads.
///
/// # Ordering (`phux-roz`)
///
/// The expensive pre-handshake work — UDS connect, `HELLO`, `ATTACH`,
/// and the `ATTACHED` wait — runs on the *cooked* outer terminal.
/// Failures there propagate as `Err(_)` without ever entering raw mode
/// or the alt screen, so a missing server / bad session name / Ctrl-C
/// during connect prints a one-line error on the normal screen and
/// exits cleanly. Only after the server's `ATTACHED` frame arrives do
/// we flip the terminal into raw + alt screen via [`RawModeGuard`].
#[allow(
    clippy::future_not_send,
    reason = "client-side libghostty Terminal is !Send; ADR-0003 binds us to current-thread"
)]
pub async fn run(socket: &Path, target: AttachTarget) -> Result<(), AttachError> {
    run_buffered(&Dial::uds(socket), target, PredictiveConfig::disabled()).await
}

/// Production attach: wrap stdout in the off-loop [`StdoutSink`](super::stdout_writer)
/// so a slow terminal never blocks the select loop (phux-fysb), then run the
/// session. Tests use the synchronous [`run_with_stdout`] seam directly.
#[allow(
    clippy::future_not_send,
    reason = "client-side libghostty Terminal is !Send; ADR-0003 binds us to current-thread"
)]
async fn run_buffered(
    dial: &Dial,
    target: AttachTarget,
    predict: PredictiveConfig,
) -> Result<(), AttachError> {
    let (mut sink, writer) = super::stdout_writer::spawn_stdout_writer();
    let resync = Arc::clone(&sink.needs_resync);
    attach_session(
        dial,
        target,
        &mut sink,
        predict,
        Some(resync.as_ref()),
        Some(writer),
        true,
    )
    .await
}

/// Like [`run`], with predictive local echo configurable per call.
///
/// `predict.enabled = false` is identical to [`run`]; `predict.enabled =
/// true` engages the Mosh-class prediction layer documented in
/// [`crate::predict`] (`phux-9gw.1`).
///
/// Kept as a separate entry point (rather than a new parameter on
/// [`run`]) so existing callers — and the integration tests in
/// `phux-server` that exercise the attach loop — continue to compile
/// without churn.
#[allow(
    clippy::future_not_send,
    reason = "client-side libghostty Terminal is !Send; ADR-0003 binds us to current-thread"
)]
pub async fn run_with_predict(
    socket: &Path,
    target: AttachTarget,
    predict: PredictiveConfig,
) -> Result<(), AttachError> {
    run_buffered(&Dial::uds(socket), target, predict).await
}

/// Dial-aware production attach (UDS *or* QUIC) with predictive echo config.
///
/// The transport-agnostic sibling of [`run_with_predict`]: the CLI builds a
/// [`Dial`] from its flags (a UDS path or a remote `--quic` target) and the
/// same off-loop-stdout production path runs regardless of byte plumbing.
#[allow(
    clippy::future_not_send,
    reason = "client-side libghostty Terminal is !Send; ADR-0003 binds us to current-thread"
)]
pub async fn run_with_predict_dial(
    dial: &Dial,
    target: AttachTarget,
    predict: PredictiveConfig,
) -> Result<(), AttachError> {
    run_buffered(dial, target, predict).await
}

/// Same as [`run`], but writes the entire composited output stream to a
/// caller-supplied [`RenderSink`](super::RenderSink) (any `Write`).
///
/// The stream covers alt-screen enter, cursor hide, every pane's per-row
/// CUP/SGR, the status bar, overlays, and cleanup.
///
/// The renderer and all chrome painters are generic over `Write`, and the
/// driver threads this one sink through `main_loop` into
/// `handle_server_frame`, `paint_full_frame`, and `dispatch_input_events`.
/// So the whole attach render path is injectable: production passes real
/// stdout via [`run`]; tests and the headless agent surface pass a
/// `Vec<u8>` (or any other `Write`) and read back the captured VT.
///
/// Exposed so tests can capture the byte stream and assert on it — in
/// particular, the regression guard for `phux-roz` asserts that the
/// pre-handshake failure path NEVER emits `\x1b[?1049h`. The stdin /
/// signal / termios cleanup paths run on real stdout regardless of the
/// injected sink (Drop / signal handlers can't reach it).
#[allow(
    clippy::future_not_send,
    reason = "client-side libghostty Terminal is !Send; ADR-0003 binds us to current-thread"
)]
pub async fn run_with_stdout<W: super::RenderSink>(
    socket: &Path,
    target: AttachTarget,
    out: &mut W,
) -> Result<(), AttachError> {
    run_with_stdout_predict(socket, target, out, PredictiveConfig::disabled()).await
}

/// As [`run_with_stdout`], but with an explicit predictive-echo config.
/// Production callers should reach for [`run_with_predict`]; this is the
/// test-injectable variant.
#[allow(
    clippy::future_not_send,
    reason = "client-side libghostty Terminal is !Send; ADR-0003 binds us to current-thread"
)]
pub async fn run_with_stdout_predict<W: super::RenderSink>(
    socket: &Path,
    target: AttachTarget,
    out: &mut W,
    predict: PredictiveConfig,
) -> Result<(), AttachError> {
    // Synchronous-sink test seam: no off-loop writer, no resync flag.
    attach_session(&Dial::uds(socket), target, out, predict, None, None, false).await
}

/// Headless one-shot: attach, ingest the session's snapshot + layout, and
/// return the client's composited multi-pane view as dense structured cells
/// (`phux snapshot --rendered`, phux-l5xa).
///
/// Unlike the side-effect-free `GET_SCREEN` read, this **attaches** (R2): it
/// drives the same client render path the live attach loop uses, so the
/// returned frame is what the human's glass would show — pane content tiled
/// per the layout, dividers, and the status bar, composited. But it never
/// installs raw mode or an alt screen and never paints VT: frames feed the
/// pane mirrors with `defer_paint = true` (mirrors ingest, stdout is
/// suppressed), then ONE `rendered::compose_full_frame_cells` pass
/// assembles the frame. There is no TTY, so the viewport `(cols, rows)` is
/// caller-supplied.
///
/// Settle policy (R3): after the ATTACHED replay and the layout `GET`,
/// frames are drained until the stream goes idle for `SETTLE_IDLE` (the
/// server's initial snapshot burst has landed) or an overall
/// `SETTLE_DEADLINE` elapses — a quiescence wait on real frame arrival, not
/// a fixed sleep.
#[allow(
    clippy::future_not_send,
    reason = "client-side libghostty Terminal is !Send; ADR-0003 binds us to current-thread"
)]
#[allow(
    clippy::too_many_lines,
    reason = "mirrors main_loop's session-scoped local setup before one ingest-and-compose; the ~12 &mut locals would otherwise be threaded through a context struct for a single caller"
)]
pub async fn run_headless_rendered(
    socket: &Path,
    target: AttachTarget,
    cols: u16,
    rows: u16,
) -> Result<phux_core::screen::RenderedFrame, AttachError> {
    use std::time::SystemTime;

    /// Idle gap that marks the server's initial snapshot burst complete.
    const SETTLE_IDLE: Duration = Duration::from_millis(120);
    /// Hard cap on the whole drain, guarding a pathological never-idle stream.
    const SETTLE_DEADLINE: Duration = Duration::from_secs(3);

    let mut conn = Connection::connect(socket).await?;
    let _mode = handshake(&mut conn, None).await?;
    send_attach(&mut conn, target).await?;
    let attached = wait_for_attached(&mut conn).await?;

    let viewport_dims = (cols.max(1), rows.max(1));
    // Throwaway sink: `defer_paint = true` emits no VT, but
    // `handle_server_frame` still needs a `Write`.
    let mut sink: Vec<u8> = Vec::new();
    let mut panes: HashMap<TerminalId, PaneSlot> = HashMap::new();
    let mut workspace = Workspace::default();
    let mut focused_pane: Option<TerminalId> = None;
    let mut zoomed: Option<TerminalId> = None;
    let mut session_name = String::new();
    let mut status_bar = build_status_bar_painter();
    // phux-4h5a: read `[sidebar]` so `phux snapshot --rendered` shows the
    // strip exactly as a live attach would. Disabled (the default) folds to
    // `None`, keeping the rendered frame byte-identical to the pre-sidebar one.
    let headless_cfg = phux_config::loader::load().ok();
    let sidebar_cfg = headless_cfg.as_ref().map(|c| c.sidebar.clone());
    let sidebar = sidebar_cfg
        .as_ref()
        .filter(|c| c.enabled)
        .map(|c| SidebarReservation {
            edge: match c.position {
                SidebarPosition::Right => SidebarEdge::Right,
                SidebarPosition::Left => SidebarEdge::Left,
            },
            width: c.width,
        });
    let sidebar_theme = headless_cfg
        .as_ref()
        .map_or_else(crate::render::Theme::default, |c| {
            crate::render::Theme::from_cfg(&c.theme)
        });
    let mut predict = PredictionState::new(
        PredictiveConfig::disabled(),
        viewport_dims.0,
        viewport_dims.1,
    );
    let overlay = Overlay;
    let mut pending_splits: HashMap<u32, PendingSplit> = HashMap::new();
    let mut pending_windows: HashMap<u32, PendingWindow> = HashMap::new();
    let mut layout_get_request_id: Option<u32> = None;
    // ADR-0040: one-shot `phux.agent/v1` reads so the composited window
    // labels prefer structured agent records, matching a live attach.
    let mut agent_meta = AgentMetaIndex::default();

    // Replay ATTACHED so the focused-pane + workspace bootstrap runs once.
    let outcome = handle_server_frame(
        &mut sink,
        attached,
        &mut panes,
        &mut workspace,
        &mut focused_pane,
        &mut zoomed,
        &mut session_name,
        status_bar.as_mut(),
        sidebar,
        viewport_dims,
        &mut predict,
        &overlay,
        layout_get_request_id,
        &mut pending_splits,
        &mut pending_windows,
        &mut agent_meta,
        false,
        true,
    )?;
    let focused_session = outcome.sessions.map(|(_, focused)| focused);

    // ADR-0040: pipeline one `phux.agent/v1` GET per pane (no SUBSCRIBE —
    // this is a one-shot composite). Replies drain through the settle loop
    // below and land in `agent_meta.records`. Request ids start high above
    // the layout GET's `1` so the two reply streams cannot collide.
    {
        let mut req_id: u32 = 1000;
        for id in panes.keys() {
            agent_meta.pending.insert(req_id, id.clone());
            conn.send(&FrameKind::GetMetadata {
                request_id: req_id,
                scope: Scope::Terminal(id.clone()),
                key: TERMINAL_AGENT_KEY.to_owned(),
            })
            .await?;
            req_id = req_id.wrapping_add(1);
        }
    }

    // Pull any persisted multi-pane layout for this session so dividers +
    // tiling match a live attach. One-shot, so we GET but do not SUBSCRIBE.
    if outcome.subscribe_layout
        && let Some(session) = focused_session
    {
        let req_id = 1;
        layout_get_request_id = Some(req_id);
        conn.send(&FrameKind::GetMetadata {
            request_id: req_id,
            scope: Scope::Group(DEFAULT_GROUP_ID),
            key: layout_key(session),
        })
        .await?;
    }

    // Drain the initial burst until the stream goes idle (or the deadline).
    let _ = tokio::time::timeout(SETTLE_DEADLINE, async {
        loop {
            tokio::select! {
                biased;
                frame = conn.recv() => {
                    let frame = frame?;
                    handle_server_frame(
                        &mut sink,
                        frame,
                        &mut panes,
                        &mut workspace,
                        &mut focused_pane,
                        &mut zoomed,
                        &mut session_name,
                        status_bar.as_mut(),
                        sidebar,
                        viewport_dims,
                        &mut predict,
                        &overlay,
                        layout_get_request_id,
                        &mut pending_splits,
                        &mut pending_windows,
                        &mut agent_meta,
                        false,
                        true,
                    )?;
                }
                () = tokio::time::sleep(SETTLE_IDLE) => break,
            }
        }
        Ok::<(), AttachError>(())
    })
    .await;

    // Seed the window/tab strip exactly as the live loop does before its
    // first bar paint, so the composited bar shows the windows.
    let windows = window_infos(&workspace, &panes, zoomed.as_ref(), &agent_meta.records);
    if let Some(sb) = status_bar.as_mut() {
        sb.set_windows(windows.clone());
    }
    // phux-4h5a: feed the same window list into the strip painter so the
    // composited frame shows the sidebar tabs when `[sidebar]` is enabled.
    let mut sidebar_painter = SidebarPainter::new(sidebar_theme);
    sidebar_painter.set_windows(windows);

    // Compose the assembled frame against the render layout (honoring zoom).
    let layout_state = workspace.render_window(zoomed.as_ref()).map_or_else(
        crate::layout::LayoutState::default,
        std::borrow::Cow::into_owned,
    );
    let frame = super::rendered::compose_full_frame_cells(
        &layout_state,
        &mut panes,
        focused_pane.as_ref(),
        viewport_dims,
        status_bar.as_ref(),
        sidebar,
        Some(&sidebar_painter),
        &session_name,
        SystemTime::now(),
    );
    Ok(frame)
}

/// The attach session body shared by the production ([`run`]) and
/// test-injectable ([`run_with_stdout_predict`]) entry points.
///
/// `resync` is the [`StdoutSink`](super::stdout_writer) backpressure flag
/// (`None` for the synchronous test sink); `main_loop` polls it to repaint
/// the latest state after the writer dropped a stale backlog. `writer` is the
/// off-loop stdout writer's handle (`None` for the test sink); it is drained
/// and joined before the terminal-reset writes on every exit path so output
/// isn't lost and the reset isn't garbled.
#[allow(
    clippy::future_not_send,
    reason = "client-side libghostty Terminal is !Send; ADR-0003 binds us to current-thread"
)]
async fn attach_session<W: super::RenderSink>(
    dial: &Dial,
    target: AttachTarget,
    out: &mut W,
    predict: PredictiveConfig,
    resync: Option<&AtomicBool>,
    mut writer: Option<super::stdout_writer::WriterHandle>,
    probe_default_colors: bool,
) -> Result<(), AttachError> {
    // STAGE 1 — pre-handshake, on the cooked outer terminal.
    //
    // We deliberately do NOT install RawModeGuard here. If anything in
    // this block fails (no server, refused, signal during connect) the
    // user's terminal stays in its original state and `Err(_)` carries
    // the actionable cause up to the CLI.
    let mut conn = Connection::connect_dial(dial).await?;
    // Attach-handshake timing (info): HELLO -> ATTACH -> ATTACHED. The
    // span's CLOSE duration is the end-to-end attach latency a trace reader
    // wants for "why was the first paint slow." Lifecycle-rate, so info.
    let handshake_span = tracing::info_span!("attach_handshake", ?target);
    let (attached, output_mode) = async {
        let default_colors = probe_default_colors
            .then(super::terminal_probe::default_colors)
            .flatten();
        let mode = handshake(&mut conn, default_colors).await?;
        send_attach(&mut conn, target).await?;
        let attached = wait_for_attached(&mut conn).await?;
        Ok::<_, AttachError>((attached, mode))
    }
    .instrument(handshake_span)
    .await?;
    // The output mode is a per-connection HELLO property; `handshake`
    // runs exactly once per connection and the re-attach loop below reuses
    // the same `conn` without re-running it, so this bool is stable across
    // an in-connection session switch. Only a `StateSync` consumer's
    // `FRAME_ACK`s feed the server's per-seq RTT/backpressure accounting;
    // a raw consumer's acks are dropped server-side, so the loop skips them.
    let wants_state_sync = output_mode == OutputMode::StateSync;

    // STAGE 2 — server accepted the attach. Now and only now do we flip
    // the outer terminal into raw + alt screen. The guard's Drop runs
    // on unwinding; the signal-handler path inside `main_loop` runs
    // `write_terminal_reset` explicitly to cover SIGINT/SIGTERM/SIGHUP.
    //
    // ADR-0035: read the `mouse` config (default on) to decide whether the
    // guard also enables the client's own outer-terminal mouse tracking, so
    // divider drag-to-resize works by default. A load failure or an
    // explicit `mouse = false` falls back to pass-through-only — no DECSET,
    // host native selection untouched.
    let mouse_capture = phux_config::loader::load()
        .map(|c| c.defaults.mouse)
        .unwrap_or(true);
    let _guard = RawModeGuard::install_with_stdout(out, mouse_capture)?;

    // Install a panic hook so an unexpected panic inside `main_loop`
    // (renderer bug, libghostty FFI surprise, etc.) still restores the
    // terminal before the default hook prints its backtrace. The hook
    // is global, so we only register it once per process.
    install_panic_hook_once();

    // phux-eb0: outer re-attach loop. `main_loop` is single-session by
    // construction (it builds ~15 session-scoped locals and replays the
    // ATTACHED frame once on entry). When the user picks another session
    // via `<leader> a` the loop returns `LoopExit::SwitchTo(name)`; here
    // we detach from the current session, re-run the ATTACH handshake
    // against `ByName(name)` on the SAME transport connection (a session
    // switch is within one server, so the UDS connection — bound to the
    // server, not to any single session — is reused, not reconnected),
    // and re-enter `main_loop` with the new ATTACHED frame. The
    // `RawModeGuard` stays installed across the switch (it lives in this
    // outer scope) so the alt screen never flickers and the terminal is
    // never left in a bad state. On `Detached` the loop exits via
    // `exit_after_detach` (which never returns — see its doc comment).
    let mut attached = attached;
    loop {
        let exit =
            match main_loop(&mut conn, attached, predict, out, resync, wants_state_sync).await {
                Ok(exit) => exit,
                Err(err) => {
                    // Drain + stop the off-loop writer before propagating; the
                    // RawModeGuard's Drop restores the terminal as we unwind.
                    if let Some(writer) = writer.take() {
                        writer.shutdown_and_join();
                    }
                    return Err(err);
                }
            };
        match exit {
            LoopExit::Detached => {
                // Lifecycle transition (info): the attach loop is exiting.
                tracing::info!("attach loop: DETACHED; exiting");
                // The session ended (user detach, server `DETACHED`, or a
                // detach-intended disconnect). Restore the terminal and
                // exit now rather than returning up the stack: a returning
                // `Ok(())` would let the tokio runtime drop block forever
                // on the uncancellable stdin read thread (see
                // `exit_after_detach`'s doc comment).
                //
                // Drain queued output + stop the writer FIRST so nothing is
                // lost and the reset writes in `exit_after_detach` aren't
                // garbled by an in-flight frame.
                if let Some(writer) = writer.take() {
                    writer.shutdown_and_join();
                }
                exit_after_detach();
            }
            LoopExit::SwitchTo(target) => {
                // Lifecycle transition (info): switching sessions on the
                // same connection. `?target` names the destination.
                tracing::info!(?target, "attach loop: SWITCH_TO; re-attaching");
                // Tear down the current session on the server so it frees
                // our per-consumer reference grid + reaps the detached
                // consumer (the just-landed per-consumer detach reaping —
                // don't leak). DETACH does NOT close the connection
                // server-side (see `phux-server::runtime`'s DETACH arm:
                // it emits DETACHED and keeps the read loop alive), so the
                // same `conn` is reusable for the new ATTACH.
                detach_and_drain(&mut conn).await?;
                // Re-run the handshake against the new target on the same
                // connection. No reconnect: one server owns all the
                // sessions, and the transport is bound to the server, not
                // to any single session. An existing session re-attaches
                // by name; a new-session request creates it (or attaches if
                // the name is already taken) via CreateIfMissing.
                let attach_target = match target {
                    ReattachTarget::Existing(name) => AttachTarget::ByName(name),
                    ReattachTarget::Create(name) => AttachTarget::CreateIfMissing {
                        name,
                        command: None,
                        cwd: None,
                    },
                };
                send_attach(&mut conn, attach_target).await?;
                attached = wait_for_attached(&mut conn).await?;
                tracing::info!("attach loop: re-attach handshake complete");
                // Re-enter `main_loop`, which rebuilds ALL session-scoped
                // state fresh (pane mirrors, workspace, predict, overlays,
                // pending-spawn maps, layout subscription) from the new
                // ATTACHED frame, then repaints. A full repaint of the new
                // session's grid happens via the replayed ATTACHED +
                // TERMINAL_SNAPSHOT frames inside the loop.
                let _ = write_terminal_clear(out);
            }
        }
    }
}

/// phux-eb0: send `DETACH` and drain frames until `DETACHED` arrives, so
/// the server-side per-consumer state (reference grid, subscriber lists)
/// is released before the next `ATTACH` on the same connection.
///
/// Frames that arrive between our `DETACH` and the server's `DETACHED`
/// (a `TERMINAL_OUTPUT` already in flight, a late `METADATA_CHANGED`) are
/// discarded — we are tearing the session down and rebuilding all
/// session-scoped state on the next attach, so nothing in this window is
/// worth applying. A server-initiated disconnect during the drain is a
/// genuine error (the switch can't complete), surfaced as
/// `AttachError::Disconnected`.
async fn detach_and_drain(conn: &mut Connection) -> Result<(), AttachError> {
    conn.send(&FrameKind::Detach).await?;
    loop {
        match conn.recv().await? {
            FrameKind::Detached => return Ok(()),
            other => {
                tracing::trace!(kind = ?other, "draining frame during session switch");
            }
        }
    }
}

/// phux-eb0: clear the alt screen between sessions so the previous
/// session's grid doesn't briefly show under the new session's first
/// paint. The new `ATTACHED` + `TERMINAL_SNAPSHOT` repaint lands
/// immediately after, so this is a one-frame clear, not a flicker.
fn write_terminal_clear<W: Write>(out: &mut W) -> io::Result<()> {
    out.write_all(b"\x1b[2J\x1b[H")?;
    out.flush()
}

/// Whether to emit a `FRAME_ACK` for an applied `TERMINAL_OUTPUT`.
///
/// Acks are load-bearing only for a `StateSync` consumer: the server
/// folds each into that consumer's per-seq RTT/backpressure accounting
/// (`on_frame_ack`). A raw broadcast consumer's acks carry no seq the
/// server tracks, so it drops them — emitting one is a wasted client
/// write plus a server decode and state lock on the same UDS that carries
/// keystrokes during a repaint storm. In raw mode the ack is skipped; in
/// state-sync mode the `(terminal_id, seq)` flows through unchanged.
///
/// Not `const`: the `(TerminalId, u64)` it threads carries a non-trivial
/// destructor (the federation `TerminalId::Satellite` variant owns a
/// `String`), which a `const fn` may not drop at compile time.
fn should_emit_frame_ack(
    wants_state_sync: bool,
    ack: Option<(TerminalId, u64)>,
) -> Option<(TerminalId, u64)> {
    if wants_state_sync { ack } else { None }
}

/// Send `HELLO` and require `HELLO_OK` before ATTACH. Returns the
/// [`OutputMode`] the client advertised — a per-connection HELLO
/// property the caller threads into the session loop to decide whether
/// `FRAME_ACK` accounting is load-bearing.
async fn handshake(
    conn: &mut Connection,
    default_colors: Option<phux_protocol::caps::TerminalDefaultColors>,
) -> Result<OutputMode, AttachError> {
    // Sniff `$COLORTERM` / `$TERM` / `$TERM_PROGRAM` per
    // `detect_color_support`. The advertised tier feeds the server's
    // per-client `downsample::rewrite_bytes` (SPEC §6.2).
    //
    // phux-4li.5: declare L3 (`Layer::L3`) so the server forwards
    // `MetadataChanged` events for the `phux.tui.layout/v1` key — the
    // reconcile-on-attach path in `main_loop` subscribes to that key
    // and re-renders multi-pane when another client mutates the layout.
    let mut client_caps = ClientCapabilities::new()
        .with_color_support(detect_color_support())
        .with_layers(LayerSet::with(&[Layer::L3]));
    if let Some(colors) = default_colors {
        client_caps = client_caps.with_default_colors(colors);
    }
    conn.send(&FrameKind::Hello {
        client_name: format!("phux-client/{}", env!("CARGO_PKG_VERSION")),
        protocol_major: PROTOCOL_VERSION.major,
        protocol_minor: PROTOCOL_VERSION.minor,
        protocol_patch: PROTOCOL_VERSION.patch,
        client_caps,
    })
    .await?;
    match conn.recv().await? {
        FrameKind::HelloOk { .. } => Ok(client_caps.output_mode),
        FrameKind::Error { message, .. } => Err(AttachError::Refused(message)),
        other => Err(AttachError::Protocol(format!(
            "expected HELLO_OK or ERROR after HELLO, got {other:?}",
        ))),
    }
}

/// Send the `ATTACH` frame using the current terminal viewport.
async fn send_attach(conn: &mut Connection, target: AttachTarget) -> Result<(), AttachError> {
    let viewport = current_viewport()?;
    conn.send(&FrameKind::Attach {
        target,
        viewport,
        // SPEC §13: clients SHOULD opt in to scrollback. The cap below
        // matches the default in docs/consumers/tui.md §X; a configurable knob lives
        // with the rest of `phux-config`.
        request_scrollback: true,
        scrollback_limit_lines: 10_000,
    })
    .await
}

/// Read frames off `conn` until we get the expected `ATTACHED` reply,
/// surfacing a structured `Error` frame as `AttachError::Refused` and
/// any other unexpected frame as `AttachError::Protocol`.
///
/// Runs entirely on the cooked terminal (pre-`RawModeGuard`) per
/// `phux-roz`. A server-side reject prints an actionable error on the
/// normal screen and exits without flicker.
async fn wait_for_attached(conn: &mut Connection) -> Result<FrameKind, AttachError> {
    let frame = conn.recv().await?;
    match frame {
        FrameKind::Attached { .. } => Ok(frame),
        FrameKind::Error {
            code: _, message, ..
        } => Err(AttachError::Refused(message)),
        other => {
            // Anything else this early is a protocol violation. The
            // server is required to answer `ATTACH` with either
            // `ATTACHED` or `ERROR`; reject otherwise rather than
            // silently soldiering on into a half-attached state.
            Err(AttachError::Protocol(format!(
                "expected ATTACHED or ERROR after ATTACH, got {other:?}",
            )))
        }
    }
}

/// phux-eb0: how the `main_loop` `select!` loop terminated.
///
/// `main_loop` is single-session by construction — it builds all the
/// session-scoped locals up front and replays one ATTACHED frame. Rather
/// than tear down and rebuild that state in place, the loop signals its
/// caller ([`run_with_stdout_predict`]'s outer loop) which way it exited:
///
/// * `Detached` — the user detached or the server sent `DETACHED`. The
///   loop already ran `exit_after_detach` on that path (which never
///   returns); the outer loop treats a returned `Detached` as a clean
///   exit too.
/// * `SwitchTo(target)` — the user committed `switch-session { name }`
///   (via the `<leader> a` picker / palette) or `new-session`. The outer
///   loop detaches from the current session and re-runs the handshake
///   against the target (`ByName` for an existing session, `CreateIfMissing`
///   for a new one), then re-enters `main_loop` with the new ATTACHED frame
///   and freshly-rebuilt session state.
#[derive(Debug)]
enum LoopExit {
    /// The session ended (detach / server DETACHED). The process exits.
    Detached,
    /// Re-attach on the same connection — to an existing session or a
    /// newly-created one.
    SwitchTo(ReattachTarget),
}

/// Drive the `tokio::select!` loop until detach or a session switch.
///
/// `initial_attached` is the `FrameKind::Attached` frame that
/// [`wait_for_attached`] already pulled off the wire; we replay it
/// through `handle_server_frame` so the focused-pane bookkeeping lives
/// in one place. Subsequent `TERMINAL_SNAPSHOT` / `TERMINAL_OUTPUT` frames come
/// off the wire as usual.
///
/// phux-eb0: returns a [`LoopExit`] so the outer loop in
/// [`run_with_stdout_predict`] can re-attach to another session without
/// dropping the transport or leaving raw mode. Every session-scoped local
/// in this function is rebuilt on each entry, so a re-attach starts from a
/// clean slate (no stale pane mirror, no carried-over predict queue).
#[allow(
    clippy::future_not_send,
    reason = "client-side libghostty Terminal is !Send; ADR-0003 binds us to current-thread"
)]
#[allow(
    clippy::too_many_lines,
    reason = "tokio::select! arms inflate function length; splitting would require carrying ~10 mutable locals through helpers"
)]
#[allow(
    clippy::cognitive_complexity,
    reason = "select! arms + phux-4li.5 outcome dispatch; ditto"
)]
async fn main_loop<W: super::RenderSink>(
    conn: &mut Connection,
    initial_attached: FrameKind,
    predict_cfg: PredictiveConfig,
    out: &mut W,
    // phux-fysb: the off-loop StdoutSink's backpressure flag. When the writer
    // drops a stale backlog under a slow terminal it sets this; we repaint the
    // latest state from scratch (a self-contained full frame supersedes the
    // dropped diffs). `None` for the synchronous test sink.
    needs_resync: Option<&AtomicBool>,
    // Whether this connection negotiated `OutputMode::StateSync`. Gates the
    // per-frame `FRAME_ACK`: only a state-sync consumer's acks are tracked
    // server-side, so a raw consumer skips them (see `should_emit_frame_ack`).
    wants_state_sync: bool,
) -> Result<LoopExit, AttachError> {
    // phux-4li.4: hold N client-side Terminals keyed by `TerminalId`,
    // not the single Terminal of the wave-A driver. Each pane's slot is
    // allocated lazily — the first `TERMINAL_SNAPSHOT` or
    // `TERMINAL_OUTPUT` carrying a given id seeds it via
    // `panes.entry(id).or_insert_with(PaneSlot::new)`. The
    // `Workspace` mirror (initialized as a single window holding one
    // pane when `ATTACHED` lands; see `handle_server_frame`) is the
    // source of truth for which leaves are live and where they sit in
    // the outer viewport. The renderer and layout helpers operate on the
    // active window (`workspace.active_window()`); the workspace
    // dimension is what gets persisted to L3.
    let mut panes: HashMap<TerminalId, PaneSlot> = HashMap::new();
    let mut workspace = Workspace::default();
    let mut focused_pane: Option<TerminalId> = None;
    // ADR-0033: this client's own server-assigned ClientId, captured from
    // ATTACHED. Used to render "you hold the wheel" vs another client in the
    // supervisory badge. `None` until ATTACHED lands.
    let mut own_client_id: Option<ClientId> = None;
    // phux-x2hm: pane-zoom view state (driver-local, like focus). `Some(id)`
    // ⇒ pane `id` is zoomed to fill the window; render/reflow then run against
    // `workspace.render_window(zoomed)` (a synthetic single-leaf layout)
    // instead of the real tiled tree, which is left untouched for mutation.
    let mut zoomed: Option<TerminalId> = None;
    // phux-4li.5: request-id allocator for L3 GET correlation. (The
    // keybind resolver is built below, from the plugin-merged
    // keybindings snapshot.)
    let mut layout_get_request_id: Option<u32> = None;
    let mut next_request_id: u32 = 1;
    // phux-4li.12: in-flight `split-pane` actions parked by request id.
    // Populated by `run_action` when it dispatches SPAWN_TERMINAL;
    // drained by `handle_server_frame`'s TerminalSpawned arm when the
    // reply arrives. The map is small (one entry per outstanding
    // user-triggered split) so a HashMap is overkill for cap but
    // matches the layout-key request-id pattern.
    let mut pending_splits: HashMap<u32, PendingSplit> = HashMap::new();
    // phux-4li.15: in-flight `new-window` actions parked by request id,
    // same lifecycle as `pending_splits`. The TerminalSpawned arm checks
    // this map first; a hit opens a new window on the spawned pane.
    let mut pending_windows: HashMap<u32, PendingWindow> = HashMap::new();
    // ADR-0040 (phux-3ert): the structured agent-identity index. Each pane
    // gets a one-shot `GET_METADATA` + a live `SUBSCRIBE_METADATA` on
    // `phux.agent/v1` (see `sync_agent_meta_subscriptions`); decoded records
    // feed the window labels so the sidebar/tab strip renders agent
    // name/state from structured data, with the OSC title as the fallback.
    let mut agent_meta = AgentMetaIndex::default();
    // phux-nz4.5: status-bar painter, built from the on-disk config.
    // Load failures fall back to an empty bar so a malformed config
    // never blocks attach — the user still gets a working pane mirror.
    let mut status_bar = build_status_bar_painter();
    // phux-5ke.4: keybindings snapshot for the help overlay. Cached so
    // pressing the help binding doesn't trigger a synchronous config
    // reload (which could surface IO errors under user fingers); on
    // load failure the help modal still works, just showing "no
    // bindings configured".
    // phux-ahv.4: load the config once and split out both the
    // keybindings snapshot (help overlay) and the color theme (chrome +
    // overlays). On load failure both fall back to defaults — the help
    // modal shows "no bindings" and chrome paints with the built-in
    // palette.
    let loaded_cfg = phux_config::loader::load().ok();
    // phux-r82.5: snapshot the enabled plugins' manifest actions once at
    // driver start (same policy as the keybindings snapshot — no config
    // I/O under user fingers). The palette lists them; manifest-declared
    // `keys` merge into the prefix table below with user config winning
    // every conflict. A broken manifest is skipped with a warning.
    let plugin_actions: Vec<PluginActionEntry> = loaded_cfg
        .as_ref()
        .map(plugin_actions::load_plugin_action_entries)
        .unwrap_or_default();
    // The plugin-events channel: spawned plugin-action tasks report
    // completion here; the select! arm below toasts failures. Sender is
    // lent to `DispatchCtx` each batch.
    let (plugin_tx, mut plugin_rx) = tokio::sync::mpsc::unbounded_channel::<PluginRunResult>();
    let keybindings_snapshot: Option<phux_config::KeybindingsCfg> = loaded_cfg.as_ref().map(|c| {
        let mut kb = c.keybindings.clone();
        plugin_actions::merge_plugin_bindings(&mut kb, &plugin_actions);
        kb
    });
    // phux-4li.5: keybind resolver, built from the plugin-merged snapshot
    // so a manifest `keys` chord resolves exactly like a user binding.
    // The resolver consumes `InputEvent::Key` events *before* they would
    // be forwarded to the focused pane; a chord that resolves to an
    // action mutates the active window here and never reaches the
    // server's input pipe.
    let mut resolver = keybindings_snapshot.as_ref().and_then(build_resolver_from);
    // phux-ahv.4: single source of truth for chrome + overlay colors,
    // owned alongside the keybindings snapshot and threaded into the
    // overlay render path via `DispatchCtx`.
    let theme: crate::render::Theme = loaded_cfg
        .as_ref()
        .map_or_else(crate::render::Theme::default, |c| {
            crate::render::Theme::from_cfg(&c.theme)
        });
    // phux-foz.1: the attention hint's chip color comes from the theme's
    // `attention` slot rather than a hardcoded SGR in the painter.
    if let Some(sb) = status_bar.as_mut() {
        sb.set_attention_color(theme.attention);
    }
    // phux-r82.6: spawn one bounded interval runner per `exec` widget. The
    // runners execute off-loop and write into the widgets' shared caches;
    // the bar's normal repaint tick picks changed cells up, so the render
    // loop never blocks on a widget command. The guard aborts the tasks
    // (and via kill_on_drop, their children) when this attach loop ends.
    let _exec_runners = spawn_exec_feed_runners(
        status_bar
            .as_ref()
            .map(StatusBarPainter::exec_feeds)
            .unwrap_or_default(),
    );
    // phux-4h5a: window-sidebar render state, driver-local like `zoomed`. The
    // `[sidebar]` config seeds the initial enabled flag, width, and edge; the
    // `toggle-sidebar` action flips `sidebar_enabled` at runtime. Each frame
    // `sidebar_reservation()` folds these into an `Option<SidebarReservation>`
    // that threads to every layout site, so panes, dividers, reflow, mouse, and
    // the strip itself agree on the same inset. Default-off keeps the disabled
    // path byte-identical.
    let sidebar_cfg = loaded_cfg.as_ref().map(|c| c.sidebar.clone());
    let mut sidebar_enabled = sidebar_cfg.as_ref().is_some_and(|c| c.enabled);
    let sidebar_width = sidebar_cfg.as_ref().map_or(20, |c| c.width);
    let sidebar_edge = match sidebar_cfg.as_ref().map(|c| c.position) {
        Some(SidebarPosition::Right) => SidebarEdge::Right,
        _ => SidebarEdge::Left,
    };
    // The strip painter, themed like the status bar. Fed `window_infos` from
    // the same snapshot that drives the tab strip; caches so an unchanged
    // repaint emits nothing.
    let mut sidebar_painter = SidebarPainter::new(theme);
    // phux-5ke.4: overlay state — initially empty. Pushed onto by the
    // `show-help` action; drained by `OverlayState::handle_key` when
    // the active overlay returns `Dismiss`. While active, key events
    // route to the overlay (no pane forwarding) and pane stdout flushes
    // are suppressed (ADR-0020 §Decision invariant 5).
    let mut overlays = OverlayState::new();
    // ADR-0035: the in-flight divider drag. `None` between drags; a press
    // on a divider records the grabbed split, motion re-tunes it, release
    // clears it. Lives across dispatch batches (press and release land in
    // different `select!` wakeups), so it is owned here and lent to
    // `DispatchCtx` by reference each batch.
    let mut drag: Option<super::input_dispatch::DragGrab> = None;
    // Track the current outer-terminal viewport so the painter knows
    // which row is "bottom". Initialized to a sensible default and
    // updated by SIGWINCH; the server doesn't drive client-side
    // viewport (clients own their chrome per DESIGN §8.5).
    let mut viewport_dims: (u16, u16) =
        current_viewport().map_or((80, 24), |v| (v.cols.max(1), v.rows.max(1)));
    let mut session_name = String::new();
    // phux-4li.20: cache of the server's session graph, refreshed from
    // every ATTACHED snapshot. The `<leader> a` session picker reads
    // this to list peer sessions; `focused_session` marks the row the
    // client is currently attached to (excluded from the picker).
    let mut sessions: Vec<phux_protocol::wire::info::SessionInfo> = Vec::new();
    let mut focused_session: Option<phux_protocol::ids::SessionId> = None;
    let mut parser = StdinParser::new();
    // Predictive local echo (phux-9gw.1). State is updated alongside
    // every keystroke and drained on every TERMINAL_OUTPUT; when
    // `predict_cfg.enabled == false` every `predict_key` returns
    // `Disabled` so the overlay never paints.
    let mut predict = PredictionState::new(predict_cfg, 80, 24);
    let overlay = Overlay;
    let mut stdin = tokio::io::stdin();
    let mut stdin_buf = [0u8; 4096];
    let mut sigwinch = signal(SignalKind::window_change()).map_err(AttachError::Io)?;
    // `phux-roz`: SIGINT/SIGTERM/SIGHUP handlers run terminal cleanup
    // before exiting non-zero. SIGKILL is uncatchable; deferring
    // alt-screen entry until after handshake covers most real failure
    // modes for that case.
    let mut sigint = signal(SignalKind::interrupt()).map_err(AttachError::Io)?;
    let mut sigterm = signal(SignalKind::terminate()).map_err(AttachError::Io)?;
    let mut sighup = signal(SignalKind::hangup()).map_err(AttachError::Io)?;
    let mut detach_pending = false;
    // Bare-ESC disambiguation deadline, anchored to the iteration where the
    // parser first went pending. Re-creating the sleep each loop pass (the
    // pre-anchor behavior) restarted the full window whenever ANY other arm
    // fired first — under a steady output stream (status-line clock, shell
    // highlight repaints) a lone Escape could be deferred far past the
    // intended window. `None` ⇔ nothing pending.
    let mut esc_deadline: Option<tokio::time::Instant> = None;
    // phux-foz.2: which-key popup arming. When the resolver sits at the
    // pending-prefix state (`<prefix>` pressed, continuation awaited) for
    // `which_key_delay` without a follow-up chord, the loop pushes a
    // which-key overlay listing the prefix-table continuations. Config
    // comes from the same `[keybindings]` snapshot the help overlay uses;
    // with no loaded config there is no resolver (and so no prefix to
    // hesitate on), so the popup is naturally inert. `None` ⇔ not armed.
    // Same anchored-deadline pattern as `esc_deadline`: the deadline is
    // set once when the pending state is first observed and survives
    // unrelated arms firing, so a busy output stream cannot starve it.
    let which_key_enabled = keybindings_snapshot.as_ref().is_some_and(|kb| kb.which_key);
    let which_key_delay = Duration::from_millis(
        keybindings_snapshot
            .as_ref()
            .map_or(600, |kb| kb.which_key_delay_ms),
    );
    let mut which_key_deadline: Option<tokio::time::Instant> = None;
    // phux-eb0: set by `apply_action_effects` when the user commits a
    // `switch-session`. Checked after each input-dispatch batch; a value
    // here makes `main_loop` return `LoopExit::SwitchTo` so the outer
    // loop re-attaches to the named session on the same connection.
    let mut switch_request: Option<ReattachTarget> = None;

    // Replay the `ATTACHED` frame so the focused-pane bookkeeping in
    // `handle_server_frame` runs exactly once, in one place. The sidebar
    // reservation for this bootstrap frame (recomputed per-iteration in the
    // loop below to track `toggle-sidebar`).
    let sidebar = sidebar_enabled.then_some(SidebarReservation {
        edge: sidebar_edge,
        width: sidebar_width,
    });
    let outcome = handle_server_frame(
        out,
        initial_attached,
        &mut panes,
        &mut workspace,
        &mut focused_pane,
        &mut zoomed,
        &mut session_name,
        status_bar.as_mut(),
        sidebar,
        viewport_dims,
        &mut predict,
        &overlay,
        layout_get_request_id,
        &mut pending_splits,
        &mut pending_windows,
        &mut agent_meta,
        overlays.is_active(),
        // Single replayed frame — no burst to coalesce, paint it.
        false,
    )?;
    if outcome.exit {
        return Ok(LoopExit::Detached);
    }
    if let Some((list, focused)) = outcome.sessions {
        sessions = list;
        focused_session = Some(focused);
    }
    // ADR-0033: cache our own ClientId (for the "you hold the wheel" badge) and
    // opt into the agent-event stream so `TerminalControl` broadcasts (lease +
    // lifecycle) reach this client. Server-scoped (`terminal: None`) so we see
    // control events for every pane, not just one.
    if outcome.own_client_id.is_some() {
        own_client_id = outcome.own_client_id;
    }
    conn.send(&FrameKind::SubscribeEvents { terminal: None })
        .await?;
    if outcome.subscribe_layout
        && let Some(session) = focused_session
    {
        // phux-4li.5: ask the server for any persisted layout, then
        // subscribe to future mutations. Both frames are best-effort —
        // if the server rejects them with an ERROR (we'd see one in a
        // later loop iteration) we just stay in the single-pane
        // bootstrap. phux-jy4t: keyed per session so we restore THIS
        // session's layout, not whatever sibling wrote the key last.
        let key = layout_key(session);
        let req_id = next_request_id;
        layout_get_request_id = Some(req_id);
        next_request_id = next_request_id.wrapping_add(1);
        conn.send(&FrameKind::GetMetadata {
            request_id: req_id,
            scope: Scope::Group(DEFAULT_GROUP_ID),
            key: key.clone(),
        })
        .await?;
        conn.send(&FrameKind::SubscribeMetadata {
            scope: Scope::Group(DEFAULT_GROUP_ID),
            key,
        })
        .await?;
    }
    // ADR-0040: read + watch every bootstrap pane's `phux.agent/v1` record
    // so window labels can prefer structured agent identity from the first
    // paint. The same sweep re-runs whenever the pane set changes.
    sync_agent_meta_subscriptions(
        conn,
        panes.keys().cloned().collect(),
        &mut agent_meta,
        &mut next_request_id,
    )
    .await?;
    // phux-4li.17: seed the window/tab strip from the bootstrap layout so
    // the first bar paint (driven by TERMINAL_SNAPSHOT) shows the window.
    // phux-4h5a: the sidebar painter tracks the same window list so the strip's
    // tab list stays current whenever the bar's does.
    {
        refresh_window_chrome(
            status_bar.as_mut(),
            &mut sidebar_painter,
            &workspace,
            &panes,
            focused_pane.as_ref(),
            zoomed.as_ref(),
            own_client_id,
            &agent_meta.records,
        );
    }

    loop {
        // phux-4h5a: fold the driver-local sidebar render state into the
        // per-frame reservation threaded to every layout site this iteration.
        // `toggle-sidebar` flips `sidebar_enabled`; the change takes effect on
        // the next iteration. `None` (the default) keeps `content_rect` the
        // full pane viewport, so the whole path is byte-identical when the
        // sidebar is off.
        let sidebar = sidebar_enabled.then_some(SidebarReservation {
            edge: sidebar_edge,
            width: sidebar_width,
        });
        // phux-fysb: the off-loop stdout writer dropped a stale backlog under
        // a slow terminal. Repaint the latest state from scratch — a
        // self-contained full frame (or overlay) supersedes the dropped
        // diffs. `swap(false)` clears the flag, but any set re-armed by THIS
        // repaint's own flushes is preserved for the next iteration. Checked
        // before parking so a resync that landed during the prior arm is
        // serviced promptly.
        if needs_resync.is_some_and(|flag| flag.swap(false, Ordering::AcqRel)) {
            if overlays.is_active() {
                paint_active_overlay(
                    out,
                    &overlays,
                    &workspace,
                    &mut panes,
                    focused_pane.as_ref(),
                    zoomed.as_ref(),
                    viewport_dims,
                    status_bar.as_mut(),
                    sidebar,
                    &session_name,
                    &theme,
                );
            } else if let Some(ls) = workspace.render_window(zoomed.as_ref()).as_deref() {
                paint_full_frame(
                    out,
                    ls,
                    &mut panes,
                    focused_pane.as_ref(),
                    viewport_dims,
                    status_bar.as_mut(),
                    sidebar,
                    Some(&mut sidebar_painter),
                    &session_name,
                );
            }
        }

        // Arm the bare-ESC idle timer only when the parser has pending
        // state, anchored to the first iteration that saw it (the deadline
        // survives other arms firing — see `esc_deadline`). When no flush
        // is pending we substitute a never-resolving future so the select!
        // arm parks forever; this keeps the steady-state cost at one
        // always-`Pending` future and avoids unused-`Option` branches
        // inside `select!`.
        if parser.has_pending() {
            esc_deadline.get_or_insert_with(|| tokio::time::Instant::now() + ESC_FLUSH_IDLE);
        } else {
            esc_deadline = None;
        }
        let flush_sleep: std::pin::Pin<Box<dyn Future<Output = ()>>> = match esc_deadline {
            Some(deadline) => Box::pin(tokio::time::sleep_until(deadline)),
            None => Box::pin(std::future::pending::<()>()),
        };

        // phux-foz.2: (dis)arm the which-key deadline from the resolver's
        // CURRENT pending state. An early continuation chord (dispatched
        // in the stdin arm) leaves the resolver non-pending, so the next
        // pass through here disarms the timer before it can fire — the
        // popup is suppressed without any explicit cancellation call.
        update_which_key_deadline(
            &mut which_key_deadline,
            resolver
                .as_ref()
                .is_some_and(phux_config::keybind::Resolver::pending_at_prefix),
            which_key_enabled,
            overlays.is_active(),
            tokio::time::Instant::now(),
            which_key_delay,
        );
        let which_key_sleep: std::pin::Pin<Box<dyn Future<Output = ()>>> = match which_key_deadline
        {
            Some(deadline) => Box::pin(tokio::time::sleep_until(deadline)),
            None => Box::pin(std::future::pending::<()>()),
        };

        // phux-nz4.5: per-bar repaint cadence. Driven by the slowest
        // widget that wants periodic refresh (currently floor-1s via the
        // `time` widget). Empty bar ⇒ `Pending` forever so this select!
        // arm never fires.
        let status_tick: std::pin::Pin<Box<dyn Future<Output = ()>>> = match status_bar
            .as_ref()
            .and_then(StatusBarPainter::min_poll_interval)
        {
            Some(interval) => Box::pin(tokio::time::sleep(interval)),
            None => Box::pin(std::future::pending::<()>()),
        };

        // Synchronized-output transactions intentionally span arbitrary
        // socket reads, so their deadline is pane state rather than a
        // per-batch timer. A stuck producer gets one bounded recovery paint;
        // later bytes re-arm suppression if mode 2026 is still set.
        let sync_output_sleep: std::pin::Pin<Box<dyn Future<Output = ()>>> = panes
            .values()
            .filter_map(|slot| slot.sync_output_since)
            .map(|since| since + SYNC_OUTPUT_WATCHDOG)
            .min()
            .map_or_else(
                || Box::pin(std::future::pending::<()>()) as _,
                |deadline| Box::pin(tokio::time::sleep_until(deadline)) as _,
            );

        tokio::select! {
            biased;

            // Stdin is polled before inbound frames so a local keystroke
            // is dispatched promptly rather than waiting behind an output
            // burst. One read is bounded by `stdin_buf`; the inbound arm is
            // bounded by `FRAME_COALESCE_CAP`, so neither starves the other.
            n = stdin.read(&mut stdin_buf) => {
                let n = n.map_err(AttachError::Io)?;
                if n == 0 {
                    // Stdin EOF — outer terminal closed. Detach cleanly.
                    if !detach_pending {
                        conn.send(&FrameKind::Detach).await?;
                        detach_pending = true;
                    }
                    continue;
                }
                let events = parser.feed(&stdin_buf[..n]);
                // phux-x2hm: capture the PRE-dispatch zoom view's rects so a
                // zoom toggle in this batch can diff against them and resize
                // each changed pane's PTY (the reflow handshake below). Taken
                // before `dispatch_input_events` mutates `zoomed`.
                let prev_zoomed = zoomed.clone();
                let prev_zoom_rects = zoom_rects(
                    &workspace,
                    prev_zoomed.as_ref(),
                    content_rect(viewport_dims, status_bar.is_some(), sidebar),
                    viewport_dims,
                );
                let mut ctx = DispatchCtx {
                    resolver: resolver.as_mut(),
                    workspace: &mut workspace,
                    viewport: viewport_dims,
                    next_request_id: &mut next_request_id,
                    pending_splits: &mut pending_splits,
                    pending_windows: &mut pending_windows,
                    overlays: &mut overlays,
                    keybindings: keybindings_snapshot.as_ref(),
                    theme: &theme,
                    sessions: &sessions,
                    focused_session,
                    session_name: &mut session_name,
                    switch_request: &mut switch_request,
                    zoomed: &mut zoomed,
                    sidebar,
                    sidebar_enabled: &mut sidebar_enabled,
                    has_bar: status_bar.is_some(),
                    drag: &mut drag,
                    plugin_actions: &plugin_actions,
                    plugin_tx: Some(&plugin_tx),
                };
                let layout_changed = dispatch_input_events(
                    out,
                    conn,
                    events,
                    &mut focused_pane,
                    &mut detach_pending,
                    &mut predict,
                    &overlay,
                    &mut panes,
                    &mut ctx,
                )
                .await?;
                // phux-4h5a: a `toggle-sidebar` in this batch flipped
                // `sidebar_enabled`. Re-fold it into the reservation so the
                // reflow + repaint below tile into the NEW content rect this
                // iteration rather than waiting a frame.
                let sidebar = sidebar_enabled.then_some(SidebarReservation {
                    edge: sidebar_edge,
                    width: sidebar_width,
                });
                // phux-eb0: a committed `switch-session` ends this loop so
                // the outer driver re-attaches. Return BEFORE any repaint
                // — the new session's ATTACHED + snapshot will repaint.
                if let Some(target) = switch_request.take() {
                    return Ok(LoopExit::SwitchTo(target));
                }
                // phux-x2hm: the zoom state flipped this batch — emit a
                // TERMINAL_RESIZE per pane whose dims changed (the zoomed pane
                // grows to fill the window; on un-zoom every pane shrinks back).
                // Sent BEFORE the repaint, mirroring the close/SIGWINCH reflow.
                if zoomed != prev_zoomed {
                    emit_zoom_reflow(
                        conn,
                        &workspace,
                        zoomed.as_ref(),
                        &prev_zoom_rects,
                        content_rect(viewport_dims, status_bar.is_some(), sidebar),
                    )
                    .await?;
                }
                if layout_changed {
                    // ADR-0040: an input action may have split/closed panes;
                    // keep the agent-metadata watches in step with the set.
                    sync_agent_meta_subscriptions(
                        conn,
                        panes.keys().cloned().collect(),
                        &mut agent_meta,
                        &mut next_request_id,
                    )
                    .await?;
                    refresh_window_chrome(
                        status_bar.as_mut(),
                        &mut sidebar_painter,
                        &workspace,
                        &panes,
                        focused_pane.as_ref(),
                        zoomed.as_ref(),
                        own_client_id,
                        &agent_meta.records,
                    );
                    // phux-5ke.4: on overlay dismiss the dispatcher
                    // sets layout_changed=true; the full-frame repaint
                    // below restores pane content under the now-gone
                    // modal. When the overlay is still active (e.g.
                    // a push happened in the same batch) we skip the
                    // pane repaint and go straight to overlay paint.
                    if !overlays.is_active()
                        && let Some(ls) = workspace.render_window(zoomed.as_ref()).as_deref()
                    {
                        paint_full_frame(
                            out,
                            ls,
                            &mut panes,
                            focused_pane.as_ref(),
                            viewport_dims,
                            status_bar.as_mut(),
                            sidebar,
                            Some(&mut sidebar_painter),
                            &session_name,
                        );
                    }
                }
                if overlays.is_active() {
                    paint_active_overlay(
                        out,
                        &overlays,
                        &workspace,
                        &mut panes,
                        focused_pane.as_ref(),
                        zoomed.as_ref(),
                        viewport_dims,
                        status_bar.as_mut(),
                        sidebar,
                        &session_name,
                        &theme,
                    );
                }
            }

            // Inbound frames are drained in a `FRAME_COALESCE_CAP`-bounded
            // batch so a redraw burst paints once; bounded so it cannot
            // starve the stdin arm polled above it.
            frame = conn.recv() => {
                match frame {
                    Ok(first) => {
                        // phux-jhv8: drain every frame already queued so a
                        // back-to-back output burst (nvim startup, a
                        // full-screen redraw) applies all its vt_writes and
                        // paints ONCE — on the final frame — instead of a
                        // render + blocking flush per frame. The non-blocking
                        // try_recv stops the moment the socket would block, so
                        // a lone frame keeps the old one-frame-one-paint path.
                        let mut batch = vec![first];
                        while batch.len() < FRAME_COALESCE_CAP {
                            match conn.try_recv() {
                                Ok(Some(more)) => batch.push(more),
                                // Socket drained, or a clean EOF the next
                                // `recv()` will surface as Disconnected.
                                Ok(None) | Err(AttachError::Disconnected) => break,
                                Err(err) => return Err(err),
                            }
                        }
                        // Per-pane last-wins: a frame defers its paint iff a
                        // LATER frame in the burst repaints the same pane, so
                        // every touched pane (focused or not) settles exactly
                        // once on its final frame. No pane is left stale, and
                        // the hot single-pane case collapses to one paint.
                        let paint_targets: Vec<Option<TerminalId>> = batch
                            .iter()
                            .map(|f| frame_paint_target(f).cloned())
                            .collect();
                        let defer_flags = coalesce_defer_flags(&paint_targets);
                        for (frame_idx, f) in batch.into_iter().enumerate() {
                        let defer_paint = frame_defers_paint(defer_flags[frame_idx], &f);
                        // phux-tnh: snapshot the current per-leaf rects
                        // BEFORE the frame may fold (close) or split the
                        // layout, so a TerminalClosed/Spawned can diff
                        // against them and resize survivors whose dims
                        // changed. Only meaningful in multi-pane mode;
                        // skipped (no cost) on the single-pane hot path.
                        // phux-x2hm: snapshot the zoom-honoring rects so a
                        // close/spawn diffs against what is actually on screen;
                        // a TerminalSpawned-ok un-zooms (sets `zoomed = None`)
                        // inside `handle_server_frame`, so the post-frame view
                        // below correctly reflows every pane back to its tile.
                        let prev_rects = workspace
                            .render_window(zoomed.as_ref())
                            .and_then(|ls| {
                                ls.tree.as_ref().map(|_| {
                                    super::multi_pane::compute_layout_in(
                                        ls.as_ref(),
                                        content_rect(
                                            viewport_dims,
                                            status_bar.is_some(),
                                            sidebar,
                                        ),
                                        viewport_dims,
                                    )
                                    .rects
                                })
                            });
                        let outcome = handle_server_frame(
                            out,
                            f,
                            &mut panes,
                            &mut workspace,
                            &mut focused_pane,
                            &mut zoomed,
                            &mut session_name,
                            status_bar.as_mut(),
                            sidebar,
                            viewport_dims,
                            &mut predict,
                            &overlay,
                            layout_get_request_id,
                            &mut pending_splits,
                            &mut pending_windows,
                            &mut agent_meta,
                            overlays.is_active(),
                            defer_paint,
                        )?;
                        if outcome.exit {
                            return Ok(LoopExit::Detached);
                        }
                        // ADR-0040: the frame may have added panes
                        // (TerminalSpawned, a peer's layout broadcast) or
                        // removed them (TerminalClosed). Re-sweep so every
                        // live pane has a `phux.agent/v1` watch; the len
                        // guard keeps the steady state zero-cost.
                        if panes.len() != agent_meta.subscribed.len() {
                            sync_agent_meta_subscriptions(
                                conn,
                                panes.keys().cloned().collect(),
                                &mut agent_meta,
                                &mut next_request_id,
                            )
                            .await?;
                        }
                        // phux-4li.20: refresh the cached session graph
                        // whenever an ATTACHED snapshot lands so the
                        // session picker lists the current peer set.
                        if let Some((list, focused)) = outcome.sessions {
                            sessions = list;
                            focused_session = Some(focused);
                        }
                        // ADR-0033 / phux-foz.1: a `TerminalControl` or `Asked`
                        // event changed a pane's lease/lifecycle/attention. The
                        // event frame paints nothing, so refresh the chrome
                        // (supervisory badge, attention hint, window markers)
                        // and repaint here — but only when a painter input
                        // actually changed (`refresh_window_chrome` reports
                        // it), so an event that alters no visible state doesn't
                        // force a full-window repaint. (`own_client_id` is
                        // fixed for the life of this loop; it was captured at
                        // bootstrap.)
                        if outcome.chrome_dirty {
                            let chrome_changed = refresh_window_chrome(
                                status_bar.as_mut(),
                                &mut sidebar_painter,
                                &workspace,
                                &panes,
                                focused_pane.as_ref(),
                                zoomed.as_ref(),
                                own_client_id,
                                &agent_meta.records,
                            );
                            if chrome_changed
                                && !overlays.is_active()
                                && let Some(ls) =
                                    workspace.render_window(zoomed.as_ref()).as_deref()
                            {
                                paint_full_frame(
                                    out,
                                    ls,
                                    &mut panes,
                                    focused_pane.as_ref(),
                                    viewport_dims,
                                    status_bar.as_mut(),
                                    sidebar,
                                    Some(&mut sidebar_painter),
                                    &session_name,
                                );
                            }
                        }
                        // phux-3uv / ADR-0018: ack the applied TERMINAL_OUTPUT
                        // so the server's per-consumer SnapshotSynthesizer
                        // clears the dirty bits that produced this frame
                        // (mark_synced) and the next state-sync tick re-diffs
                        // against the acked reference. Without this the
                        // server re-emits an ever-growing unacked delta
                        // forever (see `tick_emit`). Cumulative per SPEC §12.2.
                        // Gated on the negotiated mode: a raw consumer's acks
                        // are dropped server-side, so skipping them removes a
                        // wasted write on the keystroke-carrying UDS during a
                        // repaint burst.
                        if let Some((terminal_id, seq)) =
                            should_emit_frame_ack(wants_state_sync, outcome.ack)
                        {
                            conn.send(&FrameKind::FrameAck { terminal_id, seq }).await?;
                        }
                        // phux-4li.12: a layout mutation triggered by a
                        // server frame (TerminalSpawned ok, TerminalClosed)
                        // requires the same `SET_METADATA` broadcast as
                        // a local action — see `ActionEffects.set_metadata`
                        // for the local-action path.
                        if outcome.emit_set_metadata
                            && let Some(session) = focused_session
                            && let Some(bytes) = encode_layout_or_log(&workspace)
                        {
                            let request_id = next_request_id;
                            next_request_id = next_request_id.wrapping_add(1);
                            conn.send(&FrameKind::SetMetadata {
                                request_id,
                                scope: Scope::Group(DEFAULT_GROUP_ID),
                                key: layout_key(session),
                                value: bytes,
                            })
                            .await?;
                        }
                        // phux-tnh: a pane close/spawn changed surviving
                        // panes' dimensions. Diff the folded/split layout
                        // against the pre-frame rects and emit a
                        // TERMINAL_RESIZE per changed leaf — same path the
                        // SIGWINCH arm uses — so the server reflows each
                        // PTY (TIOCSWINSZ) and the shell redraws to fill.
                        // Without this the survivor of a close keeps its
                        // old small winsize ("survivor stays small").
                        // Sent BEFORE the repaint so the server's resync
                        // snapshot lands after the local mirror has grown.
                        if outcome.reflow_panes
                            && let Some(prev_rects) = &prev_rects
                            && let Some(ls) = workspace.render_window(zoomed.as_ref())
                            && ls.tree.is_some()
                        {
                            let new_content =
                                content_rect(viewport_dims, status_bar.is_some(), sidebar);
                            let diff = super::reflow::compute_reflow(
                                ls.as_ref(),
                                prev_rects,
                                new_content,
                            );
                            for (terminal_id, new_rect) in &diff.changed {
                                conn.send(&FrameKind::TerminalResize {
                                    terminal_id: terminal_id.clone(),
                                    cols: new_rect.w,
                                    rows: new_rect.h,
                                })
                                .await?;
                            }
                        }
                        if outcome.layout_replaced {
                            // phux-4li.5: layout changed under us
                            // (either the GET reply or a peer's broadcast).
                            // Trigger a full repaint: clear screen + paint
                            // dividers + re-render every pane.
                            // phux-5ke.4: while an overlay is up, defer
                            // the repaint — the dismiss path always
                            // triggers paint_full_frame, and the
                            // libghostty mirror is already updated.
                            refresh_window_chrome(
                                status_bar.as_mut(),
                                &mut sidebar_painter,
                                &workspace,
                                &panes,
                                focused_pane.as_ref(),
                                zoomed.as_ref(),
                                own_client_id,
                                &agent_meta.records,
                            );
                            if !overlays.is_active()
                                && let Some(ls) =
                                    workspace.render_window(zoomed.as_ref()).as_deref()
                            {
                                paint_full_frame(
                                    out,
                                    ls,
                                    &mut panes,
                                    focused_pane.as_ref(),
                                    viewport_dims,
                                    status_bar.as_mut(),
                                    sidebar,
                                    Some(&mut sidebar_painter),
                                    &session_name,
                                );
                            }
                            // The GET reply is single-use; clear the
                            // pending request id so a stray late
                            // MetadataValue can't trample state.
                            layout_get_request_id = None;
                        }
                        // ADR-0040: a `phux.agent/v1` record changed (GET
                        // reply or subscribed broadcast). The window labels
                        // derive from it, so recompose the tab strip +
                        // sidebar and repaint — the same shape as the
                        // layout_replaced arm, minus the layout bookkeeping.
                        if outcome.agent_meta_changed {
                            refresh_window_chrome(
                                status_bar.as_mut(),
                                &mut sidebar_painter,
                                &workspace,
                                &panes,
                                focused_pane.as_ref(),
                                zoomed.as_ref(),
                                own_client_id,
                                &agent_meta.records,
                            );
                            if !overlays.is_active()
                                && let Some(ls) =
                                    workspace.render_window(zoomed.as_ref()).as_deref()
                            {
                                paint_full_frame(
                                    out,
                                    ls,
                                    &mut panes,
                                    focused_pane.as_ref(),
                                    viewport_dims,
                                    status_bar.as_mut(),
                                    sidebar,
                                    Some(&mut sidebar_painter),
                                    &session_name,
                                );
                            }
                        }
                        }
                    }
                    Err(AttachError::Disconnected) if detach_pending => {
                        // Server closed the socket without a `DETACHED`
                        // frame — treat it as a clean shutdown because
                        // the user requested detach. Otherwise the loop
                        // bubbles the disconnect up unchanged.
                        return Ok(LoopExit::Detached);
                    }
                    Err(err) => return Err(err),
                }
            }

            // Bound the failure mode of an application that omits `?2026l`.
            // Expose the latest complete mirror once, then let subsequent
            // output re-arm the transaction watchdog.
            () = sync_output_sleep => {
                let now = tokio::time::Instant::now();
                let mut expired = false;
                for slot in panes.values_mut() {
                    if slot.sync_output_dirty
                        && slot.sync_output_since.is_some_and(|since| {
                            now.saturating_duration_since(since) >= SYNC_OUTPUT_WATCHDOG
                        })
                    {
                        slot.sync_output_since = None;
                        slot.sync_output_dirty = false;
                        expired = true;
                    }
                }
                if expired
                    && !overlays.is_active()
                    && let Some(ls) = workspace.render_window(zoomed.as_ref()).as_deref()
                {
                    paint_full_frame(
                        out,
                        ls,
                        &mut panes,
                        focused_pane.as_ref(),
                        viewport_dims,
                        status_bar.as_mut(),
                        sidebar,
                        Some(&mut sidebar_painter),
                        &session_name,
                    );
                }
            }

            // Bare-ESC idle timeout. Only armed when the parser has
            // pending state; resolves an ambiguous lone ESC into the
            // Escape key (see input::StdinParser::flush docs).
            () = flush_sleep => {
                let events = parser.flush();
                // phux-x2hm: a flushed bare-ESC chord can also resolve to
                // `toggle-zoom`; capture the pre-toggle zoom rects for the
                // reflow handshake, exactly as the stdin arm does.
                let prev_zoomed = zoomed.clone();
                let prev_zoom_rects = zoom_rects(
                    &workspace,
                    prev_zoomed.as_ref(),
                    content_rect(viewport_dims, status_bar.is_some(), sidebar),
                    viewport_dims,
                );
                let mut ctx = DispatchCtx {
                    resolver: resolver.as_mut(),
                    workspace: &mut workspace,
                    viewport: viewport_dims,
                    next_request_id: &mut next_request_id,
                    pending_splits: &mut pending_splits,
                    pending_windows: &mut pending_windows,
                    overlays: &mut overlays,
                    keybindings: keybindings_snapshot.as_ref(),
                    theme: &theme,
                    sessions: &sessions,
                    focused_session,
                    session_name: &mut session_name,
                    switch_request: &mut switch_request,
                    zoomed: &mut zoomed,
                    sidebar,
                    sidebar_enabled: &mut sidebar_enabled,
                    has_bar: status_bar.is_some(),
                    drag: &mut drag,
                    plugin_actions: &plugin_actions,
                    plugin_tx: Some(&plugin_tx),
                };
                let layout_changed = dispatch_input_events(
                    out,
                    conn,
                    events,
                    &mut focused_pane,
                    &mut detach_pending,
                    &mut predict,
                    &overlay,
                    &mut panes,
                    &mut ctx,
                )
                .await?;
                // phux-4h5a: re-fold a `toggle-sidebar` flip into the
                // reservation, same as the stdin arm, so the same-iteration
                // repaint tiles into the new content rect.
                let sidebar = sidebar_enabled.then_some(SidebarReservation {
                    edge: sidebar_edge,
                    width: sidebar_width,
                });
                // phux-eb0: same switch-on-commit check as the stdin arm.
                // A bare-ESC flush can carry the final chord of a
                // `<leader> a` selection committed via Enter.
                if let Some(target) = switch_request.take() {
                    return Ok(LoopExit::SwitchTo(target));
                }
                if zoomed != prev_zoomed {
                    emit_zoom_reflow(
                        conn,
                        &workspace,
                        zoomed.as_ref(),
                        &prev_zoom_rects,
                        content_rect(viewport_dims, status_bar.is_some(), sidebar),
                    )
                    .await?;
                }
                if layout_changed {
                    // ADR-0040: keep the agent-metadata watches in step
                    // with a pane set changed by this flush's actions.
                    sync_agent_meta_subscriptions(
                        conn,
                        panes.keys().cloned().collect(),
                        &mut agent_meta,
                        &mut next_request_id,
                    )
                    .await?;
                    refresh_window_chrome(
                        status_bar.as_mut(),
                        &mut sidebar_painter,
                        &workspace,
                        &panes,
                        focused_pane.as_ref(),
                        zoomed.as_ref(),
                        own_client_id,
                        &agent_meta.records,
                    );
                }
                if layout_changed
                    && !overlays.is_active()
                    && let Some(ls) = workspace.render_window(zoomed.as_ref()).as_deref()
                {
                    paint_full_frame(
                        out,
                        ls,
                        &mut panes,
                        focused_pane.as_ref(),
                        viewport_dims,
                        status_bar.as_mut(),
                        sidebar,
                        Some(&mut sidebar_painter),
                        &session_name,
                    );
                }
                if overlays.is_active() {
                    paint_active_overlay(
                        out,
                        &overlays,
                        &workspace,
                        &mut panes,
                        focused_pane.as_ref(),
                        zoomed.as_ref(),
                        viewport_dims,
                        status_bar.as_mut(),
                        sidebar,
                        &session_name,
                        &theme,
                    );
                }
            }

            // phux-foz.2: which-key idle timeout. Armed only while the
            // resolver sits at the pending-prefix state (see the update
            // above); fires once per hesitation. Pushing the popup does
            // not touch the resolver — the pending prefix stays live, so
            // the next chord executes exactly as if the popup never
            // appeared (the dispatcher's passthrough branch dismisses it
            // on the way through).
            () = which_key_sleep => {
                which_key_deadline = None;
                if push_which_key_overlay(
                    &mut overlays,
                    resolver.as_ref(),
                    keybindings_snapshot.as_ref(),
                    &theme,
                ) {
                    paint_active_overlay(
                        out,
                        &overlays,
                        &workspace,
                        &mut panes,
                        focused_pane.as_ref(),
                        zoomed.as_ref(),
                        viewport_dims,
                        status_bar.as_mut(),
                        sidebar,
                        &session_name,
                        &theme,
                    );
                }
            }

            // SIGWINCH — terminal was resized. Read the new viewport
            // and ship a VIEWPORT_RESIZE upstream (SPEC §7.1 / §10.5).
            // The server uses this to recompute layout and update the
            // attached pane's dims. On query failure we fall back to a
            // sane default (logged) rather than skip the frame — the
            // server still benefits from knowing a resize happened.
            _ = sigwinch.recv() => {
                let prev_dims = viewport_dims;
                let viewport = current_viewport_or_default();
                viewport_dims = (viewport.cols.max(1), viewport.rows.max(1));
                // Bound predict to the FOCUSED pane's current grid, not the
                // whole viewport — predictions are pane-local (phux-7ry0). The
                // pane grids resize on the server's resize-ack snapshot, which
                // re-syncs predict again; this just keeps the transient
                // post-SIGWINCH bounds pane-shaped. Single-pane / unknown
                // falls back to the viewport.
                let (predict_cols, predict_rows) = focused_pane
                    .as_ref()
                    .and_then(|fid| panes.get(fid))
                    .map_or((viewport.cols, viewport.rows), |s| {
                        (
                            s.terminal.cols().unwrap_or(viewport.cols),
                            s.terminal.rows().unwrap_or(viewport.rows),
                        )
                    });
                predict.set_viewport(predict_cols, predict_rows);
                conn.send(&viewport_resize_frame(viewport)).await?;

                // Multi-pane: emit one TERMINAL_RESIZE per leaf whose
                // (w, h) actually changed so the server ioctls TIOCSWINSZ
                // on each PTY. Single-pane: skip the reflow math entirely
                // (no per-leaf wire emissions to make).
                if let Some(ls) = workspace.render_window(zoomed.as_ref())
                    && ls.tree.is_some()
                {
                    let has_bar = status_bar.is_some();
                    // phux-4h5a: size each PTY to the inset content rect (the
                    // pane area after the status bar + sidebar reservation),
                    // not the full viewport — otherwise an enabled sidebar
                    // resizes panes to the full width while they paint inset.
                    let prev_content = content_rect(prev_dims, has_bar, sidebar);
                    let new_content = content_rect(viewport_dims, has_bar, sidebar);
                    let prev_rects =
                        super::multi_pane::compute_layout_in(ls.as_ref(), prev_content, prev_dims)
                            .rects;
                    let diff = super::reflow::compute_reflow(
                        ls.as_ref(),
                        &prev_rects,
                        new_content,
                    );
                    if diff.too_small {
                        tracing::warn!(
                            cols = viewport_dims.0,
                            rows = viewport_dims.1,
                            "viewport too small for current layout; rendering may be garbled",
                        );
                    }
                    for (terminal_id, new_rect) in &diff.changed {
                        conn.send(&FrameKind::TerminalResize {
                            terminal_id: terminal_id.clone(),
                            cols: new_rect.w,
                            rows: new_rect.h,
                        })
                        .await?;
                    }
                }
                // phux-a7fz: do not repaint stale pre-resize mirrors into the
                // new viewport. The server resize path sends an authoritative
                // resync snapshot; painting the old grid first races with the
                // shell's prompt redraw and leaves duplicated right prompts on
                // resize-heavy shells. Clear immediately, then let the snapshot
                // repopulate the viewport at the new dimensions.
                let _ = out.write_all(b"\x1b[2J\x1b[H");
                if overlays.is_active() {
                    paint_active_overlay(
                        out,
                        &overlays,
                        &workspace,
                        &mut panes,
                        focused_pane.as_ref(),
                        zoomed.as_ref(),
                        viewport_dims,
                        status_bar.as_mut(),
                        sidebar,
                        &session_name,
                        &theme,
                    );
                } else {
                    let _ = out.flush();
                }
            }

            // phux-nz4.5: periodic status-bar repaint (e.g. for the
            // `time` widget). Only fires when at least one widget has a
            // `poll_interval`. Paints in place — no pane re-render, no
            // full-screen redraw.
            () = status_tick => {
                // phux-5ke.4: an overlay above the bar would get
                // partially overwritten by the bar paint; skip ticks
                // while a modal is up.
                if !overlays.is_active() {
                    // Restore the cursor to wherever the focused pane left it
                    // so an idle tick doesn't strand the cursor in the bar.
                    let focused_cursor = focused_pane.as_ref()
                        .and_then(|fid| panes.get(fid))
                        .and_then(|slot| slot.renderer.last_cursor());
                    // phux-9xn / phux-gxy: ALWAYS provide a fallback
                    // origin. When focused_pane is None (e.g. ATTACHED
                    // hasn't seeded yet) the old code passed None →
                    // paint_bar_after_pane emitted no CUP → cursor
                    // stranded at the bar's last cell every tick.
                    let has_bar = status_bar.is_some();
                    let content = content_rect(viewport_dims, has_bar, sidebar);
                    let fallback_origin = Some(
                        focused_pane
                            .as_ref()
                            .and_then(|fid| {
                                workspace.render_window(zoomed.as_ref()).and_then(|ls| {
                                    super::multi_pane::compute_layout_in(
                                        ls.as_ref(),
                                        content,
                                        viewport_dims,
                                    )
                                    .rects
                                    .get(fid)
                                    .copied()
                                })
                            })
                            .map_or((0, 0), |r| (r.x, r.y)),
                    );
                    tracing::trace!(
                        focused_pane_set = focused_pane.is_some(),
                        has_cursor = focused_cursor.is_some(),
                        "status_tick: repaint bar"
                    );
                    paint_bar_after_pane(
                        status_bar.as_mut(),
                        out,
                        viewport_dims,
                        &session_name,
                        focused_cursor,
                        fallback_origin,
                        // Idle tick: nothing clobbered the bar row. The
                        // painter's content cache repaints only when a
                        // widget (e.g. the clock) actually changed.
                        false,
                    );
                }
            }

            // phux-r82.5: a spawned plugin action finished. Successes just
            // log (no modal to dismiss on the happy path); failures push a
            // dismissable toast carrying the captured output, so a broken
            // plugin is *seen* without ever having blocked the input loop.
            // The channel can't close while this loop holds `plugin_tx`,
            // so the `Some` pattern always matches when the arm fires.
            Some(result) = plugin_rx.recv() => {
                tracing::info!(
                    plugin = %result.plugin_id,
                    action = %result.action_id,
                    ok = plugin_actions::run_succeeded(&result),
                    "plugin action finished",
                );
                if let Some((title, lines)) = plugin_actions::failure_toast(&result) {
                    overlays.push(Box::new(crate::render::overlay::ToastOverlay::new(
                        title, lines, &theme,
                    )));
                    paint_active_overlay(
                        out,
                        &overlays,
                        &workspace,
                        &mut panes,
                        focused_pane.as_ref(),
                        zoomed.as_ref(),
                        viewport_dims,
                        status_bar.as_mut(),
                        sidebar,
                        &session_name,
                        &theme,
                    );
                }
            }

            // SIGINT — restore the terminal explicitly (Drop wouldn't
            // fire on `exit(130)`), then exit with the shell-conventional
            // 130. `phux-roz`: this is the path that fires when the user
            // hits Ctrl-C in the outer shell after `phux attach` has
            // entered the alt screen.
            _ = sigint.recv() => {
                terminal_reset_on_signal();
                #[allow(clippy::exit, reason = "signal-driven graceful exit; Drop won't run")]
                std::process::exit(130);
            }

            // SIGTERM — `kill <pid>` from a sibling tool, supervisor, or
            // the user's tmux/screen wrapping us. Same cleanup, exit 143.
            _ = sigterm.recv() => {
                terminal_reset_on_signal();
                #[allow(clippy::exit, reason = "signal-driven graceful exit; Drop won't run")]
                std::process::exit(143);
            }

            // SIGHUP — controlling terminal went away. Restore and exit
            // 129. There is no live outer terminal to clean up, but the
            // termios restore is harmless on a dead tty and keeps the
            // cleanup path uniform.
            _ = sighup.recv() => {
                terminal_reset_on_signal();
                #[allow(clippy::exit, reason = "signal-driven graceful exit; Drop won't run")]
                std::process::exit(129);
            }
        }
    }
}

/// phux-4li.5: L3 metadata key PREFIX under which the multi-pane layout
/// envelope persists (ADR-0019 decision 1). The reference TUI is the
/// sole consumer; other clients (a future GUI, an agent) never read
/// or write it.
///
/// The persisted key is per-session: [`layout_key`] suffixes this with the
/// session id so each session keeps its OWN layout, isolated in the shared
/// group's metadata. Before phux-jy4t every session wrote this bare key, so a
/// new session inherited (and clobbered) its sibling's tree.
pub(super) const LAYOUT_KEY: &str = "phux.tui.layout/v1";

/// The per-session layout metadata key: [`LAYOUT_KEY`] suffixed with the
/// session id (phux-jy4t). Two clients on the same session share one key (and
/// thus one layout + subscription); different sessions are isolated.
pub(super) fn layout_key(session: phux_protocol::ids::SessionId) -> String {
    format!("{LAYOUT_KEY}/{}", session.get())
}

/// Whether `key` is any session's layout key — the bare [`LAYOUT_KEY`] (legacy
/// persisted value) or a `LAYOUT_KEY/<session>` form. Used to recognise layout
/// `SET_METADATA` broadcasts (a client only ever receives its own session's).
pub(super) fn is_layout_key_string(key: &str) -> bool {
    key == LAYOUT_KEY || key.starts_with(&format!("{LAYOUT_KEY}/"))
}

/// phux-4li.5: the single Group v0.1 servers expose. The grouping tier
/// is not a wire lifecycle; every L3 key the reference TUI cares about
/// is scoped to this constant. Matches `phux_server::state::DEFAULT_GROUP_ID`
/// (the server picks the same numeric value; if they ever drift, the
/// L3 reconcile path silently no-ops because the broadcast scope
/// won't match).
pub(super) const DEFAULT_GROUP_ID: GroupId = GroupId::new(1);

/// phux-4li.5: build a [`phux_config::keybind::Resolver`] from a
/// keybindings snapshot (post phux-r82.5: the plugin-merged one, so
/// manifest `keys` chords resolve like user bindings — the merge already
/// validated each contributed chord, so a plugin can't poison this
/// build). Failures log and return `None` — a malformed `[keybindings]`
/// table degrades to "no actions are bound" rather than blocking attach.
/// Detach is a normal keybinding action, so a disabled resolver also
/// disables configured detach chords.
fn build_resolver_from(kb: &phux_config::KeybindingsCfg) -> Option<phux_config::keybind::Resolver> {
    match phux_config::keybind::Resolver::new(kb) {
        Ok(r) => Some(r),
        Err(err) => {
            tracing::warn!(error = %err, "keybind resolver build failed; disabled");
            None
        }
    }
}

/// phux-foz.2: (dis)arm the which-key popup deadline for one loop pass.
///
/// Arms (`Some(now + delay)`) only while ALL of: the resolver is pending
/// exactly at the prefix, the popup is enabled in config, and no overlay
/// is already active (a modal owns the screen; and once the popup itself
/// is up, re-arming would re-push it forever). Re-invocations while armed
/// keep the ORIGINAL deadline (anchored, like `esc_deadline`) so other
/// select! arms firing cannot postpone the popup. Any pass that sees the
/// conditions no longer met — e.g. an early continuation chord resolved
/// the prefix — disarms, which is how a fast chord suppresses the popup.
fn update_which_key_deadline(
    deadline: &mut Option<tokio::time::Instant>,
    pending_at_prefix: bool,
    enabled: bool,
    overlay_active: bool,
    now: tokio::time::Instant,
    delay: Duration,
) {
    if enabled && pending_at_prefix && !overlay_active {
        deadline.get_or_insert(now + delay);
    } else {
        *deadline = None;
    }
}

/// phux-foz.2: push the which-key popup when the timeout fires.
///
/// Re-checks the arming conditions against the CURRENT state (the select!
/// arm may race a same-iteration resolver mutation) and pushes a
/// [`WhichKeyOverlay`] built from the same keybindings snapshot the help
/// overlay uses. Returns `true` iff the popup was pushed (the caller then
/// paints the overlay layer). Never touches the resolver: the pending
/// prefix must stay live so the next chord still completes normally.
fn push_which_key_overlay(
    overlays: &mut OverlayState,
    resolver: Option<&phux_config::keybind::Resolver>,
    keybindings: Option<&phux_config::KeybindingsCfg>,
    theme: &crate::render::Theme,
) -> bool {
    if overlays.is_active() {
        return false;
    }
    if !resolver.is_some_and(phux_config::keybind::Resolver::pending_at_prefix) {
        return false;
    }
    let Some(kb) = keybindings else {
        return false;
    };
    tracing::debug!("which-key: prefix hesitation timeout; showing popup");
    overlays.push(Box::new(
        crate::render::overlay::WhichKeyOverlay::from_config(kb, theme),
    ));
    true
}

/// phux-ahv.3: snapshot the current [`Workspace`] as the `windows`
/// widget's input — display order with the active window flagged. The
/// `windows` status-bar widget formats and styles these.
/// Snapshot the window/tab strip, preferring each window's live OSC
/// title over its stored name.
///
/// A window's display label prefers, in order (ADR-0040):
///
/// 1. **The structured `phux.agent/v1` record** of the window's focused
///    leaf, when one is declared — [`AgentRecord::label`], e.g.
///    `reviewer (blocked)`. No title parsing, no substring heuristics.
/// 2. **The OSC 0/2 title** of the focused leaf — the title the running
///    program set (a shell shows the cwd/command, `vim` the file, an agent
///    its task) — read straight from that pane's client-side libghostty
///    mirror ([`PaneSlot::terminal`]). This is the tmux "automatic-rename"
///    behaviour and Warp's tab titling, entirely client-local: titles flow
///    in the PTY VT the mirror already consumes. It stays as the
///    compatibility path for agents that only speak title conventions.
/// 3. **The window's stored `name`**, when the focused leaf has no slot
///    yet or its title is empty.
fn window_infos(
    workspace: &Workspace,
    panes: &HashMap<TerminalId, PaneSlot>,
    // phux-x2hm: the driver's pane-zoom state. The active window's tab gets a
    // `Z` marker (`WindowInfo.zoomed`) when a pane is zoomed; non-active tabs
    // never show it (zoom is per the active window).
    zoomed: Option<&TerminalId>,
    // ADR-0040: Terminal → decoded `phux.agent/v1` record, kept live by the
    // driver's per-pane metadata subscriptions.
    agent_meta: &HashMap<TerminalId, AgentRecord>,
) -> Vec<phux_config::widget::WindowInfo> {
    workspace
        .windows
        .iter()
        .enumerate()
        .map(|(i, w)| {
            let focus = w.state.focus.as_ref();
            let agent_label = focus
                .and_then(|fid| agent_meta.get(fid))
                .map(AgentRecord::label);
            let title = focus
                .and_then(|fid| panes.get(fid))
                .and_then(|slot| slot.terminal.title().ok())
                .map(str::trim)
                .filter(|t| !t.is_empty())
                .map(ToOwned::to_owned);
            let active = i == workspace.active;
            // phux-foz.1: a window carries attention when ANY of its leaves
            // has the ADR-0035 asked flag set — not just the focused leaf —
            // so a question in a background split still marks the tab.
            let attention = w
                .state
                .tree
                .as_ref()
                .map(crate::layout::leaves)
                .unwrap_or_default()
                .iter()
                .any(|id| panes.get(id).is_some_and(|slot| slot.attention));
            phux_config::widget::WindowInfo {
                name: agent_label.or(title).unwrap_or_else(|| w.name.clone()),
                active,
                zoomed: active && zoomed.is_some(),
                attention,
            }
        })
        .collect()
}

/// phux-foz.1: clear a pane's asked-attention flag because the user sent it
/// input (the clearing rule documented in `docs/consumers/tui.md`). Returns
/// `true` when the flag actually flipped, so the caller can schedule a
/// chrome repaint only on a real transition.
pub(super) fn clear_attention_on_input(
    panes: &mut HashMap<TerminalId, PaneSlot>,
    pane: &TerminalId,
) -> bool {
    match panes.get_mut(pane) {
        Some(slot) if slot.attention => {
            slot.attention = false;
            true
        }
        _ => false,
    }
}

/// ADR-0040 (phux-3ert): reconcile the agent-metadata index with the live
/// pane set.
///
/// For every pane that has no live `phux.agent/v1` watch yet, send a
/// one-shot `GET_METADATA` (the read-back for a record set before we
/// attached; the reply is correlated through `AgentMetaIndex::pending`) plus
/// a `SUBSCRIBE_METADATA` (the push path for later `SET`/`DELETE`
/// broadcasts). Panes that closed are pruned from every side table — the
/// server already dropped their per-Terminal store and our subscription
/// with the Terminal, so pruning is purely local hygiene. Idempotent: a
/// pane already in `subscribed` is skipped, so callers can re-run the sweep
/// on every pane-set change (bootstrap, split, new window, layout
/// broadcast) without duplicate wire traffic.
async fn sync_agent_meta_subscriptions(
    conn: &mut Connection,
    // Owned id list (not `&HashMap<_, PaneSlot>`): `PaneSlot` holds a
    // libghostty mirror that is not `Send`, and holding a reference to it
    // across the sends would make this future `!Send` (clippy
    // `future_not_send`). Callers pass `panes.keys().cloned().collect()`.
    pane_ids: Vec<TerminalId>,
    agent_meta: &mut AgentMetaIndex,
    next_request_id: &mut u32,
) -> Result<(), AttachError> {
    agent_meta.subscribed.retain(|id| pane_ids.contains(id));
    agent_meta.records.retain(|id, _| pane_ids.contains(id));
    agent_meta.pending.retain(|_, id| pane_ids.contains(id));
    for id in &pane_ids {
        if agent_meta.subscribed.contains(id) {
            continue;
        }
        let request_id = *next_request_id;
        *next_request_id = next_request_id.wrapping_add(1);
        agent_meta.pending.insert(request_id, id.clone());
        conn.send(&FrameKind::GetMetadata {
            request_id,
            scope: Scope::Terminal(id.clone()),
            key: TERMINAL_AGENT_KEY.to_owned(),
        })
        .await?;
        conn.send(&FrameKind::SubscribeMetadata {
            scope: Scope::Terminal(id.clone()),
            key: TERMINAL_AGENT_KEY.to_owned(),
        })
        .await?;
        agent_meta.subscribed.insert(id.clone());
    }
    Ok(())
}

/// phux-x2hm: the per-leaf rect map of the **zoom-honoring** view, used as the
/// pre-toggle snapshot for the reflow handshake. Returns an empty map when
/// there is no active window or its tree is unseeded (single-pane bootstrap).
fn zoom_rects(
    workspace: &Workspace,
    zoomed: Option<&TerminalId>,
    content: crate::layout::Rect,
    viewport_dims: (u16, u16),
) -> HashMap<TerminalId, crate::layout::Rect> {
    workspace
        .render_window(zoomed)
        .and_then(|ls| {
            ls.tree.as_ref().map(|_| {
                super::multi_pane::compute_layout_in(ls.as_ref(), content, viewport_dims).rects
            })
        })
        .unwrap_or_default()
}

/// phux-x2hm: on a pane-zoom toggle, emit one `TERMINAL_RESIZE` per pane whose
/// dimensions changed between the pre-toggle view (`prev_rects`) and the new
/// `zoomed` view. Zooming grows the focused pane to the whole window; un-zooming
/// shrinks every pane back to its tile. Reuses the close/SIGWINCH reflow path so
/// each PTY's winsize (TIOCSWINSZ) tracks the on-screen geometry. Sent before
/// the repaint, mirroring the other reflow sites.
async fn emit_zoom_reflow(
    conn: &mut Connection,
    workspace: &Workspace,
    zoomed: Option<&TerminalId>,
    prev_rects: &HashMap<TerminalId, crate::layout::Rect>,
    content: crate::layout::Rect,
) -> Result<(), AttachError> {
    let Some(ls) = workspace.render_window(zoomed) else {
        return Ok(());
    };
    if ls.tree.is_none() {
        return Ok(());
    }
    let diff = super::reflow::compute_reflow(ls.as_ref(), prev_rects, content);
    for (terminal_id, new_rect) in &diff.changed {
        conn.send(&FrameKind::TerminalResize {
            terminal_id: terminal_id.clone(),
            cols: new_rect.w,
            rows: new_rect.h,
        })
        .await?;
    }
    Ok(())
}

/// phux-nz4.5 / phux-9vf: load the on-disk config and build a
/// [`StatusBarPainter`] from `[status]`.
///
/// A malformed config never blocks attach — but it no longer vanishes
/// silently either. On a load or build failure we surface a visible
/// error line (`StatusBarPainter::error_line`) on the bar row pointing
/// the user at `phux config show` for the full diagnostic, instead of
/// dropping to an empty bar (and, alongside [`build_resolver_from`], no
/// keybindings) with only a `tracing::warn` nobody sees. Returns `None`
/// only when the config is valid and the bar would be empty (no widgets
/// configured) — callers short-circuit on that.
fn build_status_bar_painter() -> Option<StatusBarPainter> {
    let cfg = match phux_config::loader::load() {
        Ok(c) => c,
        Err(err) => {
            tracing::warn!(error = %err, "phux-config load failed; surfacing on status bar");
            return Some(StatusBarPainter::error_line(config_error_line(&err)));
        }
    };
    let registry = phux_config::WidgetRegistry::with_builtins();
    // phux-r82.6: fold enabled plugins' `[[widgets]]` contributions in
    // after the user's own `[status]` widgets. Invalid contributions are
    // dropped with a warning inside the merge (mirroring the plugin
    // keybinding policy), so a broken plugin cannot flip the bar into the
    // error strip; a genuinely broken USER config still can, below.
    let mut status = cfg.status.clone();
    if !cfg.plugins.is_empty() {
        let config_path = phux_config::loader::config_path();
        let manifests = phux_config::plugin::load_enabled_manifests(&config_path, &cfg.plugins);
        phux_config::plugin::merge_widget_contributions(&mut status, &manifests, &registry);
    }
    match phux_config::widget::StatusBar::build(&status, &registry) {
        Ok(bar) if bar.is_empty() => None,
        Ok(bar) => {
            let mut painter = StatusBarPainter::new(bar, Position::default());
            painter.set_prefix(cfg.keybindings.prefix);
            Some(painter)
        }
        Err(err) => {
            tracing::warn!(error = %err, "status-bar build failed; surfacing on status bar");
            Some(StatusBarPainter::error_line(config_error_line(&err)))
        }
    }
}

/// phux-9vf: format a one-line, on-screen config error for the status
/// bar. Mirrors what `phux config show` prints to stderr (the
/// `Display` of the error) and appends the actionable next step.
fn config_error_line(err: &impl std::fmt::Display) -> String {
    format!("config error: {err} (run: phux config show)")
}

/// Build a `VIEWPORT_RESIZE` frame from a [`ViewportInfo`].
///
/// Pure function, factored out of [`main_loop`] so unit tests can
/// exercise the encoder-feeding side without firing a real SIGWINCH or
/// driving a tokio runtime. The wire shape matches SPEC §7.1 / §10.5.
const fn viewport_resize_frame(viewport: ViewportInfo) -> FrameKind {
    FrameKind::ViewportResize { viewport }
}

/// Read the current viewport, falling back to 80x24 with a logged
/// warning if the kernel query fails. Used by the SIGWINCH branch
/// where we'd rather ship a stale-but-plausible viewport than skip
/// the upstream notification entirely.
fn current_viewport_or_default() -> ViewportInfo {
    match current_viewport() {
        Ok(v) => v,
        Err(err) => {
            tracing::warn!(error = %err, "tcgetwinsize failed; falling back to 80x24");
            ViewportInfo::new(80, 24)
        }
    }
}

/// Read the controlling-TTY size via `tcgetwinsize` and return the
/// matching [`ViewportInfo`]. Pixel dimensions are reported when the
/// kernel provides them.
fn current_viewport() -> Result<ViewportInfo, AttachError> {
    let stdout = io::stdout();
    if !stdout.is_terminal() {
        // Fall back to a sane default if stdout isn't a TTY (rare for the
        // attach path; the early TTY check should have caught this).
        return Ok(ViewportInfo::new(80, 24));
    }
    let size = rustix::termios::tcgetwinsize(stdout.as_fd())
        .map_err(|err| AttachError::Terminal(format!("tcgetwinsize: {err}")))?;
    let pixel_w = if size.ws_xpixel == 0 {
        None
    } else {
        Some(size.ws_xpixel)
    };
    let pixel_h = if size.ws_ypixel == 0 {
        None
    } else {
        Some(size.ws_ypixel)
    };
    Ok(ViewportInfo::new(size.ws_col, size.ws_row).with_pixels(pixel_w, pixel_h))
}

/// RAII handle that flips stdin into raw mode and stdout into the alt
/// screen on construction, and restores both on drop.
///
/// Restoration runs in `Drop`, so a panic anywhere in the attach loop —
/// including the renderer or the connection — leaves the user's outer
/// terminal in a usable state.
pub struct RawModeGuard {
    original_termios: Termios,
}

impl std::fmt::Debug for RawModeGuard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RawModeGuard").finish_non_exhaustive()
    }
}

impl RawModeGuard {
    /// Install the guard, writing the alt-screen-enter + cursor-hide
    /// sequence to real stdout. Convenience wrapper around
    /// [`Self::install_with_stdout`] for the common path; tests use
    /// the writer-injecting variant. Enables mouse capture by default
    /// (ADR-0035).
    pub fn install() -> Result<Self, AttachError> {
        Self::install_with_stdout(&mut io::stdout(), true)
    }

    /// Install the guard. Errors if stdin is not a TTY or the termios
    /// dance fails. The alt-screen + cursor-hide bytes are written to
    /// `out` so tests can capture them and assert on the regression
    /// guard for `phux-roz`.
    ///
    /// `mouse` gates the client's own outer-terminal mouse tracking
    /// (ADR-0035): when `true` the entry sequence also emits DECSET
    /// `?1002h?1006h` so divider drags work without an inner program
    /// turning mouse mode on; when `false` the client emits no mouse DECSET
    /// and only sees mouse when an inner program enables tracking (the host's
    /// native selection is untouched).
    pub fn install_with_stdout<W: Write>(out: &mut W, mouse: bool) -> Result<Self, AttachError> {
        let stdin = io::stdin();
        if !stdin.is_terminal() {
            return Err(AttachError::NotATty);
        }
        let fd = stdin.as_fd();
        let original = rustix::termios::tcgetattr(fd)
            .map_err(|err| AttachError::Terminal(format!("tcgetattr: {err}")))?;
        let mut raw = original.clone();
        raw.input_modes.remove(
            rustix::termios::InputModes::IGNBRK
                | rustix::termios::InputModes::BRKINT
                | rustix::termios::InputModes::PARMRK
                | rustix::termios::InputModes::ISTRIP
                | rustix::termios::InputModes::INLCR
                | rustix::termios::InputModes::IGNCR
                | rustix::termios::InputModes::ICRNL
                | rustix::termios::InputModes::IXON,
        );
        raw.output_modes.remove(rustix::termios::OutputModes::OPOST);
        raw.local_modes.remove(
            LocalModes::ECHO
                | LocalModes::ECHONL
                | LocalModes::ICANON
                | LocalModes::ISIG
                | LocalModes::IEXTEN,
        );
        raw.control_modes
            .remove(rustix::termios::ControlModes::CSIZE | rustix::termios::ControlModes::PARENB);
        raw.control_modes.insert(rustix::termios::ControlModes::CS8);

        // Make `read` block until at least one byte is available, with
        // no timeout. Tokio's stdin uses a blocking helper thread, so
        // this matches its expectations.
        raw.special_codes[rustix::termios::SpecialCodeIndex::VMIN] = 1;
        raw.special_codes[rustix::termios::SpecialCodeIndex::VTIME] = 0;

        rustix::termios::tcsetattr(fd, OptionalActions::Now, &raw)
            .map_err(|err| AttachError::Terminal(format!("tcsetattr: {err}")))?;

        // Enter the alt screen + hide the cursor up front so the first
        // frame paint doesn't briefly show on the normal screen. With
        // `mouse` on, also enable our own outer-terminal mouse tracking so
        // divider drags work by default (ADR-0035).
        write_enter_alt_screen(out, mouse).map_err(AttachError::Io)?;

        // Remember that we entered the alt screen so signal handlers
        // know to emit the leave sequence. We deliberately set this
        // AFTER the writes succeed so a half-completed entry doesn't
        // confuse cleanup.
        ALT_SCREEN_ACTIVE.store(true, Ordering::SeqCst);

        // Park a clone of the original Termios in process-global storage
        // so the signal-handler arms and the panic hook (which can't
        // reach the instance field) can perform a true restore rather
        // than a best-effort re-cook. The instance field remains the
        // Drop-path source of truth; the global is a snapshot.
        save_termios_snapshot(original.clone());

        Ok(Self {
            original_termios: original,
        })
    }
}

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        // Best-effort restore. We deliberately swallow errors — the
        // process is on its way out and a panic in Drop is worse than
        // a slightly-wedged terminal.
        //
        // Clear the global snapshot before restoring from the instance
        // field. Either source restores the same Termios (the global is
        // a clone of `original_termios`); the clear prevents a later
        // install from inheriting a stale snapshot if the next
        // `install_with_stdout` errors out before reaching the save.
        let _ = take_termios_snapshot();
        let stdin = io::stdin();
        let _ =
            rustix::termios::tcsetattr(stdin.as_fd(), OptionalActions::Now, &self.original_termios);
        let mut out = io::stdout().lock();
        let _ = write_terminal_reset(&mut out);
        ALT_SCREEN_ACTIVE.store(false, Ordering::SeqCst);
    }
}

/// Whether the alt-screen / cursor-hide sequence is currently active.
///
/// Set inside [`RawModeGuard::install_with_stdout`] after the entry
/// sequence has been emitted, cleared by [`RawModeGuard::drop`] and the
/// signal-handler cleanup. The signal path consults this so SIGINT
/// during the pre-handshake stage (no alt-screen, no raw mode) does NOT
/// emit a spurious leave sequence that the cooked terminal would print
/// as garbage.
///
/// Kept deliberately separate from [`SAVED_TERMIOS`]: alt-screen ENTER
/// and the termios flip happen at different points in
/// [`RawModeGuard::install_with_stdout`] (termios first, then alt
/// screen). Tying the two together via a single state variable would
/// couple two independent concerns and risks leaving the alt screen
/// when we should restore termios (or vice versa) on a half-failed
/// install. Two cheap flags is the right factoring.
static ALT_SCREEN_ACTIVE: AtomicBool = AtomicBool::new(false);

/// Whether the client enabled its OWN outer-terminal mouse tracking
/// (DECSET `?1002h` button-motion + `?1006h` SGR) on attach (ADR-0035).
///
/// Set by [`write_enter_alt_screen`] when the `mouse` config is on, so the
/// client receives pointer reports over a divider even when the inner
/// program has no mouse mode (the common shell case) — that is what makes
/// drag-to-resize work by default. Cleared by [`write_terminal_reset`],
/// which emits the matching `?1006l?1002l` BEFORE the `?1049l` alt-screen
/// leave so the host terminal's native click-drag selection comes back on
/// detach. Kept separate from [`ALT_SCREEN_ACTIVE`] for the same reason
/// that flag is separate from the termios snapshot: independent concerns,
/// each restored exactly once.
static MOUSE_CAPTURE_ACTIVE: AtomicBool = AtomicBool::new(false);

/// Snapshot of the outer terminal's pre-raw Termios, parked here so
/// the signal-handler arms in `main_loop` and the panic hook installed
/// by [`install_panic_hook_once`] can perform a true `tcsetattr`
/// restore — rather than a best-effort "force ICANON|ECHO|ISIG re-cook"
/// — when [`RawModeGuard::drop`] is unreachable (process exits via
/// `std::process::exit`, which skips Drop).
///
/// Signal-safety: the signal arms in `main_loop` are tokio
/// `signal::unix::Signal::recv()` futures, which deliver on the tokio
/// runtime thread — NOT inside a POSIX async-signal-handler context.
/// The panic hook runs on the panicking thread after unwind has begun,
/// also normal Rust context. So acquiring this `Mutex` is safe in both
/// callers; we are explicitly NOT in a context that would deadlock on
/// re-entrant lock acquisition.
///
/// Written by [`RawModeGuard::install_with_stdout`] (clone of the
/// instance's `original_termios`) and cleared by [`RawModeGuard::drop`]
/// and the signal-restore path. The instance field on `RawModeGuard`
/// remains the Drop-path source of truth; this global is a snapshot
/// for the paths that can't reach the instance.
static SAVED_TERMIOS: Mutex<Option<Termios>> = Mutex::new(None);

/// Park a Termios snapshot in [`SAVED_TERMIOS`]. Errors on lock
/// poisoning are swallowed: a poisoned lock means another thread
/// panicked while holding it, in which case we still want subsequent
/// installs to succeed and the most we lose is the signal-arm's true
/// restore (fall-back path covers it).
fn save_termios_snapshot(t: Termios) {
    if let Ok(mut slot) = SAVED_TERMIOS.lock() {
        *slot = Some(t);
    }
}

/// Take the Termios snapshot out of [`SAVED_TERMIOS`], leaving `None`.
/// Returns `None` if the lock is poisoned (signal-arm falls back to
/// the re-cook path; Drop falls back to the instance field).
fn take_termios_snapshot() -> Option<Termios> {
    SAVED_TERMIOS.lock().ok().and_then(|mut slot| slot.take())
}

/// Whether [`install_panic_hook_once`] has already run. The panic hook
/// is global to the process; we don't want a re-entrant install to
/// chain hooks indefinitely.
static PANIC_HOOK_INSTALLED: AtomicBool = AtomicBool::new(false);

/// Write the alt-screen-enter + cursor-hide sequence, plus — when
/// `mouse` is on — the client's own mouse-tracking DECSET (ADR-0035).
/// Factored out so the install path and any future re-entry path share
/// one byte definition.
///
/// `?1002h` is button-event tracking (motion only while a button is held,
/// not `?1003h` any-motion which would flood the wire with hover traffic
/// we discard); `?1006h` is SGR extended coordinates, mandatory to address
/// columns past 223. Records [`MOUSE_CAPTURE_ACTIVE`] so the matching
/// reset emits the leave sequence.
fn write_enter_alt_screen<W: Write>(out: &mut W, mouse: bool) -> io::Result<()> {
    out.write_all(b"\x1b[?1049h")?;
    out.write_all(b"\x1b[?25l")?;
    if mouse {
        out.write_all(b"\x1b[?1002h\x1b[?1006h")?;
        MOUSE_CAPTURE_ACTIVE.store(true, Ordering::SeqCst);
    }
    out.flush()
}

/// Restore the outer terminal to a sane post-attach state: drop SGR,
/// show the cursor, and (if we ever entered the alt screen) leave it.
///
/// Used by both [`RawModeGuard::drop`] and the signal-handler arms in
/// the private `main_loop` function. Safe to call multiple times — the
/// second call sees
/// `ALT_SCREEN_ACTIVE == false` and skips the leave sequence.
pub fn write_terminal_reset<W: Write>(out: &mut W) -> io::Result<()> {
    write_reset(out)?;
    // ADR-0035: drop our own mouse tracking BEFORE leaving the alt screen,
    // so the host terminal's native click-drag selection is restored on
    // detach. `?1006l` then `?1002l` undoes the entry pair in reverse.
    if MOUSE_CAPTURE_ACTIVE.swap(false, Ordering::SeqCst) {
        out.write_all(b"\x1b[?1006l\x1b[?1002l")?;
        out.flush()?;
    }
    if ALT_SCREEN_ACTIVE.swap(false, Ordering::SeqCst) {
        out.write_all(b"\x1b[?1049l")?;
        out.flush()?;
    }
    Ok(())
}

/// Best-effort terminal reset from inside a signal handler arm. This
/// is the SIGINT/SIGTERM/SIGHUP path: termios goes back to the saved
/// state (recovered from [`SAVED_TERMIOS`] when populated; otherwise a
/// re-cook fall-back), and the alt-screen sequence is left if we
/// entered one. Errors are swallowed — the process is on its way out.
///
/// Behaviour change for phux-2r7 (was best-effort re-cook only,
/// committed in 63dc6ff): when [`RawModeGuard`] has parked a snapshot,
/// we now do a true `tcsetattr` restore to the user's pre-attach
/// flags, preserving customisations like IUTF8 / VEOF that the re-cook
/// would clobber. The manual SIGINT-during-attach repro that motivated
/// the original fix still passes; verifying the precise-restore
/// behaviour requires a live PTY and is not unit-testable from here.
fn terminal_reset_on_signal() {
    let stdin = io::stdin();
    let fd = stdin.as_fd();
    if let Some(saved) = take_termios_snapshot() {
        // True restore: the snapshot is exactly what `tcgetattr`
        // returned before we flipped into raw mode.
        let _ = rustix::termios::tcsetattr(fd, OptionalActions::Now, &saved);
    } else if let Ok(mut termios) = rustix::termios::tcgetattr(fd) {
        // Fall-back re-cook for the (rare) case where the snapshot is
        // missing — e.g. signal fired before `install_with_stdout`
        // reached the save, or the lock was poisoned. We force the
        // canonical-mode flags back on so the cooked shell at least
        // shows what the user types; non-default flags are NOT
        // preserved on this path and the user may want to run `reset`
        // after.
        termios.local_modes.insert(
            LocalModes::ECHO
                | LocalModes::ECHONL
                | LocalModes::ICANON
                | LocalModes::ISIG
                | LocalModes::IEXTEN,
        );
        termios.input_modes.insert(
            rustix::termios::InputModes::BRKINT
                | rustix::termios::InputModes::ICRNL
                | rustix::termios::InputModes::IXON,
        );
        termios
            .output_modes
            .insert(rustix::termios::OutputModes::OPOST);
        let _ = rustix::termios::tcsetattr(fd, OptionalActions::Now, &termios);
    }
    let mut out = io::stdout().lock();
    let _ = write_terminal_reset(&mut out);
}

/// Clean client exit after a server-acknowledged DETACH (or a
/// detach-intended disconnect). Restores the terminal and exits the
/// process immediately rather than returning up the stack.
///
/// Why not just `return Ok(())` and let `RawModeGuard::drop` + the
/// runtime teardown clean up? Because `tokio::io::stdin()` parks an
/// **uncancellable** blocking `read()` on a helper thread. The terminal
/// restore (guard Drop) does run, but the subsequent runtime drop then
/// blocks forever waiting for that stuck read to return. The result is
/// a zombie client that never exits, keeps a reader on the shared PTY,
/// and steals the first line the user types next — most painfully their
/// reattach command, which is why reattach "did nothing." Exiting here
/// closes that window: the restore mirrors the signal path, and
/// `process::exit` skips the teardown that would otherwise hang.
#[allow(
    clippy::exit,
    reason = "detach must exit now; runtime drop hangs on the stdin read thread"
)]
fn exit_after_detach() -> ! {
    terminal_reset_on_signal();
    std::process::exit(0);
}

/// Install a global panic hook that first records the panic to the
/// `tracing` file sink, then runs [`write_terminal_reset`], then chains
/// the previous (default) hook. Idempotent — repeated calls after the
/// first are no-ops.
///
/// Ordering matters and is deliberate:
///
/// 1. **Log first.** The client's `tracing` subscriber writes to a file
///    (never stderr — the alt screen is up), so we emit the panic message
///    plus a captured [`std::backtrace::Backtrace`] there BEFORE touching
///    the terminal. This is the durable record: even though the next step
///    restores the cooked terminal and the default hook's stderr backtrace
///    lands on a screen the user may not be watching, the crash is fully
///    recoverable from the log file.
/// 2. **Restore the terminal.** Without this, a panic deep inside the
///    renderer or libghostty would unwind through `main_loop` and the
///    default hook would print into the alt screen we're about to leave —
///    so the user would see nothing.
/// 3. **Chain the previous hook** (the default backtrace printer).
fn install_panic_hook_once() {
    if PANIC_HOOK_INSTALLED.swap(true, Ordering::SeqCst) {
        return;
    }
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        // (1) Durable capture to the file sink before the terminal is
        // touched. `Backtrace::capture` honors `RUST_BACKTRACE`: it is
        // `Disabled` (rendered as a hint) unless the env var is set, so
        // there's no symbolication cost in the common case while a full
        // trace is available when the operator asks for one.
        let backtrace = std::backtrace::Backtrace::capture();
        let location = info
            .location()
            .map_or_else(|| "<unknown>".to_owned(), ToString::to_string);
        tracing::error!(
            panic.location = %location,
            panic.message = %info,
            panic.backtrace = %backtrace,
            "client panic",
        );
        // (2) Restore the outer terminal so the chained hook's output
        // doesn't vanish into the dead alt screen.
        terminal_reset_on_signal();
        // (3) Default hook: prints the panic + backtrace to stderr.
        previous(info);
    }));
}

#[cfg(test)]
#[allow(clippy::expect_used, reason = "tests")]
mod tests {
    use super::*;
    use phux_protocol::caps::{ServerCapabilities, TerminalColor, TerminalDefaultColors};
    use tokio::net::UnixStream;

    static TERMINAL_RESET_TEST_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn pane_slot_initializes_nonzero_cell_pixels_for_live_kitty_render() {
        let mut slot = PaneSlot::new_with_size(10, 5).expect("slot");
        slot.terminal
            .vt_write(b"\x1b_Ga=T,f=32,s=1,v=1,i=77,q=2;/wAA/w==\x1b\\");

        let mut out = Vec::new();
        slot.renderer
            .render(&slot.terminal, &mut out)
            .expect("render");
        let replay = String::from_utf8_lossy(&out);
        assert!(
            replay.contains("\x1b_Ga=T,f=32,s=1,v=1,i=77,q=2,c=1,r=1,m=0;/wAA/w==\x1b\\"),
            "initial live render must replay classic Kitty placement; got {replay:?}"
        );
    }

    #[test]
    fn supervisory_badge_formats_every_state() {
        // ADR-0033: the focused-pane supervisory badge. Running + un-leased
        // shows nothing; frozen and lease-holder render distinct chips, and the
        // holder is "you" only when it matches this client's own id.
        let me = ClientId::new(7);
        let other = ClientId::new(9);
        assert_eq!(format_supervisory_badge(false, None, Some(me)), None);
        assert_eq!(
            format_supervisory_badge(true, None, Some(me)).as_deref(),
            Some("[ FROZEN ]")
        );
        assert_eq!(
            format_supervisory_badge(false, Some(me), Some(me)).as_deref(),
            Some("[ WHEEL:you ]")
        );
        assert_eq!(
            format_supervisory_badge(false, Some(other), Some(me)).as_deref(),
            Some("[ WHEEL:c9 ]")
        );
        assert_eq!(
            format_supervisory_badge(true, Some(other), Some(me)).as_deref(),
            Some("[ FROZEN WHEEL:c9 ]")
        );
        // No own id yet (pre-ATTACHED): a holder still renders by id, never "you".
        assert_eq!(
            format_supervisory_badge(false, Some(me), None).as_deref(),
            Some("[ WHEEL:c7 ]")
        );
    }

    /// phux-foz.1: the status-bar attention hint. Nothing asking shows
    /// nothing; one asking pane shows the plain chip; several asking panes
    /// carry the count.
    #[test]
    fn attention_hint_formats_every_count() {
        assert_eq!(format_attention_hint(0), None);
        assert_eq!(format_attention_hint(1).as_deref(), Some("[ ASK ]"));
        assert_eq!(format_attention_hint(3).as_deref(), Some("[ ASK x3 ]"));
    }

    /// phux-foz.1: key/paste input forwarded to a pane clears its asked
    /// flag exactly once — the transition reports `true`, repeats and
    /// unknown panes report `false` (no spurious chrome repaints).
    #[test]
    fn clear_attention_on_input_clears_once() {
        let id = TerminalId::local(1);
        let mut panes: HashMap<TerminalId, PaneSlot> = HashMap::new();
        let mut slot = PaneSlot::new_with_size(80, 24).expect("slot");
        slot.attention = true;
        panes.insert(id.clone(), slot);

        assert!(clear_attention_on_input(&mut panes, &id), "first clear");
        assert!(
            !panes.get(&id).expect("slot").attention,
            "flag must be down after the clear"
        );
        assert!(
            !clear_attention_on_input(&mut panes, &id),
            "already-clear pane reports no transition"
        );
        assert!(
            !clear_attention_on_input(&mut panes, &TerminalId::local(9)),
            "unknown pane reports no transition"
        );
    }

    /// phux-foz.1: `window_infos` marks a window when ANY of its leaves has
    /// the asked flag — including a non-focused leaf — and only that window.
    #[test]
    fn window_infos_flags_attention_on_the_asking_window() {
        let front = TerminalId::local(1);
        let back = TerminalId::local(2);
        let mut workspace = Workspace::single(front.clone());
        workspace.add_window("2".to_owned(), back.clone());
        workspace.select(0);
        let mut panes: HashMap<TerminalId, PaneSlot> = HashMap::new();
        panes.insert(front, PaneSlot::new_with_size(80, 24).expect("slot"));
        let mut asking = PaneSlot::new_with_size(80, 24).expect("slot");
        asking.attention = true;
        panes.insert(back.clone(), asking);

        let infos = window_infos(&workspace, &panes, None, &HashMap::new());
        assert!(
            !infos[0].attention,
            "quiet window must not carry the marker"
        );
        assert!(
            infos[1].attention,
            "the asking (background) window carries the marker"
        );

        // Clearing the flag clears the marker.
        assert!(clear_attention_on_input(&mut panes, &back));
        let infos = window_infos(&workspace, &panes, None, &HashMap::new());
        assert!(!infos[1].attention);
    }

    #[test]
    fn attach_error_io_display_includes_source() {
        let err = AttachError::Io(io::Error::other("boom"));
        let msg = err.to_string();
        assert!(msg.contains("attach loop io error"));
    }

    // -- which-key popup arming (phux-foz.2) ------------------------------

    /// Build a resolver from the shipped defaults and walk it to the
    /// pending-prefix state (`C-a` fed, continuation awaited).
    fn pending_resolver() -> phux_config::keybind::Resolver {
        let cfg =
            phux_config::parse_str(phux_config::DEFAULT_CONFIG_TOML, Path::new("default.toml"))
                .expect("default config parses");
        let mut r = phux_config::keybind::Resolver::new(&cfg.keybindings).expect("resolver builds");
        let prefix = phux_config::keybind::parse_chord(&cfg.keybindings.prefix).expect("prefix");
        assert_eq!(r.feed(prefix), phux_config::keybind::Feed::Partial);
        assert!(r.pending_at_prefix());
        r
    }

    #[test]
    fn which_key_deadline_arms_once_and_holds_its_anchor() {
        let mut deadline = None;
        let now = tokio::time::Instant::now();
        let delay = Duration::from_millis(600);
        update_which_key_deadline(&mut deadline, true, true, false, now, delay);
        assert_eq!(deadline, Some(now + delay), "arms at now + delay");
        // A later pass (other select! arms fired) keeps the ORIGINAL
        // anchor — the popup is not postponed by unrelated wakeups.
        update_which_key_deadline(
            &mut deadline,
            true,
            true,
            false,
            now + Duration::from_millis(300),
            delay,
        );
        assert_eq!(deadline, Some(now + delay), "anchor survives re-passes");
    }

    #[test]
    fn which_key_deadline_disarms_when_an_early_chord_resolves() {
        // The suppression path: prefix pressed (armed), then a fast
        // continuation resolves the chord BEFORE the timeout — the next
        // loop pass sees pending=false and must disarm, so the popup
        // never appears.
        let mut deadline = None;
        let now = tokio::time::Instant::now();
        let delay = Duration::from_millis(600);
        update_which_key_deadline(&mut deadline, true, true, false, now, delay);
        assert!(deadline.is_some());
        update_which_key_deadline(&mut deadline, false, true, false, now, delay);
        assert_eq!(deadline, None, "early chord suppresses the popup");
    }

    #[test]
    fn which_key_deadline_respects_disable_and_active_overlay() {
        let mut deadline = None;
        let now = tokio::time::Instant::now();
        let delay = Duration::from_millis(600);
        // Disabled in config: never arms.
        update_which_key_deadline(&mut deadline, true, false, false, now, delay);
        assert_eq!(deadline, None);
        // A modal already up: never arms (it owns input; the resolver was
        // reset on entry anyway).
        update_which_key_deadline(&mut deadline, true, true, true, now, delay);
        assert_eq!(deadline, None);
        // Armed, then an overlay appears before the timeout: disarms.
        update_which_key_deadline(&mut deadline, true, true, false, now, delay);
        assert!(deadline.is_some());
        update_which_key_deadline(&mut deadline, true, true, true, now, delay);
        assert_eq!(deadline, None);
    }

    #[test]
    fn which_key_timeout_pushes_the_popup_and_keeps_the_prefix_pending() {
        // The timeout path: a pending-at-prefix resolver + keybindings
        // snapshot ⇒ the popup is pushed; the resolver still holds the
        // pending prefix so the NEXT chord completes normally.
        let cfg =
            phux_config::parse_str(phux_config::DEFAULT_CONFIG_TOML, Path::new("default.toml"))
                .expect("default config parses");
        let resolver = pending_resolver();
        let mut overlays = OverlayState::new();
        let theme = crate::render::Theme::default();
        let pushed = push_which_key_overlay(
            &mut overlays,
            Some(&resolver),
            Some(&cfg.keybindings),
            &theme,
        );
        assert!(pushed, "timeout must push the which-key popup");
        assert!(overlays.is_active());
        assert!(
            overlays.top_is_passthrough(),
            "the popup must be input-passthrough so it can never eat a chord"
        );
        assert!(
            resolver.pending_at_prefix(),
            "pushing the popup must not consume the pending prefix"
        );
    }

    #[test]
    fn which_key_push_declines_without_pending_prefix_or_over_a_modal() {
        let cfg =
            phux_config::parse_str(phux_config::DEFAULT_CONFIG_TOML, Path::new("default.toml"))
                .expect("default config parses");
        let theme = crate::render::Theme::default();

        // Resolver at the root (no pending prefix): no push.
        let idle = phux_config::keybind::Resolver::new(&cfg.keybindings).expect("resolver builds");
        let mut overlays = OverlayState::new();
        assert!(!push_which_key_overlay(
            &mut overlays,
            Some(&idle),
            Some(&cfg.keybindings),
            &theme,
        ));
        assert!(!overlays.is_active());

        // A modal already up: no push (would stack over user input).
        let pending = pending_resolver();
        let mut overlays = OverlayState::new();
        overlays.push(Box::new(crate::render::overlay::HelpOverlay::from_config(
            &cfg.keybindings,
            &theme,
        )));
        assert!(!push_which_key_overlay(
            &mut overlays,
            Some(&pending),
            Some(&cfg.keybindings),
            &theme,
        ));
        assert_eq!(overlays.depth(), 1, "nothing stacked on the modal");
    }

    /// phux-jy4t: the layout metadata key is per-session, so two sessions
    /// never share (and clobber) one bucket.
    #[test]
    fn layout_key_is_per_session() {
        use phux_protocol::ids::SessionId;
        let a = layout_key(SessionId::new(1));
        let b = layout_key(SessionId::new(2));
        assert_eq!(a, "phux.tui.layout/v1/1");
        assert_eq!(b, "phux.tui.layout/v1/2");
        assert_ne!(a, b, "different sessions get different keys");
        assert!(a.starts_with(LAYOUT_KEY), "still under the layout prefix");
    }

    #[test]
    fn is_layout_key_string_matches_the_family_only() {
        // Bare legacy key + any session-suffixed key are layout keys.
        assert!(is_layout_key_string(LAYOUT_KEY));
        assert!(is_layout_key_string("phux.tui.layout/v1/7"));
        // A different key that merely shares the prefix-without-separator is
        // NOT matched, and unrelated keys aren't either.
        assert!(!is_layout_key_string("phux.tui.layout/v12"));
        assert!(!is_layout_key_string("phux.tui.other/v1"));
    }

    #[test]
    fn coalesce_defers_every_pane_frame_but_its_last() {
        // phux-jhv8: in a coalesced burst, every output frame for a pane
        // defers EXCEPT that pane's final frame, which settles the screen.
        let p = |id| Some(TerminalId::Local { id });
        // Single-pane burst: only the last frame paints.
        assert_eq!(
            coalesce_defer_flags(&[p(2), p(2), p(2)]),
            vec![true, true, false]
        );
        // A lone frame never defers (preserves the one-frame-one-paint path).
        assert_eq!(coalesce_defer_flags(&[p(2)]), vec![false]);
    }

    #[test]
    fn coalesce_keys_deferral_per_pane_not_globally() {
        // Two panes interleaved: each pane's LAST frame paints, so neither is
        // left stale even when the burst ends on the other pane's output.
        let p = |id| Some(TerminalId::Local { id });
        // A(defer, later A) B(defer, later B) A(last A) B(last B)
        assert_eq!(
            coalesce_defer_flags(&[p(1), p(2), p(1), p(2)]),
            vec![true, true, false, false]
        );
        // Burst ending on a non-focused pane B must still paint A's last frame.
        assert_eq!(
            coalesce_defer_flags(&[p(1), p(1), p(2)]),
            vec![true, false, false]
        );
    }

    #[test]
    fn snapshot_never_defers_even_behind_a_later_same_pane_frame() {
        // A snapshot is authoritative full state: even when the coalesce mask
        // says "defer" (a later same-pane output exists in the burst), it must
        // still paint, or the later incremental output paints onto a screen the
        // snapshot never drew — the attach/reattach/split "mangled" bug.
        let snap = FrameKind::TerminalSnapshot {
            terminal_id: TerminalId::Local { id: 1 },
            cols: 80,
            rows: 24,
            vt_replay_bytes: Vec::new(),
            scrollback_bytes: None,
        };
        let output = FrameKind::TerminalOutput {
            terminal_id: TerminalId::Local { id: 1 },
            seq: 1,
            bytes: Vec::new().into(),
        };
        // Mask says defer, but a snapshot overrides it and paints.
        assert!(!frame_defers_paint(true, &snap));
        // An ordinary output still honors the coalesce mask.
        assert!(frame_defers_paint(true, &output));
        assert!(!frame_defers_paint(false, &output));
    }

    #[test]
    fn coalesce_control_frames_never_defer() {
        // `None` (a non-painting control frame) never defers, and never
        // counts as a later same-pane paint for the frames before it.
        let p = |id| Some(TerminalId::Local { id });
        assert_eq!(
            coalesce_defer_flags(&[p(1), None, p(1)]),
            vec![true, false, false]
        );
        assert_eq!(coalesce_defer_flags(&[None, None]), vec![false, false]);
        assert_eq!(coalesce_defer_flags(&[]), Vec::<bool>::new());
    }

    #[test]
    fn window_infos_prefers_osc_title_over_stored_name() {
        // A program in the focused leaf sets an OSC 2 window title; the tab
        // strip must show it (tmux automatic-rename / Warp tab titling).
        let id = TerminalId::local(1);
        let workspace = Workspace::single(id.clone());
        let mut panes: HashMap<TerminalId, PaneSlot> = HashMap::new();
        let mut slot = PaneSlot::new_with_size(80, 24).expect("slot");
        slot.terminal.vt_write(b"\x1b]2;~/src/phux\x07");
        panes.insert(id, slot);

        let infos = window_infos(&workspace, &panes, None, &HashMap::new());
        assert_eq!(infos.len(), 1);
        assert_eq!(
            infos[0].name, "~/src/phux",
            "the OSC title should label the tab, overriding the stored name"
        );
        assert!(infos[0].active);
    }

    #[test]
    fn window_infos_falls_back_to_stored_name_without_title() {
        // No OSC title set ⇒ the window's stored name ("1" for the first).
        let id = TerminalId::local(1);
        let workspace = Workspace::single(id.clone());
        let mut panes: HashMap<TerminalId, PaneSlot> = HashMap::new();
        panes.insert(id, PaneSlot::new_with_size(80, 24).expect("slot"));

        let infos = window_infos(&workspace, &panes, None, &HashMap::new());
        assert_eq!(infos[0].name, "1");
    }

    #[test]
    fn window_infos_ignores_a_whitespace_only_title() {
        // A title of only spaces is not a useful label; fall back to the name.
        let id = TerminalId::local(1);
        let workspace = Workspace::single(id.clone());
        let mut panes: HashMap<TerminalId, PaneSlot> = HashMap::new();
        let mut slot = PaneSlot::new_with_size(80, 24).expect("slot");
        slot.terminal.vt_write(b"\x1b]2;   \x07");
        panes.insert(id, slot);

        let infos = window_infos(&workspace, &panes, None, &HashMap::new());
        assert_eq!(infos[0].name, "1");
    }

    #[test]
    fn window_infos_prefers_agent_record_over_osc_title() {
        // ADR-0040: a declared `phux.agent/v1` record labels the window from
        // structured data — the OSC title (set here to an unrelated string)
        // must NOT leak through, and no substring parsing is involved.
        let id = TerminalId::local(1);
        let workspace = Workspace::single(id.clone());
        let mut panes: HashMap<TerminalId, PaneSlot> = HashMap::new();
        let mut slot = PaneSlot::new_with_size(80, 24).expect("slot");
        slot.terminal.vt_write(b"\x1b]2;~/src/phux\x07");
        panes.insert(id.clone(), slot);
        let mut records: HashMap<TerminalId, AgentRecord> = HashMap::new();
        records.insert(
            id,
            AgentRecord {
                name: "reviewer".to_owned(),
                state: crate::agent_meta::AgentMetaState::Blocked,
                ..AgentRecord::default()
            },
        );

        let infos = window_infos(&workspace, &panes, None, &records);
        assert_eq!(
            infos[0].name, "!reviewer (blocked)",
            "structured record must beat the OSC title"
        );
    }

    #[test]
    fn window_infos_falls_back_to_title_when_record_cleared() {
        // ADR-0040 compatibility path: no record ⇒ the OSC title labels the
        // tab exactly as before.
        let id = TerminalId::local(1);
        let workspace = Workspace::single(id.clone());
        let mut panes: HashMap<TerminalId, PaneSlot> = HashMap::new();
        let mut slot = PaneSlot::new_with_size(80, 24).expect("slot");
        slot.terminal.vt_write(b"\x1b]2;claude task\x07");
        panes.insert(id, slot);

        let infos = window_infos(&workspace, &panes, None, &HashMap::new());
        assert_eq!(infos[0].name, "claude task");
    }

    #[test]
    fn window_infos_flags_zoom_only_on_the_active_window() {
        // phux-x2hm: the active window's `zoomed` reflects the zoom state;
        // a non-active window is never marked zoomed.
        let active = TerminalId::local(1);
        let mut workspace = Workspace::single(active.clone());
        workspace.add_window("2".to_owned(), TerminalId::local(2));
        workspace.select(0); // active window is index 0
        let panes: HashMap<TerminalId, PaneSlot> = HashMap::new();

        let infos = window_infos(&workspace, &panes, Some(&active), &HashMap::new());
        assert!(infos[0].zoomed, "active window reflects the zoom state");
        assert!(!infos[1].zoomed, "a non-active window is never zoomed");

        // No zoom ⇒ no window is marked.
        let infos = window_infos(&workspace, &panes, None, &HashMap::new());
        assert!(!infos[0].zoomed && !infos[1].zoomed);
    }

    #[test]
    fn raw_consumer_does_not_emit_frame_ack() {
        let ack = Some((TerminalId::local(7), 42u64));
        assert_eq!(
            should_emit_frame_ack(false, ack),
            None,
            "raw mode must skip the ack even when the frame carries a seq"
        );
    }

    #[test]
    fn state_sync_consumer_emits_frame_ack() {
        let ack = Some((TerminalId::local(7), 42u64));
        assert_eq!(
            should_emit_frame_ack(true, ack.clone()),
            ack,
            "state-sync mode must forward the ack the server tracks"
        );
        assert_eq!(
            should_emit_frame_ack(true, None),
            None,
            "seq=0 / no-ack frames are never acked regardless of mode"
        );
    }

    #[test]
    fn attach_error_disconnected_is_distinct_from_io() {
        let a = AttachError::Disconnected;
        let b = AttachError::Io(io::Error::other("foo"));
        assert_ne!(std::mem::discriminant(&a), std::mem::discriminant(&b),);
    }
    #[tokio::test(flavor = "current_thread")]
    async fn handshake_waits_for_hello_ok() {
        let (client_stream, server_stream) = UnixStream::pair().expect("pair");
        let mut client = Connection::from_stream(client_stream);
        let mut server = Connection::from_stream(server_stream);

        let server_side = async move {
            let frame = server.recv().await.expect("server recv hello");
            assert!(
                matches!(frame, FrameKind::Hello { .. }),
                "first client frame must be HELLO"
            );
            server
                .send(&FrameKind::HelloOk {
                    protocol_major: PROTOCOL_VERSION.major,
                    protocol_minor: PROTOCOL_VERSION.minor,
                    protocol_patch: PROTOCOL_VERSION.patch,
                    server_caps: ServerCapabilities::new(),
                    server_id: Vec::new(),
                })
                .await
                .expect("server send hello_ok");
        };

        let (res, ()) = tokio::join!(handshake(&mut client, None), server_side);
        assert!(
            res.is_ok(),
            "handshake should succeed when HELLO_OK arrives"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn handshake_advertises_probed_default_colors() {
        let colors = TerminalDefaultColors {
            foreground: TerminalColor { r: 1, g: 2, b: 3 },
            background: TerminalColor { r: 4, g: 5, b: 6 },
        };
        let (client_stream, server_stream) = UnixStream::pair().expect("pair");
        let mut client = Connection::from_stream(client_stream);
        let mut server = Connection::from_stream(server_stream);

        let server_side = async move {
            let FrameKind::Hello { client_caps, .. } =
                server.recv().await.expect("server recv hello")
            else {
                panic!("first client frame must be HELLO");
            };
            assert_eq!(client_caps.default_colors, Some(colors));
            server
                .send(&FrameKind::HelloOk {
                    protocol_major: PROTOCOL_VERSION.major,
                    protocol_minor: PROTOCOL_VERSION.minor,
                    protocol_patch: PROTOCOL_VERSION.patch,
                    server_caps: ServerCapabilities::new(),
                    server_id: Vec::new(),
                })
                .await
                .expect("server send hello_ok");
        };

        let (res, ()) = tokio::join!(handshake(&mut client, Some(colors)), server_side);
        assert!(res.is_ok());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn handshake_rejects_non_hello_ok_reply() {
        let (client_stream, server_stream) = UnixStream::pair().expect("pair");
        let mut client = Connection::from_stream(client_stream);
        let mut server = Connection::from_stream(server_stream);

        let server_side = async move {
            let frame = server.recv().await.expect("server recv hello");
            assert!(
                matches!(frame, FrameKind::Hello { .. }),
                "first client frame must be HELLO"
            );
            server
                .send(&FrameKind::Detached)
                .await
                .expect("server send detached");
        };

        let (res, ()) = tokio::join!(handshake(&mut client, None), server_side);
        match res {
            Err(AttachError::Protocol(msg)) => {
                assert!(msg.contains("HELLO_OK"));
            }
            other => panic!("expected protocol error, got {other:?}"),
        }
    }

    /// The factored builder produces a `ViewportResize` frame carrying
    /// the supplied viewport unchanged. Lets us assert the encoder-
    /// feeding side of the SIGWINCH path without firing a real signal
    /// or driving a tokio runtime.
    #[test]
    fn viewport_resize_frame_carries_viewport_unchanged() {
        let vp = ViewportInfo::new(132, 50).with_pixels(Some(1320), Some(750));
        match viewport_resize_frame(vp) {
            FrameKind::ViewportResize { viewport } => {
                assert_eq!(viewport.cols, 132);
                assert_eq!(viewport.rows, 50);
                assert_eq!(viewport.pixel_w, Some(1320));
                assert_eq!(viewport.pixel_h, Some(750));
            }
            other => panic!("expected ViewportResize, got {other:?}"),
        }
    }

    /// `current_viewport_or_default` returns _something_ even when stdout
    /// isn't a TTY (cargo test path). The exact dims aren't load-bearing
    /// — what matters is that we never return an error and always have a
    /// frame to send.
    #[test]
    fn current_viewport_or_default_never_panics() {
        let vp = current_viewport_or_default();
        // Cell dims fit in u16 by construction; just exercise the path.
        let _ = (vp.cols, vp.rows);
    }

    /// Borrow a real `Termios` from `/dev/tty` so tests that need to
    /// exercise [`save_termios_snapshot`] / [`take_termios_snapshot`]
    /// can run with a plausible value. Returns `None` when the test
    /// process has no controlling TTY (e.g. some CI sandboxes); the
    /// caller skips in that case.
    fn try_borrow_real_termios() -> Option<Termios> {
        let tty = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/tty")
            .ok()?;
        rustix::termios::tcgetattr(tty.as_fd()).ok()
    }

    /// The save/take helpers behind [`SAVED_TERMIOS`] round-trip a
    /// snapshot exactly once: after a save, the next take returns
    /// `Some(_)`; subsequent takes return `None`. This is the unit
    /// surface that backs the signal-arm true-restore path
    /// (phux-2r7). The signal arm itself is exercised by a manual
    /// SIGINT during an attach session — see the comment on
    /// [`terminal_reset_on_signal`].
    ///
    /// `SAVED_TERMIOS` is a process-global; we clear at both ends to
    /// be hygienic across the in-test serial execution model.
    #[test]
    fn saved_termios_round_trip() {
        let Some(t) = try_borrow_real_termios() else {
            // No controlling TTY in this test process; nothing to
            // assert. The save/take helpers are still type-checked.
            return;
        };
        // Pre-clean: another test (or a panic) may have left state.
        let _ = take_termios_snapshot();
        assert!(take_termios_snapshot().is_none());

        save_termios_snapshot(t);
        assert!(
            take_termios_snapshot().is_some(),
            "save then take must return the snapshot"
        );
        assert!(
            take_termios_snapshot().is_none(),
            "second take must be empty"
        );
    }

    /// Documents the manual SIGINT repro that backs phux-2r7. The
    /// signal-arm path can't be unit-tested without forking and
    /// driving a real PTY; this `#[ignore]`-stub keeps the procedure
    /// next to the code and surfaces in `cargo test -- --ignored` if
    /// someone wires up an integration harness later.
    /// ADR-0035: with mouse capture on, the alt-screen entry sequence also
    /// enables the client's own outer-terminal mouse tracking
    /// (`?1002h` button-motion + `?1006h` SGR), and the reset undoes it
    /// (`?1006l?1002l`) BEFORE leaving the alt screen so the host's native
    /// selection is restored.
    #[test]
    fn mouse_capture_enable_and_disable_bytes() {
        let _guard = TERMINAL_RESET_TEST_LOCK
            .lock()
            .expect("terminal reset test lock");
        MOUSE_CAPTURE_ACTIVE.store(false, Ordering::SeqCst);
        ALT_SCREEN_ACTIVE.store(false, Ordering::SeqCst);

        let mut entry = Vec::new();
        write_enter_alt_screen(&mut entry, true).unwrap();
        assert!(
            entry.windows(8).any(|w| w == b"\x1b[?1002h"),
            "entry must enable button-motion tracking: {entry:?}"
        );
        assert!(
            entry.windows(8).any(|w| w == b"\x1b[?1006h"),
            "entry must enable SGR coordinates: {entry:?}"
        );
        // `write_enter_alt_screen` records MOUSE_CAPTURE_ACTIVE; the
        // alt-screen flag is set separately by `install_with_stdout` on a
        // real attach. Set it here so reset exercises the full leave path
        // (mouse-disable AND alt-screen-leave) the way a live detach does.
        ALT_SCREEN_ACTIVE.store(true, Ordering::SeqCst);
        // Reset emits the leave pair before the ?1049l alt-screen leave.
        let mut reset = Vec::new();
        write_terminal_reset(&mut reset).unwrap();
        let pos_1006l = reset
            .windows(8)
            .position(|w| w == b"\x1b[?1006l")
            .expect("reset must disable SGR coordinates");
        let pos_1002l = reset
            .windows(8)
            .position(|w| w == b"\x1b[?1002l")
            .expect("reset must disable button-motion");
        let pos_1049l = reset
            .windows(8)
            .position(|w| w == b"\x1b[?1049l")
            .expect("reset must leave the alt screen");
        assert!(
            pos_1006l < pos_1049l && pos_1002l < pos_1049l,
            "mouse-disable must precede the alt-screen leave: {reset:?}"
        );
    }

    /// ADR-0035: `mouse = false` skips the DECSET entirely — the entry
    /// sequence emits no mouse tracking, host native selection untouched.
    #[test]
    fn mouse_capture_disabled_emits_no_decset() {
        let _guard = TERMINAL_RESET_TEST_LOCK
            .lock()
            .expect("terminal reset test lock");
        MOUSE_CAPTURE_ACTIVE.store(false, Ordering::SeqCst);
        ALT_SCREEN_ACTIVE.store(false, Ordering::SeqCst);

        let mut entry = Vec::new();
        write_enter_alt_screen(&mut entry, false).unwrap();
        assert!(
            !entry.windows(8).any(|w| w == b"\x1b[?1002h"),
            "mouse=false must not enable tracking: {entry:?}"
        );
        assert!(
            entry.windows(8).any(|w| w == b"\x1b[?1049h"),
            "alt-screen enter still emitted: {entry:?}"
        );
        // With capture never set, reset emits no mouse-disable bytes.
        let mut reset = Vec::new();
        write_terminal_reset(&mut reset).unwrap();
        assert!(
            !reset.windows(8).any(|w| w == b"\x1b[?1002l"),
            "no capture ⇒ no mouse-disable on reset: {reset:?}"
        );
        assert!(
            !reset.windows(8).any(|w| w == b"\x1b[?1006l"),
            "no capture ⇒ no SGR mouse-disable on reset: {reset:?}"
        );
    }

    #[test]
    #[ignore = "manual: requires a live PTY and a SIGINT during attach"]
    fn signal_arm_true_restore_manual_repro() {
        // 1. `stty -a` in an outer shell; note `iutf8` / VEOF / etc.
        // 2. `phux attach <session>` — driver enters raw mode + alt
        //    screen; `RawModeGuard::install_with_stdout` parks the
        //    pre-attach Termios in `SAVED_TERMIOS`.
        // 3. In a sibling shell: `kill -INT <phux-pid>` (or hit Ctrl-C
        //    if your outer shell forwards it without phux eating it).
        // 4. `stty -a` again; ALL flags should match step (1). Before
        //    phux-2r7, only ICANON|ECHO|ISIG round-tripped and custom
        //    flags like `iutf8` were lost.
    }
}
