---
audience: contributors, agents
stability: evolving
last-reviewed: 2026-06-01
---

# Agent SDK API Design

**TL;DR.** The agent SDK speaks L1 + L2 Agent protocol via async typed Rust handles: `phux_client_sdk::connect()` returns a `Phux` connection, `.spawn()` returns a `Terminal` handle with methods to run commands, subscribe events, query state, and introspect grid. Error enum captures timeout, connection, protocol, and server failures. This doc is the API contract before implementation.

---

## 1. Error Enum

All fallible SDK operations return `phux_client_sdk::Error`, a tagged enum capturing failure modes agents need to handle:

```rust
#[derive(Debug)]
pub enum Error {
    /// Command execution exceeded its timeout.
    /// `what` describes what operation (e.g., "run_command", "wait_for_prompt").
    /// `elapsed_ms` is wall-clock milliseconds elapsed before timeout fired.
    TimeoutError { what: String, elapsed_ms: u64 },

    /// Connection to phux-server was lost (intentionally or by crash).
    /// `reason` is human-readable: "server closed connection", "connection reset", etc.
    ConnectionLost { reason: String },

    /// Wire protocol violation: framing error, unknown frame type, field mismatch.
    /// `what` describes the violation (e.g., "unknown frame type 0xff").
    ProtocolError { what: String },

    /// Agent sent an invalid command: bad terminal_id, invalid args, out-of-range signal.
    /// `what` describes the violation (e.g., "terminal_id LOCAL(999) not found").
    InvalidCommand { what: String },

    /// Server returned an application-level error (exit code, operation failure).
    /// `code` is a server-defined status code (e.g., -1 for "terminal closed").
    /// `message` is a server-provided explanation.
    ServerError { code: i32, message: String },

    /// Other internal errors: IO, serialization, async runtime issues.
    /// `reason` is a human-readable description.
    Internal { reason: String },
}

impl std::fmt::Display for Error { .. }
impl std::error::Error for Error { .. }
```

**Invariants agents can rely on:**
- `TimeoutError` is raised by SDK timeout enforcement, never propagated from server.
- `ConnectionLost` is terminal: the handle is no longer usable; reconnect via `phux_client_sdk::connect()`.
- `ProtocolError` indicates a bug in phux-server or the wire; report it.
- `InvalidCommand` is agent error; check your `TerminalId` and command shape.
- `ServerError.code` is server-defined; consult `docs/operations.md` for the code meanings.

---

## 2. Agent Trait and Core Types

Agents interact with phux via two main types: `Phux` (the connection) and `Terminal` (a handle to a spawned terminal).

### `phux_client_sdk::Phux` (connection handle)

```rust
pub struct Phux { /* opaque */ }

impl Phux {
    /// Spawn a new terminal and return a handle.
    /// The terminal runs `cmd` immediately if provided; otherwise waits for first command.
    /// `spawn_opts` includes command, cwd, environment, timeout.
    pub async fn spawn(&mut self, opts: SpawnOpts) -> Result<Terminal>;

    /// Attach to an existing terminal by id.
    /// Returns Err if the terminal does not exist on the server.
    pub async fn attach(&mut self, id: TerminalId) -> Result<Terminal>;

    /// List all terminals owned by this agent (or the user; see server config).
    pub async fn list_terminals(&mut self) -> Result<Vec<TerminalInfo>>;

    /// Close the connection to phux-server.
    /// Existing `Terminal` handles remain valid as long as they maintain
    /// their own internal connection state.
    pub async fn close(&mut self) -> Result<()>;
}
```

### `phux_client_sdk::Terminal` (terminal handle)

```rust
pub struct Terminal { /* opaque */ }

impl Terminal {
    /// Get the terminal's id.
    pub fn id(&self) -> TerminalId;

    /// Execute a command and capture output.
    /// Returns exit code, stdout, stderr, and a timestamp (ms since epoch).
    /// If `timeout_ms` elapses before the command completes, raises TimeoutError.
    /// If the terminal is closed, raises ConnectionLost.
    pub async fn run(
        &mut self,
        cmd: &str,
        timeout_ms: u64,
    ) -> Result<CommandOutput>;

    /// Wait for the shell to reach a prompt.
    /// Returns when `shell_state` becomes `AT_PROMPT`.
    /// If `timeout_ms` elapses, raises TimeoutError.
    /// Useful for sequencing commands: spawn → wait_for_prompt → run → wait_for_prompt.
    pub async fn wait_for_prompt(&mut self, timeout_ms: u64) -> Result<()>;

    /// Snapshot the terminal's current state: grid, cursor, scrollback, process info.
    /// Returns None if the terminal is closed.
    pub async fn get_state(&mut self) -> Result<TerminalState>;

    /// Subscribe to a stream of terminal events.
    /// Returns an async iterator over TerminalEvent.
    /// Use to react to command start/end, output, shell state changes, cwd changes.
    /// Subscription is live: returns events from the moment of subscription onward.
    /// Can subscribe multiple times; each subscription is independent.
    pub async fn subscribe_events(
        &mut self,
        types: &[EventType],
    ) -> Result<EventStream>;

    /// Send a UNIX signal to the shell or running command.
    /// `signal` is the signal number (e.g., libc::SIGINT = 2, libc::SIGTERM = 15).
    /// Returns Err if the terminal is closed or the signal delivery failed.
    pub async fn send_signal(&mut self, signal: i32) -> Result<()>;

    /// Query a rectangular region of the grid (or the entire grid if None).
    /// Returns a 2D grid of cells with codepoint, width, and attributes.
    /// Useful for parsing structured terminal output (e.g., finding a prompt).
    pub async fn query_grid(
        &mut self,
        rect: Option<GridRect>,
    ) -> Result<Grid>;

    /// Extract selected text from the grid.
    /// `format` is "plaintext" or "html".
    /// Only returns text if the server has an active selection; otherwise empty string.
    pub async fn extract_selection(
        &mut self,
        format: &str,
    ) -> Result<String>;

    /// Close the terminal (send KILL_TERMINAL to server).
    /// The terminal handle is no longer usable after this.
    pub async fn close(&mut self) -> Result<()>;
}
```

### Key Types

```rust
/// Options for spawning a terminal.
pub struct SpawnOpts {
    /// Command to run immediately (e.g., "bash", "cargo build").
    /// If None, the terminal starts with an interactive shell.
    pub command: Option<String>,
    /// Arguments to the command.
    pub args: Vec<String>,
    /// Working directory. Defaults to the server's cwd or $HOME.
    pub cwd: Option<String>,
    /// Environment variables to set (as KEY=VALUE).
    pub env: Vec<(String, String)>,
    /// Timeout for the spawn operation (ms).
    pub timeout_ms: u64,
}

/// Output captured from a command run.
pub struct CommandOutput {
    /// Shell exit code (0 = success, >0 = failure).
    pub exit_code: i32,
    /// Captured stdout (bytes).
    pub stdout: Vec<u8>,
    /// Captured stderr (bytes).
    pub stderr: Vec<u8>,
    /// Server timestamp when the output was finalized (ms since epoch).
    pub captured_at_ms: i64,
}

/// Snapshot of terminal state at a moment in time.
pub struct TerminalState {
    pub terminal_id: TerminalId,
    pub grid: GridState,
    pub scrollback: ScrollbackState,
    pub processes: ProcessInfo,
    pub shell_state: ShellState,
    pub timestamp_ms: i64,
    pub seq: u32,
}

pub struct GridState {
    pub cols: u16,
    pub rows: u16,
    pub cells: Vec<Vec<Cell>>,
    pub cursor: Cursor,
}

pub struct Cell {
    pub codepoint: u32,       // Unicode scalar value
    pub width: u8,             // 0 (combining), 1, or 2 (wide)
    pub attr: CellAttr,        // color, bold, italic, etc.
}

pub struct Cursor {
    pub col: u16,
    pub row: u16,
    pub hidden: bool,
}

pub struct GridRect {
    pub top: u16,
    pub left: u16,
    pub height: u16,
    pub width: u16,
}

pub struct Grid {
    pub cells: Vec<Vec<Cell>>,
    pub cols: u16,
    pub rows: u16,
}

pub struct ScrollbackState {
    pub lines: Vec<ScrollLine>,
    pub count_total: u32,
}

pub struct ScrollLine {
    pub cells: Vec<Cell>,
    pub timestamp_ms: i64,
}

pub struct ProcessInfo {
    pub shell_pid: u32,
    pub jobs: Vec<JobInfo>,
}

pub struct JobInfo {
    pub pid: u32,
    pub command: String,
    pub status: String,  // "running", "suspended", etc.
}

#[derive(Debug, Clone, Copy)]
pub enum ShellState {
    /// Shell is waiting for input.
    AwaitingInput { cwd: String, exit_code: Option<i32> },
    /// Shell is at the prompt (ready to accept commands).
    AtPrompt,
    /// Shell is executing a command.
    ExecutingCommand { pid: u32, command: String, started_at_ms: i64 },
    /// Shell is waiting for asynchronous output.
    AwaitingOutput { pid: u32, command: String },
}

pub struct TerminalInfo {
    pub id: TerminalId,
    pub shell_state: ShellState,
    pub cwd: String,
}

/// Events streamed from `subscribe_events()`.
pub enum TerminalEvent {
    ShellStateChanged {
        old_state: ShellState,
        new_state: ShellState,
        timestamp_ms: i64,
    },
    CommandStarted {
        pid: u32,
        command: String,
        args: Vec<String>,
        timestamp_ms: i64,
    },
    CommandEnded {
        pid: u32,
        exit_code: i32,
        output_bytes: u32,
        timestamp_ms: i64,
    },
    OutputReceived {
        semantic_type: OutputType,
        length: u32,
        snippet: Option<Vec<u8>>,  // first 256 bytes
        timestamp_ms: i64,
    },
    PromptReady {
        cwd: String,
        timestamp_ms: i64,
    },
    GridChanged {
        reason: GridChangeReason,
        rows_affected: Vec<u16>,
        timestamp_ms: i64,
    },
    CwdChanged {
        cwd: String,
        timestamp_ms: i64,
    },
}

pub struct EventStream { /* async iterator */ }

impl futures::stream::Stream for EventStream {
    type Item = TerminalEvent;
    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context) -> Poll<Option<Self::Item>> { .. }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventType {
    ShellStateChanged,
    CommandStarted,
    CommandEnded,
    OutputReceived,
    PromptReady,
    GridChanged,
    CwdChanged,
}

#[derive(Debug, Clone, Copy)]
pub enum OutputType {
    Unknown,
    Prompt,
    Error,
    Warning,
    Data,
    Semantic,
}

#[derive(Debug)]
pub enum GridChangeReason {
    Scroll,
    Output,
    Cursor,
    Clear,
}

/// Terminal identity, federation-ready.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TerminalId {
    /// Local terminal (v0.1 only).
    Local(u32),
    /// Remote terminal on a satellite (v0.2+).
    Satellite { host: String, id: u32 },
}
```

---

## 3. Client Constructor

```rust
pub async fn connect(addr: &str) -> Result<Phux>;
pub async fn connect_local() -> Result<Phux>;
pub async fn attach_or_spawn(session_name: &str, opts: SpawnOpts) -> Result<Terminal>;
```

**Semantics:**

- `connect(addr)` — connect to a phux-server at the given address (e.g., `/tmp/phux.sock`).
  Assumes UDS transport in v0.1; future transport via `Transport` trait.
  Returns `Err(ConnectionLost)` if the server is unreachable.

- `connect_local()` — convenience wrapper. Resolves the default local phux-server socket
  (typically `$XDG_RUNTIME_DIR/phux.sock` or `/tmp/phux-${UID}.sock`).
  Returns `Err(Internal)` if the socket cannot be found.

- `attach_or_spawn(session_name, opts)` — helper for agent workflows:
  - If a terminal named `session_name` exists, attach to it.
  - Otherwise, spawn a new terminal with the given `opts`.
  - Returns a `Terminal` handle ready to use.
  - Useful for idempotent agent operations.

---

## 4. Handler Implementation Sketch

The SDK is thin; phux-server does the work. Each SDK method maps to an L2 Agent protocol command:

```rust
// Terminal::run(cmd, timeout_ms)
// 1. Send RUN_COMMAND frame: { terminal_id, command, args, timeout_ms, capture_output: true }
// 2. Receive command_started event (fire and forget)
// 3. Loop: collect OUTPUT_RECEIVED events until command_ended
// 4. Return CommandOutput { exit_code, stdout, stderr, captured_at_ms }
// Timeout enforcement: if elapsed > timeout_ms, send SEND_SIGNAL(SIGKILL), raise TimeoutError

// Terminal::wait_for_prompt(timeout_ms)
// 1. Send SUBSCRIBE_TERMINAL_EVENTS { event_types: [SHELL_STATE_CHANGED] }
// 2. Poll for event with new_state == AT_PROMPT
// 3. Unsubscribe when matched
// Timeout enforcement: same as above

// Terminal::subscribe_events(types) -> EventStream
// 1. Send SUBSCRIBE_TERMINAL_EVENTS { event_types: types }
// 2. Return an async iterator that yields events until unsubscribed or connection closes
// 3. Multiplexed: multiple subscribe() calls share a single gRPC stream, demultiplexed by event type

// Terminal::get_state()
// 1. Send GET_TERMINAL_STATE { terminal_id, include_scrollback: true, max_scrollback_lines: 100 }
// 2. Receive TerminalState snapshot
// 3. Deserialize and return

// Phux::spawn(opts)
// 1. Allocate a new TerminalId (server assigns on SPAWN_TERMINAL, or we pre-allocate)
// 2. Send SPAWN_TERMINAL { command, args, cwd, env }
// 3. Wait for TERMINAL_SPAWNED { terminal_id, ... }
// 4. Return Terminal { id, connection: Arc<RwLock<GrpcChannel>>, .. }
```

**Connection pooling:** All `Terminal` handles from a single `Phux` share a gRPC channel.
Event subscriptions are multiplexed over the channel via a broadcast receiver per subscription.

**Graceful shutdown:**
- `Phux::close()` sends a final goodbye frame; subsequent operations on open `Terminal` handles will fail with `ConnectionLost`.
- `Terminal::close()` sends KILL_TERMINAL.

---

## 5. Key Invariants

Agents can rely on these invariants:

### Ordering and Causality

- **Commands are serialized per terminal.** If agent sends `run(A)`, then `run(B)`, command B does not start until A has completed (the shell is back at a prompt).
- **Events are ordered per terminal.** If `command_started(P1)` is emitted before `command_started(P2)`, then `command_ended(P1)` is emitted before `command_ended(P2)` for P1 != P2.
- **State snapshots are atomic.** `get_state()` returns the grid, cursor, shell state, and scrollback as they were at a single instant. No tearing between fields.

### Timeout Semantics

- **Timeouts are wall-clock, not operation-clock.** `run(cmd, timeout_ms=5000)` will timeout if the entire operation (including network latency) exceeds 5000 ms.
- **Timeout fires on the client.** The server is not consulted for timeout decisions; the SDK enforces locally.
- **Timeout does not kill the command.** The agent must explicitly call `send_signal(SIGKILL)` if it wants to stop the command. (Future: auto-kill on timeout if requested.)

### Connection Liveness

- **One connection per `Phux` handle.** All `Terminal` handles from the same `Phux` share one gRPC channel.
- **Subscriptions are live until unsubscribed or connection closes.** `subscribe_events()` is a fire-and-forget async iterator; closing the iterator does not close the connection.
- **Terminal handles survive `Phux::close()`.** If you close the connection, open `Terminal` handles will fail on the next operation with `ConnectionLost`.

### Shell State Tracking

- **ShellState is best-effort.** The server infers shell state from PTY bytes and OSC 133 escape sequences. Shells that don't emit OSC 133 will report `EXECUTING_COMMAND` conservatively.
- **`wait_for_prompt()` is heuristic-based.** It waits for ShellState to become `AT_PROMPT`, but some shells or custom prompts may never reach that state. Agents should have a fallback (e.g., `wait_for_prompt(timeout)` → fallback to polling grid).

### Output Capture

- **`run()` captures stdout and stderr together.** There is no separate capture; both are mixed in `CommandOutput.stdout`. (Future: separate streams.)
- **Large outputs are streamed.** If a command produces >1 MB of output, the SDK returns the data in chunks; `CommandOutput` is the complete final snapshot.
- **Output format is raw bytes.** Agents must decode (UTF-8, lines, etc.) themselves.

### Grid Introspection

- **Grid is relative to the viewport.** `query_grid()` returns only what is visible on screen (rows 0 to height-1). Scrollback is separate.
- **Grid is read-only.** There is no `set_grid_cell()` or `write_to_grid()`. Agents drive the terminal via `run()` or `send_signal()`.
- **Cell attributes are best-effort.** Colors, bold, italic, etc. are parsed from VT sequences and may not round-trip perfectly on all terminal types. Plaintext queries are reliable; styled queries may degrade.

### Concurrency

- **SDK methods are async but single-threaded per `Terminal`.** Calling `run()` and `subscribe_events()` concurrently on the same `Terminal` is safe (Rust's type system enforces `&mut`), but the results are serialized. Pipelines will be executed in order.
- **Multiple agents can use the same phux-server.** Each agent has its own `Phux` connection and `Terminal` handles.

---

## 6. Common Patterns

### Spawn and Wait

```rust
let mut phux = phux_client_sdk::connect_local().await?;
let mut term = phux.spawn(SpawnOpts {
    command: Some("cargo build".to_string()),
    args: vec!["--release".to_string()],
    cwd: Some(".".to_string()),
    env: vec![],
    timeout_ms: 60000,
}).await?;

term.wait_for_prompt(60000).await?;
println!("Build succeeded");
```

### Subscribe to Events

```rust
let mut events = term.subscribe_events(&[EventType::CommandEnded]).await?;

tokio::spawn(async move {
    while let Some(event) = events.next().await {
        if let TerminalEvent::CommandEnded { exit_code, .. } = event {
            println!("Command exited with {}", exit_code);
        }
    }
});

term.run("echo hello", 5000).await?;
```

### Query Grid

```rust
let state = term.get_state().await?;
println!("Cursor: ({}, {})", state.grid.cursor.col, state.grid.cursor.row);

for row in &state.grid.cells {
    for cell in row {
        print!("{}", char::from_u32(cell.codepoint).unwrap_or('?'));
    }
    println!();
}
```

### Handle Timeout

```rust
match term.run("sleep 10", 1000).await {
    Err(Error::TimeoutError { what, elapsed_ms }) => {
        eprintln!("Timeout: {} after {} ms", what, elapsed_ms);
        term.send_signal(libc::SIGTERM).await?;
    }
    Err(e) => eprintln!("Error: {}", e),
    Ok(output) => println!("Exit: {}", output.exit_code),
}
```

---

## 7. Serialization and Framing

The SDK uses gRPC + JSON for wire encoding (see L2_AGENT_PROTOCOL.md §6). Each RPC method maps to a gRPC service:

```proto
service PhuxAgent {
  rpc GetTerminalState(GetTerminalStateRequest)
    returns (TerminalState);
  rpc RunCommand(RunCommandRequest)
    returns (stream CommandOutputEvent);
  rpc SubscribeTerminalEvents(SubscribeRequest)
    returns (stream TerminalEvent);
  rpc SendSignal(SendSignalRequest)
    returns (SendSignalResponse);
  rpc SpawnTerminal(SpawnTerminalRequest)
    returns (SpawnTerminalResponse);
  rpc AttachTerminal(AttachTerminalRequest)
    returns (AttachTerminalResponse);
  rpc ListTerminals(ListTerminalsRequest)
    returns (ListTerminalsResponse);
  rpc KillTerminal(KillTerminalRequest)
    returns (KillTerminalResponse);
}
```

The Rust SDK generates bindings from the `.proto` file using `prost` and `tonic`. Agent implementations never touch protobuf directly.

---

## 8. Future Extensions

- **Bidirectional pipes:** `run(cmd)` with `stdin: Some(Arc<dyn AsyncRead>)` to send input mid-command.
- **Session recording:** `spawn(SpawnOpts { record_to: Some(path), .. })` to save all bytes to a file.
- **Satellite routing:** `attach(TerminalId::Satellite { host: "prod-1", id: 42 })` to drive remote terminals.
- **Custom event types:** user-defined event subscriptions (e.g., "emit when the prompt matches this regex").

---

## Implementation Roadmap

**v0.1 (current):**
- Error enum + Display impl
- Phux::spawn, Phux::attach, Terminal::run, Terminal::wait_for_prompt, Terminal::get_state
- Terminal::subscribe_events (read-only)
- Terminal::send_signal, Terminal::query_grid
- gRPC transport with UDS sockets

**v0.2:**
- Satellite routing (TerminalId::Satellite)
- Terminal::extract_selection
- Enhanced scrollback queries

**v0.3+:**
- Bidirectional pipes (stdin)
- Session recording
- Custom event filtering

