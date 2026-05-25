//! [`Pane`] — the leaf entity that (eventually) backs a PTY.

use std::path::PathBuf;

use crate::ids::{PaneId, WindowId};

/// A pane: a single terminal-like surface within a window.
///
/// `phux-byc.1` defines the pane as pure data — no PTY, no grid, no async
/// state. Later epics attach the libghostty terminal and PTY plumbing on top
/// of this record, keyed by [`PaneId`].
#[derive(Debug, Clone)]
pub struct Pane {
    /// The stable identifier issued by the [`Registry`].
    ///
    /// [`Registry`]: crate::registry::Registry
    pub id: PaneId,
    /// The window that owns this pane.
    pub window: WindowId,
    /// Current pane dimensions in cells, `(cols, rows)`.
    pub dims: (u16, u16),
    /// Working directory the pane was (or will be) launched from.
    pub cwd: PathBuf,
    /// Optional human-set title, distinct from any title the shell may set.
    pub title: Option<String>,
}
