---
audience: consumers, contributors
stability: stable
last-reviewed: 2026-05-31
---

# Protocol 101: A Complete Session Walkthrough

**TL;DR.** A walkthrough of one complete phux session from HELLO to detach: what happens, what the wire looks like, and why it matters. Read this before diving into the reference specs (proto.md, L1.md, etc).

---

## The big picture

A phux session is simple: a client connects to a server, negotiates capabilities, attaches to a terminal (or creates one), receives a stream of VT bytes as the PTY emits them, sends keypresses and mouse events back, and eventually detaches. The entire wire flow is **asymmetric**: server sends bytes, client sends structured events.

This walkthrough follows one complete narrative from first byte to close.

---

## Step 1: HELLO negotiation

**What happens:** The client connects over a Unix socket (or SSH stdin/stdout). The first thing it does is declare its name and what it can speak.

```
Client sends (frame type 0x01):
  HELLO {
    versions: [{ min: 0.1.0, max: 0.1.0 }],
    client_caps: {
      layers: 0x01,              // L1 only (bit 0x01)
      color: TrueColor,
      kbd_protocols: 0x03,       // kitty + modifyOtherKeys
      mouse_protocols: 0x01,     // standard mouse
      hyperlinks: true,
      rendering: VtReplay,
    }
  }

Server replies (frame type 0x80):
  HELLO_OK {
    version: 0.1.0,
    server_caps: {
      layers: 0x07,              // L1 + L2 + L3
      features: 0x01,            // REATTACH_REPLAY enabled
      max_message_size: 16777216,
    },
    server_id: "phux-server-abc123"
  }
```

**Wire shape:** See [proto.md §6.1](./proto.md) for the full HELLO codec. The key pieces:
- Client's `versions` field lists semantic version ranges it supports. Server picks the highest match from both sides.
- `layers` is a bitset: `0x01` = L1 only, `0x07` = all three tiers. An agent SDK declares L1; a TUI declares L1+L2+L3.
- The client caps (color, keyboard, mouse) tell the server how to downregulate the output byte stream. For example, if the client says `Indexed256`, the server rewrites truecolor SGR codes to 256-color equivalents before sending.

**Why it matters:** Negotiation happens once. It defines the entire conversation's contract — version, capabilities, which message tiers will be used.

---

## Step 2: Attach to a terminal

**What happens:** After HELLO, the client chooses a terminal to watch. It can attach to an existing terminal or create one.

```
Client sends (frame type 0x02):
  ATTACH {
    target: CreateIfMissing {
      collection: CollectionId(1),
      command: None,             // use server's default shell
      cwd: None,                 // use server's default cwd
      env: None,                 // inherit server's environment
    }
  }

Server replies (frame type 0x81):
  ATTACHED {
    terminal_id: TerminalId::LOCAL(42),
    grid_size: { cols: 120, rows: 40 },
  }
```

**Wire shape:** [L1.md §state replay](./L1.md) defines ATTACH. The `target` is a tagged union:
- `AttachExisting { terminal_id: TerminalId }` — attach to a running terminal
- `CreateIfMissing { ... }` — create a new terminal if it doesn't exist, or attach to an existing one by identity

The server replies with the terminal's dimensions so the client knows the initial grid size.

**Why it matters:** This is where the client says "I want to see and control this terminal." The server allocates a subscription on its end and starts streaming output.

---

## Step 3: Receive initial state (snapshot)

**What happens:** Immediately after `ATTACHED`, the server sends the current grid state — the scrollback and viewport. This is the bootstrap payload.

```
Server sends (frame type 0x91):
  TERMINAL_SNAPSHOT {
    terminal_id: TerminalId::LOCAL(42),
    seq: 1,
    scrollback: bytes,           // all prior lines as VT bytes
    viewport: bytes,             // current grid as VT bytes
    cursor: { row: 10, col: 5 },
  }
```

**Wire shape:** [L1.md §snapshot](./L1.md) describes the full shape. The `seq` field is crucial: it's a monotonic counter, per-terminal. Every `TERMINAL_OUTPUT` increments it. The client acknowledges receipt by seq number, so the server knows what state the client has seen.

**Why it matters:** The snapshot gets the client's local libghostty Terminal up to speed. After this frame, the client's grid matches the server's canonical state.

---

## Step 4: Stream terminal output

**What happens:** Now the real work. Every time the PTY produces bytes, the server collects them (paced at a configurable refresh rate, default 60 Hz) and sends them to the client.

```
User types "ls" and presses Enter.

Server sends (frame type 0x90):
  TERMINAL_OUTPUT {
    terminal_id: TerminalId::LOCAL(42),
    seq: 2,
    bytes: b"ls\r\n"   // the raw bytes from the keyboard, echoed by PTY
  }

A moment later, the shell replies:

Server sends (frame type 0x90):
  TERMINAL_OUTPUT {
    terminal_id: TerminalId::LOCAL(42),
    seq: 3,
    bytes: b"Documents\r\nDownloads\r\n..." // directory listing
  }

Client parses these VT bytes into its local libghostty Terminal,
renders to screen, then acknowledges:

Client sends (frame type 0x21):
  FRAME_ACK {
    terminal_id: TerminalId::LOCAL(42),
    seq: 3   // "I've applied all output up to seq 3"
  }
```

**Wire shape:** [L1.md §frame model](./L1.md) and [proto.md §8.2](./proto.md). The bytes are raw VT — no re-encoding, no structuring. The server forwards them directly from the PTY. The monotonic `seq` allows the server to implement flow control: if a client falls behind (doesn't acknowledge seq N), the server can pause output and avoid memory explosion.

**Why it matters:** This is the hot path. The asymmetric design (VT bytes server→client, structured events client→server) means the server sends exactly what the PTY emitted, preserving every terminal feature. No re-encoding, no loss of fidelity.

---

## Step 5: Handle client input

**What happens:** The client sends a keystroke. The server reads it as a structured event, converts it back to VT bytes, and writes to the PTY.

```
User presses Ctrl+C.

Client sends (frame type 0x10):
  INPUT_KEY {
    terminal_id: TerminalId::LOCAL(42),
    event: {
      action: Press,
      key: KeyC,
      mods: { ctrl: true },
      text: Some("\x03"),        // the text representation (Ctrl+C = U+0003)
    }
  }

Server receives, looks up TerminalId(42), and writes the VT bytes
to its PTY encoder. The PTY process receives SIGINT or the byte,
depending on the terminal mode.

A moment later, the process exits and the shell prompt returns:

Server sends (frame type 0x90):
  TERMINAL_OUTPUT {
    terminal_id: TerminalId::LOCAL(42),
    seq: 4,
    bytes: b"^C\r\n$ "  // shell echoed the interrupt and returned prompt
  }
```

**Wire shape:** [input.md](./input.md) defines the input family: `INPUT_KEY`, `INPUT_MOUSE`, `INPUT_PASTE`, `INPUT_FOCUS`, `INPUT_RAW`. Each carries a `terminal_id` and a structured event. The server's libghostty-backed encoder converts the event to terminal-mode-aware VT bytes (application keypad mode, mouse protocol state, etc.) and writes to the PTY.

**Why it matters:** Structured input means the server faithfully transports modifier-rich key combinations, pixel-precise mouse events, IME composition, and Kitty keyboard protocol end-to-end. No ambiguity, no re-encoding loss.

---

## Step 6: Other notable events

**What happens:** The running process may emit terminal control sequences that the server parses and surfaces as structured events.

```
Process sets the window title via OSC 0:

Server parses the OSC sequence and sends (frame type 0xB1):
  TERMINAL_EVENT {
    terminal_id: TerminalId::LOCAL(42),
    event: {
      TITLE { title: "my-project — vim" }
    }
  }

Process rings the bell (BEL character):

Server sends (frame type 0xB0):
  BELL {
    terminal_id: TerminalId::LOCAL(42),
  }

Process reports its working directory via OSC 7:

Server sends (frame type 0xB1):
  TERMINAL_EVENT {
    terminal_id: TerminalId::LOCAL(42),
    event: {
      CURRENT_DIR { uri: "file:///Users/alice/workspace" }
    }
  }
```

**Wire shape:** [L1.md §1.2–1.3](./L1.md) define BELL and TERMINAL_EVENT. These are parsed once on the server (via libghostty) and never re-emitted as raw escape sequences to the client; the client gets structured data. This is load-bearing for agents and control planes — they can read "the current directory" without parsing escape sequences themselves.

**Why it matters:** Structured terminal events decouple clients from OSC parsing. An agent sees the title and cwd as fields, not as bytes to parse. A TUI consumer reads these events and updates its own metadata. A GUI consumer uses them to update the window title bar.

---

## Step 7: Detach

**What happens:** The user quits or switches clients. The client sends DETACH; the server acknowledges and closes.

```
Client sends (frame type 0x03):
  DETACH { }

Server replies (frame type 0x82):
  DETACHED {
    reason: REQUESTED,
    message: "detach acknowledged"
  }

Server closes the transport (TCP, UDS, or SSH pipe).
The terminal keeps running on the server.
Other clients can attach to the same terminal later.
```

**Wire shape:** [proto.md §7.2](./proto.md). The `reason` is an enum: `REQUESTED` (clean client detach), `SERVER_SHUTDOWN`, `SESSION_KILLED`, `REPLACED` (another client took exclusive attach), `PROTOCOL_ERROR`, `INTERNAL_ERROR`.

**Why it matters:** Detach is clean. The server does not kill the terminal; it just stops sending output to this client. The next client that attaches will receive the scrollback (if the server retains it) and pick up where the previous one left off.

---

## Putting it together

Here's the complete sequence as a timeline:

```
Client                              Server                    Terminal (PTY)
  |                                   |                           |
  |------- HELLO ------>              |                           |
  |                   <------- HELLO_OK                           |
  |                                   |                           |
  |------- ATTACH ------>             |                           |
  |                   <------ ATTACHED |                           |
  |                                   |                           |
  |              <----- TERMINAL_SNAPSHOT (scrollback + viewport) |
  |                                   |                           |
  |              <----- TERMINAL_OUTPUT (seq 2) -------- shell prompt
  |                                   |                           |
  |              user types "ls\n" ----->                         |
  |------- INPUT_KEY ------>          |------- write VT bytes --->
  |                                   |                      <---- echo "ls"
  |              <----- TERMINAL_OUTPUT (seq 3) -------- ls output
  |------- FRAME_ACK ------>          |                           |
  |                                   |                           |
  |              <----- TERMINAL_OUTPUT (seq 4) -------- prompt   |
  |------- FRAME_ACK ------>          |                           |
  |                                   |                           |
  |------- DETACH ------>             |                           |
  |                   <------ DETACHED |                           |
  |                                   | (terminal stays alive)    |
  X                                   |                           |
```

After detach, the terminal keeps running. Its PTY is still open. Another client can attach and continue.

---

## Next steps

This walkthrough covers the happy path. To understand the details:

- **Version negotiation details:** [proto.md §6](./proto.md)
- **Full frame types and encoding:** [proto.md §7](./proto.md)
- **Terminal state, snapshots, and flow control:** [L1.md](./L1.md)
- **All input event types:** [input.md](./input.md)
- **Collections (L2) and metadata (L3):** [L2.md](./L2.md), [L3.md](./L3.md)
- **Encoding primitives (varints, strings, tagged unions):** [appendix-encoding.md](./appendix-encoding.md)

For the conceptual big picture, read [CONCEPTS.md](../CONCEPTS.md) and the ADRs that shape the design:
- [ADR-0013: libghostty bytes on the wire](../../ADR/0013-libghostty-bytes-on-wire.md)
- [ADR-0015: protocol layering](../../ADR/0015-protocol-layering.md)
- [ADR-0016: terminal ID as wire primary](../../ADR/0016-terminal-id-as-wire-primary.md)
