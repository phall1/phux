//! Core domain types for phux.
//!
//! Defines sessions, windows, panes, and the layout tree as pure data —
//! no I/O, no terminal emulation, no PTY handling. The server crate
//! composes these with libghostty-vt and PTY plumbing.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod ids;
pub mod pane;
pub mod registry;
pub mod session;
pub mod window;

pub use ids::{PaneId, SessionId, WindowId};
pub use pane::Pane;
pub use registry::{Registry, RegistryError};
pub use session::Session;
pub use window::{Direction, LayoutError, LayoutNode, Rect, SplitDir, Window};
