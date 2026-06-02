//! L2 Agent Protocol command types for client-to-server requests.
//!
//! Implements the REQUEST types defined in `docs/spec/L2_AGENT_PROTOCOL.md` §4.
//! These commands enable agent-native patterns: spawn terminals, observe state,
//! subscribe to semantic events, run commands, and extract terminal output.
//!
//! All types derive `Serialize`, `Deserialize`, `Clone`, `Debug` for gRPC + JSON
//! wire serialization. `TerminalId` serializes as a tagged union.

use serde::{Deserialize, Serialize};

use phux_protocol::ids::TerminalId;

mod serde_terminal_id {
    use phux_protocol::ids::TerminalId;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub(super) fn serialize<S>(terminal_id: &TerminalId, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match terminal_id {
            TerminalId::Local { id } => {
                #[derive(Serialize)]
                struct Local {
                    id: u32,
                }
                #[derive(Serialize)]
                struct Wrapper {
                    #[serde(rename = "Local")]
                    local: Local,
                }
                Wrapper {
                    local: Local { id: *id },
                }
                .serialize(serializer)
            }
            TerminalId::Satellite { host, id } => {
                #[derive(Serialize)]
                struct Satellite {
                    host: String,
                    id: u32,
                }
                #[derive(Serialize)]
                struct Wrapper {
                    #[serde(rename = "Satellite")]
                    satellite: Satellite,
                }
                Wrapper {
                    satellite: Satellite {
                        host: host.to_string(),
                        id: *id,
                    },
                }
                .serialize(serializer)
            }
        }
    }

    pub(super) fn deserialize<'de, D>(deserializer: D) -> Result<TerminalId, D::Error>
    where
        D: Deserializer<'de>,
    {
        use serde::de::{self, Visitor};
        use std::fmt;

        struct TerminalIdVisitor;

        impl<'de> Visitor<'de> for TerminalIdVisitor {
            type Value = TerminalId;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a TerminalId (Local or Satellite)")
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: de::MapAccess<'de>,
            {
                if let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "Local" => {
                            #[derive(Deserialize)]
                            struct LocalData {
                                id: u32,
                            }
                            let local: LocalData = map.next_value()?;
                            Ok(TerminalId::local(local.id))
                        }
                        "Satellite" => {
                            #[derive(Deserialize)]
                            struct SatelliteData {
                                host: String,
                                id: u32,
                            }
                            let sat: SatelliteData = map.next_value()?;
                            Ok(TerminalId::satellite(sat.host, sat.id))
                        }
                        other => Err(de::Error::unknown_variant(other, &["Local", "Satellite"])),
                    }
                } else {
                    Err(de::Error::missing_field("Local or Satellite"))
                }
            }
        }

        deserializer.deserialize_map(TerminalIdVisitor)
    }
}

/// Event types that agents can subscribe to via `SubscribeTerminalEvents`.
///
/// Maps to `docs/spec/L2_AGENT_PROTOCOL.md` §3 `TerminalEvent` union variants.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum EventType {
    /// Shell state changed (awaiting input, at prompt, executing, or awaiting output).
    ShellStateChanged,
    /// A command was started on the terminal.
    CommandStarted,
    /// A command exited with a status code.
    CommandEnded,
    /// New output received from the terminal.
    OutputReceived,
    /// Shell prompt is ready for new input.
    PromptReady,
    /// Grid content changed (scroll, output, cursor, or clear).
    GridChanged,
    /// Current working directory changed.
    CwdChanged,
}

/// Output format for captured terminal output.
///
/// Used in `RunCommand.output_format` and determines how the server
/// serializes command output back to the client.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OutputFormat {
    /// Raw bytes as they were written to the terminal.
    Raw,
    /// Line-oriented output (each line separated).
    Lines,
}

/// Format for text extraction via `ExtractSelection`.
///
/// Determines the MIME type and encoding of the selected text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SelectionFormat {
    /// Plain text (no styling).
    Plaintext,
    /// HTML with ANSI color codes translated to HTML attributes.
    Html,
}

/// Grid rectangle specifying a region for `QueryGrid`.
///
/// All coordinates are inclusive and 0-based (top-left is row 0, col 0).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct GridRect {
    /// Starting row (0 = top of viewport).
    pub top: u16,
    /// Starting column (0 = leftmost).
    pub left: u16,
    /// Ending row (inclusive).
    pub bottom: u16,
    /// Ending column (inclusive).
    pub right: u16,
}

impl GridRect {
    /// Construct a grid rectangle from top-left and bottom-right corners.
    #[must_use]
    pub const fn new(top: u16, left: u16, bottom: u16, right: u16) -> Self {
        Self {
            top,
            left,
            bottom,
            right,
        }
    }
}

/// L2 Agent Protocol commands (client → server requests).
///
/// Implements the REQUEST types from `docs/spec/L2_AGENT_PROTOCOL.md` §4.
/// This is a tagged enum (union) sent to the server as a gRPC message or
/// wire frame. Discriminants are reserved at `0x70–0x7E` per SPEC §6.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload")]
pub enum Command {
    /// Request a snapshot of terminal state (grid, scrollback, processes, shell state).
    ///
    /// The server responds with a `TerminalState` JSON object or stream frame.
    /// Corresponds to `GET_TERMINAL_STATE` in the spec.
    #[serde(rename = "GetTerminalState")]
    GetTerminalState {
        /// Terminal to query.
        #[serde(
            serialize_with = "serde_terminal_id::serialize",
            deserialize_with = "serde_terminal_id::deserialize"
        )]
        terminal_id: TerminalId,
        /// Whether to include scrollback lines above the viewport.
        #[serde(default = "default_include_scrollback")]
        include_scrollback: bool,
        /// Maximum number of scrollback lines to return.
        #[serde(default = "default_max_scrollback_lines")]
        max_scrollback_lines: u16,
    },

    /// Query a rectangular region of the grid.
    ///
    /// Returns cell data (codepoints, widths, attributes) for the specified
    /// region. Corresponds to `QUERY_GRID` in the spec.
    #[serde(rename = "QueryGrid")]
    QueryGrid {
        /// Terminal to query.
        #[serde(
            serialize_with = "serde_terminal_id::serialize",
            deserialize_with = "serde_terminal_id::deserialize"
        )]
        terminal_id: TerminalId,
        /// Region to query; `None` returns the entire viewport.
        #[serde(skip_serializing_if = "Option::is_none")]
        rect: Option<GridRect>,
    },

    /// Run a command on the terminal and optionally capture its output.
    ///
    /// The server executes the command via the shell and streams output back.
    /// Corresponds to `RUN_COMMAND` in the spec.
    #[serde(rename = "RunCommand")]
    #[allow(clippy::enum_variant_names)]
    RunCommand {
        /// Terminal to run the command in.
        #[serde(
            serialize_with = "serde_terminal_id::serialize",
            deserialize_with = "serde_terminal_id::deserialize"
        )]
        terminal_id: TerminalId,
        /// Command name (e.g., `"cargo"`, `"ls"`).
        command: String,
        /// Command arguments (e.g., `["build", "--release"]`).
        #[serde(skip_serializing_if = "Option::is_none")]
        args: Option<Vec<String>>,
        /// Maximum time (milliseconds) to wait for the command to complete.
        timeout_ms: u32,
        /// Whether to capture stdout/stderr for return to the client.
        #[serde(default = "default_capture_output")]
        capture_output: bool,
        /// How to format captured output (`Raw` or `Lines`).
        #[serde(default = "default_output_format")]
        output_format: OutputFormat,
    },

    /// Wait for the shell to reach the prompt (awaiting input).
    ///
    /// Polls until the shell enters the prompt state or timeout expires.
    /// Useful for ensuring previous output has settled before running
    /// another command. Corresponds to `WAIT_FOR_PROMPT` in the spec.
    #[serde(rename = "WaitForPrompt")]
    WaitForPrompt {
        /// Terminal to monitor.
        #[serde(
            serialize_with = "serde_terminal_id::serialize",
            deserialize_with = "serde_terminal_id::deserialize"
        )]
        terminal_id: TerminalId,
        /// Maximum time (milliseconds) to wait.
        timeout_ms: u32,
        /// Maximum bytes of output to accumulate before returning (prevents
        /// unbounded buffering during long-running output).
        max_wait_output: u32,
    },

    /// Subscribe to a stream of typed terminal events.
    ///
    /// Opens a bidirectional stream; the server streams `TerminalEvent` objects
    /// as the terminal's state changes. Corresponds to `SUBSCRIBE_TERMINAL_EVENTS`
    /// in the spec.
    #[serde(rename = "SubscribeTerminalEvents")]
    SubscribeTerminalEvents {
        /// Terminal to monitor.
        #[serde(
            serialize_with = "serde_terminal_id::serialize",
            deserialize_with = "serde_terminal_id::deserialize"
        )]
        terminal_id: TerminalId,
        /// Which event types to forward (e.g., `[CommandStarted, CommandEnded]`).
        event_types: Vec<EventType>,
    },

    /// Send a UNIX signal to the terminal's controlling process.
    ///
    /// Equivalent to `kill -signal pid`. Corresponds to `SEND_SIGNAL` in the spec.
    #[serde(rename = "SendSignal")]
    SendSignal {
        /// Terminal whose process receives the signal.
        #[serde(
            serialize_with = "serde_terminal_id::serialize",
            deserialize_with = "serde_terminal_id::deserialize"
        )]
        terminal_id: TerminalId,
        /// UNIX signal number (e.g., `9` for SIGKILL, `15` for SIGTERM).
        signal: i32,
    },

    /// Extract the current text selection from the terminal.
    ///
    /// Used after a copy-mode interaction or OSC 52 sequence to retrieve
    /// the selected text. Corresponds to `EXTRACT_SELECTION` in the spec.
    #[serde(rename = "ExtractSelection")]
    ExtractSelection {
        /// Terminal from which to extract selection.
        #[serde(
            serialize_with = "serde_terminal_id::serialize",
            deserialize_with = "serde_terminal_id::deserialize"
        )]
        terminal_id: TerminalId,
        /// Format for the returned text (`Plaintext` or `Html`).
        format: SelectionFormat,
    },
}

// Serde defaults for optional fields with non-standard defaults.

const fn default_include_scrollback() -> bool {
    true
}

const fn default_max_scrollback_lines() -> u16 {
    100
}

const fn default_capture_output() -> bool {
    true
}

const fn default_output_format() -> OutputFormat {
    OutputFormat::Raw
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_terminal_state_serialize() {
        let cmd = Command::GetTerminalState {
            terminal_id: TerminalId::local(1),
            include_scrollback: true,
            max_scrollback_lines: 100,
        };
        let json = serde_json::to_string(&cmd).expect("serialize");
        assert!(json.contains("GetTerminalState"));
        assert!(json.contains("\"terminal_id\""));
    }

    #[test]
    fn test_run_command_serialize() {
        let cmd = Command::RunCommand {
            terminal_id: TerminalId::local(1),
            command: "cargo".to_owned(),
            args: Some(vec!["build".to_owned(), "--release".to_owned()]),
            timeout_ms: 60000,
            capture_output: true,
            output_format: OutputFormat::Lines,
        };
        let json = serde_json::to_string(&cmd).expect("serialize");
        assert!(json.contains("RunCommand"));
        assert!(json.contains("\"command\":\"cargo\""));
    }

    #[test]
    fn test_subscribe_deserialize() {
        let json = r#"{"type":"SubscribeTerminalEvents","payload":{"terminal_id":{"Local":{"id":1}},"event_types":["COMMAND_STARTED","COMMAND_ENDED"]}}"#;
        let _cmd: Command = serde_json::from_str(json).expect("deserialize");
    }

    #[test]
    fn test_event_type_roundtrip() {
        let event = EventType::ShellStateChanged;
        let json = serde_json::to_string(&event).expect("serialize");
        let deserialized: EventType = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(event, deserialized);
    }

    #[test]
    fn test_grid_rect_construction() {
        let rect = GridRect::new(0, 0, 23, 79);
        assert_eq!(rect.top, 0);
        assert_eq!(rect.bottom, 23);
        assert_eq!(rect.left, 0);
        assert_eq!(rect.right, 79);
    }
}
