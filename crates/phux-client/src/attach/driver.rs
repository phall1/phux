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
use phux_protocol::caps::{ClientCapabilities, Layer, LayerSet, detect_color_support};
use phux_protocol::ids::{CollectionId, TerminalId};
use phux_protocol::wire::frame::{AttachTarget, FrameKind, Scope, ViewportInfo};
use rustix::termios::{LocalModes, OptionalActions, Termios};
use tokio::io::AsyncReadExt;
use tokio::signal::unix::{SignalKind, signal};

use super::actions::{self, ActionError};
use super::connection::Connection;
use super::input::{InputEvent, StdinParser};
use super::render::{TerminalRenderer, write_reset};
use super::status_bar::{Position, StatusBarPainter, make_context};
use crate::layout::{Direction, LayoutState, SplitDir};
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
#[allow(
    clippy::cognitive_complexity,
    reason = "select! arms + phux-4li.5 outcome dispatch; ditto"
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
    // phux-4li.5: keybind resolver + request-id allocator for L3 GET
    // correlation. The resolver consumes `InputEvent::Key` events
    // *before* they would be forwarded to the focused pane; a chord
    // that resolves to a layout action mutates `layout_state` here
    // and never reaches the server's input pipe.
    let mut resolver = build_resolver();
    let mut layout_get_request_id: Option<u32> = None;
    let mut next_request_id: u32 = 1;
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
    let outcome = handle_server_frame(
        initial_attached,
        &mut panes,
        &mut layout_state,
        &mut focused_pane,
        &mut session_name,
        status_bar.as_mut(),
        viewport_dims,
        &mut predict,
        &overlay,
        layout_get_request_id,
    )?;
    if outcome.exit {
        return Ok(());
    }
    if outcome.subscribe_layout {
        // phux-4li.5: ask the server for any persisted layout, then
        // subscribe to future mutations. Both frames are best-effort —
        // if the server rejects them with an ERROR (we'd see one in a
        // later loop iteration) we just stay in the single-pane
        // bootstrap.
        let req_id = next_request_id;
        layout_get_request_id = Some(req_id);
        next_request_id = next_request_id.wrapping_add(1);
        conn.send(&FrameKind::GetMetadata {
            request_id: req_id,
            scope: Scope::Collection(DEFAULT_COLLECTION_ID),
            key: LAYOUT_KEY.to_owned(),
        })
        .await?;
        conn.send(&FrameKind::SubscribeMetadata {
            scope: Scope::Collection(DEFAULT_COLLECTION_ID),
            key: LAYOUT_KEY.to_owned(),
        })
        .await?;
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
                        let outcome = handle_server_frame(
                            f,
                            &mut panes,
                            &mut layout_state,
                            &mut focused_pane,
                            &mut session_name,
                            status_bar.as_mut(),
                            viewport_dims,
                            &mut predict,
                            &overlay,
                            layout_get_request_id,
                        )?;
                        if outcome.exit {
                            return Ok(());
                        }
                        if outcome.layout_replaced {
                            // phux-4li.5: layout changed under us
                            // (either the GET reply or a peer's broadcast).
                            // Trigger a full repaint: clear screen + paint
                            // dividers + re-render every pane.
                            repaint_multi_pane(
                                &layout_state,
                                &mut panes,
                                viewport_dims,
                                status_bar.as_mut(),
                                &session_name,
                            );
                            // The GET reply is single-use; clear the
                            // pending request id so a stray late
                            // MetadataValue can't trample state.
                            layout_get_request_id = None;
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
                let mut ctx = DispatchCtx {
                    resolver: resolver.as_mut(),
                    layout_state: &mut layout_state,
                    viewport: viewport_dims,
                    next_request_id: &mut next_request_id,
                };
                dispatch_input_events(
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
            }

            // Bare-ESC idle timeout. Only armed when the parser has
            // pending state; resolves an ambiguous lone ESC into the
            // Escape key (see input::StdinParser::flush docs).
            () = flush_sleep => {
                let events = parser.flush();
                let mut ctx = DispatchCtx {
                    resolver: resolver.as_mut(),
                    layout_state: &mut layout_state,
                    viewport: viewport_dims,
                    next_request_id: &mut next_request_id,
                };
                dispatch_input_events(
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

/// Mutable context the input-dispatch path needs to update on a chord
/// that resolves to a layout action (phux-4li.5). Bundles the items
/// that would otherwise inflate `dispatch_input_events`'s argument
/// list past clippy's threshold.
struct DispatchCtx<'a> {
    /// Keybind resolver state. `None` when the on-disk config failed
    /// to parse; the dispatcher then forwards every key to the focused
    /// pane unchanged.
    resolver: Option<&'a mut phux_config::keybind::Resolver>,
    /// Client-side layout mirror. Action helpers in [`super::actions`]
    /// take `&LayoutState` and return a new state which the dispatcher
    /// swaps in place.
    layout_state: &'a mut LayoutState,
    /// Outer-viewport `(cols, rows)`. Used by `apply_resize` to convert
    /// `amount` (cells) to a ratio delta.
    viewport: (u16, u16),
    /// Monotonic source of new request ids. We don't currently issue
    /// per-action correlated requests (the only side-channel today is
    /// the layout `SET_METADATA`, which doesn't need a reply), but we
    /// reserve the counter for future `SPAWN`/kill wiring.
    next_request_id: &'a mut u32,
}

/// Translate a batch of parser events into wire frames and ship them.
///
/// Detach requests short-circuit into a single `FrameKind::Detach` and
/// flip `detach_pending`. Pre-attach events (no `focused_pane` yet) are
/// dropped with a debug log — the wire spec has no "pre-attach buffer"
/// notion.
///
/// phux-4li.5: when a `KeyEvent` matches a configured keybind, the
/// chord is consumed by the dispatcher and the corresponding layout
/// action runs (focus move / resize / etc.). The key is NOT forwarded
/// to the focused pane in that case — same convention as tmux's
/// `prefix` table.
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
    focused_pane: &mut Option<TerminalId>,
    detach_pending: &mut bool,
    predict: &mut PredictionState,
    overlay: &Overlay,
    panes: &mut HashMap<TerminalId, PaneSlot>,
    ctx: &mut DispatchCtx<'_>,
) -> Result<(), AttachError> {
    let mut predicted_any = false;
    let mut layout_changed = false;
    for ev in events {
        if matches!(ev, InputEvent::DetachRequested) {
            if !*detach_pending {
                conn.send(&FrameKind::Detach).await?;
                *detach_pending = true;
            }
            continue;
        }
        // phux-4li.5: resolver intercept. Run BEFORE the predict layer
        // so a chord that resolves to e.g. `focus-direction` doesn't
        // leave a stale ghost overlay on the previous focused pane.
        if let InputEvent::Key(ref key_event) = ev
            && let Some(outcome) = consume_chord(ctx, key_event)
        {
            match outcome {
                ChordOutcome::Partial => {
                    // Still waiting on the next chord in a multi-chord
                    // sequence; absorb the byte and move on.
                    continue;
                }
                ChordOutcome::Resolved(resolved) => {
                    let effects = run_action(&resolved, ctx, focused_pane.as_ref());
                    if effects.layout_mutated {
                        layout_changed = true;
                    }
                    if effects.set_focus.is_some() {
                        *focused_pane = effects.set_focus;
                    }
                    if effects.set_metadata {
                        // Send SET_METADATA carrying the new envelope.
                        // Encoding can fail only if the state is empty
                        // (we just produced it — should not happen),
                        // but propagate cleanly if it ever does.
                        if let Some(bytes) = encode_layout_or_log(ctx.layout_state) {
                            let request_id = *ctx.next_request_id;
                            *ctx.next_request_id = ctx.next_request_id.wrapping_add(1);
                            conn.send(&FrameKind::SetMetadata {
                                request_id,
                                scope: Scope::Collection(DEFAULT_COLLECTION_ID),
                                key: LAYOUT_KEY.to_owned(),
                                value: bytes,
                            })
                            .await?;
                        }
                    }
                    if effects.bell {
                        let mut stdout = io::stdout().lock();
                        let _ = actions::write_bell(&mut stdout);
                    }
                    continue;
                }
            }
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
            && let Some(fid) = focused_pane.as_ref()
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
        let Some(pane) = focused_pane.as_ref() else {
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
    if layout_changed {
        // Repaint dividers + every leaf. Driver-side repaint is the
        // single point that decides what reaches stdout after an action
        // mutates the tree; the action helpers themselves never paint.
        repaint_multi_pane(ctx.layout_state, panes, ctx.viewport, None, "");
    }
    Ok(())
}

/// Result of feeding a key event through the resolver.
enum ChordOutcome {
    /// Chord extended a partial sequence; absorb and wait.
    Partial,
    /// Chord completed a binding; effects follow.
    Resolved(phux_config::keybind::ResolvedAction),
}

/// Convert a `KeyEvent` into a `KeyChord` and feed the resolver. Returns
/// `None` when the resolver is disabled (no config) or the chord
/// doesn't match any binding — caller forwards normally in that case.
///
/// Release / repeat events are NOT fed to the resolver — chord matching
/// is press-only, matching the convention of `phux-config::keybind`'s
/// tests and tmux's prefix table. Repeats of held keys (e.g. arrow keys
/// scrolling) would otherwise re-fire actions per-tick.
fn consume_chord(
    ctx: &mut DispatchCtx<'_>,
    key_event: &phux_protocol::input::key::KeyEvent,
) -> Option<ChordOutcome> {
    use phux_protocol::input::key::KeyAction;
    let resolver = ctx.resolver.as_deref_mut()?;
    if !matches!(key_event.action, KeyAction::Press) {
        return None;
    }
    let chord = phux_config::keybind::KeyChord {
        modifiers: key_event.mods,
        key: key_event.key,
    };
    match resolver.feed(chord) {
        phux_config::keybind::Feed::NoMatch => None,
        phux_config::keybind::Feed::Partial => Some(ChordOutcome::Partial),
        phux_config::keybind::Feed::Resolved(r) => Some(ChordOutcome::Resolved(r)),
    }
}

/// Side-effects a resolved action wants from the driver.
#[derive(Debug, Default)]
struct ActionEffects {
    /// `true` ⇒ `layout_state` was mutated in-place; driver repaints.
    layout_mutated: bool,
    /// `Some(new_focus)` ⇒ swap the driver's `focused_pane` (input
    /// routing follows). The action helper already updated
    /// `layout_state.focus`; this carries the new id so the driver
    /// doesn't have to re-read it.
    set_focus: Option<TerminalId>,
    /// `true` ⇒ emit `SET_METADATA` carrying the new layout envelope.
    set_metadata: bool,
    /// `true` ⇒ emit a terminal bell (BEL `\x07`).
    bell: bool,
}

/// Dispatch a resolved action against the driver's context.
///
/// Returns the [`ActionEffects`] the caller needs to apply. The function
/// is sync: it never touches the connection — frame I/O happens in the
/// caller (`dispatch_input_events`) so a hypothetical async wire-send
/// failure doesn't leave layout state half-mutated.
fn run_action(
    resolved: &phux_config::keybind::ResolvedAction,
    ctx: &mut DispatchCtx<'_>,
    focused: Option<&TerminalId>,
) -> ActionEffects {
    let _ = focused;
    let mut effects = ActionEffects::default();
    match resolved.action.as_str() {
        "split-pane" => {
            // SPAWN frame doesn't yet exist in the wire (no
            // `FrameKind::SpawnTerminal` / reply variant). Without it
            // we can't allocate a new `TerminalId` server-side, so
            // split-pane bells and logs in v0.1. Filed as a follow-up
            // — see phux-4li session notes.
            tracing::warn!(
                "split-pane: deferred pending SPAWN wire-frame allocation; \
                 see phux-4li.5 deferred-scope note"
            );
            effects.bell = true;
        }
        "kill-pane" => {
            // Same blocker as split-pane: no `TERMINAL_CLOSED` frame
            // and no SPAWN_KILL exists in the wire. Once either lands
            // (TERMINAL_CLOSED-driven cleanup OR an explicit kill
            // frame), wire `actions::apply_kill` here + emit
            // SET_METADATA. Until then bell + log.
            tracing::warn!(
                "kill-pane: deferred pending TERMINAL_CLOSED or kill-frame wire allocation"
            );
            effects.bell = true;
        }
        "focus-direction" => {
            if let Some(dir) = direction_arg(resolved) {
                if let Some(new_state) = actions::apply_focus(ctx.layout_state, dir) {
                    let new_focus = new_state.focus.clone();
                    *ctx.layout_state = new_state;
                    effects.layout_mutated = true;
                    effects.set_focus = new_focus;
                }
                // No-neighbour case: silently drop (tmux convention —
                // bumping into the layout edge isn't a bell).
            } else {
                tracing::warn!(args = ?resolved.args, "focus-direction missing/bad `direction` arg");
                effects.bell = true;
            }
        }
        "resize-pane" => {
            if let (Some(dir), Some(amount)) = (direction_arg(resolved), amount_arg(resolved)) {
                match actions::apply_resize(ctx.layout_state, dir, amount, ctx.viewport) {
                    Ok(Some(new_state)) => {
                        *ctx.layout_state = new_state;
                        effects.layout_mutated = true;
                        effects.set_metadata = true;
                    }
                    Ok(None) | Err(ActionError::NoResizableBoundary) => {
                        // Underflow guard tripped or no matching axis —
                        // bell-no-op (ADR-0019 decision 5).
                        effects.bell = true;
                    }
                    Err(err) => {
                        tracing::warn!(error = %err, "resize-pane failed");
                        effects.bell = true;
                    }
                }
            } else {
                tracing::warn!(args = ?resolved.args, "resize-pane missing args");
                effects.bell = true;
            }
        }
        "next-pane" => {
            if let Some(new_state) = actions::apply_next_pane(ctx.layout_state) {
                let new_focus = new_state.focus.clone();
                *ctx.layout_state = new_state;
                effects.layout_mutated = true;
                effects.set_focus = new_focus;
            }
        }
        "previous-pane" => {
            if let Some(new_state) = actions::apply_previous_pane(ctx.layout_state) {
                let new_focus = new_state.focus.clone();
                *ctx.layout_state = new_state;
                effects.layout_mutated = true;
                effects.set_focus = new_focus;
            }
        }
        other => {
            tracing::debug!(action = other, "unhandled resolved action");
        }
    }
    effects
}

/// Pull a `Direction` out of a [`ResolvedAction`]'s `direction = "..."`
/// arg.
fn direction_arg(resolved: &phux_config::keybind::ResolvedAction) -> Option<Direction> {
    let s = resolved.args.get("direction")?.as_str()?;
    match s {
        "up" => Some(Direction::Up),
        "down" => Some(Direction::Down),
        "left" => Some(Direction::Left),
        "right" => Some(Direction::Right),
        // `split-pane direction=horizontal|vertical` uses a different
        // axis vocabulary; this helper is only for focus/resize.
        _ => None,
    }
}

/// Pull an `amount = N` arg out of a [`ResolvedAction`]. TOML integers
/// decode as `i64`; we clamp to `i16` (the [`actions::apply_resize`]
/// signature). Out-of-range values are silently clamped — a `resize-pane
/// amount = 99999` user binding gets a 32767-cell amount, which the
/// underflow guard inside `apply_resize` then rejects.
#[allow(clippy::cast_possible_truncation)]
fn amount_arg(resolved: &phux_config::keybind::ResolvedAction) -> Option<i16> {
    let v = resolved.args.get("amount")?.as_integer()?;
    Some(v.clamp(i64::from(i16::MIN), i64::from(i16::MAX)) as i16)
}

/// Encode `state` for `SET_METADATA`, logging encode failures. Returns
/// `None` on failure — caller should not emit a frame in that case.
fn encode_layout_or_log(state: &LayoutState) -> Option<Vec<u8>> {
    match state.encode_cbor() {
        Ok(bytes) => Some(bytes),
        Err(err) => {
            tracing::warn!(error = %err, "layout CBOR encode failed; SET_METADATA skipped");
            None
        }
    }
}

/// Allow `SplitDir` to be parsed from a `direction = "horizontal|vertical"`
/// arg on a `split-pane` action. Lives here (not in `actions.rs`) so the
/// pure helper module stays free of `ResolvedAction` parsing.
#[allow(dead_code)] // wired in once SPAWN lands; kept for symmetry + warning-free build.
fn split_dir_arg(resolved: &phux_config::keybind::ResolvedAction) -> Option<SplitDir> {
    let s = resolved.args.get("direction")?.as_str()?;
    match s {
        "horizontal" => Some(SplitDir::Horizontal),
        "vertical" => Some(SplitDir::Vertical),
        _ => None,
    }
}

/// Repaint the multi-pane composition after a layout mutation or a
/// reconcile. Clears the viewport, then asks each pane's `TerminalRenderer`
/// to paint into its own `Rect`, then writes dividers and the status bar.
///
/// `status_bar` and `session_name` are threaded only so the bar can be
/// repainted in the same pass; callers that already paint the bar (e.g.
/// the status-tick branch) pass `None`/`""`.
fn repaint_multi_pane(
    layout_state: &LayoutState,
    panes: &mut HashMap<TerminalId, PaneSlot>,
    viewport_dims: (u16, u16),
    status_bar: Option<&mut StatusBarPainter>,
    session_name: &str,
) {
    let multi = super::multi_pane::compute_layout(layout_state, viewport_dims);
    let mut stdout = io::stdout().lock();
    // ED2 (clear screen) + cursor home. Cheap and unambiguous.
    let _ = stdout.write_all(b"\x1b[2J\x1b[H");
    // Render each pane at its allocated origin. `render_at` writes into
    // the per-pane sub-rectangle without touching pixels outside it.
    for (id, rect) in &multi.rects {
        if let Some(slot) = panes.get_mut(id) {
            // Resize the libghostty Terminal to match its new Rect so
            // the renderer's CUP math lines up with the destination
            // sub-region. Best-effort: a libghostty resize failure
            // means the pane redraws stale; not fatal.
            let _ = slot.terminal.resize(rect.w.max(1), rect.h.max(1), 0, 0);
            let _ = slot
                .renderer
                .render_at(&slot.terminal, &mut stdout, (rect.x, rect.y));
        }
    }
    let _ = super::multi_pane::paint_dividers(&mut stdout, &multi);
    paint_bar_after_pane(status_bar, &mut stdout, viewport_dims, session_name);
}

/// Outcome of processing a single server-to-client frame.
///
/// The driver translates these into async actions (send a frame, exit
/// the loop, repaint). Keeping the side-effect-free decision inside
/// [`handle_server_frame`] lets the function stay synchronous.
#[derive(Debug, Clone, Default)]
struct FrameOutcome {
    /// `true` ⇒ the loop should exit cleanly (server sent `DETACHED`).
    exit: bool,
    /// `true` ⇒ ATTACHED just landed; the driver should emit
    /// `GET_METADATA` + `SUBSCRIBE_METADATA` for the layout key so
    /// other clients' mutations broadcast back to us (ADR-0019).
    subscribe_layout: bool,
    /// `true` ⇒ `layout_state` was replaced by a server-side layout
    /// envelope (`MetadataValue` reply or `MetadataChanged` broadcast).
    /// The driver triggers a full repaint of the multi-pane composition.
    layout_replaced: bool,
}

/// Process one server-to-client frame. Returns a [`FrameOutcome`]
/// describing any follow-up the async driver needs to perform.
///
/// `status_bar` is `Option<&mut StatusBarPainter>` so an attach with no
/// configured widgets pays nothing for the chrome path. `viewport_dims`
/// is `(cols, rows)` of the outer terminal — used by the painter to
/// pick the bottom row.
#[allow(clippy::too_many_arguments)] // arg list bundles status-bar + predict state; follow-up to refactor into a context struct
#[allow(
    clippy::too_many_lines,
    reason = "phux-4li.5 added L3 reconcile branches; refactor with the status-bar arg-list cleanup"
)]
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
    pending_layout_request: Option<u32>,
) -> Result<FrameOutcome, AttachError> {
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
            //
            // phux-4li.5: signal the driver to emit GET_METADATA and
            // SUBSCRIBE_METADATA for the layout key so we (a) reconcile
            // against a persisted layout from a previous session and
            // (b) receive METADATA_CHANGED broadcasts from sibling
            // clients (ADR-0019 decision 2).
            Ok(FrameOutcome {
                subscribe_layout: true,
                ..FrameOutcome::default()
            })
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
            Ok(FrameOutcome::default())
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
            Ok(FrameOutcome::default())
        }
        FrameKind::Detached => Ok(FrameOutcome {
            exit: true,
            ..FrameOutcome::default()
        }),
        FrameKind::Bell { .. } => {
            // Forward bell to the outer terminal. The user's terminal
            // emulator decides whether to render visually, audibly, or
            // not at all.
            let mut stdout = io::stdout().lock();
            let _ = stdout.write_all(b"\x07");
            let _ = stdout.flush();
            Ok(FrameOutcome::default())
        }
        // phux-4li.5: reconcile-on-attach reply path. The driver sends
        // `GET_METADATA { request_id }` immediately after ATTACHED;
        // the server replies with `MetadataValue { request_id, value }`.
        // Match by id, decode the v1 CBOR envelope, and replace
        // `layout_state` in place. `value: None` means "no persisted
        // layout" — keep the single-pane bootstrap untouched.
        FrameKind::MetadataValue { request_id, value } => {
            if Some(request_id) != pending_layout_request {
                tracing::debug!(
                    request_id,
                    "dropping MetadataValue with no matching pending request"
                );
                return Ok(FrameOutcome::default());
            }
            let Some(bytes) = value else {
                return Ok(FrameOutcome::default());
            };
            match LayoutState::decode_cbor(&bytes) {
                Ok(new_state) => {
                    *layout_state =
                        reconcile_loaded_layout(new_state, focused_pane.as_ref(), panes);
                    Ok(FrameOutcome {
                        layout_replaced: true,
                        ..FrameOutcome::default()
                    })
                }
                Err(err) => {
                    tracing::warn!(error = %err, "failed to decode persisted layout; keeping bootstrap");
                    Ok(FrameOutcome::default())
                }
            }
        }
        // phux-4li.5: broadcast reconcile. Another attached client
        // mutated `phux.tui.layout/v1`; decode + replace + repaint.
        // Tombstones (`value: None`) are treated as "layout reset" —
        // fall back to the single-pane bootstrap so the next render
        // doesn't try to draw against a stale tree.
        FrameKind::MetadataChanged { scope, key, value } => {
            if !is_layout_key(&scope, &key) {
                return Ok(FrameOutcome::default());
            }
            if let Some(bytes) = value {
                match LayoutState::decode_cbor(&bytes) {
                    Ok(new_state) => {
                        *layout_state =
                            reconcile_loaded_layout(new_state, focused_pane.as_ref(), panes);
                        Ok(FrameOutcome {
                            layout_replaced: true,
                            ..FrameOutcome::default()
                        })
                    }
                    Err(err) => {
                        tracing::warn!(error = %err, "broadcast layout decode failed; ignoring");
                        Ok(FrameOutcome::default())
                    }
                }
            } else {
                // Tombstone: layout reset. Fall back to single-pane
                // bootstrap (or empty if there's no focus to anchor on).
                *layout_state = focused_pane
                    .clone()
                    .map_or_else(LayoutState::default, LayoutState::single);
                Ok(FrameOutcome {
                    layout_replaced: true,
                    ..FrameOutcome::default()
                })
            }
        }
        other => {
            // Anything else — `HELLO_OK`, `PONG`, future spec frames — is
            // accepted-but-ignored. The protocol decoder rejects unknown
            // discriminants; this branch handles known-but-not-yet-wired
            // frames.
            tracing::debug!(kind = ?other, "ignoring server frame");
            Ok(FrameOutcome::default())
        }
    }
}

/// Decide whether `(scope, key)` matches the layout-coordination key
/// ADR-0019 reserves (`phux.tui.layout/v1`, scoped to the default
/// Collection).
fn is_layout_key(scope: &Scope, key: &str) -> bool {
    matches!(scope, Scope::Collection(id) if *id == DEFAULT_COLLECTION_ID) && key == LAYOUT_KEY
}

/// Sanity-check a freshly decoded layout against the panes the driver
/// has slots for, and fall back to a safe focus if the persisted focus
/// no longer exists (e.g. the leaf was killed in a previous session).
///
/// We accept the persisted tree as-is — panes that don't yet have a
/// `PaneSlot` will get one lazily when their first `TERMINAL_OUTPUT`
/// arrives, so an arbitrary tree shape is fine. Focus is the one
/// invariant we can't recover from: if the persisted focused leaf
/// isn't a member of the tree the renderer would have no focused
/// pane to draw input chrome on.
fn reconcile_loaded_layout(
    mut state: LayoutState,
    bootstrap_focus: Option<&TerminalId>,
    _panes: &HashMap<TerminalId, PaneSlot>,
) -> LayoutState {
    let tree_leaves = state
        .tree
        .as_ref()
        .map(crate::layout::leaves)
        .unwrap_or_default();
    let focus_ok = state
        .focus
        .as_ref()
        .is_some_and(|f| tree_leaves.contains(f));
    if !focus_ok {
        // Prefer the bootstrap focus if it's actually in the tree;
        // otherwise pick the first leaf (ADR-0019 decision 6 default);
        // otherwise clear focus entirely.
        state.focus = bootstrap_focus
            .filter(|f| tree_leaves.contains(f))
            .cloned()
            .or_else(|| tree_leaves.into_iter().next());
    }
    state
}

/// phux-4li.5: L3 metadata key under which the multi-pane layout
/// envelope persists (ADR-0019 decision 1). The reference TUI is the
/// sole consumer; other clients (a future GUI, an agent) never read
/// or write it.
const LAYOUT_KEY: &str = "phux.tui.layout/v1";

/// phux-4li.5: the single Collection v0.1 servers expose. L2
/// (Collection lifecycle) is not yet wire-allocated; until it ships,
/// every L3 key the reference TUI cares about is scoped to this
/// constant. Matches `phux_server::state::DEFAULT_COLLECTION_ID`
/// (the server picks the same numeric value; if they ever drift, the
/// L3 reconcile path silently no-ops because the broadcast scope
/// won't match).
const DEFAULT_COLLECTION_ID: CollectionId = CollectionId::new(1);

/// phux-4li.5: build a [`phux_config::keybind::Resolver`] from the
/// on-disk config. Failures log and return `None` — a malformed
/// `[keybindings]` table degrades to "no actions are bound" rather
/// than blocking attach. The existing `Ctrl-b d` detach chord is
/// parsed by [`super::input::StdinParser`] and is independent of
/// this resolver, so even with no resolver the user can still detach.
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
