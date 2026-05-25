//! Build a [`phux_protocol::Grid`] from a `libghostty_vt::Terminal`.
//!
//! This is the seam between the terminal-emulator library (which holds the
//! authoritative state) and the protocol (which speaks in [`Cell`]s and
//! [`Grid`]s). Every server pane runs through here once per frame.
//!
//! Per ADR-0008, `Color` and `Underline` are direct re-exports of
//! libghostty-vt's types, so there is no conversion layer for them — the
//! values flow through unchanged. Only `CursorVisualStyle` and the per-bool
//! `Style` fields require translation.

use libghostty_vt::{
    RenderState, Terminal,
    render::{CellIterator, CursorVisualStyle, RowIterator},
    style::StyleColor,
};
use phux_protocol::{Cell, CellFlags, Color, CursorShape, CursorState, Grid};

/// Errors that can occur while capturing a grid from a terminal.
#[derive(Debug, thiserror::Error)]
pub enum CaptureError {
    /// Surfaced from libghostty-vt.
    #[error("libghostty: {0}")]
    Ghostty(#[from] libghostty_vt::Error),
}

/// Snapshot the current state of `terminal` into a [`Grid`].
///
/// Allocates fresh [`RenderState`], [`RowIterator`], and [`CellIterator`]
/// each call. Per-pane hot loops should hold these structs across frames;
/// this helper is for one-shot captures (testing, the diff spike, the
/// `phux capture` command).
pub fn capture(terminal: &Terminal<'_, '_>) -> Result<Grid, CaptureError> {
    let mut render_state = RenderState::new()?;
    let mut rows = RowIterator::new()?;
    let mut cells = CellIterator::new()?;

    let snapshot = render_state.update(terminal)?;
    let cols = snapshot.cols()?;
    let rows_n = snapshot.rows()?;

    let mut grid_cells: Vec<Vec<Cell>> = (0..rows_n)
        .map(|_| (0..cols).map(|_| Cell::blank()).collect())
        .collect();

    let mut row_iter = rows.update(&snapshot)?;
    let mut row_index: u16 = 0;
    while let Some(row) = row_iter.next() {
        if row_index >= rows_n {
            break;
        }
        let mut cell_iter = cells.update(row)?;
        let mut col_index: u16 = 0;
        while let Some(cell) = cell_iter.next() {
            if col_index >= cols {
                break;
            }
            grid_cells[usize::from(row_index)][usize::from(col_index)] = cell_from_iter(cell)?;
            col_index += 1;
        }
        row_index += 1;
    }

    let cursor = cursor_from_snapshot(&snapshot)?;

    Ok(Grid {
        cols,
        rows: rows_n,
        cells: grid_cells,
        cursor,
    })
}

fn cell_from_iter(
    cell: &libghostty_vt::render::CellIteration<'_, '_>,
) -> Result<Cell, CaptureError> {
    let text = cell.graphemes()?;

    // `CellIteration::{fg,bg}_color` returns the *resolved* RGB color
    // (after palette+theme lookup), not the raw `StyleColor`. We promote
    // it to `StyleColor::Rgb` so the wire carries the final pixel value;
    // `None` arrives when the cell's style is `StyleColor::None`.
    let fg = cell.fg_color()?.map_or(Color::None, Color::Rgb);
    let bg = cell.bg_color()?.map_or(Color::None, Color::Rgb);

    let style = cell.style()?;
    // `Underline` and `StyleColor` are themselves re-exports of libghostty's
    // types (ADR-0008), so these are identity copies.
    let underline = style.underline;
    let underline_color: StyleColor = style.underline_color;

    let mut flags = CellFlags::empty();
    if style.bold {
        flags |= CellFlags::BOLD;
    }
    if style.faint {
        flags |= CellFlags::FAINT;
    }
    if style.italic {
        flags |= CellFlags::ITALIC;
    }
    if style.blink {
        flags |= CellFlags::BLINK_SLOW;
    }
    if style.inverse {
        flags |= CellFlags::REVERSE;
    }
    if style.invisible {
        flags |= CellFlags::INVISIBLE;
    }
    if style.strikethrough {
        flags |= CellFlags::STRIKETHROUGH;
    }
    if style.overline {
        flags |= CellFlags::OVERLINED;
    }

    Ok(Cell {
        text,
        fg,
        bg,
        underline,
        underline_color,
        flags,
    })
}

fn cursor_from_snapshot(
    snapshot: &libghostty_vt::render::Snapshot<'_, '_>,
) -> Result<CursorState, CaptureError> {
    let visible = snapshot.cursor_visible()?;
    let blink = snapshot.cursor_blinking()?;
    let shape = match snapshot.cursor_visual_style()? {
        // Block and any future kind fall through to the Block default below.
        CursorVisualStyle::Bar => CursorShape::Bar,
        CursorVisualStyle::Underline => CursorShape::Underline,
        CursorVisualStyle::BlockHollow => CursorShape::BlockHollow,
        _ => CursorShape::Block,
    };
    let (row, col) = snapshot
        .cursor_viewport()?
        .map_or((0, 0), |cv| (cv.y, cv.x));

    Ok(CursorState {
        row,
        col,
        visible,
        shape,
        blink,
    })
}
