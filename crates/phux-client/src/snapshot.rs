//! Structured, headless screen capture — the floor of the agent surface
//! (ADR-0022, `phux-oki`).
//!
//! Connects to the per-user server, attaches to a target, replays the
//! authoritative `TERMINAL_SNAPSHOT` into a libghostty `Terminal`, and
//! walks the grid into a serializable [`ScreenState`].
//!
//! This is what lets an agent read the screen as *data* — the same grid
//! the TUI would paint, but projected to JSON instead of VT bytes.
//!
//! **Bootstrap limitation (ADR-0022 §5).** `ATTACH` carries a viewport, so
//! this transiently resizes the focused pane to `viewport` (it self-heals
//! on the next real-client paint). The read-only, side-effect-free path is
//! a server-side `GET_SCREEN` query that walks the server's own
//! `Terminal`; it is tracked under the `phux-3cw` epic. This attach-walk
//! bootstrap exists to start dogfooding the JSON contract now; the CLI
//! contract ([`ScreenState`]) is stable regardless of where the walk runs.

use std::path::Path;
use std::time::Duration;

use libghostty_vt::screen::CellWide;
use libghostty_vt::{
    Terminal, TerminalOptions, render::CellIterator, render::RenderState, render::RowIterator,
};
use phux_protocol::PROTOCOL_VERSION;
use phux_protocol::caps::{ClientCapabilities, Layer, LayerSet, detect_color_support};
use phux_protocol::wire::frame::{AttachTarget, FrameKind, ViewportInfo};
use serde::Serialize;
use tokio::time::timeout;

use crate::attach::AttachError;
use crate::attach::connection::Connection;

/// Stable JSON contract version (ADR-0022 §2). Bump on any breaking change
/// to the [`ScreenState`] shape so consumers can pin/branch.
pub const SCHEMA_VERSION: u32 = 1;

/// How long to wait for each expected frame before giving up.
const RECV_DEADLINE: Duration = Duration::from_secs(5);

/// Cursor position + visibility, viewport-relative, zero-based.
#[derive(Debug, Clone, Serialize)]
pub struct CursorState {
    /// Column, zero-based, viewport-relative.
    pub x: u16,
    /// Row, zero-based, viewport-relative.
    pub y: u16,
    /// Whether the cursor is currently visible (DECTCEM).
    pub visible: bool,
}

/// A point-in-time projection of one pane's grid as structured data.
///
/// The default shape is plain text lines + cursor + dims — what most
/// agents want. Per-cell styles and OSC-133 semantic marks are a future
/// additive field (`--cells`), not a new struct (ADR-0022 §2).
#[derive(Debug, Clone, Serialize)]
pub struct ScreenState {
    /// Contract version; see [`SCHEMA_VERSION`].
    pub schema_version: u32,
    /// Wire-local terminal id of the captured pane.
    pub pane: u32,
    /// Grid width in cells.
    pub cols: u16,
    /// Grid height in cells.
    pub rows: u16,
    /// Cursor state, or `None` when libghostty can't resolve a
    /// viewport-resident cursor (e.g. it's in scrollback or hidden).
    pub cursor: Option<CursorState>,
    /// Viewport rows, top to bottom, right-trimmed.
    pub lines: Vec<String>,
}

/// Connect, attach to `target`, and capture the focused pane's screen.
///
/// `viewport` is the size advertised in `ATTACH`; see the module-level
/// bootstrap-limitation note.
pub async fn capture(
    socket: &Path,
    target: AttachTarget,
    viewport: ViewportInfo,
) -> Result<ScreenState, AttachError> {
    let mut conn = Connection::connect(socket).await?;
    conn.send(&FrameKind::Hello {
        client_name: format!("phux-snapshot/{}", env!("CARGO_PKG_VERSION")),
        protocol_major: PROTOCOL_VERSION.major,
        protocol_minor: PROTOCOL_VERSION.minor,
        protocol_patch: PROTOCOL_VERSION.patch,
        client_caps: ClientCapabilities::new()
            .with_color_support(detect_color_support())
            .with_layers(LayerSet::with(&[Layer::L3])),
    })
    .await?;
    conn.send(&FrameKind::Attach {
        target,
        viewport,
        request_scrollback: false,
        scrollback_limit_lines: 0,
    })
    .await?;

    // ATTACHED carries the session graph; pull the focused pane id.
    let focused = loop {
        match recv(&mut conn, "ATTACHED").await? {
            FrameKind::Attached { snapshot, .. } => break snapshot.focused_pane,
            FrameKind::Error { message, .. } => return Err(AttachError::Refused(message)),
            _ => {}
        }
    };

    // The focused pane's TERMINAL_SNAPSHOT carries the replay bytes.
    let (cols, rows, replay) = loop {
        match recv(&mut conn, "TERMINAL_SNAPSHOT").await? {
            FrameKind::TerminalSnapshot {
                terminal_id,
                cols,
                rows,
                vt_replay_bytes,
                ..
            } if terminal_id == focused => break (cols, rows, vt_replay_bytes),
            _ => {}
        }
    };

    // We have what we need; release the attach (best-effort).
    let _ = conn.send(&FrameKind::Detach).await;

    let mut term = Terminal::new(TerminalOptions {
        cols,
        rows,
        max_scrollback: 1000,
    })?;
    term.vt_write(&replay);
    let (lines, cursor) = walk(&term, cols, rows)?;

    Ok(ScreenState {
        schema_version: SCHEMA_VERSION,
        pane: focused.local_id().unwrap_or(0),
        cols,
        rows,
        cursor,
        lines,
    })
}

/// Receive the next frame with a deadline, mapping a timeout to a
/// protocol error naming what we were waiting for.
async fn recv(conn: &mut Connection, awaiting: &str) -> Result<FrameKind, AttachError> {
    timeout(RECV_DEADLINE, conn.recv())
        .await
        .map_err(|_| AttachError::Protocol(format!("timed out awaiting {awaiting}")))?
}

/// Walk the terminal grid into trimmed text rows + the viewport cursor.
/// Mirrors the `Screen` test oracle and the production render walk.
fn walk(
    term: &Terminal<'_, '_>,
    cols: u16,
    rows: u16,
) -> Result<(Vec<String>, Option<CursorState>), AttachError> {
    let mut state = RenderState::new()?;
    let mut row_iter = RowIterator::new()?;
    let mut cell_iter = CellIterator::new()?;
    let snapshot = state.update(term)?;
    let cursor = snapshot.cursor_viewport()?.map(|c| CursorState {
        x: c.x,
        y: c.y,
        visible: snapshot.cursor_visible().unwrap_or(true),
    });
    let total = snapshot.rows().unwrap_or(rows);
    let mut out: Vec<String> = Vec::with_capacity(usize::from(total));
    let mut ri = row_iter.update(&snapshot)?;
    let mut idx: u16 = 0;
    while let Some(row) = ri.next() {
        if idx >= total {
            break;
        }
        let mut buf = String::with_capacity(usize::from(cols));
        if let Ok(mut ci) = cell_iter.update(row) {
            while let Some(cell) = ci.next() {
                let wide = cell
                    .raw_cell()
                    .and_then(libghostty_vt::screen::Cell::wide)
                    .unwrap_or(CellWide::Narrow);
                if matches!(wide, CellWide::SpacerTail) {
                    continue;
                }
                match cell.graphemes() {
                    Ok(g) if !g.is_empty() => buf.extend(g),
                    _ => buf.push(' '),
                }
            }
        }
        out.push(buf.trim_end().to_owned());
        idx += 1;
    }
    Ok((out, cursor))
}
