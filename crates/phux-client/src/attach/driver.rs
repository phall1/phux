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

use libghostty_vt::{Terminal as GhosttyTerminal, TerminalOptions};
use phux_protocol::PROTOCOL_VERSION;
use phux_protocol::caps::{ClientCapabilities, Layer, LayerSet, OutputMode, detect_color_support};
use phux_protocol::ids::{GroupId, TerminalId};
use phux_protocol::wire::frame::{AttachTarget, FrameKind, Scope, ViewportInfo};
use rustix::termios::{LocalModes, OptionalActions, Termios};
use tokio::io::AsyncReadExt;
use tokio::signal::unix::{SignalKind, signal};
use tracing::Instrument as _;

use super::actions::{PendingSplit, PendingWindow};
use super::connection::Connection;
use super::input::StdinParser;
use super::input_dispatch::{
    DispatchCtx, ReattachTarget, dispatch_input_events, encode_layout_or_log,
};
use super::paint::{paint_bar_after_pane, paint_full_frame, pane_viewport};
use super::render::{SelectionRect, TerminalRenderer, write_cup, write_reset};
use super::server_frame::handle_server_frame;
use crate::layout::Workspace;
use crate::predict::{Overlay, PredictionState, PredictiveConfig};
use crate::render::chrome::status_bar::{Position, StatusBarPainter};
use crate::render::overlay::OverlayState;

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
        Ok(Self {
            terminal: GhosttyTerminal::new(TerminalOptions {
                cols: cols.max(1),
                rows: rows.max(1),
                max_scrollback: 10_000,
            })?,
            renderer: TerminalRenderer::new()?,
        })
    }

    /// Allocate a fresh slot with a conservative placeholder size.
    /// Prefer [`Self::new_with_size`] whenever the attach snapshot,
    /// viewport, or layout already tells us the pane's real dimensions.
    pub(super) fn new() -> Result<Self, AttachError> {
        Self::new_with_size(80, 24)
    }
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
        format!(" copy-mode | {cell_count} cell(s) | arrows move | Enter copy | Esc cancel ");
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
    run_buffered(socket, target, PredictiveConfig::disabled()).await
}

/// Production attach: wrap stdout in the off-loop [`StdoutSink`](super::stdout_writer)
/// so a slow terminal never blocks the select loop (phux-fysb), then run the
/// session. Tests use the synchronous [`run_with_stdout`] seam directly.
#[allow(
    clippy::future_not_send,
    reason = "client-side libghostty Terminal is !Send; ADR-0003 binds us to current-thread"
)]
async fn run_buffered(
    socket: &Path,
    target: AttachTarget,
    predict: PredictiveConfig,
) -> Result<(), AttachError> {
    let (mut sink, writer) = super::stdout_writer::spawn_stdout_writer();
    let resync = Arc::clone(&sink.needs_resync);
    attach_session(
        socket,
        target,
        &mut sink,
        predict,
        Some(resync.as_ref()),
        Some(writer),
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
    run_buffered(socket, target, predict).await
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
    attach_session(socket, target, out, predict, None, None).await
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
    socket: &Path,
    target: AttachTarget,
    out: &mut W,
    predict: PredictiveConfig,
    resync: Option<&AtomicBool>,
    mut writer: Option<super::stdout_writer::WriterHandle>,
) -> Result<(), AttachError> {
    // STAGE 1 — pre-handshake, on the cooked outer terminal.
    //
    // We deliberately do NOT install RawModeGuard here. If anything in
    // this block fails (no server, refused, signal during connect) the
    // user's terminal stays in its original state and `Err(_)` carries
    // the actionable cause up to the CLI.
    let mut conn = Connection::connect(socket).await?;
    // Attach-handshake timing (info): HELLO -> ATTACH -> ATTACHED. The
    // span's CLOSE duration is the end-to-end attach latency a trace reader
    // wants for "why was the first paint slow." Lifecycle-rate, so info.
    let handshake_span = tracing::info_span!("attach_handshake", ?target);
    let (attached, output_mode) = async {
        let mode = handshake(&mut conn).await?;
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
    let _guard = RawModeGuard::install_with_stdout(out)?;

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
async fn handshake(conn: &mut Connection) -> Result<OutputMode, AttachError> {
    // Sniff `$COLORTERM` / `$TERM` / `$TERM_PROGRAM` per
    // `detect_color_support`. The advertised tier feeds the server's
    // per-client `downsample::rewrite_bytes` (SPEC §6.2).
    //
    // phux-4li.5: declare L3 (`Layer::L3`) so the server forwards
    // `MetadataChanged` events for the `phux.tui.layout/v1` key — the
    // reconcile-on-attach path in `main_loop` subscribes to that key
    // and re-renders multi-pane when another client mutates the layout.
    let client_caps = ClientCapabilities::new()
        .with_color_support(detect_color_support())
        .with_layers(LayerSet::with(&[Layer::L3]));
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
    // phux-x2hm: pane-zoom view state (driver-local, like focus). `Some(id)`
    // ⇒ pane `id` is zoomed to fill the window; render/reflow then run against
    // `workspace.render_window(zoomed)` (a synthetic single-leaf layout)
    // instead of the real tiled tree, which is left untouched for mutation.
    let mut zoomed: Option<TerminalId> = None;
    // phux-4li.5: keybind resolver + request-id allocator for L3 GET
    // correlation. The resolver consumes `InputEvent::Key` events
    // *before* they would be forwarded to the focused pane; a chord
    // that resolves to a layout action mutates the active window here
    // and never reaches the server's input pipe.
    let mut resolver = build_resolver();
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
    let keybindings_snapshot: Option<phux_config::KeybindingsCfg> =
        loaded_cfg.as_ref().map(|c| c.keybindings.clone());
    // phux-ahv.4: single source of truth for chrome + overlay colors,
    // owned alongside the keybindings snapshot and threaded into the
    // overlay render path via `DispatchCtx`.
    let theme: crate::render::Theme = loaded_cfg
        .as_ref()
        .map_or_else(crate::render::Theme::default, |c| {
            crate::render::Theme::from_cfg(&c.theme)
        });
    // phux-5ke.4: overlay state — initially empty. Pushed onto by the
    // `show-help` action; drained by `OverlayState::handle_key` when
    // the active overlay returns `Dismiss`. While active, key events
    // route to the overlay (no pane forwarding) and pane stdout flushes
    // are suppressed (ADR-0020 §Decision invariant 5).
    let mut overlays = OverlayState::new();
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
    // phux-eb0: set by `apply_action_effects` when the user commits a
    // `switch-session`. Checked after each input-dispatch batch; a value
    // here makes `main_loop` return `LoopExit::SwitchTo` so the outer
    // loop re-attaches to the named session on the same connection.
    let mut switch_request: Option<ReattachTarget> = None;

    // Replay the `ATTACHED` frame so the focused-pane bookkeeping in
    // `handle_server_frame` runs exactly once, in one place.
    let outcome = handle_server_frame(
        out,
        initial_attached,
        &mut panes,
        &mut workspace,
        &mut focused_pane,
        &mut zoomed,
        &mut session_name,
        status_bar.as_mut(),
        viewport_dims,
        &mut predict,
        &overlay,
        layout_get_request_id,
        &mut pending_splits,
        &mut pending_windows,
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
    // phux-4li.17: seed the window/tab strip from the bootstrap layout so
    // the first bar paint (driven by TERMINAL_SNAPSHOT) shows the window.
    if let Some(sb) = status_bar.as_mut() {
        sb.set_windows(window_infos(&workspace, &panes, zoomed.as_ref()));
    }

    loop {
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
                    pane_viewport(viewport_dims, status_bar.is_some()),
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
                        pane_viewport(viewport_dims, status_bar.is_some()),
                    )
                    .await?;
                }
                if layout_changed {
                    if let Some(sb) = status_bar.as_mut() {
                        sb.set_windows(window_infos(&workspace, &panes, zoomed.as_ref()));
                    }
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
                        let defer_paint = defer_flags[frame_idx];
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
                                    super::multi_pane::compute_layout(
                                        ls.as_ref(),
                                        pane_viewport(viewport_dims, status_bar.is_some()),
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
                            viewport_dims,
                            &mut predict,
                            &overlay,
                            layout_get_request_id,
                            &mut pending_splits,
                            &mut pending_windows,
                            overlays.is_active(),
                            defer_paint,
                        )?;
                        if outcome.exit {
                            return Ok(LoopExit::Detached);
                        }
                        // phux-4li.20: refresh the cached session graph
                        // whenever an ATTACHED snapshot lands so the
                        // session picker lists the current peer set.
                        if let Some((list, focused)) = outcome.sessions {
                            sessions = list;
                            focused_session = Some(focused);
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
                            let new_pane_dims =
                                pane_viewport(viewport_dims, status_bar.is_some());
                            let diff = super::reflow::compute_reflow(
                                ls.as_ref(),
                                prev_rects,
                                new_pane_dims,
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
                            if let Some(sb) = status_bar.as_mut() {
                                sb.set_windows(window_infos(&workspace, &panes, zoomed.as_ref()));
                            }
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
                                    &session_name,
                                );
                            }
                            // The GET reply is single-use; clear the
                            // pending request id so a stray late
                            // MetadataValue can't trample state.
                            layout_get_request_id = None;
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
                    pane_viewport(viewport_dims, status_bar.is_some()),
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
                        pane_viewport(viewport_dims, status_bar.is_some()),
                    )
                    .await?;
                }
                if layout_changed && let Some(sb) = status_bar.as_mut() {
                    sb.set_windows(window_infos(&workspace, &panes, zoomed.as_ref()));
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
                    let prev_pane_dims = pane_viewport(prev_dims, has_bar);
                    let new_pane_dims = pane_viewport(viewport_dims, has_bar);
                    let prev_rects =
                        super::multi_pane::compute_layout(ls.as_ref(), prev_pane_dims).rects;
                    let diff = super::reflow::compute_reflow(
                        ls.as_ref(),
                        &prev_rects,
                        new_pane_dims,
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
                // phux-5ke.4: SIGWINCH while overlay is up: redraw the
                // overlay at the new size instead of repainting panes
                // (which would scribble over the modal). On dismiss the
                // dispatch path triggers a full-frame repaint and the
                // user sees the resized layout.
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
                        &session_name,
                    );
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
                    let pane_dims = pane_viewport(viewport_dims, has_bar);
                    let fallback_origin = Some(
                        focused_pane
                            .as_ref()
                            .and_then(|fid| {
                                workspace.render_window(zoomed.as_ref()).and_then(|ls| {
                                    super::multi_pane::compute_layout(ls.as_ref(), pane_dims)
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

/// phux-4li.5: build a [`phux_config::keybind::Resolver`] from the
/// on-disk config. Failures log and return `None` — a malformed
/// `[keybindings]` table degrades to "no actions are bound" rather
/// than blocking attach. Detach is a normal keybinding action, so a
/// disabled resolver also disables configured detach chords.
fn build_resolver() -> Option<phux_config::keybind::Resolver> {
    let cfg = match phux_config::loader::load() {
        Ok(c) => c,
        Err(err) => {
            tracing::warn!(error = %err, "phux-config load failed; keybind resolver disabled");
            return None;
        }
    };
    match phux_config::keybind::Resolver::new(&cfg.keybindings) {
        Ok(r) => Some(r),
        Err(err) => {
            tracing::warn!(error = %err, "keybind resolver build failed; disabled");
            None
        }
    }
}

/// phux-ahv.3: snapshot the current [`Workspace`] as the `windows`
/// widget's input — display order with the active window flagged. The
/// `windows` status-bar widget formats and styles these.
/// Snapshot the window/tab strip, preferring each window's live OSC
/// title over its stored name.
///
/// A window's display label is the OSC 0/2 title of its focused leaf — the
/// title the running program set (a shell shows the cwd/command, `vim` the
/// file, an agent its task) — read straight from that pane's client-side
/// libghostty mirror ([`PaneSlot::terminal`]). This is the tmux
/// "automatic-rename" behaviour and Warp's tab titling, entirely
/// client-local: titles flow in the PTY VT the mirror already consumes, so
/// no wire frame or L3 key is involved. When the focused leaf has no slot
/// yet or its title is empty, fall back to the window's stored `name`.
fn window_infos(
    workspace: &Workspace,
    panes: &HashMap<TerminalId, PaneSlot>,
    // phux-x2hm: the driver's pane-zoom state. The active window's tab gets a
    // `Z` marker (`WindowInfo.zoomed`) when a pane is zoomed; non-active tabs
    // never show it (zoom is per the active window).
    zoomed: Option<&TerminalId>,
) -> Vec<phux_config::widget::WindowInfo> {
    workspace
        .windows
        .iter()
        .enumerate()
        .map(|(i, w)| {
            let title = w
                .state
                .focus
                .as_ref()
                .and_then(|fid| panes.get(fid))
                .and_then(|slot| slot.terminal.title().ok())
                .map(str::trim)
                .filter(|t| !t.is_empty())
                .map(ToOwned::to_owned);
            let active = i == workspace.active;
            phux_config::widget::WindowInfo {
                name: title.unwrap_or_else(|| w.name.clone()),
                active,
                zoomed: active && zoomed.is_some(),
            }
        })
        .collect()
}

/// phux-x2hm: the per-leaf rect map of the **zoom-honoring** view, used as the
/// pre-toggle snapshot for the reflow handshake. Returns an empty map when
/// there is no active window or its tree is unseeded (single-pane bootstrap).
fn zoom_rects(
    workspace: &Workspace,
    zoomed: Option<&TerminalId>,
    pane_dims: (u16, u16),
) -> HashMap<TerminalId, crate::layout::Rect> {
    workspace
        .render_window(zoomed)
        .and_then(|ls| {
            ls.tree
                .as_ref()
                .map(|_| super::multi_pane::compute_layout(ls.as_ref(), pane_dims).rects)
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
    pane_dims: (u16, u16),
) -> Result<(), AttachError> {
    let Some(ls) = workspace.render_window(zoomed) else {
        return Ok(());
    };
    if ls.tree.is_none() {
        return Ok(());
    }
    let diff = super::reflow::compute_reflow(ls.as_ref(), prev_rects, pane_dims);
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
/// dropping to an empty bar (and, alongside [`build_resolver`], no
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
    match phux_config::widget::StatusBar::build(&cfg.status, &registry) {
        Ok(bar) if bar.is_empty() => None,
        Ok(bar) => Some(StatusBarPainter::new(bar, Position::default())),
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
    /// the writer-injecting variant.
    pub fn install() -> Result<Self, AttachError> {
        Self::install_with_stdout(&mut io::stdout())
    }

    /// Install the guard. Errors if stdin is not a TTY or the termios
    /// dance fails. The alt-screen + cursor-hide bytes are written to
    /// `out` so tests can capture them and assert on the regression
    /// guard for `phux-roz`.
    pub fn install_with_stdout<W: Write>(out: &mut W) -> Result<Self, AttachError> {
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
        // frame paint doesn't briefly show on the normal screen.
        write_enter_alt_screen(out).map_err(AttachError::Io)?;

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

/// Write the alt-screen-enter + cursor-hide sequence. Factored out so
/// the install path and any future re-entry path share one byte
/// definition.
fn write_enter_alt_screen<W: Write>(out: &mut W) -> io::Result<()> {
    out.write_all(b"\x1b[?1049h")?;
    out.write_all(b"\x1b[?25l")?;
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
    use phux_protocol::caps::ServerCapabilities;
    use tokio::net::UnixStream;

    #[test]
    fn attach_error_io_display_includes_source() {
        let err = AttachError::Io(io::Error::other("boom"));
        let msg = err.to_string();
        assert!(msg.contains("attach loop io error"));
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

        let infos = window_infos(&workspace, &panes, None);
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

        let infos = window_infos(&workspace, &panes, None);
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

        let infos = window_infos(&workspace, &panes, None);
        assert_eq!(infos[0].name, "1");
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

        let infos = window_infos(&workspace, &panes, Some(&active));
        assert!(infos[0].zoomed, "active window reflects the zoom state");
        assert!(!infos[1].zoomed, "a non-active window is never zoomed");

        // No zoom ⇒ no window is marked.
        let infos = window_infos(&workspace, &panes, None);
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

        let (res, ()) = tokio::join!(handshake(&mut client), server_side);
        assert!(
            res.is_ok(),
            "handshake should succeed when HELLO_OK arrives"
        );
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

        let (res, ()) = tokio::join!(handshake(&mut client), server_side);
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
