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
The wire is **asymmetric**:

- **Server → Client (pane content):** VT bytes. The server forwards the
  byte stream produced by each pane's PTY (after canonical parsing into
  the server's `libghostty_vt::Terminal` for state ownership, and after
  per-client capability downsampling — see §6.2, §8).
- **Client → Server (input events):** structured `KeyEvent`,
  `MouseEvent`, `FocusEvent`, paste, and viewport messages — never raw
  VT bytes (§9).

A `libghostty_vt::Terminal` runs on **both** ends. The server's
Terminal is the canonical state (authoritative grid, scrollback,
cursor, modes). The client parses the received VT bytes into its own
local Terminal for rendering. Cell data, cursor position, and pane
modes are queried out of libghostty's `Terminal` API on each end; they
are not separate wire concepts.

This is the protocol's defining trait. Everything else follows from
it. See [ADR-0013] for the design rationale.

[ADR-0013]: ./ADR/0013-libghostty-bytes-on-wire.md

---

## 2. Terminology

| Term | Definition |
|------|------------|
| **Server** | A long-lived process owning all multiplexer state for one operating-system user. |
| **Client** | A process that attaches to a server, presenting sessions to a user. |
| **Session** | A named, persistent container for one or more windows. Survives client disconnect. |
| **Window** | A tab inside a session. Contains a layout tree of panes. |
| **Pane** | A leaf of a window's layout tree. Has one PTY and one terminal grid. |
| **Frame** | A server-emitted `PANE_OUTPUT` carrying a contiguous batch of VT bytes for one pane, identified by a monotonically increasing per-pane `seq`. |
| **Grid** | The two-dimensional cell matrix that is a pane's visible viewport. |
| **Scrollback** | Lines that have scrolled out of the grid but are retained for review. |
| **Cell** | One character position in a grid: a grapheme cluster plus rendering attributes. |

---

## 3. Architecture overview

```
┌────────────────────────────┐                  ┌─────────────────────────┐
│        phux server         │ ◄─── transport ►│      phux client        │
│                            │                  │                         │
│  Sessions                  │   PANE_OUTPUT    │  Renderer               │
│  └─ Windows                │  (VT bytes, S→C) │  ├─ Terminal            │
│      └─ Panes              │  ───────────────►│  │   (libghostty-vt;    │
│          ├─ PTY            │                  │  │    local parse for   │
│          └─ Terminal       │     INPUT_KEY    │  │    rendering)        │
│             (libghostty-vt)│  ◄───────────────│  └─ Render loop         │
│             — canonical    │                  │     (per-row dirty)     │
└────────────────────────────┘                  └─────────────────────────┘
```

The server is authoritative for all state. The client's local Terminal
is a mirror, fed by the server's downsampled VT byte stream; the
client's renderer uses libghostty's `RenderState` per-row dirty
tracking for efficient redraw. The server is the only source of truth.

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
    rendering: RenderingMode,      // Diff | VtReplay (deprecated; see prose below)
}

ServerCapabilities {
    features: bitset<ServerFeature>,
    // ServerFeature variants:
    //   REATTACH_REPLAY    — server retains scrollback for reattaching clients
    //   PANE_RECORDING     — server can record pane I/O to disk
    //   AGENT_HOOKS        — server supports typed agent-style hooks
    //   IMAGE_PASSTHROUGH  — server forwards image protocols transparently
    //   CC_FRONTEND        — server can speak tmux control mode in
    //                        addition to native cell-diff (reserved;
    //                        unset in v0.1; see ADR-0010)
    max_message_size: u32,
}
```

Servers MUST adapt outbound `PANE_OUTPUT` (§8) byte streams to each
client's capabilities. The downsampling is performed as a server-side
**VT byte stream rewrite**, not a per-cell structured transform:

- **Color.** For a client advertising `Indexed256`, the server MUST
  rewrite truecolor SGR sequences (`CSI 38;2;R;G;B m` / `CSI 48;2;R;G;B m`)
  to their indexed equivalents (`CSI 38;5;N m` / `CSI 48;5;N m`) before
  forwarding. For a client advertising `Indexed16`, the server MUST
  further quantize to the standard / bright ANSI ranges
  (`CSI 3N m` / `CSI 9N m` and their background counterparts).
- **Images.** For each image protocol the client does not advertise
  (`Sixel`, `KittyGraphics`, `Iterm2`), the server MUST drop or
  transform the corresponding escape sequences before forwarding so the
  client never receives bytes for a protocol it cannot render.
- **Keyboard protocols.** APC keyboard-reply sequences (kitty keyboard
  protocol, modifyOtherKeys, etc.) MUST be gated to clients advertising
  the matching `kbd_protocols` bit; the server's canonical Terminal
  still processes them locally, but they are stripped from the outbound
  byte stream for clients that did not negotiate the protocol.
- **Hyperlinks (OSC 8) and other terminal features** SHOULD be stripped
  when the corresponding capability bit is unset.

The downsampling MUST be deterministic and MUST NOT alter the visible
grid state on the client beyond what the capability reduction implies.
See [ADR-0013] for the rationale and the byte-stream rewriter design.

The legacy `RenderingMode` field on `ClientCapabilities` (`Diff` vs.
`VtReplay`) is **deprecated** as of this revision: with `PANE_OUTPUT`
carrying VT bytes, every client renders via local libghostty parse —
there is no longer a structured-diff alternative. Decoders MUST accept
the field for forward-compat and SHOULD ignore its value.

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

The **Status** column below tracks reference-implementation coverage in
this repository as of 2026-05-26. It is informative, not normative: a
conforming peer implements whatever the negotiated version of this
spec requires (see §16), independent of what `phux-protocol` happens
to ship today.

- `shipped` — message is in [`phux_protocol::wire::frame::FrameKind`]
  and round-trips through the encoder/decoder.
- `partial` — message is on the wire but at least one end does not yet
  produce or consume it (e.g. the client does not yet emit
  `VIEWPORT_RESIZE` even though the frame round-trips).
- `spec-only` — defined here, no codec entry yet.

[`phux_protocol::wire::frame::FrameKind`]: ./crates/phux-protocol/src/wire/frame.rs

### 7.1 Client → Server

| ID    | Name              | Reference | Status    |
|-------|-------------------|-----------|-----------|
| 0x01  | `HELLO`           | §6.1      | shipped   |
| 0x02  | `ATTACH`          | §13       | shipped   |
| 0x03  | `DETACH`          | §7.3      | shipped   |
| 0x10  | `INPUT_KEY`       | §9.1      | shipped   |
| 0x11  | `INPUT_PASTE`     | §9.4      | partial (server decodes; client doesn't emit yet) |
| 0x12  | `INPUT_MOUSE`     | §9.2      | partial (server decodes; client doesn't emit yet) |
| 0x13  | `INPUT_RAW`       | §9.5      | spec-only |
| 0x14  | `INPUT_FOCUS`     | §9.3      | partial (server decodes; client doesn't emit yet) |
| 0x20  | `VIEWPORT_RESIZE` | §10.5     | partial (frame defined; SIGWINCH not wired) |
| 0x21  | `FRAME_ACK`       | §12       | spec-only |
| 0x30  | `COMMAND`         | §11       | spec-only |
| 0x40  | `SUBSCRIBE`       | §7.4      | spec-only |
| 0x7F  | `PING`            | §7.5      | shipped   |

### 7.2 Server → Client

| ID    | Name              | Reference | Status    |
|-------|-------------------|-----------|-----------|
| 0x80  | `HELLO_OK`        | §6.1      | spec-only (today: `HELLO` is used symmetrically) |
| 0x81  | `ATTACHED`        | §13       | shipped   |
| 0x82  | `DETACHED`        | §7.3      | shipped   |
| 0x90  | `PANE_OUTPUT`     | §8        | shipped   |
| 0x91  | `PANE_SNAPSHOT`   | §8.4      | shipped   |
| 0x92  | `PANE_RESIZED`    | §10.5     | spec-only |
| 0xA0  | `PANE_OPENED`     | §10.2     | spec-only |
| 0xA1  | `PANE_CLOSED`     | §10.2     | spec-only |
| 0xA2  | `WINDOW_OPENED`   | §10.1     | spec-only |
| 0xA3  | `WINDOW_CLOSED`   | §10.1     | spec-only |
| 0xA4  | `WINDOW_RENAMED`  | §10.1     | spec-only |
| 0xA5  | `LAYOUT_CHANGED`  | §10.3     | spec-only |
| 0xA6  | `SESSION_OPENED`  | §10       | spec-only |
| 0xA7  | `SESSION_CLOSED`  | §10       | spec-only |
| 0xA8  | `SESSION_RENAMED` | §10       | spec-only |
| 0xA9  | `FOCUS_CHANGED`   | §10.4     | spec-only |
| 0xB0  | `BELL`            | §7.6      | shipped   |
| 0xB1  | `OSC_EVENT`       | §7.7      | spec-only |
| 0xB2  | `ALERT`           | §7.8      | spec-only |
| 0xC0  | `COMMAND_RESULT`  | §11       | spec-only |
| 0xC1  | `ERROR`           | §14       | shipped   |
| 0xFF  | `PONG`            | §7.5      | partial (server sends as a pre-encoded raw frame; no `FrameKind` variant yet) |

> **Note (deprecation).** Earlier drafts of this SPEC carried
> `PANE_DIFF` at `0x90` (S → C) with a structured ops body. It is
> **superseded** by `PANE_OUTPUT` at the same discriminant per
> [ADR-0013]. The discriminant slot is reused; the body shape and
> semantics are entirely different (VT bytes, not `DiffOp` list).
> Implementations of earlier drafts MUST be updated; there is no
> on-wire compatibility between `PANE_DIFF` and `PANE_OUTPUT`.

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

This section is the protocol's centerpiece. The server's
`libghostty_vt::Terminal` is the **canonical** owner of each pane's
grid, scrollback, cursor, and modes. The client runs its own local
`libghostty_vt::Terminal` as a rendering mirror. Pane content flows
between them as a **stream of VT bytes**, not as structured diffs. See
[ADR-0013] for the design rationale (libghostty-bytes-on-wire).

### 8.1 The frame model

A pane's content on the wire is a sequence of `PANE_OUTPUT` frames:

```
PANE_OUTPUT {
    pane_id: PaneId,
    seq: u64,        // monotonic per-pane sequence id, for ack /
                     //   predictive-echo correlation (see §12)
    bytes: bytes,    // VT bytes from the PTY (canonicalised by the
                     //   server's libghostty Terminal and possibly
                     //   downsampled for this client's caps per §6.2)
}
```

The flow is:

1. The pane's PTY emits VT bytes.
2. The server feeds those bytes to the pane's canonical
   `libghostty_vt::Terminal`, which becomes the authoritative parse
   (grid, scrollback, cursor, modes).
3. The server forwards bytes to each attached client, having applied
   per-client capability downsampling (§6.2) — for example rewriting
   truecolor SGR sequences to 256-color or 16-color forms, or stripping
   unsupported image escape sequences.
4. The client feeds the received bytes into its own
   `libghostty_vt::Terminal`. Both ends now hold equivalent (post-
   downsampling) grid state.

Coalescing remains a server concern: the server SHOULD batch bytes
between transport writes, and MAY rate-limit the per-pane output
stream (default cap 60 Hz of `PANE_OUTPUT` emissions; configurable),
but the emissions themselves carry raw PTY bytes — no structured frame
boundaries, no `frame_id` / `base_frame_id` relationship, no
per-emission cursor/modes block. Frame identity is replaced by the
sequence number `seq`, used solely for acknowledgement (§12) and
predictive-echo correlation; `seq` carries no structural meaning.

A `PANE_SNAPSHOT` (§8.4) is a self-contained replay: a synthesized VT
byte sequence that, when applied to a fresh Terminal of the matching
dimensions, reproduces the current grid (and optionally the scrollback).

### 8.2 Cells

Cells are not wire-level concepts in phux. Each end's
`libghostty_vt::Terminal` owns its own grid representation. Clients
that need rendered cell data (for layout, copy/paste, search) use
libghostty's `Terminal::grid_ref()` and related APIs to query their
local Terminal; they do not reconstruct cells from wire frames. Cell
attribute encoding on the wire is whatever the PTY's byte stream
produces (SGR sequences, OSC 8 hyperlinks, etc.), as canonicalised by
the server's Terminal and downsampled per the client's capabilities.

### 8.3 Diff operations

Diff operations are not present on the wire. See [ADR-0013]. The PTY's
own byte stream IS the canonical delta from one observed state to the
next; libghostty's VT parser applies it deterministically and
identically on the server (canonical) and on each client (mirror).

Local rendering optimisation — skipping unchanged rows on redraw — is a
**client-local** concern using libghostty's `RenderState` per-row
dirty tracking. It is invisible to the wire format. There is no notion
of `CELL_RUN`, `REPEAT`, `CLEAR`, `ERASE_LINE`, `SCROLL_UP`, or
`SCROLL_DOWN` as wire operations; those concepts live inside
libghostty's parser implementation.

Hyperlinks (OSC 8) and image escape sequences (sixel, kitty graphics,
iTerm2) flow as bytes within the same `PANE_OUTPUT` stream, subject
to the capability gating in §6.2.

### 8.4 Snapshots

```
PANE_SNAPSHOT {
    pane_id: PaneId,
    cols: u16,
    rows: u16,
    vt_replay_bytes: bytes,
    scrollback_bytes: optional<bytes>,
}
```

`vt_replay_bytes` is a self-contained VT byte sequence synthesized by
the server from its canonical Terminal's current grid state. When the
client writes the bytes to a fresh `libghostty_vt::Terminal` of the
declared `cols × rows`, the result MUST reproduce the server's grid
state at the moment of snapshot emission. The byte sequence is
**Mosh-style** and **opaque** to the client: the client MUST NOT
attempt to parse or rewrite it beyond feeding it to its Terminal.

Servers SHOULD produce `vt_replay_bytes` whose effect is independent
of any prior Terminal state. A typical implementation begins with
cursor-home + erase-display (resetting visible screen), emits per-row
SGR and cell text, ends with a final cursor-position move and the
appropriate DECSET/DECRST pairs to re-establish modes, and avoids
escape sequences whose meaning depends on prior parser state. The
exact construction is implementation-defined; only the **end result**
(client Terminal grid == server Terminal grid at snapshot time) is
normative.

`scrollback_bytes` is present iff the attaching client requested
scrollback replay (`ATTACH.request_scrollback = true`, §13), bounded
by `ATTACH.scrollback_limit_lines`. It is also an opaque VT byte
sequence; when applied to a fresh Terminal **before** `vt_replay_bytes`
(or under whatever construction the server chooses), it reproduces
the requested scrollback history.

Servers emit `PANE_SNAPSHOT` when:

1. A client first attaches (§13).
2. Backpressure forced the server to compact pending output and resume
   from a known state (§12).
3. The grid resized in a way that requires full retransmission (§10.5).
4. The protocol requires it for correctness in any future case.

After a `PANE_SNAPSHOT`, the next `PANE_OUTPUT` for the same pane
continues the live byte stream. The client's local Terminal is in
sync after applying the snapshot bytes and before consuming the next
`PANE_OUTPUT`.

### 8.5 Cursor and modes

Cursor state (position, visibility, shape, blink) and pane modes
(altscreen, bracketed paste, app cursor keys, mouse protocol,
focus reporting, origin mode, etc.) live entirely inside each end's
`libghostty_vt::Terminal`. They are **not** separate wire concepts.

Clients that need cursor or mode state — for example to render a
local cursor overlay, to decide whether to forward mouse events, or
to enable bracketed paste in the outer terminal — MUST query their
local Terminal via libghostty's API (`Terminal::screen()`,
`Terminal::modes()`, etc.). They MUST NOT expect a `CursorState` or
`PaneModes` block in `PANE_OUTPUT` or `PANE_SNAPSHOT`.

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
[`ADR/0013-libghostty-bytes-on-wire.md`](./ADR/0013-libghostty-bytes-on-wire.md)
(which supersedes ADR-0002) and [`CONTRIBUTING.md`](./CONTRIBUTING.md).

---

## 12. Flow control

### 12.1 Output pacing

The server MUST cap per-pane `PANE_OUTPUT` emission at a configurable
refresh rate (default 60 Hz). Between emissions, PTY bytes are
accumulated and shipped as a single coalesced `PANE_OUTPUT` carrying
the batched VT bytes. There is no "every byte emits a frame" mode;
that would not survive a `yes` flood.

Coalescing operates at the byte level: the server concatenates the
PTY's output across the pacing interval into the next `PANE_OUTPUT`'s
`bytes` field. Because libghostty's parser is deterministic over the
full byte stream, coalescing has no observable effect on the client's
local Terminal state beyond timing.

### 12.2 Per-pane acknowledgement

Clients acknowledge `PANE_OUTPUT` emissions they have processed
(applied to their local libghostty `Terminal`):

```
FRAME_ACK { pane_id: PaneId, seq: u64 }
```

`seq` is the monotonic per-pane sequence number from `PANE_OUTPUT`
(§8.1). An ack is cumulative: acknowledging `seq = N` implies all
prior `PANE_OUTPUT`s for that pane up to and including `N` have been
applied.

The server tracks per-client `last_acked_seq` per pane. When
`pending_unacked_bytes` (or equivalently the count of unacked
`PANE_OUTPUT` emissions) for a pane exceeds a configurable
`flow_control_threshold` (default: 32 unacked emissions, per-server
configurable, never disable-able), the server:

1. Stops sending live `PANE_OUTPUT` for that pane to that client.
2. Drops the queued byte backlog for that pane / client.
3. Emits a single `PANE_SNAPSHOT` (§8.4) synthesized from the
   server's canonical Terminal — `vt_replay_bytes` reproduces the
   current grid on a fresh client Terminal.
4. Resumes live `PANE_OUTPUT` from the post-snapshot byte stream.
   The next `seq` after the snapshot establishes a fresh base
   (§13); clients MUST NOT assume `seq` continuity across the
   snapshot boundary.

This is the playbook Mosh uses, generalized to per-pane streams. It
ensures a slow client cannot block the server, and the worst-case
catch-up cost is one snapshot's worth of synthesized VT bytes, not an
unbounded queue of accumulated PTY output.

Scrollback that scrolls off during a backpressure-induced snapshot is
**not** retransmitted to the lagging client; clients that require
gap-free scrollback during heavy output SHOULD configure their server
with a higher `flow_control_threshold` or accept snapshot-driven
truncation. Servers MAY include bounded scrollback in
`PANE_SNAPSHOT.scrollback_bytes` if configured to do so on
backpressure (implementation-defined; not normative).

### 12.3 Per-client isolation

Each connected client has its own outbound queue. A wedged client whose
queue exceeds its bound is forcibly disconnected with
`DETACHED { reason: PROTOCOL_ERROR }`. Other clients are unaffected.

---

## 13. State replay on attach

When a client sends `ATTACH`, the server's response sequence is:

1. `ATTACHED { snapshot: SessionSnapshot, initial_client_id }` — full
   graph of sessions, windows, panes, layouts, the attaching client's
   initial focus, and per-pane size. This is **session graph metadata
   only**; it carries no pane content.
2. For each pane in the focused window of the targeted session, one
   `PANE_SNAPSHOT { pane_id, cols, rows, vt_replay_bytes, scrollback_bytes? }`
   per §8.4. The client applies `scrollback_bytes` (if present) and
   then `vt_replay_bytes` to a fresh `libghostty_vt::Terminal` of the
   declared dimensions; the client's local Terminal is then in sync
   with the server's canonical Terminal for that pane.
3. Subsequent `PANE_OUTPUT { pane_id, seq, bytes }` messages flow
   live, continuing the per-pane VT byte stream from where the
   snapshot left off.

The per-pane `seq` numbering used by `PANE_OUTPUT` resumes from the
server's chosen base after snapshot emission; clients MUST treat the
first `PANE_OUTPUT` after a `PANE_SNAPSHOT` as authoritative for the
sequence base and MUST NOT assume `seq` continuity across the
snapshot boundary. See [ADR-0013] for the bytes-on-wire rationale.

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
   - `PANE_OUTPUT`, `PANE_SNAPSHOT`, `PANE_RESIZED`,
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

`PhysicalKey` enum values and `ErrorCode` enum values are allocated
sequentially. Implementers proposing new values open a PR against
this document.

(Earlier drafts of this SPEC reserved a `DiffOp` tag range here; per
[ADR-0013], pane content is now a VT byte stream and `DiffOp` no
longer exists as a wire concept.)

---

## Appendix C. Changelog

| Version | Date       | Notes                                        |
|---------|------------|----------------------------------------------|
| 0.1.0-draft | 2026-05-24 | Initial draft. Subject to change.            |
| 0.1.0-draft.2 | 2026-05-24 | §7.7, §9, §10.5 revised to mirror libghostty input/OSC APIs. ADR-0006. |
| 0.1.0-draft.3 | 2026-05-25 | §8 rewritten for bytes-on-wire pane state sync; `PANE_DIFF` superseded by `PANE_OUTPUT`; `PANE_SNAPSHOT` carries `vt_replay_bytes`; §6.2 capability downsampling described as a server-side VT byte stream rewrite; §13 replay sequence and §16 conformance updated. ADR-0013. |
| 0.1.0-draft.4 | 2026-05-26 | Post-ADR-0013 cleanup: §2 Frame term re-anchored on per-pane `seq`; §6.2 inline comment on deprecated `RenderingMode`; §11.1 ADR cross-reference points at ADR-0013; §12 flow control rewritten for `PANE_OUTPUT` / per-pane `seq` (was `PANE_DIFF` / `frame_id`); Appendix B reserved-range guidance drops `DiffOp`. |
| 0.1.0-draft.5 | 2026-05-26 | Editorial: §7.1 / §7.2 message catalogs grow a `Status` column tracking reference-implementation coverage (informative, non-normative). No wire change. |
