---
audience: agents, consumers, contributors
stability: scratch
last-reviewed: 2026-06-01
---

# L2 Agent Protocol Specification

**TL;DR.** The L2 agent protocol layers on L1 (Terminal substrate) to provide structured terminal state queries, semantic event streams, and agent-native command execution semantics. Wire is gRPC + JSON; agents speak only L1 + L2 Agent (skip L2 Collection and L3 metadata). The shape is designed to answer agent SDK patterns: spawn, observe, wait-for-semantic-events, extract output, drive via typed commands, tear down.

---

## 1. Design Principles

Per ADR-0015, agents are a consumer category distinct from the TUI:
- **TUI** speaks L1 + L2 Collection + L3 metadata; sees "sessions, windows, panes"
- **Agents** speak L1 + L2 Agent; never see sessions/windows (TUI-specific L3)
- No consumer is privileged on the wire (ADR-0017)
- Agent SDK is thin wrapper that maps agent-native patterns to L1 wire

**L2 Agent tier is OPTIONAL and SEPARATE from L2 Collection:**
- L2 Collection: named lifecycle bundles (sessions); scheduled for v0.2
- L2 Agent: semantic queries + structured events + typed command execution for agent-driven automation; scheduled for v0.3+

---

## 2. Semantic Model

**TerminalState** — what agents query via `GET_TERMINAL_STATE`:

```python
TerminalState {
  terminal_id: TerminalId
  
  # Grid state (libghostty)
  grid: {
    cols: u16
    rows: u16
    cells: [[Cell]]        # grid[row][col]
    cursor: Cursor         # {col, row, hidden: bool}
  }
  
  # Scrollback (retained lines above viewport)
  scrollback: {
    lines: [ScrollLine]    # most recent N lines
    count_total: u32       # total lines ever scrolled
  }
  
  # Process table (from PTY)
  processes: {
    shell_pid: u32         # controlling process of PTY
    jobs: [JobInfo]        # background jobs
  }
  
  # Shell state (parsed from OSC 133 + exit codes)
  shell_state: ShellState
  
  # Recent command (what was last issued)
  pending_command: optional<PendingCommand>
  
  # Metadata
  timestamp: i64           # server timestamp (ms since epoch)
  seq: u32                 # per-terminal output sequence number
}

ShellState = union {
  AWAITING_INPUT {
    cwd: str               # from OSC 7
    exit_code: optional<i32>
  }
  AT_PROMPT
  EXECUTING_COMMAND {
    pid: u32
    command: str
    started_at: i64
  }
  AWAITING_OUTPUT {
    pid: u32
    command: str
  }
}

PendingCommand {
  pid: u32
  command: str
  args: [str]
  semantic_type: OutputType
  issued_at: i64
  timeout_ms: u32
}

Cell {
  codepoint: u32         # Unicode scalar
  width: u8              # 0 (combining), 1, 2 (wide)
  attr: CellAttr         # color, bold, underline
}

Cursor {
  col: u16
  row: u16
  hidden: bool
}
```

---

## 3. Events — typed terminal state changes

Agents subscribe via `SUBSCRIBE_TERMINAL_EVENTS` and receive:

```python
TerminalEvent = union {
  SHELL_STATE_CHANGED {
    old_state: ShellState
    new_state: ShellState
    timestamp: i64
  }
  
  COMMAND_STARTED {
    terminal_id: TerminalId
    pid: u32
    command: str
    args: [str]
    timestamp: i64
  }
  
  COMMAND_ENDED {
    terminal_id: TerminalId
    pid: u32
    exit_code: i32
    timestamp: i64
    output_bytes: u32
  }
  
  OUTPUT_RECEIVED {
    terminal_id: TerminalId
    semantic_type: OutputType
    length: u32
    snippet: optional<bytes>  # first 256 bytes
    timestamp: i64
  }
  
  PROMPT_READY {
    terminal_id: TerminalId
    cwd: str
    timestamp: i64
  }
  
  GRID_CHANGED {
    terminal_id: TerminalId
    reason: "scroll" | "output" | "cursor" | "clear"
    rows_affected: [u16]
    timestamp: i64
  }
  
  CWD_CHANGED {
    terminal_id: TerminalId
    cwd: str
    timestamp: i64
  }
}

OutputType = enum {
  UNKNOWN = 0
  PROMPT = 1
  ERROR = 2
  WARNING = 3
  DATA = 4
  SEMANTIC = 5
}
```

---

## 4. Commands — agent actions

```python
Command_L2Agent = union {
  GET_TERMINAL_STATE {
    terminal_id: TerminalId
    include_scrollback: bool = true
    max_scrollback_lines: u16 = 100
  }
  
  QUERY_GRID {
    terminal_id: TerminalId
    rect: optional<GridRect>
  }
  
  RUN_COMMAND {
    terminal_id: TerminalId
    command: str
    args: optional<[str]>
    timeout_ms: u32
    capture_output: bool = true
    output_format: "raw" | "lines"
  }
  
  WAIT_FOR_PROMPT {
    terminal_id: TerminalId
    timeout_ms: u32
    max_wait_output: u32
  }
  
  SUBSCRIBE_TERMINAL_EVENTS {
    terminal_id: TerminalId
    event_types: [EventType]
  }
  
  SEND_SIGNAL {
    terminal_id: TerminalId
    signal: i32  # UNIX signal
  }
  
  EXTRACT_SELECTION {
    terminal_id: TerminalId
    format: "plaintext" | "html"
  }
}
```

---

## 5. Agent SDK Patterns

**Spawn and observe:**
```python
agent = await phux.attach_or_spawn("my-task")
state = await agent.get_state()

# Subscribe to command lifecycle
events = await agent.subscribe_events([COMMAND_STARTED, COMMAND_ENDED])
await agent.run_command("cargo build", timeout_ms=30000)

async for event in events:
    if event.type == "COMMAND_ENDED":
        print(f"Build exited with {event.exit_code}")
        break

# Wait for prompt before next command
await agent.wait_for_prompt(timeout_ms=30000)
await agent.run_command("echo done", timeout_ms=5000)
```

**Polling (no subscription):**
```python
while True:
    state = await agent.get_state()
    if state.shell_state == "AWAITING_INPUT":
        break
    await asyncio.sleep(0.1)
```

---

## 6. Wire Format

Uses same framing as L1:
- Frame: `[length: u32][type: u8][payload: bytes]`
- Type discriminants: allocated from `0x70..=0x7E` (reserved in `docs/spec/appendix-reserved.md`)
- Payload: field-tagged, extensible

### Message Types

Discriminants defined in `crates/phux-protocol/src/wire/l2_agent.rs` and re-exported via `crates/phux-protocol/src/wire/mod.rs`:

| Message | Constant | Value | Direction | Purpose |
|---------|----------|-------|-----------|---------|
| `GET_TERMINAL_STATE` | `TYPE_GET_TERMINAL_STATE` | `0x70` | C→S | Agent requests Terminal state snapshot |
| `SUBSCRIBE_TERMINAL_EVENTS` | `TYPE_SUBSCRIBE_TERMINAL_EVENTS` | `0x71` | C→S | Agent opts into typed event stream |
| `L2_RESPONSE` | `TYPE_L2_RESPONSE` | `0x72` | S→C | Reply to GET_TERMINAL_STATE / etc. (correlated by request_id) |
| `L2_EVENT` | `TYPE_L2_EVENT` | `0x73` | S→C | Streamed TerminalEvent (per SUBSCRIBE_TERMINAL_EVENTS) |

The three remaining slots (`0x74..=0x7E`) are reserved for future L2 Agent commands
(e.g., `RUN_COMMAND`, `WAIT_FOR_PROMPT`, `QUERY_GRID` — see §4).

**Handler Integration:** See `docs/architecture/l2-agent-handler-integration.md` for the server-side dispatch and event emission architecture.

### Alternative: gRPC + JSON

Agents MAY connect via gRPC instead of raw wire protocol:

```protobuf
service PhuxAgent {
  rpc GetTerminalState(GetTerminalStateRequest)
    returns (stream TerminalState);
  rpc RunCommand(RunCommandRequest)
    returns (stream CommandOutputEvent);
  rpc SubscribeTerminalEvents(SubscribeRequest)
    returns (stream TerminalEvent);
  rpc SendSignal(SendSignalRequest)
    returns (SendSignalResponse);
}
```

The wire-protocol path is the authoritative v0.1 implementation; gRPC is a
future convenience wrapper for agents running out-of-process.

---

## 7. Implementation

**phux-server must:**
1. Export TerminalState snapshots (query libghostty, process table, scrollback)
2. Emit typed TerminalEvents as things happen (via broadcast channels)
3. Implement gRPC service (or wire-protocol handler) for agent commands
4. Parse OSC 133 for semantic shell state tracking

**phux-mcp-server must:**
1. Connect to phux-server's L2 gRPC service
2. Expose MCP tools: `phux_attach`, `phux_run_command`, `phux_query_state`, `phux_subscribe`
3. Handle JSON marshaling, streaming, timeouts

---

## Examples

**Example 1: Compile and capture output**
```
Agent sends:
  RUN_COMMAND {
    terminal_id: LOCAL(1)
    command: "cargo build 2>&1"
    timeout_ms: 60000
    capture_output: true
  }

Server responds:
  {
    exit_code: 0
    stdout: "Compiling phux...\n[100%] Finished"
    lines: ["Compiling phux...", "[100%] Finished"]
  }
```

**Example 2: Subscribe and wait for command end**
```
Agent sends:
  SUBSCRIBE_TERMINAL_EVENTS {
    terminal_id: LOCAL(1)
    event_types: [COMMAND_STARTED, COMMAND_ENDED]
  }

Server streams:
  COMMAND_STARTED { pid: 1234, command: "cargo test", timestamp: ... }
  OUTPUT_RECEIVED { semantic_type: DATA, length: 256, ... }
  COMMAND_ENDED { exit_code: 0, output_bytes: 4096, ... }
```

---

## Future

- **Shell-specific parsing**: bash job control, zsh
- **Bidirectional pipes**: agent stdout → terminal stdin
- **Session recording**: save to file
- **Cross-machine routing**: via `TerminalId::Satellite`
