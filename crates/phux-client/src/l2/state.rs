//! L2 Collection-level state snapshot — structured representation of terminal
//! and session metadata at a point in time.
//!
//! The [`TerminalState`] struct captures the full state of a terminal pane
//! at snapshot time: grid dimensions, cursor position, scrollback history,
//! shell process information, and pending command state. It is the
//! serializable contract for L2 Collection-aware agents (ADR-0015).
//!
//! This module is organized as a pure-data container with minimal logic,
//! designed for seamless JSON serialization/deserialization and snapshot
//! capture from the wire protocol.

use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

/// A single cell in the terminal grid.
///
/// Carries the text content and display attributes of a cell at a specific
/// (row, col) position.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cell {
    /// Zero-based column within the row.
    pub col: u16,
    /// Zero-based row within the grid (viewport-relative).
    pub row: u16,
    /// The Unicode grapheme cluster(s) occupying this cell.
    pub text: String,
    /// Cell width in columns (1 for ASCII, 2 for wide glyphs/emoji).
    pub width: u8,
    /// Whether the cell is currently selected (for copy/paste operations).
    pub selected: bool,
}

/// Cursor state: position and visibility.
///
/// Captures the current cursor location within the viewport and its
/// visibility state (DECTCEM).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cursor {
    /// Zero-based column, viewport-relative.
    pub x: u16,
    /// Zero-based row, viewport-relative.
    pub y: u16,
    /// Whether the cursor is currently visible (DECTCEM).
    pub visible: bool,
}

/// A single line of scrollback history.
///
/// Represents one row in the terminal's scrollback buffer, stored in
/// chronological order (oldest first).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScrollLine {
    /// The text content of the line, right-trimmed.
    pub text: String,
    /// Individual cells in the scrollback line with style/semantic information.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cells: Vec<Cell>,
}

/// Shell state information for the terminal.
///
/// Captures the current shell process information, job list, and shell type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShellState {
    /// The PID of the shell process (e.g., `/bin/bash`, `/bin/zsh`).
    pub shell_pid: u32,
    /// Human-readable shell name or path (e.g., "bash", "zsh", "/bin/bash").
    pub shell_name: String,
    /// List of background job names or identifiers.
    #[serde(default)]
    pub jobs: Vec<String>,
    /// Whether the shell is currently in copy mode or selection mode.
    pub in_copy_mode: bool,
}

/// Pending command state.
///
/// Represents a command that has been sent but not yet completed or whose
/// result has not yet been displayed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingCommand {
    /// The command text that was issued.
    pub command: String,
    /// Sequence number at the time the command was sent.
    pub seq_sent: u64,
    /// Sequence number at which the command completed (or None if still pending).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seq_completed: Option<u64>,
}

/// L2 Collection-level terminal state snapshot.
///
/// A point-in-time projection of a terminal pane's grid, cursor, scrollback,
/// shell state, and command metadata. Serves as the stable contract for
/// Collection-aware agents to inspect and interact with terminal state
/// without tying them to libghostty internals.
///
/// The `seq` field provides a logical clock: agents can poll snapshots and
/// detect changes by comparing sequence numbers, enabling efficient polling
/// without subscribing to frame streams.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalState {
    /// Grid width in cells.
    pub cols: u16,
    /// Grid height in cells.
    pub rows: u16,
    /// All cells in the viewport, in row-major order. May be sparse
    /// (not including blank cells, depending on capture options).
    pub cells: Vec<Cell>,
    /// Cursor state (position + visibility).
    pub cursor: Option<Cursor>,
    /// Scrollback history lines, oldest first (empty if not captured).
    pub scrollback: Vec<ScrollLine>,
    /// Total number of scrollback lines retained by the server (may exceed
    /// the length of the `scrollback` vec if only a recent window was captured).
    pub scrollback_count_total: u32,
    /// Shell process information.
    pub shell_state: Option<ShellState>,
    /// Pending command state (if any command is in flight).
    pub pending_command: Option<PendingCommand>,
    /// Unix timestamp (seconds since `UNIX_EPOCH`) when this snapshot was captured.
    pub timestamp_secs: u64,
    /// Logical sequence number / version of this terminal state. Increments
    /// on each update; agents can use this to detect changes without polling.
    pub seq: u64,
}

impl TerminalState {
    /// Create a new `TerminalState` with the given dimensions and sequence number.
    ///
    /// All other fields default to empty or None.
    #[must_use]
    pub fn new(cols: u16, rows: u16, seq: u64) -> Self {
        Self {
            cols,
            rows,
            cells: Vec::new(),
            cursor: None,
            scrollback: Vec::new(),
            scrollback_count_total: 0,
            shell_state: None,
            pending_command: None,
            timestamp_secs: current_unix_timestamp(),
            seq,
        }
    }

    /// Create a snapshot of the current state.
    ///
    /// This is a full clone that can be stored for later comparison or
    /// JSON serialization. The sequence number is preserved to enable
    /// change detection.
    #[must_use]
    pub fn snapshot(&self) -> Self {
        self.clone()
    }

    /// Update the grid dimensions and clear cells.
    ///
    /// Called when the grid is resized or when a new grid capture arrives.
    pub fn update_grid(&mut self, cols: u16, rows: u16, cells: Vec<Cell>) {
        self.cols = cols;
        self.rows = rows;
        self.cells = cells;
        self.timestamp_secs = current_unix_timestamp();
    }

    /// Update the cursor position and visibility.
    pub fn update_cursor(&mut self, cursor: Option<Cursor>) {
        self.cursor = cursor;
        self.timestamp_secs = current_unix_timestamp();
    }

    /// Update the scrollback history.
    ///
    /// `lines` is the viewport-relative or historical lines.
    /// `total_count` is the total number of scrollback lines available on
    /// the server (may be larger than `lines.len()` if only a window was captured).
    pub fn update_scrollback(&mut self, lines: Vec<ScrollLine>, total_count: u32) {
        self.scrollback = lines;
        self.scrollback_count_total = total_count;
        self.timestamp_secs = current_unix_timestamp();
    }

    /// Update the shell process information.
    pub fn update_shell_state(&mut self, shell_state: Option<ShellState>) {
        self.shell_state = shell_state;
        self.timestamp_secs = current_unix_timestamp();
    }

    /// Update the pending command state.
    pub fn update_pending_command(&mut self, pending: Option<PendingCommand>) {
        self.pending_command = pending;
        self.timestamp_secs = current_unix_timestamp();
    }

    /// Advance the sequence number and update the timestamp.
    ///
    /// Called when the state is updated from the wire protocol.
    pub fn increment_seq(&mut self) {
        self.seq = self.seq.wrapping_add(1);
        self.timestamp_secs = current_unix_timestamp();
    }

    /// Check if this state differs from another by sequence number.
    ///
    /// Agents can use this for efficient polling: only fully deserialize
    /// or compare if `seq` has changed.
    #[must_use]
    pub const fn has_changed(&self, other: &Self) -> bool {
        self.seq != other.seq
    }

    /// Compute a rough "screen distance" metric for layout changes.
    ///
    /// Returns the Euclidean distance between cursor positions as a simple
    /// heuristic for detecting significant viewport changes.
    #[must_use]
    pub fn cursor_distance(&self, other: &Self) -> u32 {
        match (self.cursor.as_ref(), other.cursor.as_ref()) {
            (Some(c1), Some(c2)) => {
                let dx = (i32::from(c1.x) - i32::from(c2.x)).unsigned_abs();
                let dy = (i32::from(c1.y) - i32::from(c2.y)).unsigned_abs();
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                (f64::from(dx * dx + dy * dy).sqrt() as u32)
            }
            _ => 0,
        }
    }
}

/// Get the current Unix timestamp in seconds.
#[inline]
fn current_unix_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_creates_empty_state() {
        let state = TerminalState::new(80, 24, 1);
        assert_eq!(state.cols, 80);
        assert_eq!(state.rows, 24);
        assert_eq!(state.seq, 1);
        assert!(state.cells.is_empty());
        assert!(state.cursor.is_none());
        assert!(state.scrollback.is_empty());
        assert_eq!(state.scrollback_count_total, 0);
        assert!(state.shell_state.is_none());
        assert!(state.pending_command.is_none());
    }

    #[test]
    fn snapshot_creates_independent_copy() {
        let mut state = TerminalState::new(80, 24, 1);
        state.cells.push(Cell {
            col: 0,
            row: 0,
            text: "a".to_string(),
            width: 1,
            selected: false,
        });
        let snap = state.snapshot();
        assert_eq!(snap, state);
        assert_eq!(snap.cells.len(), 1);
    }

    #[test]
    fn update_grid_clears_cells() {
        let mut state = TerminalState::new(80, 24, 1);
        state.cells.push(Cell {
            col: 0,
            row: 0,
            text: "a".to_string(),
            width: 1,
            selected: false,
        });
        let new_cells = vec![Cell {
            col: 1,
            row: 1,
            text: "b".to_string(),
            width: 1,
            selected: false,
        }];
        state.update_grid(120, 40, new_cells.clone());
        assert_eq!(state.cols, 120);
        assert_eq!(state.rows, 40);
        assert_eq!(state.cells, new_cells);
    }

    #[test]
    fn update_cursor_sets_position() {
        let mut state = TerminalState::new(80, 24, 1);
        state.update_cursor(Some(Cursor {
            x: 10,
            y: 5,
            visible: true,
        }));
        let cursor = state.cursor.unwrap();
        assert_eq!(cursor.x, 10);
        assert_eq!(cursor.y, 5);
        assert!(cursor.visible);
    }

    #[test]
    fn update_scrollback_sets_lines() {
        let mut state = TerminalState::new(80, 24, 1);
        let lines = vec![ScrollLine {
            text: "scrollback line".to_string(),
            cells: vec![],
        }];
        state.update_scrollback(lines.clone(), 42);
        assert_eq!(state.scrollback, lines);
        assert_eq!(state.scrollback_count_total, 42);
    }

    #[test]
    fn update_shell_state_sets_process() {
        let mut state = TerminalState::new(80, 24, 1);
        let shell = ShellState {
            shell_pid: 1234,
            shell_name: "bash".to_string(),
            jobs: vec!["job1".to_string()],
            in_copy_mode: false,
        };
        state.update_shell_state(Some(shell.clone()));
        assert_eq!(state.shell_state.unwrap(), shell);
    }

    #[test]
    fn increment_seq_advances_version() {
        let mut state = TerminalState::new(80, 24, 1);
        assert_eq!(state.seq, 1);
        state.increment_seq();
        assert_eq!(state.seq, 2);
        state.increment_seq();
        assert_eq!(state.seq, 3);
    }

    #[test]
    fn has_changed_detects_seq_difference() {
        let state1 = TerminalState::new(80, 24, 1);
        let mut state2 = TerminalState::new(80, 24, 2);
        assert!(state1.has_changed(&state2));
        state2.seq = 1;
        assert!(!state1.has_changed(&state2));
    }

    #[test]
    fn cursor_distance_computes_euclidean_metric() {
        let mut state1 = TerminalState::new(80, 24, 1);
        state1.update_cursor(Some(Cursor {
            x: 0,
            y: 0,
            visible: true,
        }));
        let mut state2 = TerminalState::new(80, 24, 2);
        state2.update_cursor(Some(Cursor {
            x: 3,
            y: 4,
            visible: true,
        }));
        let distance = state1.cursor_distance(&state2);
        assert_eq!(distance, 5); // 3-4-5 right triangle
    }

    #[test]
    fn round_trip_serialization() {
        let mut state = TerminalState::new(80, 24, 1);
        state.cells.push(Cell {
            col: 0,
            row: 0,
            text: "test".to_string(),
            width: 2,
            selected: false,
        });
        state.update_cursor(Some(Cursor {
            x: 10,
            y: 5,
            visible: true,
        }));
        state.update_scrollback(
            vec![ScrollLine {
                text: "history".to_string(),
                cells: vec![],
            }],
            10,
        );

        let json = serde_json::to_string(&state).expect("serialize");
        let decoded: TerminalState = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(decoded, state);
    }

    #[test]
    fn shell_state_serde() {
        let shell = ShellState {
            shell_pid: 5678,
            shell_name: "zsh".to_string(),
            jobs: vec!["fg_job".to_string(), "bg_job".to_string()],
            in_copy_mode: true,
        };
        let json = serde_json::to_string(&shell).expect("serialize");
        let decoded: ShellState = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(decoded, shell);
    }

    #[test]
    fn pending_command_serde() {
        let cmd = PendingCommand {
            command: "ls -la".to_string(),
            seq_sent: 100,
            seq_completed: Some(105),
        };
        let json = serde_json::to_string(&cmd).expect("serialize");
        let decoded: PendingCommand = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(decoded, cmd);
    }
}
