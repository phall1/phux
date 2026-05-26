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
//! * a local `libghostty_vt::Terminal` + [`PaneRenderer`] for the focused
//!   pane (under ADR-0013 the client is bytes-in / `vt_write` / dirty-row
//!   redraw — see `research/2026-05-25-libghostty-renderstate.md`).

#![allow(
    clippy::result_large_err,
    reason = "AttachError carries an io::Error which is naturally large; the variants are mutually exclusive and we never carry the result in a hot loop."
)]

use std::io::{self, IsTerminal, Write};
use std::os::fd::AsFd;
use std::path::Path;

use libghostty_vt::{Terminal, TerminalOptions};
use phux_protocol::PROTOCOL_VERSION;
use phux_protocol::ids::PaneId;
use phux_protocol::wire::frame::{AttachTarget, FrameKind, ViewportInfo};
use rustix::termios::{LocalModes, OptionalActions, Termios};
use tokio::io::AsyncReadExt;
use tokio::signal::unix::{SignalKind, signal};

use super::connection::Connection;
use super::input::{InputEvent, StdinParser};
use super::render::{PaneRenderer, write_reset};

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
#[allow(
    clippy::future_not_send,
    reason = "client-side libghostty Terminal is !Send; ADR-0003 binds us to current-thread"
)]
pub async fn run(socket: &Path, target: AttachTarget) -> Result<(), AttachError> {
    // Raw mode + alt screen first, before any other I/O — if the user
    // sees us print something on the normal screen because UDS connect
    // failed, that's surprising. Going to alt-screen first means the
    // outer terminal is restored cleanly even on early-exit errors.
    let _guard = RawModeGuard::install()?;

    let mut conn = Connection::connect(socket).await?;
    handshake(&mut conn).await?;
    send_attach(&mut conn, target).await?;

    main_loop(&mut conn).await
}

/// Send `HELLO` and (when the server starts sending it) wait for
/// `HELLO_OK`. Today the server does not send a `HELLO_OK` and the
/// protocol crate does not yet define the variant; we proceed
/// optimistically.
async fn handshake(conn: &mut Connection) -> Result<(), AttachError> {
    conn.send(&FrameKind::Hello {
        client_name: format!("phux-client/{}", env!("CARGO_PKG_VERSION")),
        protocol_major: PROTOCOL_VERSION.major,
        protocol_minor: PROTOCOL_VERSION.minor,
        protocol_patch: PROTOCOL_VERSION.patch,
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

/// Drive the `tokio::select!` loop until detach.
#[allow(
    clippy::future_not_send,
    reason = "client-side libghostty Terminal is !Send; ADR-0003 binds us to current-thread"
)]
async fn main_loop(conn: &mut Connection) -> Result<(), AttachError> {
    // Client-side Terminal + renderer for the focused pane. Dimensions
    // get replaced on the first PANE_SNAPSHOT; sizing to (80, 24) is the
    // safest no-content default for a Terminal that may receive
    // PANE_OUTPUT before its snapshot for any reason.
    let mut terminal: Terminal<'static, 'static> = Terminal::new(TerminalOptions {
        cols: 80,
        rows: 24,
        max_scrollback: 10_000,
    })?;
    let mut renderer = PaneRenderer::new()?;
    let mut focused_pane: Option<PaneId> = None;
    let mut parser = StdinParser::new();
    let mut stdin = tokio::io::stdin();
    let mut stdin_buf = [0u8; 4096];
    let mut sigwinch = signal(SignalKind::window_change()).map_err(AttachError::Io)?;
    let mut detach_pending = false;

    loop {
        tokio::select! {
            biased;

            // Server frames take priority — process them as fast as the
            // network delivers so the user sees output promptly.
            frame = conn.recv() => {
                match frame {
                    Ok(f) => {
                        let exit = handle_server_frame(
                            f,
                            &mut terminal,
                            &mut renderer,
                            &mut focused_pane,
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
                for ev in events {
                    if matches!(ev, InputEvent::DetachRequested) {
                        if !detach_pending {
                            conn.send(&FrameKind::Detach).await?;
                            detach_pending = true;
                        }
                        continue;
                    }
                    let Some(pane) = focused_pane else {
                        // Pre-`ATTACHED`: drop input. The server hasn't
                        // told us which pane is focused yet, and the
                        // wire spec has no "pre-attach key buffer".
                        tracing::debug!("dropping input received before ATTACHED");
                        continue;
                    };
                    if let Some(frame) = ev.into_frame(pane.get()) {
                        conn.send(&frame).await?;
                    }
                }
            }

            // SIGWINCH — terminal was resized.
            _ = sigwinch.recv() => {
                // TODO(phux-4hp): when the `VIEWPORT_RESIZE` FrameKind
                // variant exists, encode `current_viewport()?` and send
                // it upstream. The wire frame is defined in SPEC §10.5
                // but is not yet a FrameKind variant, so the v0 attach
                // loop just notices the resize and re-renders.
                let _ = current_viewport();
                let mut stdout = io::stdout().lock();
                let _ = renderer.render(&terminal, &mut stdout);
            }
        }
    }
}

/// Process one server-to-client frame. Returns `true` if the loop should
/// exit cleanly (i.e. the server sent `DETACHED`).
fn handle_server_frame(
    frame: FrameKind,
    terminal: &mut Terminal<'static, 'static>,
    renderer: &mut PaneRenderer<'static>,
    focused_pane: &mut Option<PaneId>,
) -> Result<bool, AttachError> {
    match frame {
        FrameKind::Attached {
            snapshot,
            initial_client_id: _,
        } => {
            // Capture the initial focused pane so subsequent INPUT_* frames
            // know where to route.
            *focused_pane = Some(snapshot.focused_pane);
            // `ATTACHED` per SPEC §13 carries the session/window/pane
            // graph; the per-pane initial cells arrive separately via
            // PANE_SNAPSHOT.
            Ok(false)
        }
        FrameKind::PaneSnapshot {
            pane_id,
            cols,
            rows,
            vt_replay_bytes,
            scrollback_bytes,
        } => {
            // For v0 only the focused pane's snapshot drives our local
            // Terminal — multi-pane composition is downstream of phux-9gw.3
            // (the windowing / layout work).
            if Some(pane_id) == *focused_pane {
                terminal.resize(cols, rows, 0, 0)?;
                // Apply scrollback first (if any), then the visible-state
                // replay — order per SPEC §8.4 / §13.
                if let Some(sb) = scrollback_bytes {
                    terminal.vt_write(&sb);
                }
                terminal.vt_write(&vt_replay_bytes);
                let mut stdout = io::stdout().lock();
                let _ = renderer.render(terminal, &mut stdout);
            }
            Ok(false)
        }
        FrameKind::PaneOutput {
            pane_id,
            seq: _,
            bytes,
        } => {
            if Some(PaneId::new(pane_id)) == *focused_pane {
                terminal.vt_write(&bytes);
                let mut stdout = io::stdout().lock();
                let _ = renderer.render(terminal, &mut stdout);
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
    /// Install the guard. Errors if stdin is not a TTY or the termios
    /// dance fails.
    pub fn install() -> Result<Self, AttachError> {
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
        let mut out = io::stdout().lock();
        out.write_all(b"\x1b[?1049h").map_err(AttachError::Io)?;
        out.write_all(b"\x1b[?25l").map_err(AttachError::Io)?;
        out.flush().map_err(AttachError::Io)?;

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
        let _ = write_reset(&mut out);
        let _ = out.write_all(b"\x1b[?1049l");
        let _ = out.flush();
    }
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
}
