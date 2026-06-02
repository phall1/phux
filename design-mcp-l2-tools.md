---
audience: agents, contributors
stability: scratch
last-reviewed: 2026-06-01
---

# MCP Server Interface Design for phux L2

**TL;DR.** MCP (Model Context Protocol) server design sketch for agent control of phux terminals. Defines 5 tool signatures that bridge Claude/Anthropic SDK agents to the phux wire protocol (L1 + L2), shows the connection topology, and specifies error mapping. Design only; no implementation.

---

## 1. Tool Definitions

Each tool below carries:
- **Signature**: parameter names, types, required vs optional
- **Expected response**: success shape + error conditions
- **Maps to**: corresponding wire `Command` variant or L2 operation

---

### 1.1. `phux_get_terminal_state`

Fetch the current state of a Terminal without attaching or resizing.

```
Tool name: phux_get_terminal_state
Description: Get screen content, dimensions, and metadata for a terminal

Parameters (JSON schema):
  terminal_id: string (required)
    - Format: "LOCAL:{u32}" or "SATELLITE:{host}:{u32}"
    - Example: "LOCAL:42", "SATELLITE:prod-box-3:42"

  include_scrollback: object (optional)
    - Type: one of:
      - null (viewport only, default)
      - { type: "all" } (all retained history)
      - { type: "lines", count: u32 } (most recent N lines)

  include_cell_semantics: boolean (optional, default: false)
    - When true, response includes per-cell OSC-133 semantic marks

Success response (JSON):
{
  "terminal_id": "LOCAL:42",
  "cols": 80,
  "rows": 24,
  "cursor_col": 15,
  "cursor_row": 8,
  "screen": [
    { "text": "$ ", "style": "normal" },
    { "text": "echo hello", "style": "bold" },
    ...
  ],
  "viewport": {
    "top_row": 0,
    "bottom_row": 23
  },
  "scrollback_lines": 1000,
  "is_running": true
}

Error responses (tool errors):
- terminal_not_found: terminal_id does not exist (maps to ErrorCode::TerminalNotFound)
- not_attached: client not attached to this terminal (maps to ErrorCode::NotAttached)
- malformed_terminal_id: terminal_id format invalid
- unsupported_satellite: SATELLITE id on non-federation server (ErrorCode::UnsupportedSatelliteRoute)

Wire mapping:
  COMMAND { GET_SCREEN { terminal_id, request_scrollback, cells } }
  → COMMAND_RESULT { Ok_With(Json(ScreenState)) } or Error
```

---

### 1.2. `phux_run_command`

Execute a command in a Terminal and optionally wait for completion.

```
Tool name: phux_run_command
Description: Send a command to a terminal and wait for output

Parameters (JSON schema):
  terminal_id: string (required)
    - Format: "LOCAL:{u32}" or "SATELLITE:{host}:{u32}"

  command: string (required)
    - Shell command to execute (e.g., "echo hello", "ls -la")

  args: array of strings (optional, default: [])
    - Additional arguments to pass (only used if structured exec is supported)
    - Currently ignored; command is sent as-is to shell

  timeout_ms: u32 (optional, default: 30000)
    - Wall-clock timeout in milliseconds
    - Fires a `SIGTERM` to the command's process group on timeout

Success response (JSON):
{
  "terminal_id": "LOCAL:42",
  "command_sent": "echo hello",
  "exit_status": 0,
  "output": {
    "lines": [
      "hello",
      ""
    ],
    "byte_count": 6
  },
  "wall_clock_ms": 145
}

Error responses (tool errors):
- terminal_not_found: terminal_id does not exist
- timeout: command did not finish within timeout_ms (partial output included)
- command_rejected: terminal is busy or read-only
- malformed_terminal_id: terminal_id format invalid

Wire mapping:
  1. COMMAND { ROUTE_INPUT { terminal_id, InputEvent::Key(...) } }
     → sends keystroke sequence for "command\n"
  2. (polling loop via phux_get_terminal_state + phux_wait_for_prompt)
  3. On timeout: COMMAND { KILL_TERMINAL { terminal_id } } (if escalation needed)
  Note: This tool uses L1 (routing structured input) + polling; pure L1 operation.
```

---

### 1.3. `phux_wait_for_prompt`

Block until a Terminal reaches a stable prompt state.

```
Tool name: phux_wait_for_prompt
Description: Wait for terminal to show a shell prompt (idle state)

Parameters (JSON schema):
  terminal_id: string (required)
    - Format: "LOCAL:{u32}" or "SATELLITE:{host}:{u32}"

  timeout_ms: u32 (optional, default: 60000)
    - Maximum time to wait before giving up

  prompt_pattern: string (optional)
    - Regex or literal string to match prompt (e.g., "$ ", "# ", "> ")
    - If omitted, uses heuristic (newline after non-control text)

Success response (JSON):
{
  "terminal_id": "LOCAL:42",
  "detected_prompt": "$ ",
  "wait_ms": 234,
  "stable": true,
  "line_count": 42
}

Error responses (tool errors):
- terminal_not_found: terminal_id does not exist
- timeout: prompt not detected within timeout_ms (returns last state)
- malformed_terminal_id: terminal_id format invalid

Wire mapping:
  (polling loop)
  1. Repeatedly: COMMAND { GET_SCREEN { terminal_id, ... } }
  2. Parse response for newline + non-whitespace pattern
  3. Sleep & retry until match or timeout
  Note: Pure L1 polling operation; no special wire frame.
```

---

### 1.4. `phux_subscribe_terminal_events`

Register for real-time event notifications from a Terminal.

```
Tool name: phux_subscribe_terminal_events
Description: Subscribe to terminal events (OSC 133 command markers, bell, etc)

Parameters (JSON schema):
  terminal_id: string (required)
    - Format: "LOCAL:{u32}" or "SATELLITE:{host}:{u32}"

  event_types: array of strings (required)
    - One or more of:
      - "osc_133_command_start" (OSC 133 ; 4 ; Pt ST)
      - "osc_133_command_end" (OSC 133 ; 0 ; Pt ST, exit status in Pt)
      - "bell" (terminal bell, BEL / ESC BEL)
      - "title_changed" (OSC 0 / OSC 2)
      - "cwd_changed" (OSC 7 / ITERM2)
      - "output" (any TERMINAL_OUTPUT)

  delivery_method: string (optional, default: "polling")
    - "polling" — tool returns immediately, caller polls for updates
    - "stream" — (future) WebSocket push to agent sandbox

Success response (JSON):
{
  "terminal_id": "LOCAL:42",
  "subscription_id": "sub_abc123def456",
  "events": [
    {
      "type": "osc_133_command_start",
      "timestamp": "2026-06-01T14:23:45.123Z",
      "payload": "echo test"
    },
    {
      "type": "osc_133_command_end",
      "timestamp": "2026-06-01T14:23:45.456Z",
      "exit_status": 0
    }
  ]
}

Error responses (tool errors):
- terminal_not_found: terminal_id does not exist
- invalid_event_type: one of event_types is unknown
- unsupported_delivery: delivery_method not available
- subscription_limit: too many active subscriptions on this terminal
- malformed_terminal_id: terminal_id format invalid

Wire mapping:
  COMMAND { SUBSCRIBE_EVENTS { terminal_id, event_types: [u8; ...] } }
  → S→C TERMINAL_EVENT frames (once subscribed, auto-pushed by server)
     or polling pattern: repeated GET_SCREEN with heuristic detection
  Note: Per SPEC §7.5 (partial), wire support exists for events; polling fallback.
```

---

### 1.5. `phux_send_signal`

Deliver a Unix signal to a Terminal's process group.

```
Tool name: phux_send_signal
Description: Send a signal to a terminal's running process

Parameters (JSON schema):
  terminal_id: string (required)
    - Format: "LOCAL:{u32}" or "SATELLITE:{host}:{u32}"

  signal: string (required)
    - One of:
      - "SIGTERM" (terminate gracefully, default)
      - "SIGKILL" (force terminate)
      - "SIGINT" (interrupt, Ctrl-C)
      - "SIGSTOP" (pause)
      - "SIGCONT" (resume)
      - (future: other signals as needed)

  wait_for_close: boolean (optional, default: false)
    - If true, block until TERMINAL_CLOSED; if false, return immediately

Success response (JSON):
{
  "terminal_id": "LOCAL:42",
  "signal_sent": "SIGTERM",
  "process_group": 12345,
  "terminal_closed": false,
  "wait_ms": 0
}

Error responses (tool errors):
- terminal_not_found: terminal_id does not exist
- permission_denied: signal not allowed for this client (maps to ErrorCode::PermissionDenied)
- invalid_signal: signal name unknown
- signal_failed: kernel rejected the signal (e.g., already exited)
- malformed_terminal_id: terminal_id format invalid

Wire mapping:
  For SIGTERM / SIGKILL (process termination):
    COMMAND { KILL_TERMINAL { terminal_id } }
    → S→C TERMINAL_CLOSED { terminal_id, exit_status }
  
  For other signals (future):
    (wire extension needed; not yet in SPEC)
    COMMAND { SEND_SIGNAL { terminal_id, signal: str } }
  
  Note: v0.1 supports kill (SIGTERM implied); other signals require L1 extension.
```

---

## 2. Tool → Wire Command Mapping

| MCP Tool | Wire Command(s) | Response Type | Notes |
|----------|-----------------|---------------|-------|
| `phux_get_terminal_state` | `GET_SCREEN` | `CommandResult::OkWith(Json)` | Snapshot; no attach/resize side effects |
| `phux_run_command` | `ROUTE_INPUT` + polling `GET_SCREEN` | Composite | Uses L1 input routing + state polling |
| `phux_wait_for_prompt` | `GET_SCREEN` (polling) | Composite | Pure polling; no wire command |
| `phux_subscribe_terminal_events` | `SUBSCRIBE_EVENTS` (future) or `GET_SCREEN` (polling) | `TERMINAL_EVENT` or JSON | v0.1: polling fallback; v0.2+: real events |
| `phux_send_signal` | `KILL_TERMINAL` (SIGTERM/SIGKILL) | `TERMINAL_CLOSED` | `KILL_TERMINAL` is the only signal path in v0.1 |

---

## 3. Connection Topology

```
┌─────────────────────────────────────────────────────────────────┐
│ Claude / Anthropic SDK Agent (in subprocess)                    │
│                                                                 │
│  [Tool use: phux_get_terminal_state("LOCAL:42")]                │
└─────────────────────────┬───────────────────────────────────────┘
                          │ (JSON-RPC)
                          │
        ┌─────────────────▼─────────────────┐
        │ MCP Server                        │
        │ (crates/phux-client/src/mcp/...) │
        │                                  │
        │  - JSON schema codec             │
        │  - Tool router                   │
        │  - Error handling                │
        │  - State polling loop (opt)      │
        └─────────────────┬─────────────────┘
                          │ (wire protocol frames)
                          │ (L1 + optional L2)
        ┌─────────────────▼────────────────────────────┐
        │ phux-client (UDS socket, wire codec)         │
        │                                              │
        │  - Frame encoder/decoder                    │
        │  - Command / CommandResult handlers         │
        │  - Polling state machine (wait_for_prompt)  │
        └─────────────────┬────────────────────────────┘
                          │ (wire frames)
                          │ (binary TLV)
        ┌─────────────────▼──────────────────────────────┐
        │ phux-server (UDS listener, tokio)             │
        │                                               │
        │  - Frame dispatch                           │
        │  - GET_SCREEN: walk Terminal, synthesize    │
        │  - ROUTE_INPUT: inject keystroke            │
        │  - KILL_TERMINAL: signal PTY                │
        │  - SUBSCRIBE_EVENTS: broadcast (v0.2+)      │
        └─────────────────┬──────────────────────────────┘
                          │ (PTY I/O)
        ┌─────────────────▼──────────────────────────────┐
        │ Shell / User Process (PTY)                    │
        │                                               │
        │  $ echo hello                                │
        │  $ <waits for input>                         │
        └────────────────────────────────────────────────┘
```

**Flow example: `phux_run_command("LOCAL:42", "echo test")`**

1. Agent calls MCP tool with JSON parameters
2. MCP server parses JSON → struct { terminal_id, command, timeout_ms }
3. MCP server converts to wire format: `COMMAND { ROUTE_INPUT { ... } }`
4. Frame sent via UDS to phux-client → forwarded to phux-server
5. Server injects keystroke bytes into PTY
6. Shell executes `echo test`, writes output to PTY
7. phux-server reads output, emits `TERMINAL_OUTPUT` frames
8. phux-client receives frames, updates local Terminal state
9. MCP server polls via `GET_SCREEN` (polling loop, internal)
10. Detects exit status / newline + prompt pattern
11. Returns JSON result to agent

**Connection initialization:**
- MCP server starts with default socket path (`~/.phux/default.sock`)
- Lazy connect on first tool call
- Reconnect on connection loss
- Graceful degradation (tool error if server unavailable)

---

## 4. Error Handling Strategy

### Error Sources → MCP Tool Errors

The wire carries structured `ErrorCode` values (u16 enum). MCP tools surface them as `{ error: string, code?: string, details?: object }`.

**Mapping table:**

| Wire ErrorCode | MCP Tool Error | HTTP-ish | Retry? |
|---|---|---|---|
| `VersionIncompatible` | `version_mismatch` | 400 Bad Request | No |
| `UnknownMessageType` | `internal_error` | 500 Internal | No |
| `MalformedMessage` | `internal_error` | 500 Internal | No |
| `FrameTooLarge` | `internal_error` | 500 Internal | No |
| `NotAttached` | `not_attached` | 400 Bad Request | No (client error) |
| `AlreadyAttached` | `already_attached` | 409 Conflict | No |
| `SessionNotFound` | `session_not_found` | 404 Not Found | No |
| `WindowNotFound` | `window_not_found` | 404 Not Found | No |
| `TerminalNotFound` | `terminal_not_found` | 404 Not Found | No |
| `ClientNotFound` | `client_not_found` | 404 Not Found | No |
| `UnsupportedSatelliteRoute` | `unsupported_satellite` | 400 Bad Request | No |
| `InvalidCommand` | `invalid_command` | 400 Bad Request | No |
| `PermissionDenied` | `permission_denied` | 403 Forbidden | No |
| `ResourceExhausted` | `resource_exhausted` | 503 Service Unavailable | Yes (transient) |
| `InternalError` | `server_error` | 500 Internal | Yes (transient) |

### MCP Tool Error Format

```json
{
  "error": {
    "message": "Terminal not found",
    "code": "terminal_not_found",
    "wire_code": 104,
    "details": {
      "terminal_id": "LOCAL:999",
      "reason": "requested terminal does not exist on server"
    }
  }
}
```

### Client-Side Error Handling (MCP Server Logic)

1. **Transient errors** (`ResourceExhausted`, `InternalError` on write):
   - Backoff + retry (up to 3 attempts)
   - Exponential backoff: 100ms, 200ms, 400ms
   - Log all retries

2. **Connection errors**:
   - UDS ENOENT → return `server_unavailable`
   - UDS permission denied → return `permission_denied`
   - Connection refused → backoff + reconnect

3. **Timeout**:
   - Polling loop exceeds `timeout_ms` → return `timeout` error
   - Includes partial state in response (current screen)

4. **Malformed responses**:
   - Decoder failure on wire frame → `internal_error`
   - JSON parse failure in `CommandResult` → `internal_error`

### Agent-Side Error Handling (User of MCP Tools)

Agents using these tools should:

```python
# Pseudocode
result = await mcp.call("phux_get_terminal_state", {
    "terminal_id": "LOCAL:42"
})

if result.error:
    code = result.error["code"]
    if code in ["terminal_not_found", "not_attached"]:
        # User/agent error; fail loudly
        raise Exception(f"Invalid request: {code}")
    elif code in ["resource_exhausted", "server_error"]:
        # Transient; caller can retry
        await asyncio.sleep(1)
        return await retry(...)
    else:
        # Other error; inspect details
        raise Exception(result.error["message"])
```

---

## 5. L2 Operations (Future, v0.2)

phux L2 (Collection lifecycle) is wire-reserved but not wire-allocated in v0.1. When L2 lands, the following tools extend naturally:

### Future: `phux_create_collection`
```
Parameters:
  name: string (optional)
  
Returns:
  collection_id: CollectionId { LOCAL { id: u32 } }

Wire: COMMAND { CREATE_COLLECTION { name } }
      → COMMAND_RESULT { OkWith(CollectionId) }
```

### Future: `phux_add_terminal_to_collection`
```
Parameters:
  collection_id: string ("LOCAL:1")
  terminal_id: string ("LOCAL:42")

Wire: COMMAND { ADD_TERMINAL_TO_COLLECTION { collection_id, terminal_id } }
      → COMMAND_RESULT { Ok }
```

### Future: `phux_kill_collection`
```
Parameters:
  collection_id: string

Semantics: kills all member Terminals atomically

Wire: COMMAND { KILL_COLLECTION { collection_id } }
      → S→C TERMINAL_CLOSED for each member, then COLLECTION_CLOSED
```

These tools are **not implemented** until the L2 wire frame allocations land. The MCP server can:
- Return `unsupported` error if called on a v0.1 server
- Probe `HELLO` capabilities to detect v0.2+
- Auto-enable tools once L2 is detected

---

## 6. Connection Configuration

MCP tools accept an optional **context hint** in the constructor (not shown above, but expected in the MCP server bootstrap):

```rust
// Sketch: phux-client/src/mcp/tools.rs module structure
pub struct ToolsConfig {
    /// UDS socket path for phux-server (default: ~/.phux/default.sock)
    pub socket_path: Option<PathBuf>,
    
    /// Polling interval for wait_for_prompt / subscribe_terminal_events (default: 100ms)
    pub poll_interval_ms: u32,
    
    /// Max retries on transient wire errors (default: 3)
    pub max_retries: u32,
    
    /// Connection timeout (default: 5 seconds)
    pub connect_timeout_ms: u32,
}

impl ToolsConfig {
    /// Create tools bound to the default socket
    pub fn new() -> Self { ... }
    
    /// Create tools bound to a custom socket
    pub fn with_socket(path: PathBuf) -> Self { ... }
}
```

---

## 7. Sketch: Module Structure

**Planned:** `crates/phux-client/src/mcp/`

```
mcp/
  mod.rs              — public tools::*, connection setup, config
  tools.rs            — tool definitions, JSON schema, wire mapping
  connection.rs       — UDS transport, frame I/O, retry logic (reuse from attach/)
  polling.rs          — wait_for_prompt, subscribe_terminal_events state machine
  error.rs            — ErrorCode → MCP tool error conversion
  codec.rs            — JSON ↔ tool parameter structs (serde)
```

**Interdependencies:**
- Reuse `phux_client::attach::connection::Connection` (UDS frame I/O)
- Reuse `phux_protocol::wire` codec
- New: polling state machine in `polling.rs`
- New: error mapping layer in `error.rs`

---

## 8. Design Decisions

### Why L1 only for v0.1?

L2 (Collection lifecycle) commands are wire-reserved but not wire-allocated until v0.2. Agents can spawn and manage individual Terminals (L1) today; grouping into Collections waits for the spec bump.

### Why polling for events?

SPEC §7.5 (`SUBSCRIBE_EVENTS`) is **partial** — the wire frame exists, but server-side broadcast is not implemented in v0.1. The MCP server falls back to polling `GET_SCREEN` and applying heuristics (newline detection, OSC 133 extraction from the Terminal state). This is "good enough" for `wait_for_prompt`; a real event stream (WebSocket push) arrives in v0.2.

### Why no structured shell execution?

phux does not interpret shells. L1 `ROUTE_INPUT` sends raw keystrokes. Structured execution (parse argv, auto-quote, etc.) belongs in the agent SDK or agent code, not the wire. `phux_run_command` is a convenience for simple use cases; agents needing full control use `phux_send_keys` instead.

### Why `TerminalId` format as string?

MCP tools use JSON. Bare `{ tag: "LOCAL", id: 42 }` is also valid, but string format ("LOCAL:42") is more readable in agent logs and markdown docs. The MCP server accepts both; it parses the format on the server side.

### Why no wildcard selection?

Tools operate on explicit `terminal_id`. Querying all Terminals (list) uses `phux_get_terminal_state` with a special id ("ALL"?) — **future work**, not v0.1. This forces agents to be explicit about which Terminal they're controlling.

---

## 9. Security & Isolation Notes

- **No authentication** in v0.1 wire protocol. Process-level UDS ownership enforces access control.
- **MCP server runs in-process** with the agent. It has the same privileges as the agent.
- **UDS socket must be owned by the user.** phux-server rejects connections from other users (OS-level check).
- **No rate limiting** in this design sketch. Future: per-Terminal request budgets.
- **Signal delivery** (`phux_send_signal`) requires the Terminal's process group to be killable by the caller's uid. Kernel enforces; server reflects errors.

---

## 10. Success Criteria for Implementation

- [ ] Tool definitions parse & validate JSON schema
- [ ] Each tool maps to exactly one wire Command (or polling sequence)
- [ ] Error codes round-trip through wire ↔ MCP error format
- [ ] v0.1 server is unreachable → graceful `server_unavailable` error
- [ ] v0.1 server rejects L2 tools → return `unsupported` (or backoff, wire-probe for v0.2)
- [ ] Polling loops do not starve the event loop (async, bounded sleep)
- [ ] Tests cover happy path + all error codes
- [ ] Agent examples spawn terminal, run command, wait for prompt, extract output

---

## References

- `docs/CONCEPTS.md` — phux architecture overview
- `docs/spec/L1.md` — Terminal lifecycle, `GET_SCREEN`, `ROUTE_INPUT`, `KILL_TERMINAL`
- `docs/spec/L2.md` — Collection lifecycle (v0.2 wire-reserved, not allocated)
- `docs/spec/proto.md` — wire framing, `COMMAND`, `COMMAND_RESULT`, error handling
- `ADR-0016` — `TerminalId` as wire primary (federation-ready addressing)
- `ADR-0013` — libghostty bytes on the wire
- `crates/phux-protocol/src/wire/frame.rs` — `Command`, `CommandValue`, `ErrorCode` Rust types
