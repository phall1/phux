//! Core domain types for phux.
//!
//! Defines sessions, windows, terminals, and the layout tree as pure data —
//! no I/O, no terminal emulation, no PTY handling. The server crate
//! composes these with libghostty-vt and PTY plumbing.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod ids;
pub mod registry;
pub mod screen;
pub mod session;
pub mod session_list;
pub mod terminal;
pub mod window;

pub use ids::{SessionId, TerminalId, WindowId};
pub use registry::{Registry, RegistryError};
pub use screen::{CursorState, SCHEMA_VERSION, ScreenState};
pub use session::Session;
pub use terminal::Terminal;
pub use window::{Direction, LayoutError, LayoutNode, Rect, SplitDir, Window};
