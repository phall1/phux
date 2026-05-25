//! The [`Registry`] — single source of truth for sessions, windows, and panes.
//!
//! All domain entities live in [`slotmap::SlotMap`]s keyed by the typed IDs
//! from [`crate::ids`]. The registry preserves parent → child invariants:
//!
//! * Removing a [`Pane`] removes it from its parent [`Window`]'s `panes`
//!   list and collapses it out of the layout tree.
//! * Removing a [`Window`] cascades to all of its panes and unlinks the
//!   window from its parent [`Session`].
//! * Removing a [`Session`] cascades fully to every window and pane it owns.
//!
//! Lookups by an unknown (e.g. removed) key return `None`. Mutating calls
//! that reference an unknown parent return [`RegistryError`].

use std::path::PathBuf;
use std::time::SystemTime;

use slotmap::SlotMap;
use thiserror::Error;

use crate::ids::{PaneId, SessionId, WindowId};
use crate::pane::Pane;
use crate::session::Session;
use crate::window::{SplitDir, Window};

/// Errors returned by the [`Registry`] when a parent ID does not resolve.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[allow(clippy::enum_variant_names)] // variants name the *kind* of ID — the shared `Unknown` prefix is the point.
pub enum RegistryError {
    /// The provided [`SessionId`] does not refer to a live session.
    #[error("unknown session id: {0:?}")]
    UnknownSession(SessionId),
    /// The provided [`WindowId`] does not refer to a live window.
    #[error("unknown window id: {0:?}")]
    UnknownWindow(WindowId),
    /// The provided [`PaneId`] does not refer to a live pane.
    #[error("unknown pane id: {0:?}")]
    UnknownPane(PaneId),
}

/// Owns every session, window, and pane in a running phux server.
///
/// The registry is single-threaded and synchronous; concurrent access is the
/// caller's responsibility (the server crate wraps it behind its actor /
/// event-loop boundary).
#[derive(Debug, Default)]
pub struct Registry {
    sessions: SlotMap<SessionId, Session>,
    windows: SlotMap<WindowId, Window>,
    panes: SlotMap<PaneId, Pane>,
}

impl Registry {
    /// Create an empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    // ---- creation ---------------------------------------------------------

    /// Insert a new session with the given name and return its ID.
    ///
    /// `created_at` is stamped with [`SystemTime::now`]. The new session has
    /// no windows and no active window.
    pub fn new_session(&mut self, name: String) -> SessionId {
        self.sessions.insert_with_key(|id| Session {
            id,
            name,
            windows: Vec::new(),
            active: None,
            created_at: SystemTime::now(),
        })
    }

    /// Insert a new window under `session` and return its ID.
    ///
    /// The new window is appended to the session's `windows` list. If the
    /// session previously had no active window, the new window becomes
    /// active.
    pub fn new_window(&mut self, session: SessionId) -> Result<WindowId, RegistryError> {
        if !self.sessions.contains_key(session) {
            return Err(RegistryError::UnknownSession(session));
        }
        let window_id = self.windows.insert_with_key(|id| Window {
            id,
            session,
            panes: Vec::new(),
            layout: None,
            active: None,
        });
        // Safe: existence checked above.
        if let Some(s) = self.sessions.get_mut(session) {
            s.windows.push(window_id);
            if s.active.is_none() {
                s.active = Some(window_id);
            }
        }
        Ok(window_id)
    }

    /// Insert a new pane under `window` and return its ID.
    ///
    /// The pane is appended to the window's `panes` list and inserted into
    /// the layout tree. If the window was empty the new pane becomes the
    /// sole [`Leaf`](crate::window::LayoutNode::Leaf); otherwise it is added
    /// by splitting the currently active pane horizontally at `0.5`
    /// (tmux-default behavior). If the window had no active pane, the new
    /// pane becomes active. Default dims are `(80, 24)`; cwd defaults to
    /// the empty path; title is `None`.
    pub fn new_pane(&mut self, window: WindowId) -> Result<PaneId, RegistryError> {
        if !self.windows.contains_key(window) {
            return Err(RegistryError::UnknownWindow(window));
        }
        let pane_id = self.panes.insert_with_key(|id| Pane {
            id,
            window,
            dims: (80, 24),
            cwd: PathBuf::new(),
            title: None,
        });
        if let Some(w) = self.windows.get_mut(window) {
            let target = w.active;
            w.panes.push(pane_id);
            match target {
                None => {
                    // Window was empty — seed the layout. This cannot fail
                    // because the layout is None here.
                    let _ = w.seed_layout(pane_id);
                    w.active = Some(pane_id);
                }
                Some(t) => {
                    // Split the active pane horizontally at the tmux default
                    // ratio. If the active pane is somehow not in the tree
                    // (shouldn't happen), fall back to seeding — but we
                    // intentionally swallow the error here rather than
                    // making `new_pane` fallible on layout grounds; the
                    // proptest invariants would catch any drift.
                    let _ = w.split(t, pane_id, SplitDir::Horizontal, 0.5);
                }
            }
        }
        Ok(pane_id)
    }

    // ---- removal ----------------------------------------------------------

    /// Remove a pane and unlink it from its parent window.
    ///
    /// Returns the removed [`Pane`] if it existed, otherwise `None`. The
    /// parent window's `panes`, layout tree, and `active` are all updated
    /// to drop the removed key. When the removed pane was the only leaf the
    /// window's `layout` is cleared (the window persists with no panes
    /// until [`Self::remove_window`] is called).
    pub fn remove_pane(&mut self, id: PaneId) -> Option<Pane> {
        let pane = self.panes.remove(id)?;
        if let Some(w) = self.windows.get_mut(pane.window) {
            w.panes.retain(|p| *p != id);
            // Collapse the layout. `LastPane` is fine — the layout becomes
            // None and the window can be removed by a subsequent call.
            let _ = w.kill_pane(id);
            if w.active == Some(id) {
                w.active = w.panes.first().copied();
            }
        }
        Some(pane)
    }

    /// Remove a window, cascading to all of its panes.
    ///
    /// Returns the removed [`Window`] if it existed. All panes that belonged
    /// to the window are removed from the pane map. The parent session's
    /// `windows` and `active` are updated.
    pub fn remove_window(&mut self, id: WindowId) -> Option<Window> {
        let window = self.windows.remove(id)?;
        for pane_id in &window.panes {
            let _ = self.panes.remove(*pane_id);
        }
        if let Some(s) = self.sessions.get_mut(window.session) {
            s.windows.retain(|w| *w != id);
            if s.active == Some(id) {
                s.active = s.windows.first().copied();
            }
        }
        Some(window)
    }

    /// Remove a session, cascading to all of its windows and their panes.
    ///
    /// Returns the removed [`Session`] if it existed.
    pub fn remove_session(&mut self, id: SessionId) -> Option<Session> {
        let session = self.sessions.remove(id)?;
        for window_id in &session.windows {
            if let Some(window) = self.windows.remove(*window_id) {
                for pane_id in &window.panes {
                    let _ = self.panes.remove(*pane_id);
                }
            }
        }
        Some(session)
    }

    // ---- lookups ----------------------------------------------------------

    /// Borrow a session by ID, or `None` if the ID is unknown.
    #[must_use]
    pub fn session(&self, id: SessionId) -> Option<&Session> {
        self.sessions.get(id)
    }

    /// Mutably borrow a session by ID, or `None` if the ID is unknown.
    #[must_use]
    pub fn session_mut(&mut self, id: SessionId) -> Option<&mut Session> {
        self.sessions.get_mut(id)
    }

    /// Borrow a window by ID, or `None` if the ID is unknown.
    #[must_use]
    pub fn window(&self, id: WindowId) -> Option<&Window> {
        self.windows.get(id)
    }

    /// Mutably borrow a window by ID, or `None` if the ID is unknown.
    #[must_use]
    pub fn window_mut(&mut self, id: WindowId) -> Option<&mut Window> {
        self.windows.get_mut(id)
    }

    /// Borrow a pane by ID, or `None` if the ID is unknown.
    #[must_use]
    pub fn pane(&self, id: PaneId) -> Option<&Pane> {
        self.panes.get(id)
    }

    /// Mutably borrow a pane by ID, or `None` if the ID is unknown.
    #[must_use]
    pub fn pane_mut(&mut self, id: PaneId) -> Option<&mut Pane> {
        self.panes.get_mut(id)
    }

    // ---- counts -----------------------------------------------------------

    /// Number of live sessions.
    #[must_use]
    pub fn session_count(&self) -> usize {
        self.sessions.len()
    }

    /// Number of live windows.
    #[must_use]
    pub fn window_count(&self) -> usize {
        self.windows.len()
    }

    /// Number of live panes.
    #[must_use]
    pub fn pane_count(&self) -> usize {
        self.panes.len()
    }
}
