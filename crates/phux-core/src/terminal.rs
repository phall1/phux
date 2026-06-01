//! [`TerminalDescriptor`] — the leaf descriptor for a terminal-like pane.

use std::path::PathBuf;

use crate::ids::{TerminalId, WindowId};

/// Descriptor for a single terminal-like surface within a window.
///
/// `phux-byc.1` defines this as pure data — no PTY, no grid, no async state.
/// The server attaches the libghostty terminal and PTY plumbing on top of this
/// record, keyed by [`TerminalId`].
#[derive(Debug, Clone)]
pub struct TerminalDescriptor {
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
