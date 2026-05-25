# phux Wire Protocol

**Version:** 0.1.0-draft
**Status:** Working draft. Not stable.

This document specifies the bytes on the wire between a phux server and
a phux client. It is **normative**: implementations conform to this
document, not to whatever the reference implementation happens to do.

The key words "MUST", "MUST NOT", "REQUIRED", "SHALL", "SHALL NOT",
"SHOULD", "SHOULD NOT", "RECOMMENDED", "MAY", and "OPTIONAL" in this
document are to be interpreted as described in [RFC 2119].

[RFC 2119]: https://datatracker.ietf.org/doc/html/rfc2119

---

## Conventions

Throughout this document:

- Multi-byte integers are **big-endian** on the wire.
- `u8`, `u16`, `u32`, `u64`, `i8`, `i16`, `i32`, `i64` denote fixed-width
  integers.
- `varint` is unsigned [LEB128]: 7 data bits per byte, MSB set on
  continuation. Encoders MUST emit the minimum-length encoding. Decoders
  MUST reject non-canonical encodings (length-extended representations).
- `bytes` is `varint length || raw bytes`.
- `str` is `bytes` whose contents are valid UTF-8.
- `bool` is `u8` with `0` for false, `1` for true, all other values
  reserved.
- `optional<T>` is `bool present || T value` (where `T` is only present
  if `bool` is `1`).
- Field IDs and message IDs are stable: once assigned, they never change
  meaning.

[LEB128]: https://en.wikipedia.org/wiki/LEB128

---

## 1. Introduction

phux is a terminal multiplexer. A long-lived server owns sessions of
windows of panes, where each pane backs one PTY and one terminal grid.
Clients attach to the server over a reliable byte stream and present
sessions to users — as a TUI inside another terminal, or as a native
GUI, or as something else entirely.

The protocol described here is the contract between server and client.
Unlike traditional multiplexers, phux does **not** transport raw VT
bytes between server and client. The server is the canonical owner of
every pane's screen state; the wire format carries **structured
cell-level diffs**. Clients render those diffs directly.

This decision is the protocol's defining trait. Everything else follows
from it.

---

## 2. Terminology

| Term | Definition |
|------|------------|
| **Server** | A long-lived process owning all multiplexer state for one operating-system user. |
| **Client** | A process that attaches to a server, presenting sessions to a user. |
| **Session** | A named, persistent container for one or more windows. Survives client disconnect. |
| **Window** | A tab inside a session. Contains a layout tree of panes. |
| **Pane** | A leaf of a window's layout tree. Has one PTY and one terminal grid. |
| **Frame** | A coherent server-rendered view of one pane, identified by a monotonically increasing `frame_id`. |
| **Grid** | The two-dimensional cell matrix that is a pane's visible viewport. |
| **Scrollback** | Lines that have scrolled out of the grid but are retained for review. |
| **Cell** | One character position in a grid: a grapheme cluster plus rendering attributes. |

---

## 3. Architecture overview

```
┌────────────────────────────┐                  ┌─────────────────────────┐
│        phux server         │ ◄─── transport ►│      phux client        │
│                            │                  │                         │
│  Sessions                  │     PANE_DIFF    │  Renderer               │
│  └─ Windows                │  ───────────────►│  (TUI composes panes    │
│      └─ Panes              │                  │   into outer screen;    │
│          ├─ PTY            │     INPUT_KEY    │   GUI renders to        │
│          └─ Terminal       │  ◄───────────────│   surfaces)             │
│             (libghostty-vt)│                  │                         │
└────────────────────────────┘                  └─────────────────────────┘
```

The server is authoritative for all state. Clients hold only what they
currently render, derived entirely from server messages.

---

## 4. Transport

The protocol runs over any reliable, ordered, bidirectional, octet-
oriented byte stream. This version defines two concrete transports:

- **Unix domain socket** of type `SOCK_STREAM`, for local clients.
- **Standard I/O of an SSH command**, for remote attaches. The client
  invokes `ssh host phux serve --stdio`; the protocol flows over the
  remote process's stdin/stdout.

Future protocol versions MAY define additional transports (for example,
a UDP-based resilient transport in the style of Mosh). Such transports
MUST satisfy the reliable/ordered/bidirectional property; if they do
not, they require a new major protocol version.

The transport is responsible for authentication and confidentiality.
The protocol assumes both. Servers MUST NOT accept connections on
transports that lack peer authentication appropriate to the deployment.

---

## 5. Framing

Every message on the wire is a length-prefixed frame:

```
 0               1               2               3
 0 1 2 3 4 5 6 7 0 1 2 3 4 5 6 7 0 1 2 3 4 5 6 7 0 1 2 3 4 5 6 7
+---------------+---------------+---------------+---------------+
|                       length (u32, BE)                        |
+---------------+-----------------------------------------------+
|   type (u8)   |                  payload ...                  |
+---------------+-----------------------------------------------+
|                          ... payload                          |
+-------------------------------------------+-------------------+
                                            |  (end of frame)
                                            +
```

- `length` is the number of bytes following the length field — i.e. the
  `type` byte plus the payload. A frame is therefore `4 + length` bytes
  total.
- `length` MUST be at least `1` (for the `type` byte) and at most
  `16_777_216` (16 MiB). A peer receiving a frame with `length` outside
  this range MUST send `ERROR { code: FRAME_TOO_LARGE }` and close the
  transport.
- `type` is the message discriminant defined in §7.
- The payload format is determined by `type`.

There is no second framing layer. Application-level structure is encoded
within the payload as defined per-message and per-field.

---

## 6. Version negotiation

The protocol uses semantic versioning: `major.minor.patch`. This
document specifies version `0.1.0`.

- **Major** version changes are wire-breaking.
- **Minor** version changes add new messages or trailing fields. A
  peer encountering an unknown message type at a known minor version
  MUST log and drop the message. A peer encountering trailing fields it
  does not recognize within a known message MUST skip them by length.
- **Patch** version changes are editorial and MUST NOT change behavior.

### 6.1 The HELLO handshake

Every connection opens with a HELLO exchange. The client speaks first:

```
Client → Server:  HELLO {
    versions: list<VersionRange>,
    client_caps: ClientCapabilities,
}

Server → Client:  HELLO_OK {
    version: Version,
    server_caps: ServerCapabilities,
    server_id: bytes,
}
```

`VersionRange` is `{ min: Version, max: Version }` inclusive. The
client's `versions` field lists ranges it supports (typically one).

The server MUST select the highest version that lies in some range of
the client's `versions` AND is supported by the server itself, and echo
it back as `version`. If no such version exists, the server MUST send
`ERROR { code: VERSION_INCOMPATIBLE }` and close.

After `HELLO_OK`, the negotiated version governs the rest of the
connection. Sending HELLO twice on the same connection is an error.

### 6.2 Capability negotiation

Capabilities are advertised once, at HELLO time, and apply for the life
of the connection. They are not renegotiated.

```
ClientCapabilities {
    kbd_protocols: bitset<KeyboardProtocol>,
    mouse_protocols: bitset<MouseProtocol>,
    color: ColorSupport,           // TrueColor | Indexed256 | Indexed16
    images: bitset<ImageProtocol>, // Sixel | KittyGraphics | Iterm2
    hyperlinks: bool,
    unicode_version: u8,
    rendering: RenderingMode,      // Diff | VtReplay (TUI clients can request VtReplay)
}

ServerCapabilities {
    features: bitset<ServerFeature>,
    // ServerFeature variants:
    //   REATTACH_REPLAY    — server retains scrollback for reattaching clients
    //   PANE_RECORDING     — server can record pane I/O to disk
    //   AGENT_HOOKS        — server supports typed agent-style hooks
    //   IMAGE_PASSTHROUGH  — server forwards image protocols transparently
    max_message_size: u32,
}
```

Servers MUST adapt outbound messages to the client's capabilities. For
example, a client advertising `Indexed256` MUST never receive truecolor
RGB cells; the server must downsample.

---

## 7. Message catalog

Messages are identified by a single `u8`. The space is partitioned:

- `0x00 – 0x7F`: client-originated.
- `0x80 – 0xFF`: server-originated.

Within each half:

- `0x01 – 0x0F` / `0x80 – 0x8F`: connection lifecycle.
- `0x10 – 0x2F` / `0x90 – 0xAF`: high-frequency / hot path.
- `0x30 – 0x3F` / `0xC0 – 0xCF`: control plane.
- `0x40 – 0x4F` / `0xB0 – 0xBF`: events and signals.
- `0x7F` / `0xFF`: PING / PONG.

### 7.1 Client → Server

| ID    | Name              | Reference |
|-------|-------------------|-----------|
| 0x01  | `HELLO`           | §6.1      |
| 0x02  | `ATTACH`          | §13       |
| 0x03  | `DETACH`          | §7.3      |
| 0x10  | `INPUT_KEY`       | §9.1      |
| 0x11  | `INPUT_PASTE`     | §9.4      |
| 0x12  | `INPUT_MOUSE`     | §9.2      |
| 0x13  | `INPUT_RAW`       | §9.5      |
| 0x14  | `INPUT_FOCUS`     | §9.3      |
| 0x20  | `VIEWPORT_RESIZE` | §10.5     |
| 0x21  | `FRAME_ACK`       | §12       |
| 0x30  | `COMMAND`         | §11       |
| 0x40  | `SUBSCRIBE`       | §7.4      |
| 0x7F  | `PING`            | §7.5      |

### 7.2 Server → Client

| ID    | Name              | Reference |
|-------|-------------------|-----------|
| 0x80  | `HELLO_OK`        | §6.1      |
| 0x81  | `ATTACHED`        | §13       |
| 0x82  | `DETACHED`        | §7.3      |
| 0x90  | `PANE_DIFF`       | §8        |
| 0x91  | `PANE_SNAPSHOT`   | §8.4      |
| 0x92  | `PANE_RESIZED`    | §10.5     |
| 0xA0  | `PANE_OPENED`     | §10.2     |
| 0xA1  | `PANE_CLOSED`     | §10.2     |
| 0xA2  | `WINDOW_OPENED`   | §10.1     |
| 0xA3  | `WINDOW_CLOSED`   | §10.1     |
| 0xA4  | `WINDOW_RENAMED`  | §10.1     |
| 0xA5  | `LAYOUT_CHANGED`  | §10.3     |
| 0xA6  | `SESSION_OPENED`  | §10       |
| 0xA7  | `SESSION_CLOSED`  | §10       |
| 0xA8  | `SESSION_RENAMED` | §10       |
| 0xA9  | `FOCUS_CHANGED`   | §10.4     |
| 0xB0  | `BELL`            | §7.6      |
| 0xB1  | `OSC_EVENT`       | §7.7      |
| 0xB2  | `ALERT`           | §7.8      |
| 0xC0  | `COMMAND_RESULT`  | §11       |
| 0xC1  | `ERROR`           | §14       |
| 0xFF  | `PONG`            | §7.5      |

### 7.3 DETACH / DETACHED

`DETACH` (client → server) signals the client is leaving cleanly.

```
DETACH { }
```

`DETACHED` (server → client) is sent when the server is ending the
session, the client's attach was forcibly closed, or after a successful
`DETACH` is acknowledged. After `DETACHED`, the server MUST close the
transport.

```
DETACHED { reason: DetachReason, message: str }

DetachReason = enum {
    REQUESTED         = 0,  // client asked
    SERVER_SHUTDOWN   = 1,
    SESSION_KILLED    = 2,
    REPLACED          = 3,  // another client took over an exclusive attach
    PROTOCOL_ERROR    = 4,
    INTERNAL_ERROR    = 255,
}
```

### 7.4 SUBSCRIBE

Reserved for opting in/out of notification streams (e.g. only the focused
client should receive `BELL` for inactive panes). Format defined in v0.2.

### 7.5 PING / PONG

```
PING { nonce: u64 }
PONG { nonce: u64 }
```

A peer receiving `PING` MUST respond with `PONG` carrying the same nonce
within a reasonable interval. PING/PONG is liveness only — clients and
servers MAY use it for keepalive; absence of pongs SHOULD NOT be
interpreted as anything other than a transport failure.

### 7.6 BELL

```
BELL { pane_id: PaneId }
```

The pane received a bell character. The server MUST NOT translate this
into VT output; clients decide policy.

### 7.7 OSC_EVENT

A channel for terminal-originated events the server has parsed (via
libghostty-vt's OSC parser) and chooses to surface to clients.

```
OSC_EVENT {
    pane_id: PaneId,
    event: OscEvent,
}

OscEvent = tagged_union {
    TITLE             { title: str },                              // OSC 0/1/2
    CHANGE_WINDOW_ICON,                                            // OSC 1 (icon-only)
    CURRENT_DIR       { uri: str },                                // OSC 7
    HYPERLINK_START   { id: u32, uri: str, params: str },          // OSC 8 begin
    HYPERLINK_END     { id: u32 },                                 // OSC 8 end
    USER_NOTIFICATION { body: str, tag: optional<str> },           // OSC 9 / iTerm2 / OSC 777
    SEMANTIC_PROMPT   { kind: PromptMarkKind, info: optional<str> }, // OSC 133
    CLIPBOARD         { selection: ClipboardSelection, data: bytes }, // OSC 52
    MOUSE_SHAPE       { shape: str },                              // OSC 22
    PROGRESS_REPORT   { state: ProgressState, value: optional<u8> }, // ConEmu OSC 9;4
    EXIT_CODE         { code: i32 },                               // synthesized at PTY exit
    CUSTOM            { kind: u32, payload: bytes },               // pass-through escape hatch
}

PromptMarkKind = enum {
    PROMPT_START   = 1,  // OSC 133;A
    COMMAND_START  = 2,  // OSC 133;B
    COMMAND_END    = 3,  // OSC 133;C
    PROMPT_END     = 4,  // OSC 133;D (optional exit code in `info`)
}

ProgressState = enum {
    REMOVE      = 0,
    DEFAULT     = 1,
    ERROR       = 2,
    INDETERMINATE = 3,
    WARNING     = 4,
    PAUSED      = 5,
}

ClipboardSelection = enum {
    SYSTEM     = 0,
    PRIMARY    = 1,
    SECONDARY  = 2,
}
```

The server does NOT forward every OSC type libghostty recognises. Color
operations, kitty color protocol commands, and kitty text-sizing are
purely terminal-state concerns; they are applied to the pane's
`libghostty_vt::Terminal` and clients see their effect through normal
cell diffs. The variants listed above are those that affect *client* UX
(chrome, notifications, clipboard, status bar widgets).

### 7.8 ALERT

Server-internal notifications about a pane:

```
ALERT { pane_id: PaneId, kind: AlertKind }

AlertKind = enum {
    ACTIVITY  = 0,  // pane wrote output while window was inactive
    SILENCE   = 1,  // pane has been quiet for the configured threshold
    BELL      = 2,  // duplicate of §7.6 for clients that prefer one channel
}
```

---

## 8. Pane state synchronization — the hot path

This section is the protocol's centerpiece. The server owns each pane's
canonical grid (in a `libghostty_vt::Terminal`). Clients render that
grid. The protocol carries the changes between them.

### 8.1 The frame model

Each pane has a monotonically increasing `frame_id`, a `u64`. Frame `0`
is the empty grid at pane creation; subsequent frames represent the
state of the grid after some change.

The server emits frames at most at a per-pane refresh-rate cap (default
60 Hz, configurable, may be lowered for background panes). Between
frames, output is coalesced into the next frame.

A `PANE_DIFF` describes the transition from one frame to the next:

```
PANE_DIFF {
    pane_id: PaneId,
    frame_id: u64,           // the frame this produces
    base_frame_id: u64,      // the frame this applies on top of; 0 = empty
    ops: list<DiffOp>,
    cursor: CursorState,
    modes: PaneModes,
    revision: u8,            // 0 today; reserved for compression schemes
}
```

A `PANE_SNAPSHOT` (§8.4) is a self-contained frame: a full grid plus
scrollback. It is functionally equivalent to a `PANE_DIFF` whose
`base_frame_id` is `0` and whose `ops` describe the entire grid.

### 8.2 Cells

The unit on the wire is the **cell**: one grapheme cluster plus rendering
attributes, fully resolved server-side. There is no SGR ambiguity in
transit.

```
Cell {
    text: GraphemeCluster,         // 1+ codepoints
    fg: Color,
    bg: Color,
    underline: Underline,
    underline_color: Color,
    flags: CellFlags,              // bitset
    hyperlink_id: optional<u32>,
}

GraphemeCluster = list<u32>        // codepoints; length-prefixed

Color = tagged_union {
    DEFAULT,                       // foreground or background "default"
    INDEXED(u8),                   // 0..=255, terminal palette
    RGB(u8, u8, u8),               // truecolor; servers MUST NOT emit this
                                   //   to clients without TrueColor cap
}

Underline = enum {
    NONE = 0, SINGLE = 1, DOUBLE = 2, CURLY = 3, DOTTED = 4, DASHED = 5,
}

CellFlags = bitset {
    BOLD              = 0x0001,
    FAINT             = 0x0002,
    ITALIC            = 0x0004,
    BLINK_SLOW        = 0x0008,
    BLINK_FAST        = 0x0010,
    REVERSE           = 0x0020,
    INVISIBLE         = 0x0040,
    STRIKETHROUGH     = 0x0080,
    OVERLINED         = 0x0100,
    WIDE_LEFT         = 0x0200,   // first half of a wide character
    WIDE_RIGHT        = 0x0400,   // second half (always follows WIDE_LEFT)
    PROTECTED         = 0x0800,
}
```

### 8.3 Diff operations

```
DiffOp = tagged_union {
    CELL_RUN     {  row: u16, col: u16,
                    attrs: CellAttrs,
                    cells: list<TextRun>  },
    REPEAT       {  row: u16, col: u16,
                    cell: Cell,
                    count: u16  },
    CLEAR        {  row: u16, col: u16, count: u16  },
    ERASE_LINE   {  row: u16, mode: EraseLineMode  },
    SCROLL_UP    {  region: ScrollRegion, lines: u16  },
    SCROLL_DOWN  {  region: ScrollRegion, lines: u16  },
    HYPERLINK    {  id: u32, uri: str, params: str  },
    IMAGE        {  placement: ImagePlacement  },
}

CellAttrs = Cell with the `text` field omitted; describes a run of cells
that share rendering attributes.

TextRun = GraphemeCluster

EraseLineMode = enum {
    LEFT_OF_CURSOR = 0,
    RIGHT_OF_CURSOR = 1,
    ALL = 2,
}

ScrollRegion = { top: u16, bottom: u16, left: u16, right: u16 }
```

Notes on the operation set:

- `CELL_RUN` is the dominant op: a contiguous horizontal run of cells
  that share attributes. `cells` is a list of grapheme clusters, one per
  cell position.
- `REPEAT` is run-length encoding for cases where the same cell repeats
  (a blank line, a box of dashes).
- `CLEAR` zeros a horizontal span with default attributes; smaller wire
  than a CELL_RUN of spaces.
- `SCROLL_UP` / `SCROLL_DOWN` are **preserved as semantic operations**.
  Clients with scrollback can keep their history intact. This is one of
  the explicit wins over tmux's tty.c, which clobbers history on scroll.
- `HYPERLINK` registers a hyperlink in a per-pane intern table referenced
  by `hyperlink_id` on cells.
- `IMAGE` is reserved for sixel / kitty graphics. Concrete encoding in
  v0.2.

### 8.4 Snapshots

```
PANE_SNAPSHOT {
    pane_id: PaneId,
    frame_id: u64,
    grid: Grid,
    scrollback: optional<Scrollback>,  // present iff the client opted in
    cursor: CursorState,
    modes: PaneModes,
}

Grid {
    cols: u16,
    rows: u16,
    cells: list<DiffOp>,   // typically a sequence of CELL_RUN covering the grid
}

Scrollback {
    compression: CompressionKind,  // NONE | LZ4 | ZSTD
    rows: u32,                     // number of scrollback rows
    data: bytes,                   // compressed payload of CELL_RUN ops
}
```

Servers emit `PANE_SNAPSHOT` when:

1. A client first attaches (§13).
2. Backpressure forced the server to drop intermediate diffs (§12).
3. The grid resized (§10.5).
4. The protocol requires it for correctness in any future case.

### 8.5 Cursor and modes

Cursor state and pane-wide modes ride along with every diff. They are
small and changing them mid-frame is common; pulling them out into
separate messages would increase wire chatter for no benefit.

```
CursorState {
    row: u16,
    col: u16,
    visible: bool,
    shape: CursorShape,    // BLOCK | BAR | UNDERLINE
    blink: bool,
}

PaneModes = bitset {
    ALTSCREEN_ACTIVE  = 0x0001,
    BRACKETED_PASTE   = 0x0002,
    APP_CURSOR_KEYS   = 0x0004,
    APP_KEYPAD        = 0x0008,
    MOUSE_PROTOCOL    = 0x00F0,  // 4 bits of MouseProtocol enum
    MOUSE_ENCODING    = 0x0F00,  // 4 bits of MouseEncoding enum
    FOCUS_REPORTING   = 0x1000,
    ORIGIN_MODE       = 0x2000,
}
```

A pane's `modes` are part of the protocol because clients need to know
them — for example, a client must know whether `MOUSE_PROTOCOL` is
active before forwarding pointer events.

---

## 9. Input events

This section is the second-most-important part of the protocol. Carrying
input events as structured data — not raw VT bytes — is what allows
phux to faithfully transport the kitty keyboard protocol, IME
composition, modifier-rich chords, and pixel-precise mouse events
end-to-end through the multiplexer. It is also what lets the protocol
serve a future GUI client and a TUI client from the same wire format.

Per ADR-0008, the input *atom* types (`KeyAction`, `PhysicalKey`,
`ModSet`, `MouseAction`, `MouseButton`, `FocusEvent`) **are** libghostty-
vt's types — re-exported under phux-flavored names. The outer wrapper
structs (`KeyEvent`, `MouseEvent`) are phux-defined because libghostty's
event objects are allocator-lifetime-bound and not directly serializable;
their fields are still libghostty's types.

The server constructs libghostty events from wire events with a
field-for-field copy (no enum conversion), hands them to libghostty's
encoders (which know per-terminal state: KIP flags, cursor-key mode,
mouse protocol, etc.), and writes the resulting bytes to the PTY.
Encoder configuration never traverses the wire — the server is the one
with the `Terminal` and the encoder. See [ADR-0006] and [ADR-0008].

[ADR-0006]: ./ADR/0006-input-mirrors-libghostty.md
[ADR-0008]: ./ADR/0008-use-libghostty-types-directly.md

### 9.1 INPUT_KEY

```
INPUT_KEY {
    pane_id: PaneId,
    event: KeyEvent,
}

KeyEvent {
    action: KeyAction,
    key: PhysicalKey,
    mods: ModSet,
    consumed_mods: ModSet,
    composing: bool,
    text: optional<str>,
    unshifted_codepoint: optional<u32>,
}

KeyAction = enum {
    PRESS   = 0,
    RELEASE = 1,
    REPEAT  = 2,
}
```

#### 9.1.1 `key` — `PhysicalKey`

`PhysicalKey` is a physical key code, **independent of keyboard layout
or modifiers**. It is the W3C UI Events `code`-style enum that
libghostty's `key::Key` carries. A US-QWERTY user pressing the leftmost
home-row key produces `KeyA`; an AZERTY user pressing the *same physical
key* also produces `KeyA`. The layout-resolved text appears in `text`
and `unshifted_codepoint`.

Values are stable; numeric assignments match libghostty's `key::Key`:

```
PhysicalKey = enum (u32) {
    UNIDENTIFIED   = 0,

    // Writing-system keys (US-QWERTY positions)
    BACKQUOTE      = 1,   BACKSLASH        = 2,   BRACKET_LEFT     = 3,
    BRACKET_RIGHT  = 4,   COMMA            = 5,
    DIGIT_0        = 6 ..= DIGIT_9          = 15,
    EQUAL          = 16,  INTL_BACKSLASH   = 17,  INTL_RO          = 18,
    INTL_YEN       = 19,
    KEY_A          = 20 ..= KEY_Z           = 45,
    MINUS          = 46,  PERIOD           = 47,  QUOTE            = 48,
    SEMICOLON      = 49,  SLASH            = 50,

    // Functional keys
    ALT_LEFT       = 51,  ALT_RIGHT        = 52,  BACKSPACE        = 53,
    CAPS_LOCK      = 54,  CONTEXT_MENU     = 55,  CONTROL_LEFT     = 56,
    CONTROL_RIGHT  = 57,  ENTER            = 58,  META_LEFT        = 59,
    META_RIGHT     = 60,  SHIFT_LEFT       = 61,  SHIFT_RIGHT      = 62,
    SPACE          = 63,  TAB              = 64,
    CONVERT        = 65,  KANA_MODE        = 66,  NON_CONVERT      = 67,

    // Control pad
    DELETE = 68,  END = 69, HELP = 70, HOME = 71, INSERT = 72,
    PAGE_DOWN = 73, PAGE_UP = 74,

    // Arrow keys
    ARROW_DOWN = 75, ARROW_LEFT = 76, ARROW_RIGHT = 77, ARROW_UP = 78,

    // Numpad
    NUM_LOCK = 79,
    NUMPAD_0 = 80 ..= NUMPAD_9 = 89,
    NUMPAD_ADD = 90, NUMPAD_BACKSPACE = 91, NUMPAD_CLEAR = 92,
    NUMPAD_CLEAR_ENTRY = 93, NUMPAD_COMMA = 94, NUMPAD_DECIMAL = 95,
    NUMPAD_DIVIDE = 96, NUMPAD_ENTER = 97, NUMPAD_EQUAL = 98,
    NUMPAD_MEMORY_ADD = 99, NUMPAD_MEMORY_CLEAR = 100,
    NUMPAD_MEMORY_RECALL = 101, NUMPAD_MEMORY_STORE = 102,
    NUMPAD_MEMORY_SUBTRACT = 103, NUMPAD_MULTIPLY = 104,
    NUMPAD_PAREN_LEFT = 105, NUMPAD_PAREN_RIGHT = 106,
    NUMPAD_SUBTRACT = 107, NUMPAD_SEPARATOR = 108,
    NUMPAD_UP = 109, NUMPAD_DOWN = 110, NUMPAD_RIGHT = 111,
    NUMPAD_LEFT = 112, NUMPAD_BEGIN = 113, NUMPAD_HOME = 114,
    NUMPAD_END = 115, NUMPAD_INSERT = 116, NUMPAD_DELETE = 117,
    NUMPAD_PAGE_UP = 118, NUMPAD_PAGE_DOWN = 119,

    // Function keys
    ESCAPE = 120,
    F1 = 121 ..= F25 = 145,
    FN = 146, FN_LOCK = 147,
    PRINT_SCREEN = 148, SCROLL_LOCK = 149, PAUSE = 150,

    // Browser / app
    BROWSER_BACK = 151, BROWSER_FAVORITES = 152, BROWSER_FORWARD = 153,
    BROWSER_HOME = 154, BROWSER_REFRESH = 155, BROWSER_SEARCH = 156,
    BROWSER_STOP = 157,
    EJECT = 158, LAUNCH_APP_1 = 159, LAUNCH_APP_2 = 160, LAUNCH_MAIL = 161,

    // Media / system
    MEDIA_PLAY_PAUSE = 162, MEDIA_SELECT = 163, MEDIA_STOP = 164,
    MEDIA_TRACK_NEXT = 165, MEDIA_TRACK_PREVIOUS = 166,
    POWER = 167, SLEEP = 168,
    AUDIO_VOLUME_DOWN = 169, AUDIO_VOLUME_MUTE = 170, AUDIO_VOLUME_UP = 171,
    WAKE_UP = 172, COPY = 173, CUT = 174, PASTE = 175,
}
```

This enum is **non-exhaustive** in spirit: minor protocol versions may
add new values. Decoders MUST treat unknown values as `UNIDENTIFIED`.

#### 9.1.2 `mods` — `ModSet`

```
ModSet = bitset (u16) {
    SHIFT        = 0x0001,
    ALT          = 0x0002,
    CTRL         = 0x0004,
    SUPER        = 0x0008,    // also macOS Command, Windows key
    CAPS_LOCK    = 0x0010,
    NUM_LOCK     = 0x0020,

    // Left-vs-right discrimination. Each *_SIDE bit is only meaningful
    // when the corresponding modifier bit is set: 0 = left key,
    // 1 = right key. Platforms that cannot distinguish sides MUST
    // leave these bits zero.
    SHIFT_SIDE   = 0x0040,
    ALT_SIDE     = 0x0080,
    CTRL_SIDE    = 0x0100,
    SUPER_SIDE   = 0x0200,
}
```

Note the deliberate absence of `HYPER` and `META` as separate flags.
libghostty's `Mods` does not distinguish them from `SUPER`; on
platforms where they exist (X11 with custom XKB), they map to `SUPER`
with appropriate XKB configuration. Modeling them separately at the
protocol level would introduce a degree of freedom no downstream
encoder can honor.

#### 9.1.3 `consumed_mods`

The subset of `mods` that the operating system *consumed* to produce
`text`. For example, on a US layout pressing Shift+2 produces the text
`@` with `SHIFT` in `consumed_mods`; the KIP encoder uses this to avoid
double-applying the shift modifier in its escape sequence.

Clients that do not have this information from their platform SHOULD
emit `ModSet::empty()` — the encoder degrades gracefully.

#### 9.1.4 `composing`

`true` if this key event is part of an active IME composition sequence.
The encoder uses this to suppress text production where appropriate.

#### 9.1.5 `text` and `unshifted_codepoint`

- `text`: the UTF-8 text the keypress produced under the current
  layout, *before* any Ctrl/Meta transformation. MUST NOT contain C0
  control characters (`U+0000–U+001F`, `U+007F`) — for those, pass
  `None` and let the encoder derive bytes from `key + mods`. MUST NOT
  contain platform PUA function-key codes (`U+F700–U+F8FF`).
- `unshifted_codepoint`: the layout-resolved codepoint that would have
  been produced if no modifiers were held. Used by KIP's
  `REPORT_ALTERNATES` mode to report the "base" key alongside the
  modified one.

Both fields are optional. KIP-aware clients SHOULD supply both for
maximum fidelity; legacy clients MAY omit them.

#### 9.1.6 Server-side encoding pipeline

The server's per-pane state includes:

- A `libghostty_vt::Terminal` (canonical pane state, ADR-0004).
- A `libghostty_vt::key::Encoder` (key-to-bytes converter).

When the server receives an `INPUT_KEY`:

1. Translate the wire `KeyEvent` into a `libghostty::key::Event` —
   every field maps one-to-one.
2. Refresh the encoder's options against the current terminal state via
   `Encoder::set_options_from_terminal(&terminal)`. This pulls cursor-
   key application mode, keypad mode, alt-esc-prefix, modifyOtherKeys,
   and KIP progressive-enhancement flags from the terminal's current
   modes.
3. Call `Encoder::encode_to_vec(&event, &mut buf)`.
4. Write `buf` to the pane's PTY.

The client never sees encoder options. The client never produces VT
bytes. The protocol is the seam.

This pipeline supports, end-to-end:

- **The kitty keyboard protocol (KIP)** in its progressive-enhancement
  entirety — disambiguation, report-events, report-alternates,
  report-all, report-associated.
- Unambiguous distinction between Ctrl+I and Tab, between Esc and
  Alt-letter, between Ctrl+Enter and a literal `J`.
- IME composition and dead keys.
- Modifier-rich combinations (Super, side-discriminated) for tiling-WM-
  style bindings, with correct passthrough.

These are the things tmux structurally cannot do because it speaks raw
VT between client and server.

### 9.2 INPUT_MOUSE

```
INPUT_MOUSE {
    pane_id: PaneId,
    event: MouseEvent,
}

MouseEvent {
    action: MouseAction,
    button: optional<MouseButton>,
    mods: ModSet,
    position: MousePosition,
}

MouseAction = enum {
    PRESS   = 0,
    RELEASE = 1,
    MOTION  = 2,
}

MouseButton = enum (u32) {
    UNKNOWN = 0,
    LEFT    = 1,
    RIGHT   = 2,
    MIDDLE  = 3,
    FOUR    = 4,    FIVE    = 5,    SIX    = 6,    SEVEN  = 7,
    EIGHT   = 8,    NINE    = 9,    TEN    = 10,   ELEVEN = 11,
}

MousePosition {
    // Pane-local surface-space pixels. Always present.
    // f64 (not u32) to mirror libghostty's `mouse::Position` exactly —
    // sub-pixel input is real on macOS trackpads and Wayland HiDPI surfaces;
    // cell-quantizing clients pass integer-valued f64s (`12.0`).
    pixel_x: f64,
    pixel_y: f64,
}
```

Values map one-to-one to libghostty's `mouse::Action`, `mouse::Button`,
and `mouse::Position`. Buttons 4..=11 carry their libghostty meaning;
scroll-wheel events arrive as `PRESS` of buttons 4 (up) / 5 (down) /
6 (left) / 7 (right) following xterm convention.

#### 9.2.1 Pixel positions and the cell-geometry contract

Mouse positions on the wire are **pixels in pane-local surface space**.
The server reconstructs `mouse::EncoderSize` (cell width/height,
padding, full screen geometry) from the most recent `VIEWPORT_RESIZE`
(§10.5) and per-pane layout. Cell-quantized clients (TUIs without true
pixel-precision input) emit positions at `cell_index × cell_size`; the
server's encoder produces correct output in both cell-format (SGR,
URXVT) and pixel-format (SGR-Pixels) mouse protocols.

#### 9.2.2 Server-side encoding pipeline

Identical in spirit to §9.1.6: each pane has a
`libghostty_vt::mouse::Encoder`. On `INPUT_MOUSE`, the server refreshes
the encoder via `set_options_from_terminal`, sets the encoder's
`EncoderSize` from current pane/cell geometry, builds a
`libghostty::mouse::Event`, encodes, and writes to PTY.

### 9.3 INPUT_FOCUS

```
INPUT_FOCUS {
    pane_id: PaneId,
    event: FocusKind,
}

FocusKind = enum { GAINED = 0, LOST = 1 }
```

The client emits `INPUT_FOCUS` when its window gains or loses focus on
the host OS. If the pane has DEC mode 1004 (focus reporting) active,
the server encodes a `CSI I` / `CSI O` via `libghostty_vt::focus`
and writes to the PTY; otherwise the event is dropped server-side.

This is separate from `FOCUS_CHANGED` (§10.4), which is server-to-
client and concerns *which pane the client is interacting with* — not
whether the client itself is in the foreground.

### 9.4 INPUT_PASTE

```
INPUT_PASTE {
    pane_id: PaneId,
    data: bytes,
    bracketed: bool,
    trust: PasteTrust,
}

PasteTrust = enum {
    UNTRUSTED = 0,   // server SHOULD apply paste::is_safe; reject or sanitize
                     //   per server config
    TRUSTED   = 1,   // server forwards verbatim; caller asserted safety
}
```

Server uses `libghostty_vt::paste` utilities: `paste::is_safe(data)` to
classify content, `paste::encode(data, bracketed, buf)` to produce
final bytes (handles bracketed-paste sequences and unsafe-control-byte
stripping).

When `trust = UNTRUSTED`, the server's per-pane policy applies:
`reject` (default — return an `ERROR { code: UNSAFE_PASTE }`),
`sanitize` (use `paste::encode` to strip), or `allow` (forward anyway).
When `trust = TRUSTED`, the server invokes `paste::encode` for
bracketing but skips safety classification.

### 9.5 INPUT_RAW

```
INPUT_RAW {
    pane_id: PaneId,
    data: bytes,
}
```

Escape hatch. Bytes in `data` are written verbatim to the pane's PTY.
Reserved for cases not modelled by `INPUT_KEY` / `INPUT_PASTE` /
`INPUT_MOUSE` / `INPUT_FOCUS` (chiefly: direct PTY testing and command
interpolation from configs). Servers MUST NOT silently re-interpret
`INPUT_RAW`; clients SHOULD avoid using it in normal operation.

---

## 10. Sessions, windows, panes, layout

### 10.1 Sessions and windows

A session is a named container. Sessions are listed in `ATTACHED`
(§13) and notified as they change via `SESSION_OPENED`, `SESSION_CLOSED`,
`SESSION_RENAMED`.

```
SESSION_OPENED  { session_id: SessionId, name: str, created_unix_micros: u64 }
SESSION_CLOSED  { session_id: SessionId }
SESSION_RENAMED { session_id: SessionId, name: str }

WINDOW_OPENED  { window_id: WindowId, session_id: SessionId,
                 index: u16, name: str, layout: LayoutTree }
WINDOW_CLOSED  { window_id: WindowId }
WINDOW_RENAMED { window_id: WindowId, name: str }
```

`SessionId` is a tagged union; `WindowId` is an opaque `u32`. Both are
stable for the life of the server and are not reused after close (the
counter is monotonic for the server's lifetime).

```
SessionId = tagged_union {
    LOCAL     { id: u32 },              // tag = 0
    SATELLITE { host: str, id: u32 },   // tag = 1; reserved for v0.2+ (ADR-0007)
}
```

v0.1 servers only ever construct `LOCAL`. v0.1 decoders MUST accept the
`SATELLITE` tag and, if not configured as a federation hub, respond with
an `ERROR { code: UnsupportedSatelliteRoute }` (SPEC §14) rather than
failing the frame. This forward-compat reservation costs one tag byte per
session reference and avoids a wire-format break when satellites land.

`WindowId` remains an opaque `u32` — windows are always scoped to a
known session, so their addressability is inherited from the session URI.

### 10.2 Panes

```
PANE_OPENED {
    pane_id: PaneId,
    window_id: WindowId,
    initial_size: { cols: u16, rows: u16 },
    cwd: str,
    command: list<str>,
}

PANE_CLOSED {
    pane_id: PaneId,
    exit_status: optional<ExitStatus>,
}

ExitStatus = tagged_union {
    EXITED(u8),     // process called _exit(n)
    SIGNALED(u8),   // killed by signal n
    UNKNOWN,
}
```

### 10.3 Layout

```
LAYOUT_CHANGED { window_id: WindowId, layout: LayoutTree }

LayoutTree = LayoutNode

LayoutNode = tagged_union {
    LEAF  { pane_id: PaneId, weight: u16 },
    SPLIT { direction: SplitDirection,
            children: list<LayoutNode>,
            weights: list<u16> },
    TABBED { children: list<LayoutNode>, active: u32 },  // reserved for v0.2
}

SplitDirection = enum { HORIZONTAL = 0, VERTICAL = 1 }
```

Layout is a tree; the server publishes the whole tree on any change.
Clients render chrome (borders, status bars) per the tree. The protocol
does not specify how chrome looks; that is a client concern.

### 10.4 Focus

```
FOCUS_CHANGED { window_id: WindowId, pane_id: PaneId, client_id: ClientId }
```

Focus is per-client. The server tracks which window and pane each
attached client is currently focused on; `FOCUS_CHANGED` is emitted to
all attached clients whenever any one of them changes focus.

### 10.5 Viewport resize

The client's outer terminal size and cell geometry are signalled with
`VIEWPORT_RESIZE`:

```
VIEWPORT_RESIZE {
    cols: u16,                          // outer terminal width in cells
    rows: u16,                          // outer terminal height in cells
    pixel_w: optional<u16>,             // outer terminal width in pixels
    pixel_h: optional<u16>,             // outer terminal height in pixels
    cell_w: optional<u16>,              // single-cell width in pixels
    cell_h: optional<u16>,              // single-cell height in pixels
    padding_top: optional<u16>,         // chrome padding around the cell grid
    padding_bottom: optional<u16>,
    padding_left: optional<u16>,
    padding_right: optional<u16>,
}
```

`cell_w` / `cell_h` / `padding_*` are required for accurate mouse
encoding in pixel-format mouse protocols (`SgrPixels`). Cell-quantized
clients (TUIs without real pixel metrics) MAY pass `cell_w = 1,
cell_h = 1, padding_* = 0` — the server's encoder produces correct
output in cell-format protocols regardless. Pixel-precise clients
(GUIs) SHOULD provide real metrics.

The server recomputes layout against the new viewport. Per-pane resize
events are then emitted as `PANE_RESIZED`:

```
PANE_RESIZED { pane_id: PaneId, cols: u16, rows: u16 }
```

When multiple clients are attached to the same session with different
viewport sizes, the server uses the smallest common bounding box per
window (configurable: `aggressive` mode resizes per attached client).
This matches tmux's well-understood behavior and avoids surprising
shrink-and-grow on attach/detach.

---

## 11. Commands

Commands are typed messages, not strings. They are sent over the same
connection and correlated via `request_id`.

```
COMMAND { request_id: u32, cmd: Command }
COMMAND_RESULT { request_id: u32, result: CommandResult }

Command = tagged_union {
    NEW_SESSION   { name: optional<str>, cwd: optional<str>, command: optional<list<str>> },
    NEW_WINDOW    { session_id: SessionId, cwd: optional<str>, command: optional<list<str>> },
    NEW_PANE      { window_id: WindowId, target: optional<PaneId>,
                    direction: SplitDirection, cwd: optional<str>,
                    command: optional<list<str>> },
    KILL_PANE     { pane_id: PaneId },
    KILL_WINDOW   { window_id: WindowId },
    KILL_SESSION  { session_id: SessionId },
    RENAME_WINDOW { window_id: WindowId, name: str },
    RENAME_SESSION{ session_id: SessionId, name: str },
    SWITCH_WINDOW { client_id: ClientId, window_id: WindowId },
    SWITCH_PANE   { client_id: ClientId, pane_id: PaneId },
    MOVE_PANE     { pane_id: PaneId, target: PaneRef, position: PanePosition },
    RESIZE_PANE   { pane_id: PaneId, direction: ResizeDirection, amount: i16 },
    SET_OPTION    { scope: OptionScope, key: str, value: ConfigValue },
    GET_STATE     { scope: StateScope },
    RUN_HOOK      { name: str, args: list<str> },
}

CommandResult = tagged_union {
    OK,
    OK_WITH(CommandValue),
    ERROR(ErrorCode, str),
}

CommandValue = tagged_union {
    SESSION_ID(SessionId),
    WINDOW_ID(WindowId),
    PANE_ID(PaneId),
    STATE(StateSnapshot),
    JSON(str),                   // for GET_STATE and structured returns
}
```

A `COMMAND` is asynchronous: the server MAY emit other messages
(including events relevant to the command's effect) before
`COMMAND_RESULT`. Clients MUST tolerate that ordering.

### 11.1 What is deliberately absent

The protocol exposes no string-based command DSL, no expression
evaluator, no formatting language. Commands are an enum. Strings appear
only as user-supplied names, paths, and arguments.

This is a directional decision documented in
[`ADR/0002-diff-based-protocol.md`](./ADR/0002-diff-based-protocol.md)
and [`CONTRIBUTING.md`](./CONTRIBUTING.md).

---

## 12. Flow control

### 12.1 Frame pacing

The server MUST cap per-pane diff emission at a configurable refresh
rate (default 60 Hz). Between emissions, output is coalesced into a
single forthcoming frame. There is no "every change emits a frame" mode;
that would not survive a `yes` flood.

### 12.2 Frame acknowledgement

Clients acknowledge frames they have rendered:

```
FRAME_ACK { pane_id: PaneId, frame_id: u64 }
```

The server tracks per-client `last_acked_frame` per pane. When
`pending_unacked` exceeds `flow_control_threshold` (default: 32 frames,
configurable per-server, never disable-able), the server:

1. Stops sending `PANE_DIFF` for that pane to that client.
2. Coalesces all pending state for that pane.
3. Emits a single `PANE_SNAPSHOT` representing the current state.
4. Resumes diffs from the snapshot.

This is the playbook Mosh uses, generalized to per-pane streams. It
ensures a slow client cannot block the server, and the worst-case
catch-up cost is one snapshot, not an unbounded queue replay.

### 12.3 Per-client isolation

Each connected client has its own outbound queue. A wedged client whose
queue exceeds its bound is forcibly disconnected with
`DETACHED { reason: PROTOCOL_ERROR }`. Other clients are unaffected.

---

## 13. State replay on attach

When a client sends `ATTACH`, the server's response sequence is:

1. `ATTACHED { snapshot: SessionSnapshot }` — full graph of sessions,
   windows, panes, layouts, the attaching client's initial focus, and
   per-pane size.
2. For each pane in the focused window of the targeted session, one
   `PANE_SNAPSHOT` with grid and (optionally) scrollback.
3. Subsequent `PANE_DIFF` messages flow live.

```
ATTACH {
    target: AttachTarget,
    viewport: { cols: u16, rows: u16, pixel_w: optional<u16>, pixel_h: optional<u16> },
    request_scrollback: bool,
    scrollback_limit_lines: u32,
}

AttachTarget = tagged_union {
    LAST,                            // most-recently-attached session
    BY_NAME(str),
    BY_ID(SessionId),
    CREATE_IF_MISSING { name: str, command: optional<list<str>>, cwd: optional<str> },
}

ATTACHED {
    snapshot: SessionSnapshot,
    initial_client_id: ClientId,
}

SessionSnapshot {
    sessions: list<SessionInfo>,
    windows: list<WindowInfo>,
    panes: list<PaneInfo>,
    focused_session: SessionId,
    focused_window: WindowId,
    focused_pane: PaneId,
}
```

This is the protocol's killer feature: a client reconnecting after
hours of detached work receives the **full state** of every pane,
including scrollback up to the configured limit. tmux loses scrollback
on detach; phux does not.

---

## 14. Errors

Errors carry a structured code and a human-readable message:

```
ERROR {
    request_id: optional<u32>,   // present if the error is associated with a COMMAND
    code: ErrorCode,
    message: str,
}

ErrorCode = enum {
    VERSION_INCOMPATIBLE = 1,
    UNKNOWN_MESSAGE_TYPE = 2,
    MALFORMED_MESSAGE    = 3,
    FRAME_TOO_LARGE      = 4,

    NOT_ATTACHED         = 100,
    ALREADY_ATTACHED     = 101,
    SESSION_NOT_FOUND    = 102,
    WINDOW_NOT_FOUND     = 103,
    PANE_NOT_FOUND       = 104,
    CLIENT_NOT_FOUND     = 105,

    INVALID_COMMAND      = 200,
    PERMISSION_DENIED    = 201,
    RESOURCE_EXHAUSTED   = 202,

    INTERNAL_ERROR       = 65535,
}
```

A fatal error MUST be followed by `DETACHED { reason: PROTOCOL_ERROR }`
and transport close.

---

## 15. Security

The protocol delegates authentication and confidentiality to the
transport.

- **Unix sockets:** rely on filesystem permissions (mode `0600`, owned
  by the user). Servers MUST refuse to create sockets with broader
  permissions.
- **SSH:** rely on the SSH session's authentication and channel
  confidentiality.

The protocol does **not** define cookies, tokens, or in-band auth. If a
future deployment requires per-attachment authorization, it is the
transport's responsibility to deliver an authenticated peer identity to
the server.

---

## 16. Conformance

An implementation conforms to this specification if:

1. It frames every message per §5.
2. It performs the §6.1 HELLO handshake with `versions` consistent with
   §6 ordering and `version` selection.
3. It implements every REQUIRED message of the negotiated version. The
   set of REQUIRED messages is:
   - `HELLO`, `HELLO_OK`, `ATTACH`, `ATTACHED`, `DETACH`, `DETACHED`,
     `PING`, `PONG`, `ERROR`,
   - `PANE_DIFF`, `PANE_SNAPSHOT`, `PANE_RESIZED`,
   - `INPUT_KEY`, `INPUT_PASTE`, `VIEWPORT_RESIZE`, `FRAME_ACK`,
   - `PANE_OPENED`, `PANE_CLOSED`,
     `WINDOW_OPENED`, `WINDOW_CLOSED`,
     `SESSION_OPENED`, `SESSION_CLOSED`,
     `LAYOUT_CHANGED`, `FOCUS_CHANGED`,
   - `COMMAND` for at least `NEW_SESSION`, `NEW_WINDOW`, `NEW_PANE`,
     `KILL_PANE`, `KILL_WINDOW`, `KILL_SESSION`, `SWITCH_WINDOW`,
     `SWITCH_PANE`, `RESIZE_PANE`,
   - `COMMAND_RESULT`.
4. It tolerates unknown messages by logging and dropping them (§6).
5. It tolerates unknown trailing fields per the encoding rules
   (Appendix A).

`INPUT_MOUSE`, `INPUT_RAW`, `OSC_EVENT`, `ALERT`, `BELL`,
`PANE_RENAMED`-family, image diff ops, `TABBED` layout nodes, and the
full command set are RECOMMENDED but not REQUIRED for conformance.

The reference test suite for this specification will live at
`crates/phux-protocol/tests/` and at `tests/conformance/` in the
implementation repository.

---

## Appendix A. Encoding primitives

Every payload is encoded as a sequence of fields. Fields are
self-delimiting: a decoder can skip an unknown field without knowing its
semantics.

A field is `{ field_id: varint, wire_type: u8, value: ... }`, where
`wire_type` determines how `value` is encoded:

| wire_type | Name       | Encoding                                          |
|-----------|------------|---------------------------------------------------|
| 0         | `VARINT`   | LEB128 unsigned integer                           |
| 1         | `SVARINT`  | LEB128 zig-zag signed integer                     |
| 2         | `FIXED32`  | 4 bytes, big-endian                               |
| 3         | `FIXED64`  | 8 bytes, big-endian                               |
| 4         | `BYTES`    | `varint length || bytes`                          |
| 5         | `MESSAGE`  | `varint length || nested encoded fields`          |
| 6         | `LIST`     | `varint length || elements with type prefix`      |
| 7         | `TAGGED`   | `varint tag || nested encoded fields`             |

Messages and tagged unions are encoded as a sequence of fields, each
prefixed with its `field_id` and `wire_type`. Decoders match by
`field_id` (not by position) and skip unknown `field_id`s by reading
their declared `wire_type`.

This format is intentionally similar in spirit to Protocol Buffers'
wire format, but designed for the specific concerns of this protocol:

- Big-endian for hex-dump readability and "network feel".
- No `varint`-only restriction on integers; fixed widths exist where
  natural (e.g. timestamps, color channels) so the wire matches the
  conceptual width.
- A first-class `TAGGED` wire type for tagged unions, so they don't have
  to be reified as `oneof`-style hacks.

A canonical hex dump of a `HELLO_OK` selecting version `0.1.0` is
included in `crates/phux-protocol/tests/snapshots/hello_ok_v0_1_0.snap`
once the codec exists.

---

## Appendix B. Reserved ranges

For implementers extending the protocol:

- Message IDs `0x04..=0x0F` and `0x83..=0x8F`: reserved for connection-
  lifecycle messages.
- Message IDs `0x14..=0x1F` and `0x93..=0x9F`: reserved for hot-path
  messages.
- Message IDs `0x31..=0x3F` and `0xC2..=0xCF`: reserved for control
  plane.
- Message IDs `0x41..=0x4F` and `0xB3..=0xBF`: reserved for events.

DiffOp tag values, NamedKey enum values, and ErrorCode enum values are
allocated sequentially. Implementers proposing new values open a PR
against this document.

---

## Appendix C. Changelog

| Version | Date       | Notes                                        |
|---------|------------|----------------------------------------------|
| 0.1.0-draft | 2026-05-24 | Initial draft. Subject to change.            |
| 0.1.0-draft.2 | 2026-05-24 | §7.7, §9, §10.5 revised to mirror libghostty input/OSC APIs. ADR-0006. |
