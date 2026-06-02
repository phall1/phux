//! L2 Agent protocol layer.
//!
//! Per [`phux-protocol`] `docs/spec/L2_AGENT_PROTOCOL.md`, the L2 Agent tier
//! provides semantic events, structured state queries, and typed commands for
//! agent-driven automation.
//!
//! This module is re-exported from [`crate`] so consumers keep stable paths
//! like `phux_client::l2::TerminalEvent`.

pub mod commands;
pub mod events;
pub mod state;

pub use commands::{Command, EventType, GridRect, OutputFormat, SelectionFormat};
pub use events::{GridChangeReason, OutputType, TerminalEvent};
pub use state::{Cell, Cursor, PendingCommand, ScrollLine, ShellState, TerminalState};
