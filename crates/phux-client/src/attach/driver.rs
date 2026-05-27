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
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime};

use libghostty_vt::{Terminal, TerminalOptions};
use phux_protocol::PROTOCOL_VERSION;
use phux_protocol::caps::{ClientCapabilities, detect_color_support};
use phux_protocol::ids::TerminalId;
use phux_protocol::wire::frame::{AttachTarget, FrameKind, ViewportInfo};
use rustix::termios::{LocalModes, OptionalActions, Termios};
use tokio::io::AsyncReadExt;
use tokio::signal::unix::{SignalKind, signal};

use super::connection::Connection;
use super::input::{InputEvent, StdinParser};
use super::render::{TerminalRenderer, write_reset};
use super::status_bar::{Position, StatusBarPainter, make_context};
use crate::layout::LayoutState;
use crate::predict::{
    Overlay, PredictionState, PredictiveConfig, reconcile_terminal_output_per_cell,
};

/// One pane's mirror: the libghostty Terminal that ingests
/// `TERMINAL_OUTPUT` and the renderer that paints it to the outer
/// terminal. Grown from "one of these per attach" (single-pane v0) to
/// "one of these per leaf in the layout tree" by phux-4li.4. The driver
/// keeps a [`PaneMap`] of these keyed by [`TerminalId`].
pub(super) struct PaneSlot {
    /// libghostty mirror for this pane.
    pub terminal: Terminal<'static, 'static>,
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
    /// Allocate a fresh slot with a default-sized libghostty Terminal.
    /// Dimensions get replaced on the first `TERMINAL_SNAPSHOT` for this
    /// pane; 80x24 is the safest no-content placeholder.
    fn new() -> Result<Self, AttachError> {
        Ok(Self {
            terminal: Terminal::new(TerminalOptions {
                cols: 80,
                rows: 24,
                max_scrollback: 10_000,
            })?,
            renderer: TerminalRenderer::new()?,
        })
    }
}

/// Idle window before a parser-pending bare ESC is interpreted as the
/// Escape key. Chosen to be long enough to absorb same-burst arrival of
/// `ESC [` / `ESC O` sequences over local UDS-or-PTY paths (which are
/// effectively zero-latency), but short enough that the user's perception
/// of pressing Escape stays snappy. xterm uses ~50ms by default; we match.
const ESC_FLUSH_IDLE: Duration = Duration::from_millis(50);

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
    run_with_stdout(socket, target, &mut io::stdout()).await
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
    run_with_stdout_predict(socket, target, &mut io::stdout(), predict).await
}

/// Same as [`run`], but writes any terminal-control bytes (alt-screen
/// enter, cursor hide, the renderer's per-row CUP/SGR, cleanup) to a
/// caller-supplied `Write`.
///
/// Exposed so tests can capture the byte stream and assert on it — in
/// particular, the regression guard for `phux-roz` asserts that the
/// pre-handshake failure path NEVER emits `\x1b[?1049h`. Production
/// callers should use [`run`] which targets real stdout; the stdin /
/// signal / termios paths are unchanged.
///
/// The writer is only used for terminal-control bytes emitted *outside*
/// the renderer in pre-handshake setup. The renderer itself still
/// writes to `io::stdout()` because the libghostty render iterators
/// are not generic over `Write`. The pre-handshake regression guard
/// (no alt-screen on failure) does not need the renderer to participate
/// because that path never reaches the renderer.
#[allow(
    clippy::future_not_send,
    reason = "client-side libghostty Terminal is !Send; ADR-0003 binds us to current-thread"
)]
pub async fn run_with_stdout<W: Write>(
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
pub async fn run_with_stdout_predict<W: Write>(
    socket: &Path,
    target: AttachTarget,
    out: &mut W,
    predict: PredictiveConfig,
) -> Result<(), AttachError> {
    // STAGE 1 — pre-handshake, on the cooked outer terminal.
    //
    // We deliberately do NOT install RawModeGuard here. If anything in
    // this block fails (no server, refused, signal during connect) the
    // user's terminal stays in its original state and `Err(_)` carries
    // the actionable cause up to the CLI.
    let mut conn = Connection::connect(socket).await?;
    handshake(&mut conn).await?;
    send_attach(&mut conn, target).await?;
    let attached = wait_for_attached(&mut conn).await?;

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

    main_loop(&mut conn, attached, predict).await
}

/// Send `HELLO` and (when the server starts sending it) wait for
/// `HELLO_OK`. Today the server does not send a `HELLO_OK` and the
/// protocol crate does not yet define the variant; we proceed
/// optimistically.
async fn handshake(conn: &mut Connection) -> Result<(), AttachError> {
    // Sniff `$COLORTERM` / `$TERM` / `$TERM_PROGRAM` per
    // `detect_color_support`. The advertised tier feeds the server's
    // per-client `downsample::rewrite_bytes` (SPEC §6.2).
    let client_caps = ClientCapabilities::new().with_color_support(detect_color_support());
    conn.send(&FrameKind::Hello {
        client_name: format!("phux-client/{}", env!("CARGO_PKG_VERSION")),
        protocol_major: PROTOCOL_VERSION.major,
        protocol_minor: PROTOCOL_VERSION.minor,
        protocol_patch: PROTOCOL_VERSION.patch,
        client_caps,
    })
    .await
}

/// Send the `ATTACH` frame using the current terminal viewport.
async fn send_attach(conn: &mut Connection, target: AttachTarget) -> Result<(), AttachError> {
    let viewport = current_viewport()?;
    conn.send(&FrameKind::Attach {
        target,
        viewport,
        // SPEC §13: clients SHOULD opt in to scrollback. The cap below
        // matches the default in DESIGN.md §X; a configurable knob lives
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

/// Drive the `tokio::select!` loop until detach.
///
/// `initial_attached` is the `FrameKind::Attached` frame that
/// [`wait_for_attached`] already pulled off the wire; we replay it
/// through `handle_server_frame` so the focused-pane bookkeeping lives
/// in one place. Subsequent `TERMINAL_SNAPSHOT` / `TERMINAL_OUTPUT` frames come
/// off the wire as usual.
#[allow(
    clippy::future_not_send,
    reason = "client-side libghostty Terminal is !Send; ADR-0003 binds us to current-thread"
)]
#[allow(
    clippy::too_many_lines,
    reason = "tokio::select! arms inflate function length; splitting would require carrying ~10 mutable locals through helpers"
)]
async fn main_loop(
    conn: &mut Connection,
    initial_attached: FrameKind,
    predict_cfg: PredictiveConfig,
) -> Result<(), AttachError> {
    // phux-4li.4: hold N client-side Terminals keyed by `TerminalId`,
    // not the single Terminal of the wave-A driver. Each pane's slot is
    // allocated lazily — the first `TERMINAL_SNAPSHOT` or
    // `TERMINAL_OUTPUT` carrying a given id seeds it via
    // `panes.entry(id).or_insert_with(PaneSlot::new)`. The
    // `LayoutState` mirror (initialized as a single-pane fallback when
    // `ATTACHED` lands; see `handle_server_frame`) is the source of
    // truth for which leaves should be live and where they sit in the
    // outer viewport. Sibling-ticket .7's SIGWINCH reflow will use
    // `layout_state` + `attach::multi_pane::compute_layout` to recompute
    // every pane's Rect on resize; .5's action dispatch will mutate it
    // on split/kill. v0.1 only ever sees the single bootstrap leaf, so
    // the existing single-pane render path keeps working unchanged.
    let mut panes: HashMap<TerminalId, PaneSlot> = HashMap::new();
    let mut layout_state = LayoutState::default();
    let mut focused_pane: Option<TerminalId> = None;
    // phux-nz4.5: status-bar painter, built from the on-disk config.
    // Load failures fall back to an empty bar so a malformed config
    // never blocks attach — the user still gets a working pane mirror.
    let mut status_bar = build_status_bar_painter();
    // Track the current outer-terminal viewport so the painter knows
    // which row is "bottom". Initialized to a sensible default and
    // updated by SIGWINCH; the server doesn't drive client-side
    // viewport (clients own their chrome per DESIGN §8.5).
    let mut viewport_dims: (u16, u16) =
        current_viewport().map_or((80, 24), |v| (v.cols.max(1), v.rows.max(1)));
    let mut session_name = String::new();
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

    // Replay the `ATTACHED` frame so the focused-pane bookkeeping in
    // `handle_server_frame` runs exactly once, in one place.
    let exit = handle_server_frame(
        initial_attached,
        &mut panes,
        &mut layout_state,
        &mut focused_pane,
        &mut session_name,
        status_bar.as_mut(),
        viewport_dims,
        &mut predict,
        &overlay,
    )?;
    if exit {
        return Ok(());
    }

    loop {
        // Arm the bare-ESC idle timer only when the parser has pending
        // state. When no flush is pending we substitute a never-resolving
        // future so the select! arm parks forever; this keeps the steady-
        // state cost at one always-`Pending` future and avoids unused-
        // `Option` branches inside `select!`.
        let flush_sleep: std::pin::Pin<Box<dyn Future<Output = ()>>> = if parser.has_pending() {
            Box::pin(tokio::time::sleep(ESC_FLUSH_IDLE))
        } else {
            Box::pin(std::future::pending::<()>())
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

            // Server frames take priority — process them as fast as the
            // network delivers so the user sees output promptly.
            frame = conn.recv() => {
                match frame {
                    Ok(f) => {
                        let exit = handle_server_frame(
                            f,
                            &mut panes,
                            &mut layout_state,
                            &mut focused_pane,
                            &mut session_name,
                            status_bar.as_mut(),
                            viewport_dims,
                            &mut predict,
                            &overlay,
                        )?;
                        if exit {
                            return Ok(());
                        }
                    }
                    Err(AttachError::Disconnected) if detach_pending => {
                        // Server closed the socket without a `DETACHED`
                        // frame — treat it as a clean shutdown because
                        // the user requested detach. Otherwise the loop
                        // bubbles the disconnect up unchanged.
                        return Ok(());
                    }
                    Err(err) => return Err(err),
                }
            }

            // Stdin → upstream. `read` returns 0 on EOF (terminal closed).
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
                dispatch_input_events(
                    conn,
                    events,
                    focused_pane.as_ref(),
                    &mut detach_pending,
                    &mut predict,
                    &overlay,
                    &mut panes,
                )
                .await?;
            }

            // Bare-ESC idle timeout. Only armed when the parser has
            // pending state; resolves an ambiguous lone ESC into the
            // Escape key (see input::StdinParser::flush docs).
            () = flush_sleep => {
                let events = parser.flush();
                dispatch_input_events(
                    conn,
                    events,
                    focused_pane.as_ref(),
                    &mut detach_pending,
                    &mut predict,
                    &overlay,
                    &mut panes,
                )
                .await?;
            }

            // SIGWINCH — terminal was resized. Read the new viewport
            // and ship a VIEWPORT_RESIZE upstream (SPEC §7.1 / §10.5).
            // The server uses this to recompute layout and update the
            // attached pane's dims. On query failure we fall back to a
            // sane default (logged) rather than skip the frame — the
            // server still benefits from knowing a resize happened.
            _ = sigwinch.recv() => {
                let viewport = current_viewport_or_default();
                viewport_dims = (viewport.cols.max(1), viewport.rows.max(1));
                // Resize clears the prediction queue (anchored to the
                // previous viewport); the next render reseeds the
                // cursor from authoritative state.
                predict.set_viewport(viewport.cols, viewport.rows);
                conn.send(&viewport_resize_frame(viewport)).await?;
                // phux-4li.4: re-render the focused pane after a resize.
                // Per-pane reflow (resizing each leaf's libghostty
                // Terminal to its new Rect, recomputing dividers) is
                // sibling-ticket .7's `attach::reflow` integration; this
                // branch keeps the single-pane behaviour from wave A
                // pending that wire-up.
                let mut stdout = io::stdout().lock();
                if let Some(fid) = focused_pane.as_ref()
                    && let Some(slot) = panes.get_mut(fid)
                {
                    let _ = slot.renderer.render(&slot.terminal, &mut stdout);
                }
                // phux-nz4.5: viewport dims changed — force a fresh
                // status-bar paint so the bar lands on the new bottom row.
                if let Some(p) = status_bar.as_mut() {
                    p.invalidate();
                    let _ = p.paint(
                        &mut stdout,
                        viewport_dims.0,
                        viewport_dims.1,
                        &make_context(&session_name, SystemTime::now()),
                    );
                }
            }

            // phux-nz4.5: periodic status-bar repaint (e.g. for the
            // `time` widget). Only fires when at least one widget has a
            // `poll_interval`. Paints in place — no pane re-render, no
            // full-screen redraw.
            () = status_tick => {
                if let Some(p) = status_bar.as_mut() {
                    let mut stdout = io::stdout().lock();
                    let _ = p.paint(
                        &mut stdout,
                        viewport_dims.0,
                        viewport_dims.1,
                        &make_context(&session_name, SystemTime::now()),
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

/// Translate a batch of parser events into wire frames and ship them.
///
/// Detach requests short-circuit into a single `FrameKind::Detach` and
/// flip `detach_pending`. Pre-attach events (no `focused_pane` yet) are
/// dropped with a debug log — the wire spec has no "pre-attach buffer"
/// notion.
// arg list bundles transport + render + predict context; follow-up to
// refactor into a context struct.
#[allow(clippy::too_many_arguments, reason = "see comment above")]
#[allow(
    clippy::future_not_send,
    reason = "client-side libghostty Terminal is !Send; ADR-0003 binds us to current-thread"
)]
async fn dispatch_input_events(
    conn: &mut Connection,
    events: Vec<InputEvent>,
    focused_pane: Option<&TerminalId>,
    detach_pending: &mut bool,
    predict: &mut PredictionState,
    overlay: &Overlay,
    panes: &mut HashMap<TerminalId, PaneSlot>,
) -> Result<(), AttachError> {
    let mut predicted_any = false;
    for ev in events {
        if matches!(ev, InputEvent::DetachRequested) {
            if !*detach_pending {
                conn.send(&FrameKind::Detach).await?;
                *detach_pending = true;
            }
            continue;
        }
        // Predictive echo only fires for key events; mouse / paste / focus
        // intentionally bypass the prediction layer (they target the
        // server's input model, not the visual grid). The branch is
        // skipped entirely when the config flag is off — `predict_key`
        // returns `Disabled` and no overlay paint is scheduled.
        //
        // Arrows over a known cell on the current line (phux-9gw.1.3)
        // need a grid peek to know the width of the grapheme they step
        // over; we hand `read_grapheme_at` to the predict layer so it
        // can refuse the prediction when the cell is blank.
        //
        // phux-4li.4: peek the focused pane's grid, not "the" grid.
        // Multi-pane v0.1 routes input to the client's focused leaf
        // (per ADR-0019 decision 6); the predict layer follows.
        if let InputEvent::Key(ref key_event) = ev
            && predict.is_enabled()
            && let Some(fid) = focused_pane
            && let Some(slot) = panes.get_mut(fid)
        {
            use crate::predict::PredictionOutcome;
            let outcome = predict.predict_key_with_grid(key_event, |r, c| {
                slot.renderer
                    .read_grapheme_at(&slot.terminal, r, c)
                    .ok()
                    .flatten()
            });
            if matches!(outcome, PredictionOutcome::Predicted) {
                predicted_any = true;
            }
        }
        let Some(pane) = focused_pane else {
            tracing::debug!("dropping input received before ATTACHED");
            continue;
        };
        if let Some(frame) = ev.into_frame((*pane).clone()) {
            conn.send(&frame).await?;
        }
    }
    // Paint the prediction overlay once per dispatch batch so a burst of
    // keystrokes produces a single positioned write run, not one per
    // event. The overlay is a no-op on an empty queue.
    if predicted_any {
        let mut stdout = io::stdout().lock();
        let _ = overlay.render(predict, &mut stdout);
    }
    Ok(())
}

/// Process one server-to-client frame. Returns `true` if the loop should
/// exit cleanly (i.e. the server sent `DETACHED`).
///
/// `status_bar` is `Option<&mut StatusBarPainter>` so an attach with no
/// configured widgets pays nothing for the chrome path. `viewport_dims`
/// is `(cols, rows)` of the outer terminal — used by the painter to
/// pick the bottom row.
#[allow(clippy::too_many_arguments)] // arg list bundles status-bar + predict state; follow-up to refactor into a context struct
fn handle_server_frame(
    frame: FrameKind,
    panes: &mut HashMap<TerminalId, PaneSlot>,
    layout_state: &mut LayoutState,
    focused_pane: &mut Option<TerminalId>,
    session_name: &mut String,
    status_bar: Option<&mut StatusBarPainter>,
    viewport_dims: (u16, u16),
    predict: &mut PredictionState,
    overlay: &Overlay,
) -> Result<bool, AttachError> {
    match frame {
        FrameKind::Attached {
            snapshot,
            initial_client_id: _,
        } => {
            // Capture the initial focused pane so subsequent INPUT_* frames
            // know where to route.
            let bootstrap = snapshot.focused_pane;
            *focused_pane = Some(bootstrap.clone());
            // phux-4li.4: seed the layout mirror with a single leaf so
            // the existing single-pane render path keeps working. The
            // L3 metadata-fetch path (.2/.3) replaces this with the
            // server-stored tree when present; until that ticket lands
            // every attach is single-pane.
            *layout_state = LayoutState::single(bootstrap.clone());
            // Ensure the focused pane has a slot ready for output
            // frames; output may race ahead of the snapshot. If
            // libghostty refuses to allocate a Terminal we surface
            // the failure rather than silently dropping the bootstrap.
            if let std::collections::hash_map::Entry::Vacant(v) = panes.entry(bootstrap) {
                v.insert(PaneSlot::new()?);
            }
            // phux-nz4.5: stash the session name for the status-bar
            // `WidgetContext`. The Snapshot type names the session via
            // its window graph; for v0 the session-name widget reads
            // from a string slot we maintain here, defaulting to the
            // empty string until a session-graph carrier lands.
            *session_name = String::new();
            // `ATTACHED` per SPEC §13 carries the session/window/pane
            // graph; the per-pane initial cells arrive separately via
            // TERMINAL_SNAPSHOT.
            Ok(false)
        }
        FrameKind::TerminalSnapshot {
            terminal_id,
            cols,
            rows,
            vt_replay_bytes,
            scrollback_bytes,
        } => {
            // phux-4li.4: route per-pane snapshots into per-pane slots.
            // Allocate a fresh slot on first sight so output frames for
            // pre-split panes don't drop on the floor.
            let is_focused = Some(&terminal_id) == focused_pane.as_ref();
            let slot = match panes.entry(terminal_id) {
                std::collections::hash_map::Entry::Occupied(o) => o.into_mut(),
                std::collections::hash_map::Entry::Vacant(v) => v.insert(PaneSlot::new()?),
            };
            slot.terminal.resize(cols, rows, 0, 0)?;
            // Apply scrollback first (if any), then the visible-state
            // replay — order per SPEC §8.4 / §13.
            if let Some(sb) = scrollback_bytes {
                slot.terminal.vt_write(&sb);
            }
            slot.terminal.vt_write(&vt_replay_bytes);
            if is_focused {
                // A fresh snapshot replaces the world — drop any
                // outstanding predictions and resize the predict layer.
                predict.set_viewport(cols, rows);
                let mut stdout = io::stdout().lock();
                let _ = slot.renderer.render(&slot.terminal, &mut stdout);
                if let Some((row, col)) = slot.renderer.last_cursor() {
                    predict.set_cursor(row, col);
                }
                // Snapshot is authoritative — overlay only repaints if
                // new keystrokes arrived after the snapshot was issued
                // and before reconcile cleared the queue. In v0 we
                // simply leave the queue empty.
                let _ = overlay;
                // phux-nz4.5: the pane renderer just wrote to the
                // bottom row of its own grid; force a status-bar
                // repaint over it.
                paint_bar_after_pane(status_bar, &mut stdout, viewport_dims, session_name);
            }
            Ok(false)
        }
        FrameKind::TerminalOutput {
            terminal_id,
            seq: _,
            bytes,
        } => {
            // phux-4li.4: ingest output into the matching pane's
            // libghostty Terminal even when it's not focused, so the
            // mirror stays warm for when the user focuses it. Render +
            // predict-reconcile only fire for the focused pane.
            let is_focused = Some(&terminal_id) == focused_pane.as_ref();
            let slot = match panes.entry(terminal_id) {
                std::collections::hash_map::Entry::Occupied(o) => o.into_mut(),
                std::collections::hash_map::Entry::Vacant(v) => v.insert(PaneSlot::new()?),
            };
            slot.terminal.vt_write(&bytes);
            if is_focused {
                let mut stdout = io::stdout().lock();
                let _ = slot.renderer.render(&slot.terminal, &mut stdout);
                // Per-cell match reconcile (phux-9gw.1.1): walk pending
                // predictions against the freshly painted cell grid;
                // confirmed predictions drop, contradictions drop their
                // suffix, predictions still ahead of confirmed state
                // stay alive. See [`crate::predict`] for the truth
                // table.
                if let Some((row, col)) = slot.renderer.last_cursor() {
                    let _stats = reconcile_terminal_output_per_cell(predict, row, col, |r, c| {
                        slot.renderer
                            .read_grapheme_at(&slot.terminal, r, c)
                            .ok()
                            .flatten()
                    });
                } else {
                    // Cursor hidden — we can't anchor reliably; fall
                    // back to the wholesale drain. Rare path (programs
                    // that hide the cursor before a redraw).
                    predict.clear();
                }
                // Overlay paints any predictions still alive (the tail
                // of a partial confirmation). On a fully-drained queue
                // this is a no-op.
                let _ = overlay.render(predict, &mut stdout);
                paint_bar_after_pane(status_bar, &mut stdout, viewport_dims, session_name);
            }
            Ok(false)
        }
        FrameKind::Detached => Ok(true),
        FrameKind::Bell { .. } => {
            // Forward bell to the outer terminal. The user's terminal
            // emulator decides whether to render visually, audibly, or
            // not at all.
            let mut stdout = io::stdout().lock();
            let _ = stdout.write_all(b"\x07");
            let _ = stdout.flush();
            Ok(false)
        }
        other => {
            // Anything else — `HELLO_OK`, `PONG`, future spec frames — is
            // accepted-but-ignored. The protocol decoder rejects unknown
            // discriminants; this branch handles known-but-not-yet-wired
            // frames.
            tracing::debug!(kind = ?other, "ignoring server frame");
            Ok(false)
        }
    }
}

/// phux-nz4.5: load the on-disk config and build a [`StatusBarPainter`]
/// from `[status]`. Errors fall back to no bar (logged) so a malformed
/// config never blocks attach. Returns `None` when the bar would be
/// empty (no widgets configured) — callers can short-circuit on that.
fn build_status_bar_painter() -> Option<StatusBarPainter> {
    let cfg = match phux_config::loader::load() {
        Ok(c) => c,
        Err(err) => {
            tracing::warn!(error = %err, "phux-config load failed; status bar disabled");
            return None;
        }
    };
    let registry = phux_config::WidgetRegistry::with_builtins();
    match phux_config::widget::StatusBar::build(&cfg.status, &registry) {
        Ok(bar) if bar.is_empty() => None,
        Ok(bar) => Some(StatusBarPainter::new(bar, Position::default())),
        Err(err) => {
            tracing::warn!(error = %err, "status-bar build failed; status bar disabled");
            None
        }
    }
}

/// phux-nz4.5: shared helper invoked after every pane render so the
/// status row is restored on top of whatever VT the pane renderer just
/// wrote. No-op when there is no painter or no live viewport.
fn paint_bar_after_pane<W: Write>(
    status_bar: Option<&mut StatusBarPainter>,
    out: &mut W,
    viewport_dims: (u16, u16),
    session_name: &str,
) {
    let Some(painter) = status_bar else {
        return;
    };
    // The pane renderer wrote into the bottom row — invalidate so the
    // painter unconditionally re-emits.
    painter.invalidate();
    let _ = painter.paint(
        out,
        viewport_dims.0,
        viewport_dims.1,
        &make_context(session_name, SystemTime::now()),
    );
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
static ALT_SCREEN_ACTIVE: AtomicBool = AtomicBool::new(false);

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
/// state (recovered from the live stdin tty), and the alt-screen
/// sequence is left if we entered one. Errors are swallowed — the
/// process is on its way out.
fn terminal_reset_on_signal() {
    // Restore termios. We can't reach the `RawModeGuard`'s captured
    // `original_termios` from here without a global; instead we ask
    // the kernel to re-cook the tty by setting ICANON|ECHO|ISIG back.
    // That's not a perfect restore (it ignores user-customised flags
    // like IUTF8 / IUCLC / VEOF), but it's close enough that the user
    // can type `reset` if they want a precise restore. The important
    // bit is that the alt-screen + cursor-hide is undone.
    if let Ok(mut termios) = rustix::termios::tcgetattr(io::stdin().as_fd()) {
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
        let _ = rustix::termios::tcsetattr(io::stdin().as_fd(), OptionalActions::Now, &termios);
    }
    let mut out = io::stdout().lock();
    let _ = write_terminal_reset(&mut out);
}

/// Install a global panic hook that runs [`write_terminal_reset`]
/// before the previous (default) hook prints the panic. Idempotent —
/// repeated calls after the first are no-ops.
///
/// Without this, a panic deep inside the renderer or libghostty would
/// unwind through `main_loop`, but the default hook would print the
/// backtrace into the alt screen — the user sees nothing because we're
/// about to leave the alt screen, and then on cooked-terminal restore
/// the panic message is already gone. The hook flips the cleanup
/// BEFORE the panic message lands.
fn install_panic_hook_once() {
    if PANIC_HOOK_INSTALLED.swap(true, Ordering::SeqCst) {
        return;
    }
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        terminal_reset_on_signal();
        previous(info);
    }));
}

#[cfg(test)]
#[allow(clippy::expect_used, reason = "tests")]
mod tests {
    use super::*;

    #[test]
    fn attach_error_io_display_includes_source() {
        let err = AttachError::Io(io::Error::other("boom"));
        let msg = err.to_string();
        assert!(msg.contains("attach loop io error"));
    }

    #[test]
    fn attach_error_disconnected_is_distinct_from_io() {
        let a = AttachError::Disconnected;
        let b = AttachError::Io(io::Error::other("foo"));
        assert_ne!(std::mem::discriminant(&a), std::mem::discriminant(&b),);
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
}
