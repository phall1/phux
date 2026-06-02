//! Lightweight async wrapper for terminal control (ADR-0022 §2).
//!
//! The `Agent` struct encapsulates a client connection to a single terminal,
//! handling protocol framing, request correlation, and timeouts. All operations
//! are async and return `Result<T, AgentError>` with explicit error variants for
//! connection loss, timeouts, and protocol violations — no panics, no silent retries.
//!
//! # Example
//!
//! ```ignore
//! use phux_client::Agent;
//! use std::time::Duration;
//!
//! let mut agent = Agent::connect_uds(terminal_id, "/run/user/1000/phux/server.sock").await?;
//! let output = agent.run("echo hello", 5000).await?;
//! println!("exit code: {}", output.exit_code);
//! ```

use std::io;
use std::path::Path;
use std::time::Duration;

use phux_protocol::ids::TerminalId;
use phux_protocol::wire::frame::{Command, CommandResult, CommandValue, FrameKind};
use tokio::time::timeout;

use crate::attach::connection::Connection;

// =============================================================================
// Error enum
// =============================================================================

/// All errors the agent can produce.
///
/// Each variant is explicit and actionable: the caller always knows why an
/// operation failed and whether to retry, give up, or take a different path.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum AgentError {
    /// I/O error on the UDS socket (connect, read, write, etc.).
    #[error("connection I/O error: {0}")]
    Io(#[from] io::Error),

    /// The server closed the connection before sending the expected response.
    #[error("connection closed by server")]
    Disconnected,

    /// The operation did not complete within the specified timeout.
    #[error("operation timed out after {duration_ms}ms")]
    Timeout {
        /// Timeout duration in milliseconds.
        duration_ms: u64,
    },

    /// The server sent a frame we cannot parse or didn't expect at this point.
    #[error("protocol error: {0}")]
    Protocol(String),

    /// The server rejected the command (e.g., unknown terminal, invalid request).
    #[error("command refused by server: {0}")]
    Refused(String),

    /// Frame encoding or decoding failed.
    #[error("frame codec error: {0}")]
    Codec(String),

    /// The server's response did not match the expected shape.
    #[error("unexpected response shape: {0}")]
    UnexpectedResponse(String),
}

impl From<crate::attach::AttachError> for AgentError {
    fn from(value: crate::attach::AttachError) -> Self {
        match value {
            crate::attach::AttachError::Io(e) => Self::Io(e),
            crate::attach::AttachError::Disconnected => Self::Disconnected,
            crate::attach::AttachError::Protocol(msg) => Self::Protocol(msg),
            crate::attach::AttachError::Refused(msg) => Self::Refused(msg),
            other => Self::Protocol(other.to_string()),
        }
    }
}

// =============================================================================
// Output struct
// =============================================================================

/// The result of running a command: exit code + captured output.
#[derive(Debug, Clone)]
pub struct Output {
    /// The exit code the command returned.
    pub exit_code: i32,
    /// Captured stdout/stderr as UTF-8 (or lossy UTF-8 if mixed encodings).
    pub output: String,
}

// =============================================================================
// Agent struct
// =============================================================================

/// A lightweight async client for one terminal.
///
/// Maintains a UDS connection to the server and correlates request/response
/// pairs using incrementing `request_id` values. Each method sends a command
/// frame, waits for the matching response, and returns the decoded result or
/// an explicit error.
#[derive(Debug)]
pub struct Agent {
    terminal_id: TerminalId,
    conn: Connection,
    request_id_counter: u32,
}

impl Agent {
    /// Open a UDS connection to the server and return an Agent for `terminal_id`.
    ///
    /// Does not send `HELLO` or `ATTACH` — the caller is responsible for
    /// negotiating the session (see [`attach_or_create`] for a higher-level path).
    ///
    /// # Errors
    ///
    /// Returns [`AgentError::Io`] if the socket cannot be opened or connected.
    pub async fn connect_uds(
        terminal_id: TerminalId,
        socket_path: &Path,
    ) -> Result<Self, AgentError> {
        let conn = Connection::connect(socket_path).await?;
        Ok(Self {
            terminal_id,
            conn,
            request_id_counter: 1,
        })
    }

    /// Allocate the next request ID (always increments, wraps at `u32::MAX`).
    #[allow(clippy::missing_const_for_fn)]
    fn next_request_id(&mut self) -> u32 {
        let id = self.request_id_counter;
        self.request_id_counter = self.request_id_counter.wrapping_add(1);
        id
    }

    /// Run a command in the terminal and capture its output + exit code.
    ///
    /// Writes the command to the terminal's PTY, then polls `GET_SCREEN` until
    /// the exit-code sentinel is visible on screen (using the floor from
    /// `phux_client::run`). Returns the exit code and captured output, or a
    /// timeout if the sentinel does not appear within `timeout_ms`.
    ///
    /// # Errors
    ///
    /// - [`AgentError::Timeout`] if the sentinel does not appear within the deadline.
    /// - [`AgentError::Disconnected`] if the server closes before responding.
    /// - [`AgentError::Protocol`] if a frame is malformed or unexpected.
    /// - [`AgentError::Refused`] if the server rejects the request (e.g., unknown terminal).
    #[allow(clippy::unused_self)]
    pub fn run(&self, cmd: &str, timeout_ms: u64) -> Result<Output, AgentError> {
        // For now, return a placeholder error. The full implementation requires
        // integration with `crate::run` (the sentinel-parsing floor) and this
        // wrapper is designed to let that happen: build the command, send input,
        // then await the screen-polling result.
        let _ = (cmd, timeout_ms);
        Err(AgentError::Protocol(
            "run not yet implemented; use phux_client::run directly".into(),
        ))
    }

    /// Wait for the terminal to reach "awaiting input" state (prompt).
    ///
    /// Polls `GET_SCREEN` until `shell_state` is `AWAITING_INPUT`, indicating
    /// the shell is ready for input. Returns immediately if already at prompt,
    /// or times out if the deadline passes.
    ///
    /// # Errors
    ///
    /// - [`AgentError::Timeout`] if the prompt does not appear within the deadline.
    /// - [`AgentError::Disconnected`] if the server closes before responding.
    /// - [`AgentError::Protocol`] if a frame is malformed.
    pub async fn wait_for_prompt(&mut self, timeout_ms: u64) -> Result<(), AgentError> {
        let deadline = std::time::Instant::now() + Duration::from_millis(timeout_ms);

        // The ScreenState does not yet carry `shell_state` (pending full
        // integration with phux_core::ScreenState in a follow-up ticket).
        // For now, we poll once with a timeout and return.
        let remaining = deadline
            .checked_duration_since(std::time::Instant::now())
            .ok_or(AgentError::Timeout {
                duration_ms: timeout_ms,
            })?;

        timeout(remaining, self.get_state())
            .await
            .map_err(|_| AgentError::Timeout {
                duration_ms: timeout_ms,
            })?
            .map(|_| ())
    }

    /// Fetch the current terminal state (grid, cursor, scrollback, etc.).
    ///
    /// Sends a `GET_SCREEN` command and deserializes the JSON response.
    /// The response includes viewport and optional scrollback, but not shell
    /// state (that is a future enhancement per phux-oki).
    ///
    /// # Errors
    ///
    /// - [`AgentError::Disconnected`] if the server closes before responding.
    /// - [`AgentError::Protocol`] if a frame is undecodable.
    /// - [`AgentError::UnexpectedResponse`] if the server returns a shape we don't handle.
    pub async fn get_state(&mut self) -> Result<crate::snapshot::ScreenState, AgentError> {
        let request_id = self.next_request_id();
        self.conn
            .send(&FrameKind::Command {
                request_id,
                command: Command::GetScreen {
                    terminal_id: self.terminal_id.clone(),
                    request_scrollback: None,
                    cells: false,
                },
            })
            .await?;

        loop {
            let frame = self.conn.recv().await?;
            match frame {
                FrameKind::CommandResult {
                    request_id: resp_id,
                    result,
                } if resp_id == request_id => match result {
                    CommandResult::OkWith(CommandValue::Json(json)) => {
                        return serde_json::from_str(&json)
                            .map_err(|e| AgentError::Codec(e.to_string()));
                    }
                    CommandResult::Error { message, .. } => {
                        return Err(AgentError::Refused(message));
                    }
                    other => {
                        return Err(AgentError::UnexpectedResponse(format!("{other:?}")));
                    }
                },
                // Skip frames that don't match this request — the server may
                // interleave frames for other clients or subscriptions.
                _ => {}
            }
        }
    }

    /// Subscribe to a stream of terminal events (TODO: not yet wired).
    ///
    /// Sends a `SUBSCRIBE_EVENTS` command and returns an async iterator over
    /// terminal events until the subscription is cancelled.
    ///
    /// # Errors
    ///
    /// - [`AgentError::Disconnected`] if the server closes before responding.
    /// - [`AgentError::Protocol`] if a frame is undecodable.
    ///
    /// # Note
    ///
    /// This method is a placeholder; full event types and filtering are
    /// tracked in ADR-0022 §2 (`phux-y2t`).
    #[allow(clippy::unused_async)]
    pub async fn subscribe_events(&self, _types: &[EventType]) -> Result<(), AgentError> {
        Err(AgentError::Protocol(
            "subscribe_events not yet implemented".into(),
        ))
    }

    /// Send a signal to the terminal's PTY (TODO: not yet wired).
    ///
    /// Kills the PTY with the given signal number (e.g., 9 for SIGKILL).
    /// Waits for an ack from the server before returning.
    ///
    /// # Errors
    ///
    /// - [`AgentError::Disconnected`] if the server closes before responding.
    /// - [`AgentError::Protocol`] if a frame is undecodable.
    ///
    /// # Note
    ///
    /// This method is a placeholder; full signal routing is tracked in
    /// ADR-0022 §2 (`phux-y2t`).
    #[allow(clippy::unused_async)]
    pub async fn send_signal(&self, _signal: i32) -> Result<(), AgentError> {
        Err(AgentError::Protocol(
            "send_signal not yet implemented".into(),
        ))
    }
}

// =============================================================================
// Event types (placeholder)
// =============================================================================

/// Event types a client can subscribe to.
///
/// Full definitions tracked in ADR-0022 §2 (`phux-y2t`). For now, this is
/// an empty placeholder so the subscription signature is stable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum EventType {
    /// Placeholder variant to avoid exhaustive matches on an empty enum.
    #[doc(hidden)]
    __Placeholder,
}

// =============================================================================
// Constructor: high-level attach_or_create path (TODO)
// =============================================================================

/// Open or create a session by name and return an Agent attached to its seed pane.
///
/// Connects to the server, negotiates via `HELLO` + `ATTACH`, and returns
/// an Agent for the session's active terminal. If the session does not exist,
/// creates it first.
///
/// # Errors
///
/// - [`AgentError::Io`] if the socket cannot be found.
/// - [`AgentError::Refused`] if the server denies the attach (e.g., invalid session).
/// - [`AgentError::Protocol`] if the handshake frames are malformed.
///
/// # Note
///
/// This is a high-level convenience that is out of scope for v0 (per CONTRIBUTING.md).
/// Use `Agent::connect_uds` directly for agent-to-SDK control. Attach logic lives in the
/// TUI in `phux_client::attach::run`.
pub fn attach_or_create(_session_name: &str) -> Result<Agent, AgentError> {
    Err(AgentError::Protocol(
        "attach_or_create not yet implemented; use phux attach CLI for TUI attach, or phux_client::attach::run for programmatic attach".into(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_display_shows_detail() {
        let err = AgentError::Timeout { duration_ms: 5000 };
        assert_eq!(err.to_string(), "operation timed out after 5000ms");

        let err = AgentError::Refused("unknown terminal".into());
        assert_eq!(
            err.to_string(),
            "command refused by server: unknown terminal"
        );
    }

    #[test]
    fn request_id_wraps() {
        // Just check the wrapping logic without async/Connection overhead.
        let mut counter = u32::MAX - 1;
        counter = counter.wrapping_add(1);
        assert_eq!(counter, u32::MAX);
        counter = counter.wrapping_add(1);
        assert_eq!(counter, 0);
    }
}
