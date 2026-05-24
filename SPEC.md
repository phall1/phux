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

A general-purpose channel for terminal-originated events that don't
deserve a dedicated message type:

```
OSC_EVENT {
    pane_id: PaneId,
    event: OscEvent,
}

OscEvent = tagged_union {
    TITLE         { title: str },                 // OSC 0/1/2
    CURRENT_DIR   { uri: str },                   // OSC 7
    HYPERLINK     { id: u32, uri: str, params: str },  // OSC 8
    USER_NOTIFICATION { body: str },              // OSC 9 / iTerm2 / OSC 777
    PROMPT_MARK   { kind: PromptMarkKind, info: str },  // OSC 133
    CLIPBOARD     { selection: ClipboardSelection, data: bytes },  // OSC 52
    EXIT_CODE     { code: i32 },                  // synthesized when PTY exits
    CUSTOM        { id: u32, payload: bytes },    // pass-through, for future extensions
}
```

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

This section is the second-most-important part of the protocol. Getting
input encoding right is what allows phux to faithfully carry modifier-
rich key events through to inner programs, even when traditional
multiplexers cannot.

### 9.1 INPUT_KEY

```
INPUT_KEY {
    pane_id: PaneId,
    key: KeyEvent,
}

KeyEvent {
    key: Key,
    mods: ModSet,
    event: KeyEventKind,
    text: optional<str>,
    associated: optional<Key>,
}

Key = tagged_union {
    CHAR(u32),               // Unicode codepoint of the layout-resolved key
    NAMED(NamedKey),         // a non-character key
}

NamedKey = enum {
    ESCAPE = 1, ENTER = 2, TAB = 3, BACKSPACE = 4, DELETE = 5, INSERT = 6,
    HOME = 7, END = 8, PAGE_UP = 9, PAGE_DOWN = 10,
    ARROW_UP = 11, ARROW_DOWN = 12, ARROW_LEFT = 13, ARROW_RIGHT = 14,
    F1 = 20, F2 = 21, /* ... */ F35 = 54,
    KEYPAD_0 = 60, /* ... */ KEYPAD_9 = 69,
    KEYPAD_DECIMAL = 70, KEYPAD_DIVIDE = 71, KEYPAD_MULTIPLY = 72,
    KEYPAD_SUBTRACT = 73, KEYPAD_ADD = 74, KEYPAD_ENTER = 75, KEYPAD_EQUAL = 76,
    KEYPAD_SEPARATOR = 77, KEYPAD_LEFT = 78, KEYPAD_RIGHT = 79,
    KEYPAD_UP = 80, KEYPAD_DOWN = 81, KEYPAD_PAGE_UP = 82, KEYPAD_PAGE_DOWN = 83,
    KEYPAD_HOME = 84, KEYPAD_END = 85, KEYPAD_INSERT = 86, KEYPAD_DELETE = 87,
    KEYPAD_BEGIN = 88,
    CAPS_LOCK = 100, SCROLL_LOCK = 101, NUM_LOCK = 102,
    PRINT_SCREEN = 103, PAUSE = 104, MENU = 105,
    LEFT_SHIFT = 110, RIGHT_SHIFT = 111,
    LEFT_CONTROL = 112, RIGHT_CONTROL = 113,
    LEFT_ALT = 114, RIGHT_ALT = 115,
    LEFT_SUPER = 116, RIGHT_SUPER = 117,
    LEFT_HYPER = 118, RIGHT_HYPER = 119,
    LEFT_META = 120, RIGHT_META = 121,
    ISO_LEVEL3_SHIFT = 130, ISO_LEVEL5_SHIFT = 131,
    MEDIA_PLAY = 200, MEDIA_PAUSE = 201, MEDIA_PLAY_PAUSE = 202,
    MEDIA_STOP = 203, MEDIA_NEXT = 204, MEDIA_PREVIOUS = 205,
    VOLUME_UP = 210, VOLUME_DOWN = 211, MUTE = 212,
    // 1..=255 reserved for protocol use.
}

ModSet = bitset {
    SHIFT     = 0x01,
    CTRL      = 0x02,
    ALT       = 0x04,
    SUPER     = 0x08,
    HYPER     = 0x10,
    META      = 0x20,
    CAPS_LOCK = 0x40,
    NUM_LOCK  = 0x80,
}

KeyEventKind = enum {
    PRESS = 0,
    RELEASE = 1,
    REPEAT = 2,
}
```

Notes:

- `key` is the **layout-resolved** key. A US-QWERTY user pressing the
  `a` key produces `CHAR(0x61)`. An AZERTY user pressing the same
  physical key but with their layout produces `CHAR(0x71)` (`q`).
- `mods` reports which modifiers were held at the time of the event.
  CAPS_LOCK and NUM_LOCK are reported as state, not as key events of
  their own (those keys also produce a NAMED event when pressed).
- `text` is the IME-resolved text produced by the event, if any.
  Encoders SHOULD include it when it differs from the simple result of
  applying `mods` to `key`.
- `associated` carries the alternate-layout representation for
  applications that opt into kitty keyboard protocol features. Encoders
  MAY include it; decoders MAY ignore it.

### 9.2 INPUT_MOUSE

```
INPUT_MOUSE {
    pane_id: PaneId,
    event: MouseEvent,
}

MouseEvent {
    kind: MouseEventKind,
    button: MouseButton,
    col: u16,           // 0-based pane-local cell column
    row: u16,           // 0-based pane-local cell row
    pixel_x: optional<u16>,  // for sub-cell precision; pane-local
    pixel_y: optional<u16>,
    mods: ModSet,
}

MouseEventKind = enum {
    PRESS = 0, RELEASE = 1, DRAG = 2, MOVE = 3, SCROLL = 4,
}

MouseButton = enum {
    NONE = 0,
    LEFT = 1, MIDDLE = 2, RIGHT = 3,
    SCROLL_UP = 4, SCROLL_DOWN = 5, SCROLL_LEFT = 6, SCROLL_RIGHT = 7,
    EXTRA_1 = 8, EXTRA_2 = 9, EXTRA_3 = 10, EXTRA_4 = 11,
}
```

The server is responsible for re-encoding the event for whatever mouse
protocol the inner program asked for (legacy X10, SGR-1006, etc.).

### 9.3 Server-side encoding of input

When the server receives an `INPUT_KEY` or `INPUT_MOUSE` for a pane,
it queries the pane's negotiated terminal-side keyboard / mouse
protocol (from libghostty-vt's mode state) and renders the event to the
bytes that protocol expects, writing them to the PTY. Clients never
emit raw key bytes.

This design point is what allows phux to support, end-to-end through
the multiplexer:

- **The kitty keyboard protocol (KIP)** in its progressive-enhancement
  entirety: modifier-only events, release events, alternate-key
  reporting, associated text. Carrying KIP cleanly is the **principal
  reason this protocol exists in this shape**; structured key events
  are the only way to get it right.
- Unambiguous distinction between Ctrl+I and Tab, between Esc and
  Alt-letter, between Ctrl+Enter and a literal `J`.
- IME composition and dead keys.
- Modifier-rich combinations (Hyper, Super) for tiling-WM-style
  bindings, with correct passthrough to inner applications.

Multiplexers that speak raw VT between client and server cannot
faithfully transport these events, and lossy translation has been a
durable source of papercuts in tmux. phux solves this categorically.

### 9.4 INPUT_PASTE

```
INPUT_PASTE {
    pane_id: PaneId,
    data: bytes,
    bracketed: bool,
}
```

A paste is a bulk text insertion. If `bracketed` is true and the pane
has `BRACKETED_PASTE` active, the server emits the appropriate
bracketed-paste sequence around the data. If false, the data is sent
as-is.

### 9.5 INPUT_RAW

```
INPUT_RAW {
    pane_id: PaneId,
    data: bytes,
}
```

A deliberate escape hatch. Bytes in `data` are written verbatim to the
pane's PTY. Reserved for the small set of cases not modelled by
`INPUT_KEY` / `INPUT_PASTE` / `INPUT_MOUSE` (chiefly: direct PTY testing
and command interpolation from configs). Servers MUST NOT silently
re-interpret `INPUT_RAW`; clients SHOULD avoid using it in normal
operation.

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

`SessionId` and `WindowId` are opaque `u32` values, stable for the life
of the server. They are not reused after a session/window closes (the
counter is monotonic for the server's lifetime).

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

The client's outer terminal size is signalled with `VIEWPORT_RESIZE`:

```
VIEWPORT_RESIZE {
    cols: u16,
    rows: u16,
    pixel_w: optional<u16>,
    pixel_h: optional<u16>,
}
```

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
