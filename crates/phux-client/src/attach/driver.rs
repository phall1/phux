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
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use libghostty_vt::{Terminal, TerminalOptions};
use phux_protocol::PROTOCOL_VERSION;
use phux_protocol::caps::{ClientCapabilities, Layer, LayerSet, detect_color_support};
use phux_protocol::ids::{CollectionId, TerminalId};
use phux_protocol::wire::frame::{AttachTarget, FrameKind, Scope, ViewportInfo};
use rustix::termios::{LocalModes, OptionalActions, Termios};
use tokio::io::AsyncReadExt;
use tokio::signal::unix::{SignalKind, signal};

use super::actions::PendingSplit;
use super::connection::Connection;
use super::input::StdinParser;
use super::input_dispatch::{DispatchCtx, dispatch_input_events, encode_layout_or_log};
use super::paint::{paint_bar_after_pane, paint_full_frame, pane_viewport};
use super::render::{TerminalRenderer, write_reset};
use super::server_frame::handle_server_frame;
use crate::layout::LayoutState;
use crate::predict::{Overlay, PredictionState, PredictiveConfig};
use crate::render::chrome::status_bar::{Position, StatusBarPainter};
use crate::render::overlay::OverlayState;

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
    pub(super) fn new() -> Result<Self, AttachError> {
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
    // phux-4li.12: in-flight `split-pane` actions parked by request id.
    // Populated by `run_action` when it dispatches SPAWN_TERMINAL;
    // drained by `handle_server_frame`'s TerminalSpawned arm when the
    // reply arrives. The map is small (one entry per outstanding
    // user-triggered split) so a HashMap is overkill for cap but
    // matches the layout-key request-id pattern.
    let mut pending_splits: HashMap<u32, PendingSplit> = HashMap::new();
    // phux-nz4.5: status-bar painter, built from the on-disk config.
    // Load failures fall back to an empty bar so a malformed config
    // never blocks attach — the user still gets a working pane mirror.
    let mut status_bar = build_status_bar_painter();
    // phux-5ke.4: keybindings snapshot for the help overlay. Cached so
    // pressing the help binding doesn't trigger a synchronous config
    // reload (which could surface IO errors under user fingers); on
    // load failure the help modal still works, just showing "no
    // bindings configured".
    let keybindings_snapshot: Option<phux_config::KeybindingsCfg> =
        phux_config::loader::load().ok().map(|c| c.keybindings);
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
        &mut pending_splits,
        overlays.is_active(),
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
                            &mut pending_splits,
                            overlays.is_active(),
                        )?;
                        if outcome.exit {
                            return Ok(());
                        }
                        // phux-4li.12: a layout mutation triggered by a
                        // server frame (TerminalSpawned ok, TerminalClosed)
                        // requires the same `SET_METADATA` broadcast as
                        // a local action — see `ActionEffects.set_metadata`
                        // for the local-action path.
                        if outcome.emit_set_metadata
                            && let Some(bytes) = encode_layout_or_log(&layout_state)
                        {
                            let request_id = next_request_id;
                            next_request_id = next_request_id.wrapping_add(1);
                            conn.send(&FrameKind::SetMetadata {
                                request_id,
                                scope: Scope::Collection(DEFAULT_COLLECTION_ID),
                                key: LAYOUT_KEY.to_owned(),
                                value: bytes,
                            })
                            .await?;
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
                            if !overlays.is_active() {
                                paint_full_frame(
                                    &layout_state,
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
                    pending_splits: &mut pending_splits,
                    overlays: &mut overlays,
                    keybindings: keybindings_snapshot.as_ref(),
                };
                let layout_changed = dispatch_input_events(
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
                if layout_changed {
                    // phux-5ke.4: on overlay dismiss the dispatcher
                    // sets layout_changed=true; the full-frame repaint
                    // below restores pane content under the now-gone
                    // modal. When the overlay is still active (e.g.
                    // a push happened in the same batch) we skip the
                    // pane repaint and go straight to overlay paint.
                    if !overlays.is_active() {
                        paint_full_frame(
                            &layout_state,
                            &mut panes,
                            focused_pane.as_ref(),
                            viewport_dims,
                            status_bar.as_mut(),
                            &session_name,
                        );
                    }
                }
                if overlays.is_active() {
                    let mut stdout = io::stdout().lock();
                    let _ = stdout.write_all(b"\x1b[2J\x1b[H");
                    let _ = overlays.paint(&mut stdout, viewport_dims);
                }
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
                    pending_splits: &mut pending_splits,
                    overlays: &mut overlays,
                    keybindings: keybindings_snapshot.as_ref(),
                };
                let layout_changed = dispatch_input_events(
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
                if layout_changed && !overlays.is_active() {
                    paint_full_frame(
                        &layout_state,
                        &mut panes,
                        focused_pane.as_ref(),
                        viewport_dims,
                        status_bar.as_mut(),
                        &session_name,
                    );
                }
                if overlays.is_active() {
                    let mut stdout = io::stdout().lock();
                    let _ = stdout.write_all(b"\x1b[2J\x1b[H");
                    let _ = overlays.paint(&mut stdout, viewport_dims);
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
                predict.set_viewport(viewport.cols, viewport.rows);
                conn.send(&viewport_resize_frame(viewport)).await?;

                // Multi-pane: emit one TERMINAL_RESIZE per leaf whose
                // (w, h) actually changed so the server ioctls TIOCSWINSZ
                // on each PTY. Single-pane: skip the reflow math entirely
                // (no per-leaf wire emissions to make).
                if layout_state.tree.is_some() {
                    let has_bar = status_bar.is_some();
                    let prev_pane_dims = pane_viewport(prev_dims, has_bar);
                    let new_pane_dims = pane_viewport(viewport_dims, has_bar);
                    let prev_rects =
                        super::multi_pane::compute_layout(&layout_state, prev_pane_dims).rects;
                    let diff = super::reflow::compute_reflow(
                        &layout_state,
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
                    let mut stdout = io::stdout().lock();
                    let _ = stdout.write_all(b"\x1b[2J\x1b[H");
                    let _ = overlays.paint(&mut stdout, viewport_dims);
                } else {
                    paint_full_frame(
                        &layout_state,
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
                    let mut stdout = io::stdout().lock();
                    // Restore the cursor to wherever the focused pane left it
                    // so an idle tick doesn't strand the cursor in the bar.
                    let focused_cursor = focused_pane.as_ref()
                        .and_then(|fid| panes.get(fid))
                        .and_then(|slot| slot.renderer.last_cursor());
                    paint_bar_after_pane(
                        status_bar.as_mut(),
                        &mut stdout,
                        viewport_dims,
                        &session_name,
                        focused_cursor,
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

/// phux-4li.5: L3 metadata key under which the multi-pane layout
/// envelope persists (ADR-0019 decision 1). The reference TUI is the
/// sole consumer; other clients (a future GUI, an agent) never read
/// or write it.
pub(super) const LAYOUT_KEY: &str = "phux.tui.layout/v1";

/// phux-4li.5: the single Collection v0.1 servers expose. L2
/// (Collection lifecycle) is not yet wire-allocated; until it ships,
/// every L3 key the reference TUI cares about is scoped to this
/// constant. Matches `phux_server::state::DEFAULT_COLLECTION_ID`
/// (the server picks the same numeric value; if they ever drift, the
/// L3 reconcile path silently no-ops because the broadcast scope
/// won't match).
pub(super) const DEFAULT_COLLECTION_ID: CollectionId = CollectionId::new(1);

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
