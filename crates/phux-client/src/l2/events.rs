//! L2 Agent protocol: terminal events.
//!
//! Per [`phux-protocol`] `docs/spec/L2_AGENT_PROTOCOL.md`, agents subscribe to
//! [`TerminalEvent`] streams to observe typed state changes on a terminal —
//! shell state transitions, command lifecycle, grid changes, output, and
//! working directory updates. Wire type discriminants reserved in `docs/spec/proto.md`.
//!
//! # Event types
//!
//! - [`TerminalEvent::ShellStateChanged`] — shell execution state transition
//! - [`TerminalEvent::CommandStarted`] — command execution began
//! - [`TerminalEvent::CommandEnded`] — command exited
//! - [`TerminalEvent::OutputReceived`] — terminal output bytes
//! - [`TerminalEvent::PromptReady`] — shell ready for input
//! - [`TerminalEvent::GridChanged`] — terminal grid mutation
//! - [`TerminalEvent::CwdChanged`] — working directory changed
//!
//! All variants carry `timestamp: i64` (milliseconds since epoch, server-side).

use serde::{Deserialize, Serialize};

/// Semantic classification of terminal output.
///
/// Agents use this to understand what kind of data they received — whether
/// it's a shell prompt, error text, application output, or semantically-tagged
/// output (via OSC 133 or similar).
///
/// Wire representation: u8 discriminant (0–5).
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum OutputType {
    /// Unclassified output; type unknown or not yet determined.
    Unknown = 0,
    /// Shell prompt or ready signal.
    Prompt = 1,
    /// Error message (e.g., from command exit code or stderr marker).
    Error = 2,
    /// Warning message.
    Warning = 3,
    /// Application data (stdout, command results).
    Data = 4,
    /// Semantically-tagged output (OSC 133 markers, structured format).
    Semantic = 5,
}

impl OutputType {
    /// Convert to wire discriminant.
    #[must_use]
    pub const fn to_u8(self) -> u8 {
        self as u8
    }

    /// Build from wire discriminant; `None` if unknown.
    #[must_use]
    pub const fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::Unknown),
            1 => Some(Self::Prompt),
            2 => Some(Self::Error),
            3 => Some(Self::Warning),
            4 => Some(Self::Data),
            5 => Some(Self::Semantic),
            _ => None,
        }
    }
}

impl TryFrom<u8> for OutputType {
    type Error = u8;
    fn try_from(v: u8) -> Result<Self, u8> {
        Self::from_u8(v).ok_or(v)
    }
}

impl std::fmt::Display for OutputType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unknown => write!(f, "Unknown"),
            Self::Prompt => write!(f, "Prompt"),
            Self::Error => write!(f, "Error"),
            Self::Warning => write!(f, "Warning"),
            Self::Data => write!(f, "Data"),
            Self::Semantic => write!(f, "Semantic"),
        }
    }
}

/// Reason code for grid mutations.
///
/// Wire representation: string discriminant to match agent-facing semantics
/// in `L2_AGENT_PROTOCOL.md` §3.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum GridChangeReason {
    /// Scrollback shifted; lines scrolled out of viewport.
    Scroll,
    /// New output written to grid cells.
    Output,
    /// Cursor moved.
    Cursor,
    /// Grid cleared (e.g., `\x1b[2J`).
    Clear,
}

impl std::fmt::Display for GridChangeReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Scroll => write!(f, "scroll"),
            Self::Output => write!(f, "output"),
            Self::Cursor => write!(f, "cursor"),
            Self::Clear => write!(f, "clear"),
        }
    }
}

/// Union of all terminal state change events.
///
/// Agents subscribe via `SUBSCRIBE_TERMINAL_EVENTS` and stream events from
/// the server as terminal state changes. Each variant is a specific class of
/// state transition: command lifecycle, output, grid changes, or shell state.
///
/// Per `docs/spec/L2_AGENT_PROTOCOL.md` §3, all variants include a
/// `timestamp: i64` field (milliseconds since server epoch).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum TerminalEvent {
    /// Shell execution state changed (e.g., idle → running → idle).
    ///
    /// Agents use this to react when the shell transitions between states,
    /// e.g., from `AWAITING_INPUT` (idle, at prompt) to
    /// `EXECUTING_COMMAND` (running). This is independent of `COMMAND_STARTED`
    /// and `COMMAND_ENDED` — it carries the prior and new shell states.
    ///
    /// # Fields
    /// - `old_state` — previous shell state (e.g., at prompt)
    /// - `new_state` — new shell state (e.g., running command)
    /// - `timestamp` — server timestamp in milliseconds since epoch
    #[serde(rename = "SHELL_STATE_CHANGED")]
    ShellStateChanged {
        /// Previous shell state (JSON: opaque field).
        old_state: serde_json::Value,
        /// New shell state (JSON: opaque field).
        new_state: serde_json::Value,
        /// Server timestamp (milliseconds since epoch).
        timestamp: i64,
    },

    /// A command started executing.
    ///
    /// Emitted when the shell launches a new process. Agents can track
    /// command invocation, collect the PID, and correlate with later
    /// `COMMAND_ENDED` events.
    ///
    /// # Fields
    /// - `terminal_id` — which terminal (wire format: string tag)
    /// - `pid` — process ID assigned by PTY
    /// - `command` — executable name or path
    /// - `args` — command-line arguments
    /// - `timestamp` — server timestamp in milliseconds since epoch
    #[serde(rename = "COMMAND_STARTED")]
    CommandStarted {
        /// Terminal identifier (serialized as string, e.g., "LOCAL(1)").
        terminal_id: String,
        /// Process ID.
        pid: u32,
        /// Command name or path.
        command: String,
        /// Command-line arguments (may be empty).
        args: Vec<String>,
        /// Server timestamp (milliseconds since epoch).
        timestamp: i64,
    },

    /// A command finished executing.
    ///
    /// Emitted when a process tracked by `COMMAND_STARTED` exits. Carries
    /// the exit code and byte count of output produced. Agents use this to
    /// await command completion and check for success/failure.
    ///
    /// # Fields
    /// - `terminal_id` — which terminal
    /// - `pid` — process ID that exited
    /// - `exit_code` — process exit status
    /// - `timestamp` — server timestamp in milliseconds since epoch
    /// - `output_bytes` — total bytes of output produced by the command
    #[serde(rename = "COMMAND_ENDED")]
    CommandEnded {
        /// Terminal identifier (serialized as string).
        terminal_id: String,
        /// Process ID that exited.
        pid: u32,
        /// Exit code (0 = success; non-zero = failure).
        exit_code: i32,
        /// Server timestamp (milliseconds since epoch).
        timestamp: i64,
        /// Total output bytes produced by this command.
        output_bytes: u32,
    },

    /// Terminal received output bytes.
    ///
    /// Emitted when the server forwards PTY output to clients. Can carry
    /// a snippet (first 256 bytes) of the output for lightweight inspection
    /// without streaming the entire buffer. Agents can use `semantic_type`
    /// to classify the content and decide whether to collect it.
    ///
    /// # Fields
    /// - `terminal_id` — which terminal
    /// - `semantic_type` — output classification (prompt, error, data, etc.)
    /// - `length` — total byte count (may exceed snippet length)
    /// - `snippet` — optional first 256 bytes (base64-encoded in JSON)
    /// - `timestamp` — server timestamp in milliseconds since epoch
    #[serde(rename = "OUTPUT_RECEIVED")]
    OutputReceived {
        /// Terminal identifier (serialized as string).
        terminal_id: String,
        /// Semantic classification of the output.
        semantic_type: OutputType,
        /// Total output length in bytes.
        length: u32,
        /// Optional snippet of first 256 bytes (base64-encoded in wire format).
        snippet: Option<Vec<u8>>,
        /// Server timestamp (milliseconds since epoch).
        timestamp: i64,
    },

    /// Shell is ready for input.
    ///
    /// Emitted when the shell reaches a prompt state and is ready to accept
    /// user input. Carries the current working directory. Agents use this to
    /// synchronize — "wait until the shell is ready before issuing the next
    /// command."
    ///
    /// # Fields
    /// - `terminal_id` — which terminal
    /// - `cwd` — current working directory (from OSC 7 or fallback)
    /// - `timestamp` — server timestamp in milliseconds since epoch
    #[serde(rename = "PROMPT_READY")]
    PromptReady {
        /// Terminal identifier (serialized as string).
        terminal_id: String,
        /// Current working directory.
        cwd: String,
        /// Server timestamp (milliseconds since epoch).
        timestamp: i64,
    },

    /// Terminal grid changed.
    ///
    /// Emitted when libghostty's grid state changes — output was written,
    /// the cursor moved, or the grid was cleared. The `reason` indicates
    /// the class of change, and `rows_affected` lists row indices (0-based,
    /// viewport-relative) that were modified. Agents monitoring screen
    /// content use this to know when to query the full state.
    ///
    /// # Fields
    /// - `terminal_id` — which terminal
    /// - `reason` — reason for the change (output, cursor, clear, scroll)
    /// - `rows_affected` — list of row indices modified (viewport-relative)
    /// - `timestamp` — server timestamp in milliseconds since epoch
    #[serde(rename = "GRID_CHANGED")]
    GridChanged {
        /// Terminal identifier (serialized as string).
        terminal_id: String,
        /// Reason for grid change.
        reason: GridChangeReason,
        /// Row indices affected (viewport-relative, 0-based).
        rows_affected: Vec<u16>,
        /// Server timestamp (milliseconds since epoch).
        timestamp: i64,
    },

    /// Working directory changed.
    ///
    /// Emitted when the shell's current working directory changes. Sourced
    /// from OSC 7 (standard `cd` hook) or fallback mechanism. Agents use this
    /// to track filesystem context across commands.
    ///
    /// # Fields
    /// - `terminal_id` — which terminal
    /// - `cwd` — new working directory path
    /// - `timestamp` — server timestamp in milliseconds since epoch
    #[serde(rename = "CWD_CHANGED")]
    CwdChanged {
        /// Terminal identifier (serialized as string).
        terminal_id: String,
        /// New working directory path.
        cwd: String,
        /// Server timestamp (milliseconds since epoch).
        timestamp: i64,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_type_round_trip() {
        for &typ in &[
            OutputType::Unknown,
            OutputType::Prompt,
            OutputType::Error,
            OutputType::Warning,
            OutputType::Data,
            OutputType::Semantic,
        ] {
            assert_eq!(OutputType::from_u8(typ.to_u8()), Some(typ));
        }
    }

    #[test]
    fn output_type_invalid() {
        assert_eq!(OutputType::from_u8(99), None);
    }

    #[test]
    fn terminal_event_serialize() {
        let event = TerminalEvent::CommandStarted {
            terminal_id: "LOCAL(1)".to_string(),
            pid: 1234,
            command: "cargo".to_string(),
            args: vec!["build".to_string()],
            timestamp: 1_234_567_890,
        };
        let json = serde_json::to_string(&event).expect("serialize");
        assert!(json.contains("COMMAND_STARTED"));
        assert!(json.contains("1234"));
    }

    #[test]
    fn terminal_event_deserialize() {
        let json = r#"{"type":"COMMAND_ENDED","data":{"terminal_id":"LOCAL(1)","pid":1234,"exit_code":0,"timestamp":1234567890,"output_bytes":256}}"#;
        let event: TerminalEvent = serde_json::from_str(json).expect("deserialize");
        match event {
            TerminalEvent::CommandEnded {
                exit_code,
                output_bytes,
                ..
            } => {
                assert_eq!(exit_code, 0);
                assert_eq!(output_bytes, 256);
            }
            _ => panic!("wrong variant"),
        }
    }
}
