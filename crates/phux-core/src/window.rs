//! [`Window`] — a session's tab-like container of panes.

use crate::ids::{PaneId, SessionId, WindowId};

/// A window: an ordered collection of panes belonging to a session.
///
/// For `phux-byc.1` the layout is a placeholder linear list ([`LayoutNode`]).
/// `phux-byc.2` replaces it with a proper split tree (enum). Until then
/// callers should treat `panes` as the source of truth for which panes are in
/// this window; `layout` will mirror it.
#[derive(Debug, Clone)]
pub struct Window {
    /// The stable identifier issued by the [`Registry`].
    ///
    /// [`Registry`]: crate::registry::Registry
    pub id: WindowId,
    /// The session that owns this window.
    pub session: SessionId,
    /// Panes belonging to this window, in insertion order.
    pub panes: Vec<PaneId>,
    /// The pane layout. Placeholder list for byc.1; tree-typed in byc.2.
    pub layout: LayoutNode,
    /// The currently focused pane, if any.
    pub active: Option<PaneId>,
}

/// Placeholder layout container — a flat list of panes.
///
/// Replaced in `phux-byc.2` by a split-tree enum (`Leaf` / `Horizontal` /
/// `Vertical`). Kept here to fix the surface that downstream code references.
#[derive(Debug, Clone, Default)]
pub struct LayoutNode {
    /// Panes in display order.
    pub panes: Vec<PaneId>,
}
