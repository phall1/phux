//! Copy-mode overlay (phux-wave-a-copy-mode).
//!
//! Provides terminal-based text selection with visual feedback. The overlay
//! captures arrow keys to adjust selection boundaries and Enter to copy the
//! selected text. Per [ADR-0030](../../../../ADR/0030-engine-delegated-wire-and-projection-consumers.md),
//! selection is a *client-local projection*: the overlay tracks the selection
//! rectangle in pane-local viewport cells, and on Enter the dispatcher
//! resolves it against the focused pane's own libghostty engine
//! (`format_selection_alloc`) and writes the text to the host clipboard via
//! OSC 52. Nothing about the selection touches the wire.

use phux_protocol::input::key::{KeyEvent, PhysicalKey};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};

use super::{CopyRequest, OverlayCommand, RenderOverlay};

/// How copy-mode interprets the selection rectangle.
///
/// Client-local UI state (phux-q1ni, [ADR-0030]): selection is a consumer-side
/// projection, so the mode lives with the overlay rather than on the wire.
/// `Char` is the default linear selection; `Rect` is Mosh-style block
/// selection; `Line` selects whole lines.
///
/// [ADR-0030]: ../../../../ADR/0030-engine-delegated-wire-and-projection-consumers.md
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SelectionMode {
    /// Character-wise (linear) selection — the default.
    #[default]
    Char,
    /// Line-wise selection (whole lines).
    Line,
    /// Rectangular (block) selection.
    Rect,
}

/// Rectangular selection state: (row, col) coordinates for start and end.
/// Normalized so that start <= end.
#[derive(Debug, Clone, Copy)]
struct CellRange {
    start_row: u16,
    start_col: u16,
    end_row: u16,
    end_col: u16,
}

impl CellRange {
    /// Create a range from cursor and endpoint. Normalizes so start <= end.
    fn from_points(cursor_row: u16, cursor_col: u16, end_row: u16, end_col: u16) -> Self {
        if (cursor_row, cursor_col) <= (end_row, end_col) {
            Self {
                start_row: cursor_row,
                start_col: cursor_col,
                end_row,
                end_col,
            }
        } else {
            Self {
                start_row: end_row,
                start_col: end_col,
                end_row: cursor_row,
                end_col: cursor_col,
            }
        }
    }

    /// Check if a cell is within the selection.
    const fn contains(self, row: u16, col: u16) -> bool {
        if row < self.start_row || row > self.end_row {
            return false;
        }
        if row == self.start_row && col < self.start_col {
            return false;
        }
        if row == self.end_row && col > self.end_col {
            return false;
        }
        true
    }
}

/// Copy-mode overlay state.
#[derive(Debug)]
pub struct CopyModeOverlay {
    /// Current cursor position (row, col) in pane-local coords.
    pub cursor_row: u16,
    /// Column position of cursor in pane-local coords.
    pub cursor_col: u16,
    /// Anchor point where selection started.
    pub anchor_row: u16,
    /// Column position where selection started.
    pub anchor_col: u16,
    /// Selection mode (char, line, rect).
    pub mode: SelectionMode,
    /// Pane dimensions (cols, rows) — used to clamp cursor movement.
    pub pane_cols: u16,
    /// Number of rows in the pane.
    pub pane_rows: u16,
}

impl CopyModeOverlay {
    /// Create a copy-mode overlay with cursor at the given position.
    /// `pane_cols` and `pane_rows` are used to clamp cursor movement.
    #[must_use]
    pub fn new(cursor_row: u16, cursor_col: u16, pane_cols: u16, pane_rows: u16) -> Self {
        // Clamp cursor to valid range
        let cursor_row = cursor_row.min(pane_rows.saturating_sub(1));
        let cursor_col = cursor_col.min(pane_cols.saturating_sub(1));

        Self {
            cursor_row,
            cursor_col,
            anchor_row: cursor_row,
            anchor_col: cursor_col,
            mode: SelectionMode::Char,
            pane_cols,
            pane_rows,
        }
    }

    /// Get the current normalized selection range.
    fn selection_range(&self) -> CellRange {
        CellRange::from_points(
            self.anchor_row,
            self.anchor_col,
            self.cursor_row,
            self.cursor_col,
        )
    }

    /// Move cursor by a delta, clamping to pane bounds.
    fn move_cursor(&mut self, delta_row: i16, delta_col: i16) {
        let max_row = self.pane_rows.saturating_sub(1);
        let max_col = self.pane_cols.saturating_sub(1);

        #[allow(clippy::cast_sign_loss)]
        {
            self.cursor_row = if delta_row >= 0 {
                self.cursor_row
                    .saturating_add(delta_row as u16)
                    .min(max_row)
            } else {
                self.cursor_row
                    .saturating_sub(delta_row.unsigned_abs())
                    .min(max_row)
            };

            self.cursor_col = if delta_col >= 0 {
                self.cursor_col
                    .saturating_add(delta_col as u16)
                    .min(max_col)
            } else {
                self.cursor_col
                    .saturating_sub(delta_col.unsigned_abs())
                    .min(max_col)
            };
        }
    }

    /// Build the client-local copy request for the current selection: the
    /// normalized inclusive viewport rectangle plus the block/linear flag.
    /// The dispatcher resolves it against the focused pane's own engine.
    fn copy_request(&self) -> CopyRequest {
        let range = self.selection_range();
        CopyRequest {
            start_row: range.start_row,
            start_col: range.start_col,
            end_row: range.end_row,
            end_col: range.end_col,
            rectangle: self.mode == SelectionMode::Rect,
        }
    }
}

impl RenderOverlay for CopyModeOverlay {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        use ratatui::prelude::Position;

        let range = self.selection_range();
        let highlight_style = Style::default()
            .bg(Color::Blue)
            .fg(Color::White)
            .add_modifier(Modifier::BOLD);

        // Paint selection highlight over already-rendered cells.
        // Buffer coords are (x, y) = (col, row) in outer-viewport space.
        for row in 0..area.height {
            for col in 0..area.width {
                let pane_row = area.y + row;
                let pane_col = area.x + col;

                // Convert outer-viewport coords to pane-local coords.
                let cell_row = pane_row.saturating_sub(area.y);
                let cell_col = pane_col.saturating_sub(area.x);

                if range.contains(cell_row, cell_col)
                    && let Some(cell) = buf.cell_mut(Position::new(pane_col, pane_row))
                {
                    // Apply highlight to selected cell — modify existing cell, don't replace.
                    cell.set_style(highlight_style);
                }

                // Draw a cursor marker at the anchor position.
                if cell_row == self.anchor_row
                    && cell_col == self.anchor_col
                    && let Some(cell) = buf.cell_mut(Position::new(pane_col, pane_row))
                {
                    cell.set_style(
                        Style::default()
                            .fg(Color::Yellow)
                            .add_modifier(Modifier::UNDERLINED),
                    );
                }
            }
        }

        // Status line at the bottom: "copy-mode | <N> cell(s) selected"
        let range = self.selection_range();
        let cells_selected = u32::from(range.end_row - range.start_row + 1)
            * u32::from(range.end_col - range.start_col + 1);
        let status = format!(
            " copy-mode | {cells_selected} cell(s) selected | (↑↓←→) move | Enter to copy | Esc to cancel "
        );

        // Paint status line at the bottom of the area.
        if area.height > 0 {
            let status_row = area.y + area.height - 1;
            let status_style = Style::default().bg(Color::DarkGray).fg(Color::White);

            for (col_offset, ch) in status.chars().take(area.width as usize).enumerate() {
                #[allow(clippy::cast_possible_truncation)]
                let col = area.x + col_offset as u16;
                if let Some(cell) = buf.cell_mut(Position::new(col, status_row)) {
                    cell.set_char(ch);
                    cell.set_style(status_style);
                }
            }
        }
    }

    fn handle_key(&mut self, key: &KeyEvent) -> OverlayCommand {
        use phux_protocol::input::key::KeyAction;

        if key.action != KeyAction::Press {
            return OverlayCommand::Stay;
        }

        match key.key {
            // Arrow keys adjust the selection in-place; the driver repaints
            // the overlay after every key while it is active, so `Stay` is
            // enough to reflect the moved cursor. No wire traffic (ADR-0030).
            PhysicalKey::ArrowUp => {
                self.move_cursor(-1, 0);
                OverlayCommand::Stay
            }
            PhysicalKey::ArrowDown => {
                self.move_cursor(1, 0);
                OverlayCommand::Stay
            }
            PhysicalKey::ArrowLeft => {
                self.move_cursor(0, -1);
                OverlayCommand::Stay
            }
            PhysicalKey::ArrowRight => {
                self.move_cursor(0, 1);
                OverlayCommand::Stay
            }
            // Enter copies the selection client-locally (the dispatcher
            // resolves it against the focused pane's engine and emits OSC 52)
            // and exits copy-mode, tmux-style.
            PhysicalKey::Enter => OverlayCommand::Copy(self.copy_request()),
            PhysicalKey::Escape => OverlayCommand::Dismiss,
            _ => OverlayCommand::Stay,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cell_range_normalization() {
        let range = CellRange::from_points(5, 10, 2, 3);
        assert_eq!(range.start_row, 2);
        assert_eq!(range.start_col, 3);
        assert_eq!(range.end_row, 5);
        assert_eq!(range.end_col, 10);
    }

    #[test]
    fn cell_range_contains() {
        let range = CellRange::from_points(1, 1, 3, 5);
        assert!(range.contains(1, 1));
        assert!(range.contains(2, 3));
        assert!(range.contains(3, 5));
        assert!(!range.contains(0, 1));
        assert!(!range.contains(1, 0));
        assert!(!range.contains(4, 1));
    }

    #[test]
    fn cursor_clamped_to_pane() {
        let overlay = CopyModeOverlay::new(100, 100, 80, 24);
        assert_eq!(overlay.cursor_row, 23);
        assert_eq!(overlay.cursor_col, 79);
    }
}
