//! Opaque, type-distinct identifiers for the multiplexer's domain entities.
//!
//! Each ID is a [`slotmap`] key newtype. They are `Copy`, `Eq`, `Hash`, and
//! `Debug`, and the compiler rejects mixing one kind of ID for another — a
//! [`SessionId`] cannot be passed where a [`WindowId`] is expected.
//!
//! IDs are *opaque*: callers should treat them as cookies and never inspect
//! their bits. They are only meaningful relative to the [`Registry`] that
//! issued them.
//!
//! [`Registry`]: crate::registry::Registry

use slotmap::new_key_type;

new_key_type! {
    /// Identifies a session — the top-level container for windows.
    pub struct SessionId;

    /// Identifies a window — a tab-like container of panes within a session.
    pub struct WindowId;

    /// Identifies a pane — the leaf entity backed (eventually) by a PTY.
    pub struct PaneId;
}
