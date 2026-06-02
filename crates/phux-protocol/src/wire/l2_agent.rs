//! L2 Agent wire protocol discriminants.
//!
//! See `docs/spec/L2_AGENT_PROTOCOL.md` §6 for the semantic definitions.
//! These discriminants slot into the L2 Agent range (`0x70..=0x7E`)
//! reserved in Appendix B.

/// Discriminant for `GET_TERMINAL_STATE` (client to server, `docs/spec/L2_AGENT_PROTOCOL.md` §6).
///
/// Agent requests a snapshot of a Terminal's full state: grid, scrollback,
/// processes, shell state, and command history. The reply rides on
/// [`TYPE_L2_RESPONSE`] correlated by `request_id`.
pub const TYPE_GET_TERMINAL_STATE: u8 = 0x70;

/// Discriminant for `SUBSCRIBE_TERMINAL_EVENTS` (client to server, `docs/spec/L2_AGENT_PROTOCOL.md` §6).
///
/// Agent opts into a stream of typed `TerminalEvent`s for `terminal_id`.
/// Events flow as `L2_EVENT` frames (type `TYPE_L2_EVENT`) as they occur.
/// Unlike `SUBSCRIBE_METADATA` and `SUBSCRIBE_EVENTS`, this subscription
/// requires an active agent lifecycle — the server MAY reject if the client
/// is not in an agent-valid auth context (a future feature; v0.1 allows all).
pub const TYPE_SUBSCRIBE_TERMINAL_EVENTS: u8 = 0x71;

/// Discriminant for `L2_RESPONSE` (server to client, `docs/spec/L2_AGENT_PROTOCOL.md` §6).
///
/// Reply to a command that requests data: `GET_TERMINAL_STATE`, etc.
/// Correlated to the originating request by `request_id`. Carries a typed
/// result enum with success and error arms (shaped like `CommandResult`).
/// This is the L2 Agent counterpart to the generic control-plane
/// `COMMAND_RESULT` frame (`TYPE_COMMAND_RESULT`, `0xC2`).
pub const TYPE_L2_RESPONSE: u8 = 0x72;

/// Discriminant for `L2_EVENT` (server to client, `docs/spec/L2_AGENT_PROTOCOL.md` §6).
///
/// Server pushes one `TerminalEvent` to an agent that opted in via
/// `SUBSCRIBE_TERMINAL_EVENTS`. The event body is TLV-encoded (tag + length +
/// body) so older agents can skip unrecognised event kinds via an
/// `Unknown` variant rather than failing the parse. This is the L2 Agent
/// counterpart to the agent-surface `EVENT` frame (`TYPE_EVENT`, `0xB3`).
pub const TYPE_L2_EVENT: u8 = 0x73;
