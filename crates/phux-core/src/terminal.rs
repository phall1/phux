//! [`Terminal`] — the leaf entity that (eventually) backs a PTY.

use std::path::PathBuf;

use crate::ids::{TerminalId, WindowId};

/// A terminal: a single terminal-like surface within a window.
///
/// `phux-byc.1` defines the terminal as pure data — no PTY, no grid, no async
/// state. Later epics attach the libghostty terminal and PTY plumbing on top
/// of this record, keyed by [`TerminalId`].
#[derive(Debug, Clone)]
pub struct Terminal {
    /// The stable identifier issued by the [`Registry`].
    ///
    /// [`Registry`]: crate::registry::Registry
    pub id: TerminalId,
    /// The window that owns this terminal.
    pub window: WindowId,
    /// Current terminal dimensions in cells, `(cols, rows)`.
    pub dims: (u16, u16),
    /// Working directory the terminal was (or will be) launched from.
    pub cwd: PathBuf,
    /// Optional human-set title, distinct from any title the shell may set.
    pub title: Option<String>,
}
