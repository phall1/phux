# phux Wire Protocol

**Version:** 0.1.0-draft
**Status:** Working draft. Not stable.

This document specifies the bytes on the wire between a phux server and
a phux client. It is **normative**: implementations conform to this
document, not to whatever the reference implementation happens to do.

The protocol is organized into three layers per
[ADR-0015](./ADR/0015-protocol-layering.md): **L1** (Terminal substrate,
MUST), **L2** (Collection lifecycle, OPTIONAL service), **L3**
(Metadata storage, OPTIONAL service). The Terminal is the wire's
primary identity ([ADR-0016](./ADR/0016-terminal-id-as-wire-primary.md));
session-window-pane-layout-focus vocabulary is a convention of the
reference TUI consumer, not a wire concept
([ADR-0017](./ADR/0017-tui-not-protocol-privileged.md)). See those
ADRs for the rationale that shapes this document.

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

phux is a terminal multiplexer. A long-lived server owns **Terminals**:
each Terminal backs one PTY and one libghostty grid. Clients attach to
the server over a reliable byte stream and present Terminals to users —
as a TUI inside another terminal, as a native GUI, as an agent harness,
or as something else entirely. The Terminal is the wire's load-bearing
primitive; everything else is an optional layered service on top of it.

The protocol described here is the contract between server and client.
The wire is **asymmetric**:

- **Server → Client (Terminal content):** VT bytes. The server
  forwards the byte stream produced by each Terminal's PTY (after
  canonical parsing into the server's `libghostty_vt::Terminal` for
  state ownership, and after per-client capability downsampling — see
  §6.2, §8).
- **Client → Server (input events):** structured `KeyEvent`,
  `MouseEvent`, `FocusEvent`, paste, and viewport messages — never raw
  VT bytes (§9).

A `libghostty_vt::Terminal` runs on **both** ends. The server's
Terminal is the canonical state (authoritative grid, scrollback,
cursor, modes). The client parses the received VT bytes into its own
local Terminal for rendering. Cell data, cursor position, and Terminal
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
| **Client** | A process that attaches to a server, presenting Terminals to a user. |
| **Terminal** | A managed terminal: one PTY, one `libghostty_vt::Terminal` parsing its bytes, one stable `TerminalId`. The L1 substrate primitive (ADR-0015, ADR-0016). |
| **Collection** | An L2 optional service: a named lifecycle bundle of Terminals (ADR-0015 §"L2"). May not be implemented; consumers opt in via `HELLO.layers`. |
| **Metadata** | An L3 optional service: a typed key-value store the server hosts but does not interpret (ADR-0015 §"L3"). |
| **Frame** | A server-emitted `TERMINAL_OUTPUT` carrying a contiguous batch of VT bytes for one Terminal, identified by a monotonically increasing per-Terminal `seq`. |
| **Grid** | The two-dimensional cell matrix that is a Terminal's visible viewport. |
| **Scrollback** | Lines that have scrolled out of the grid but are retained for review. |
| **Cell** | One character position in a grid: a grapheme cluster plus rendering attributes. |
| **Tier** | A conformance layer: L1, L2, or L3 (§7, §16). |
| **Substrate consumer** | A consumer that speaks only L1: an agent, a recorder, a CI orchestrator. Sees Terminals; never sees Collections or Metadata. |
| **Reference TUI** | The first-party tmux-shaped consumer. Speaks L1+L2+L3. Session, window, pane, layout, and focus are this consumer's conventions, implemented as L3 metadata; they are not wire concepts (ADR-0017). |

---

## 3. Architecture overview

```
┌────────────────────────────┐                  ┌─────────────────────────┐
│        phux server         │ ◄─── transport ►│      phux client        │
│                            │                  │                         │
│  L1: Terminals             │ TERMINAL_OUTPUT  │  Renderer               │
│  ├─ PTY                    │  (VT bytes, S→C) │  ├─ Terminal            │
│  └─ libghostty Terminal    │  ───────────────►│  │   (libghostty-vt;    │
│     (canonical)            │                  │  │    local parse for   │
│                            │     INPUT_KEY    │  │    rendering)        │
│  L2: Collections (opt)     │  ◄───────────────│  └─ Render loop         │
│  L3: Metadata    (opt)     │                  │     (per-row dirty)     │
└────────────────────────────┘                  └─────────────────────────┘
```

The server is authoritative for all state. L1 (Terminal substrate) is
always on; L2 (Collection) and L3 (Metadata) are optional services
that the server may or may not mount, and consumers opt in via
`HELLO.layers`. The client's local libghostty `Terminal` is a mirror,
fed by the server's downsampled VT byte stream; the client's renderer
uses libghostty's `RenderState` per-row dirty tracking for efficient
redraw. The server is the only source of truth.

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
    client_caps: ClientCapabilities,   // includes layers: bitset<Layer>
}

Server → Client:  HELLO_OK {
    version: Version,
    server_caps: ServerCapabilities,   // includes layers: bitset<Layer>
    server_id: bytes,
}
```

`VersionRange` is `{ min: Version, max: Version }` inclusive. The
client's `versions` field lists ranges it supports (typically one).

The server MUST select the highest version that lies in some range of
the client's `versions` AND is supported by the server itself, and echo
it back as `version`. If no such version exists, the server MUST send
`ERROR { code: VERSION_INCOMPATIBLE }` and close.

The `layers` bit-field on `ClientCapabilities` and `ServerCapabilities`
declares which conformance tiers (§16) each side speaks. Per
[ADR-0015](./ADR/0015-protocol-layering.md) §"Conformance tiers":

- The client's `layers` lists what it wants. L1 is always implied; a
  client MAY omit higher tiers (an agent SDK declares L1 only).
- The server's `layers` (in `HELLO_OK`) lists what it implements. L1
  is always implemented; the server MAY mount L2, L3, or neither.
- The **negotiated tier set** is the intersection of the two `layers`
  bit-fields. The server MUST NOT send messages from tiers outside
  the intersection, and the client MUST NOT send messages from tiers
  outside the intersection. Decoders MUST treat the receipt of an
  out-of-tier message as a protocol error.

After `HELLO_OK`, the negotiated version and tier set govern the rest
of the connection. Sending HELLO twice on the same connection is an
error.

### 6.2 Capability negotiation

Capabilities are advertised once, at HELLO time, and apply for the life
of the connection. They are not renegotiated.

```
Layer = bitset (u8) {
    L1 = 0x01,   // Terminal substrate (always implemented; MUST be set)
    L2 = 0x02,   // Collection lifecycle (optional service)
    L3 = 0x04,   // Metadata storage (optional service)
}

ClientCapabilities {
    kbd_protocols: bitset<KeyboardProtocol>,
    mouse_protocols: bitset<MouseProtocol>,
    color: ColorSupport,           // TrueColor | Indexed256 | Indexed16
    images: bitset<ImageProtocol>, // Sixel | KittyGraphics | Iterm2
    hyperlinks: bool,
    unicode_version: u8,
    rendering: RenderingMode,      // Diff | VtReplay (deprecated; see prose below)
    layers: bitset<Layer>,         // tiers the client speaks (§16; ADR-0015)
}

ServerCapabilities {
    features: bitset<ServerFeature>,
    // ServerFeature variants:
    //   REATTACH_REPLAY    — server retains scrollback for reattaching clients
    //   TERMINAL_RECORDING — server can record Terminal I/O to disk
    //   AGENT_HOOKS        — server supports typed agent-style hooks
    //   IMAGE_PASSTHROUGH  — server forwards image protocols transparently
    //   <reserved>         — slot formerly `CC_FRONTEND` per ADR-0010;
    //                        **reclaimed** per ADR-0017. Decoders MUST
    //                        ignore the bit if set. v0.2 may re-use the
    //                        slot.
    max_message_size: u32,
    layers: bitset<Layer>,         // tiers the server implements (§16; ADR-0015)
}
```

The `layers` field is encoded as an additional field within
`ClientCapabilities` / `ServerCapabilities`. Per Appendix A, payloads
are self-delimiting field-id-tagged blobs; decoders that do not
recognize the field MUST skip it. v0.1 wire bytes are unchanged.

The `CC_FRONTEND` bit on `features` is **reclaimed** per
[ADR-0017](./ADR/0017-tui-not-protocol-privileged.md). Earlier drafts
reserved it for a server that could "speak tmux control mode as an
alternative frontend." Under ADR-0017 the reference TUI has no
protocol-level privilege, and `tmux control mode` (when added) is one
L1/L2/L3 consumer among several — no capability bit required.
Decoders MUST ignore the slot.

Servers MUST adapt outbound `TERMINAL_OUTPUT` (§8) byte streams to each
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
`VtReplay`) is **deprecated** as of this revision: with `TERMINAL_OUTPUT`
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

The catalog is organized by **tier** per
[ADR-0015](./ADR/0015-protocol-layering.md):

- **proto** — protocol meta (lifecycle, flow control, errors).
  Required of every consumer that completes a HELLO. Not tier-
  specific.
- **L1** — Terminal substrate. Every conforming consumer speaks L1
  (§16). Carries `TerminalId` per
  [ADR-0016](./ADR/0016-terminal-id-as-wire-primary.md).
- **L2** — Collection lifecycle. Optional service. Servers MAY
  decline to mount L2; clients MAY decline to speak L2.
- **L3** — Metadata storage. Optional service. The reference TUI
  uses L3 to persist its layout tree, window order, focus pointer,
  and other consumer-private state (ADR-0017).
- **cmd** — typed command messages (§11). Carry an L1/L2/L3 payload
  depending on the variant.

The **Status** column tracks reference-implementation coverage in
this repository as of 2026-05-26. It is informative, not normative.

- `shipped` — message is in [`phux_protocol::wire::frame::FrameKind`]
  and round-trips through the encoder/decoder.
- `partial` — message is on the wire but at least one end does not
  yet produce or consume it (e.g. the client does not yet emit
  `VIEWPORT_RESIZE` even though the frame round-trips).
- `spec-only` — defined here, no codec entry yet.
- `TBD` — message family is reserved by ADR-0015 at this tier but
  not yet wire-allocated. Discriminant byte will be assigned when
  the tier ships (target: v0.2). Decoders MUST NOT speculatively
  assume any particular discriminant slot.

[`phux_protocol::wire::frame::FrameKind`]: ./crates/phux-protocol/src/wire/frame.rs

### 7.1 proto — connection lifecycle and flow control

| ID    | Direction | Name              | Reference | Status    |
|-------|-----------|-------------------|-----------|-----------|
| 0x01  | C → S     | `HELLO`           | §6.1      | shipped   |
| 0x02  | C → S     | `ATTACH`          | §13       | shipped   |
| 0x03  | C → S     | `DETACH`          | §7.proto.1| shipped   |
| 0x21  | C → S     | `FRAME_ACK`       | §12       | spec-only |
| 0x40  | C → S     | `SUBSCRIBE`       | §7.proto.2| spec-only |
| 0x7F  | C → S     | `PING`            | §7.proto.3| shipped   |
| 0x80  | S → C     | `HELLO_OK`        | §6.1      | spec-only |
| 0x81  | S → C     | `ATTACHED`        | §13       | shipped   |
| 0x82  | S → C     | `DETACHED`        | §7.proto.1| shipped   |
| 0xC1  | S → C     | `ERROR`           | §14       | shipped   |
| 0xFF  | S → C     | `PONG`            | §7.proto.3| partial   |

### 7.2 L1 — Terminal substrate (MUST)

These are the messages every conforming consumer (L1, L1+L3,
L1+L2+L3) speaks. They carry `TerminalId` per ADR-0016 and form the
substrate over which higher tiers compose. L1 is always implemented
by the server.

| ID    | Direction | Name                 | Reference | Status    |
|-------|-----------|----------------------|-----------|-----------|
| 0x10  | C → S     | `INPUT_KEY`          | §9.1      | shipped   |
| 0x11  | C → S     | `INPUT_PASTE`        | §9.4      | partial   |
| 0x12  | C → S     | `INPUT_MOUSE`        | §9.2      | partial   |
| 0x13  | C → S     | `INPUT_RAW`          | §9.5      | spec-only |
| 0x14  | C → S     | `INPUT_FOCUS`        | §9.3      | partial   |
| 0x20  | C → S     | `VIEWPORT_RESIZE`    | §10.2     | partial   |
| 0x90  | S → C     | `TERMINAL_OUTPUT`    | §8        | shipped   |
| 0x91  | S → C     | `TERMINAL_SNAPSHOT`  | §8.4      | shipped   |
| 0x92  | S → C     | `TERMINAL_RESIZED`   | §10.2     | spec-only |
| 0xA0  | S → C     | `TERMINAL_OPENED`    | §10.1     | spec-only |
| 0xA1  | S → C     | `TERMINAL_CLOSED`    | §10.1     | spec-only |
| 0xB0  | S → C     | `BELL`               | §7.L1.1   | shipped   |
| 0xB1  | S → C     | `TERMINAL_EVENT`     | §7.L1.2   | spec-only |
| 0xB2  | S → C     | `ALERT`              | §7.L1.3   | spec-only |

L1 commands (§11): `SPAWN`, `ATTACH_TERMINAL`, `DETACH_TERMINAL`,
`KILL_TERMINAL`. These ride on the generic `COMMAND` envelope and
expect L1-shaped `CommandResult` payloads (typically a `TerminalId`).

> **Note (deprecation).** Earlier drafts of this SPEC carried
> `PANE_DIFF` at `0x90` (S → C) with a structured ops body, and named
> the byte-stream successor `PANE_OUTPUT`. The wire bytes (frame body
> shape) match the current `TERMINAL_OUTPUT` exactly; the rename to
> `TERMINAL_*` and `terminal_id` per ADR-0016 is naming-only.
> Implementations of pre-0.1.0-draft.3 drafts MUST be updated; there
> is no on-wire compatibility between `PANE_DIFF` and
> `TERMINAL_OUTPUT`.

### 7.3 L2 — Collection lifecycle (OPTIONAL)

A Collection is a named lifecycle bundle of Terminals
([ADR-0015](./ADR/0015-protocol-layering.md) §"L2"). A Terminal MAY
belong to zero or one Collection. Killing a Collection MUST kill its
member Terminals atomically. Detaching all clients from a Collection
MUST leave the Collection and its Terminals alive.

L2 messages, **reserved** by ADR-0015 but not yet wire-allocated:

| Direction | Name                              | Reference | Status |
|-----------|-----------------------------------|-----------|--------|
| C → S (cmd) | `CREATE_COLLECTION`             | §11.L2    | TBD    |
| C → S (cmd) | `ADD_TERMINAL_TO_COLLECTION`    | §11.L2    | TBD    |
| C → S (cmd) | `REMOVE_TERMINAL_FROM_COLLECTION`| §11.L2   | TBD    |
| C → S (cmd) | `RENAME_COLLECTION`             | §11.L2    | TBD    |
| C → S (cmd) | `KILL_COLLECTION`               | §11.L2    | TBD    |
| C → S (cmd) | `LIST_COLLECTIONS`              | §11.L2    | TBD    |
| S → C     | `COLLECTION_OPENED`               | §7.L2     | TBD    |
| S → C     | `COLLECTION_CLOSED`               | §7.L2     | TBD    |
| S → C     | `COLLECTION_RENAMED`              | §7.L2     | TBD    |
| S → C     | `COLLECTION_MEMBERSHIP_CHANGED`   | §7.L2     | TBD    |

Discriminant bytes are allocated when L2 lands (target: v0.2). The
earlier `SESSION_OPENED` / `SESSION_CLOSED` / `SESSION_RENAMED`
messages at `0xA6` / `0xA7` / `0xA8` from `0.1.0-draft.6` are
**withdrawn**: under ADR-0017 "session" is a TUI convention, not a
wire concept. The bundle-as-named-lifecycle semantics it implied
re-appear here as Collection, with the same atomic-kill invariant.

### 7.4 L3 — Metadata storage (OPTIONAL)

A typed key-value store the server hosts and does not interpret.
Scopes:

- `Terminal { terminal_id, key, value }`
- `Collection { collection_id, key, value }` — present only if L2
  is also implemented
- `Global { key, value }`

Values are opaque bytes. The server enforces nothing beyond size
limits. A recommended convention is CBOR-encoded structured data
with a versioned key (`phux.tui.layout/v1`,
`phux.tui.window_order/v1`); see §17 for the reference-TUI schema.

L3 messages, **reserved** by ADR-0015 but not yet wire-allocated:

| Direction | Name                | Reference | Status |
|-----------|---------------------|-----------|--------|
| C → S (cmd) | `GET_METADATA`    | §11.L3    | TBD    |
| C → S (cmd) | `SET_METADATA`    | §11.L3    | TBD    |
| C → S (cmd) | `DELETE_METADATA` | §11.L3    | TBD    |
| C → S (cmd) | `LIST_METADATA`   | §11.L3    | TBD    |
| C → S     | `SUBSCRIBE_METADATA`| §11.L3    | TBD    |
| S → C     | `METADATA_CHANGED`  | §7.L3     | TBD    |

Discriminant bytes are allocated when L3 lands (target: v0.2). A
client subscribing to `METADATA_CHANGED` for a scope MUST receive an
event when any key in that scope is written or deleted; the value
itself is not carried in the event (consumers `GET_METADATA` after
the change-notification).

### 7.proto.1 DETACH / DETACHED

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
    SESSION_KILLED    = 2,  // legacy name; retained for v0.1 wire compat.
                            //   Under ADR-0015 this maps to "the
                            //   Collection the attach was rooted in was
                            //   killed." Renamed in v0.2.
    REPLACED          = 3,  // another client took over an exclusive attach
    PROTOCOL_ERROR    = 4,
    INTERNAL_ERROR    = 255,
}
```

### 7.proto.2 SUBSCRIBE

Reserved for opting in/out of notification streams (e.g. only the focused
client should receive `BELL` for inactive Terminals). Format defined in
v0.2.

### 7.proto.3 PING / PONG

```
PING { nonce: u64 }
PONG { nonce: u64 }
```

A peer receiving `PING` MUST respond with `PONG` carrying the same nonce
within a reasonable interval. PING/PONG is liveness only — clients and
servers MAY use it for keepalive; absence of pongs SHOULD NOT be
interpreted as anything other than a transport failure.

### 7.L1.1 BELL

```
BELL { terminal_id: TerminalId }
```

The Terminal received a bell character. The server MUST NOT translate
this into VT output; clients decide policy.

### 7.L1.2 TERMINAL_EVENT

A channel for terminal-originated events the server has parsed (via
libghostty-vt's OSC parser) and chooses to surface to clients. Under
[ADR-0015](./ADR/0015-protocol-layering.md), `TERMINAL_EVENT` is a
load-bearing L1 surface: it is how an L1-only consumer (agent,
recorder, CI orchestrator) answers questions like "did the command
finish, what was the exit code, what directory am I in?"

```
TERMINAL_EVENT {
    terminal_id: TerminalId,
    event: TerminalEventBody,
}

TerminalEventBody = tagged_union {
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

Earlier drafts called this message `OSC_EVENT` and its body
`OscEvent`. The rename to `TERMINAL_EVENT` / `TerminalEventBody`
under ADR-0015 reflects that the union now carries synthesized
non-OSC events (`EXIT_CODE`) in addition to parsed OSC sequences;
the wire body shape is unchanged.

The server does NOT forward every OSC type libghostty recognises.
Color operations, kitty color protocol commands, and kitty text-
sizing are purely terminal-state concerns; they are applied to the
Terminal's `libghostty_vt::Terminal` and clients see their effect
through normal cell diffs. The variants listed above are those that
affect *client* UX (chrome, notifications, clipboard, status bar
widgets) or that an L1-only consumer needs for command-boundary
detection.

### 7.L1.3 ALERT

Server-internal notifications about a Terminal:

```
ALERT { terminal_id: TerminalId, kind: AlertKind }

AlertKind = enum {
    ACTIVITY  = 0,  // Terminal wrote output while consumer was inactive
    SILENCE   = 1,  // Terminal has been quiet for the configured threshold
    BELL      = 2,  // duplicate of §7.L1.1 for clients that prefer one channel
}
```

---

## 8. Terminal state synchronization — the hot path

This section is the protocol's centerpiece. The server's
`libghostty_vt::Terminal` is the **canonical** owner of each
Terminal's grid, scrollback, cursor, and modes. The client runs its
own local `libghostty_vt::Terminal` as a rendering mirror. Terminal
content flows between them as a **stream of VT bytes**, not as
structured diffs. See [ADR-0013] for the design rationale
(libghostty-bytes-on-wire).

### 8.1 The frame model

A Terminal's content on the wire is a sequence of `TERMINAL_OUTPUT`
frames:

```
TERMINAL_OUTPUT {
    terminal_id: TerminalId,
    seq: u64,        // monotonic per-Terminal sequence id, for ack /
                     //   predictive-echo correlation (see §12)
    bytes: bytes,    // VT bytes from the PTY (canonicalised by the
                     //   server's libghostty Terminal and possibly
                     //   downsampled for this client's caps per §6.2)
}
```

The flow is:

1. The Terminal's PTY emits VT bytes.
2. The server feeds those bytes to the Terminal's canonical
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
between transport writes, and MAY rate-limit the per-Terminal output
stream (default cap 60 Hz of `TERMINAL_OUTPUT` emissions; configurable),
but the emissions themselves carry raw PTY bytes — no structured frame
boundaries, no `frame_id` / `base_frame_id` relationship, no
per-emission cursor/modes block. Frame identity is replaced by the
sequence number `seq`, used solely for acknowledgement (§12) and
predictive-echo correlation; `seq` carries no structural meaning.

A `TERMINAL_SNAPSHOT` (§8.4) is a self-contained replay: a synthesized
VT byte sequence that, when applied to a fresh Terminal of the matching
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
iTerm2) flow as bytes within the same `TERMINAL_OUTPUT` stream,
subject to the capability gating in §6.2.

### 8.4 Snapshots

```
TERMINAL_SNAPSHOT {
    terminal_id: TerminalId,
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

Servers emit `TERMINAL_SNAPSHOT` when:

1. A client first attaches (§13).
2. Backpressure forced the server to compact pending output and resume
   from a known state (§12).
3. The grid resized in a way that requires full retransmission (§10.2).
4. The protocol requires it for correctness in any future case.

After a `TERMINAL_SNAPSHOT`, the next `TERMINAL_OUTPUT` for the same
Terminal continues the live byte stream. The client's local Terminal
is in sync after applying the snapshot bytes and before consuming the
next `TERMINAL_OUTPUT`.

### 8.5 Cursor and modes

Cursor state (position, visibility, shape, blink) and Terminal modes
(altscreen, bracketed paste, app cursor keys, mouse protocol,
focus reporting, origin mode, etc.) live entirely inside each end's
`libghostty_vt::Terminal`. They are **not** separate wire concepts.

Clients that need cursor or mode state — for example to render a
local cursor overlay, to decide whether to forward mouse events, or
to enable bracketed paste in the outer terminal — MUST query their
local Terminal via libghostty's API (`Terminal::screen()`,
`Terminal::modes()`, etc.). They MUST NOT expect a `CursorState` or
`TerminalModes` block in `TERMINAL_OUTPUT` or `TERMINAL_SNAPSHOT`.

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
    terminal_id: TerminalId,
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

The server's per-Terminal state includes:

- A `libghostty_vt::Terminal` (canonical Terminal state, ADR-0004).
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
4. Write `buf` to the Terminal's PTY.

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
    terminal_id: TerminalId,
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
    // Terminal-local surface-space pixels. Always present.
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

Mouse positions on the wire are **pixels in Terminal-local surface
space**. The server reconstructs `mouse::EncoderSize` (cell width/
height, padding, full screen geometry) from the most recent
`VIEWPORT_RESIZE` (§10.2) and per-Terminal layout. Cell-quantized
clients (TUIs without true
pixel-precision input) emit positions at `cell_index × cell_size`; the
server's encoder produces correct output in both cell-format (SGR,
URXVT) and pixel-format (SGR-Pixels) mouse protocols.

#### 9.2.2 Server-side encoding pipeline

Identical in spirit to §9.1.6: each Terminal has a
`libghostty_vt::mouse::Encoder`. On `INPUT_MOUSE`, the server refreshes
the encoder via `set_options_from_terminal`, sets the encoder's
`EncoderSize` from current Terminal/cell geometry, builds a
`libghostty::mouse::Event`, encodes, and writes to PTY.

### 9.3 INPUT_FOCUS

```
INPUT_FOCUS {
    terminal_id: TerminalId,
    event: FocusKind,
}

FocusKind = enum { GAINED = 0, LOST = 1 }
```

The client emits `INPUT_FOCUS` when its window gains or loses focus on
the host OS. If the Terminal has DEC mode 1004 (focus reporting)
active, the server encodes a `CSI I` / `CSI O` via
`libghostty_vt::focus` and writes to the PTY; otherwise the event is
dropped server-side.

This event is purely L1: it reports the host-OS focus state of the
client to the Terminal, so OSC-aware programs (Vim, fzf, etc.) can
pause animation. A "which Terminal does the consumer want input
routed to" indicator is **not** a wire concept — that's an L3
metadata convention of the TUI consumer (see §17.4).

### 9.4 INPUT_PASTE

```
INPUT_PASTE {
    terminal_id: TerminalId,
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

When `trust = UNTRUSTED`, the server's per-Terminal policy applies:
`reject` (default — return an `ERROR { code: UNSAFE_PASTE }`),
`sanitize` (use `paste::encode` to strip), or `allow` (forward anyway).
When `trust = TRUSTED`, the server invokes `paste::encode` for
bracketing but skips safety classification.

### 9.5 INPUT_RAW

```
INPUT_RAW {
    terminal_id: TerminalId,
    data: bytes,
}
```

Escape hatch. Bytes in `data` are written verbatim to the Terminal's PTY.
Reserved for cases not modelled by `INPUT_KEY` / `INPUT_PASTE` /
`INPUT_MOUSE` / `INPUT_FOCUS` (chiefly: direct PTY testing and command
interpolation from configs). Servers MUST NOT silently re-interpret
`INPUT_RAW`; clients SHOULD avoid using it in normal operation.

---

## 10. Terminal lifecycle and viewport (L1)

This section defines the L1 lifecycle messages and the viewport-
resize protocol. Both are normative for every conforming consumer.

### 10.1 Terminal lifecycle

```
TERMINAL_OPENED {
    terminal_id: TerminalId,
    initial_size: { cols: u16, rows: u16 },
    cwd: str,
    command: list<str>,
}

TERMINAL_CLOSED {
    terminal_id: TerminalId,
    exit_status: optional<ExitStatus>,
}

ExitStatus = tagged_union {
    EXITED(u8),     // process called _exit(n)
    SIGNALED(u8),   // killed by signal n
    UNKNOWN,
}
```

`TerminalId` is a tagged union per
[ADR-0016](./ADR/0016-terminal-id-as-wire-primary.md), federation-
routable like every other identity in the protocol:

```
TerminalId = tagged_union {
    LOCAL     { id: u32 },              // tag = 0
    SATELLITE { host: str, id: u32 },   // tag = 1; reserved for v0.2+ (ADR-0007)
}
```

v0.1 servers only ever construct `LOCAL`. v0.1 decoders MUST accept
the `SATELLITE` tag and, if not configured as a federation hub,
respond with an `ERROR { code: UnsupportedSatelliteRoute }` (§14)
rather than failing the frame. This forward-compat reservation
avoids a wire-format break when satellites land.

`TerminalId`s are stable for the life of the server and are not
reused after close (the counter is monotonic for the server's
lifetime).

The server-side L1 command set (carried by the generic `COMMAND`
envelope, §11) is:

- `SPAWN { cwd, command, initial_size, parent_collection: optional<CollectionId> }`
  — returns `TerminalId`. Asynchronously emits `TERMINAL_OPENED`.
  `parent_collection` is meaningful only when L2 is in the
  negotiated tier set.
- `ATTACH_TERMINAL { terminal_id }` — wire the calling client to
  receive `TERMINAL_OUTPUT` for that Terminal. A Terminal MAY be
  attached by multiple clients simultaneously; the server
  multicasts.
- `DETACH_TERMINAL { terminal_id }` — stop receiving output. The
  Terminal itself is not affected.
- `KILL_TERMINAL { terminal_id }` — terminate the underlying PTY.
  Asynchronously emits `TERMINAL_CLOSED`.

`ATTACH_TERMINAL` / `DETACH_TERMINAL` are per-consumer subscription
operations; they do not affect the Terminal's existence. `KILL_TERMINAL`
is the only L1 command that destroys state.

### 10.2 Viewport resize

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

The server recomputes per-Terminal sizes against the new viewport.
Per-Terminal resize events are then emitted as `TERMINAL_RESIZED`:

```
TERMINAL_RESIZED { terminal_id: TerminalId, cols: u16, rows: u16 }
```

When multiple clients attach to the same Terminal with different
viewport sizes, the server uses the smallest common bounding box
(configurable: `aggressive` mode resizes per attached client). This
matches tmux's well-understood behavior and avoids surprising
shrink-and-grow on attach/detach. *How* Terminals are laid out
within an attached client's viewport is a consumer concern: TUIs
paint borders and chrome; agents may not paint anything at all;
layout-tree state is L3 metadata (§17), not a wire concept.

---

## 11. Commands

Commands are typed messages, not strings. They are sent over the same
connection and correlated via `request_id`. Commands are partitioned
by tier; the server MUST reject (with `ERROR { code: INVALID_COMMAND }`)
any command outside the negotiated tier set (§6.1).

```
COMMAND { request_id: u32, cmd: Command }
COMMAND_RESULT { request_id: u32, result: CommandResult }

CommandResult = tagged_union {
    OK,
    OK_WITH(CommandValue),
    ERROR(ErrorCode, str),
}

CommandValue = tagged_union {
    TERMINAL_ID(TerminalId),
    COLLECTION_ID(CollectionId),  // L2 only
    STATE(StateSnapshot),
    JSON(str),                    // for structured returns
    BYTES(bytes),                 // for L3 metadata values
}
```

A `COMMAND` is asynchronous: the server MAY emit other messages
(including events relevant to the command's effect) before
`COMMAND_RESULT`. Clients MUST tolerate that ordering.

### 11.L1 L1 commands (Terminal substrate)

```
Command_L1 = tagged_union {
    SPAWN            { cwd: optional<str>, command: optional<list<str>>,
                       initial_size: optional<{cols: u16, rows: u16}>,
                       parent_collection: optional<CollectionId> },  // L2 if set
    ATTACH_TERMINAL  { terminal_id: TerminalId },
    DETACH_TERMINAL  { terminal_id: TerminalId },
    KILL_TERMINAL    { terminal_id: TerminalId },
    RESIZE_TERMINAL  { terminal_id: TerminalId, cols: u16, rows: u16 },
    GET_STATE        { scope: StateScope },
    RUN_HOOK         { name: str, args: list<str> },
}
```

### 11.L2 L2 commands (Collections, OPTIONAL)

Reserved by [ADR-0015](./ADR/0015-protocol-layering.md). Wire
discriminants TBD; allocated when L2 lands.

```
Command_L2 = tagged_union {
    CREATE_COLLECTION                { name: optional<str> },
    RENAME_COLLECTION                { collection_id: CollectionId, name: str },
    KILL_COLLECTION                  { collection_id: CollectionId },
    LIST_COLLECTIONS,
    ADD_TERMINAL_TO_COLLECTION       { collection_id: CollectionId, terminal_id: TerminalId },
    REMOVE_TERMINAL_FROM_COLLECTION  { collection_id: CollectionId, terminal_id: TerminalId },
}

CollectionId = tagged_union {
    LOCAL     { id: u32 },
    SATELLITE { host: str, id: u32 },
}
```

`KILL_COLLECTION` MUST kill the Collection's member Terminals
atomically: clients observe `TERMINAL_CLOSED` for every member, then
`COLLECTION_CLOSED`. A partial-kill outcome is a protocol error.

### 11.L3 L3 commands (Metadata, OPTIONAL)

Reserved by ADR-0015. Wire discriminants TBD; allocated when L3
lands.

```
Command_L3 = tagged_union {
    GET_METADATA     { scope: MetadataScope, key: str },
    SET_METADATA     { scope: MetadataScope, key: str, value: bytes },
    DELETE_METADATA  { scope: MetadataScope, key: str },
    LIST_METADATA    { scope: MetadataScope, prefix: optional<str> },
}

MetadataScope = tagged_union {
    TERMINAL   (TerminalId),
    COLLECTION (CollectionId),   // L2 must also be in tier set
    GLOBAL,
}
```

The server MUST NOT interpret metadata values. Implementations MAY
enforce a per-key size limit (recommended: 256 KiB) and return
`RESOURCE_EXHAUSTED` if exceeded.

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

The server MUST cap per-Terminal `TERMINAL_OUTPUT` emission at a
configurable refresh rate (default 60 Hz). Between emissions, PTY
bytes are accumulated and shipped as a single coalesced
`TERMINAL_OUTPUT` carrying the batched VT bytes. There is no "every
byte emits a frame" mode; that would not survive a `yes` flood.

Coalescing operates at the byte level: the server concatenates the
PTY's output across the pacing interval into the next
`TERMINAL_OUTPUT`'s `bytes` field. Because libghostty's parser is
deterministic over the full byte stream, coalescing has no observable
effect on the client's local Terminal state beyond timing.

### 12.2 Per-Terminal acknowledgement

Clients acknowledge `TERMINAL_OUTPUT` emissions they have processed
(applied to their local libghostty `Terminal`):

```
FRAME_ACK { terminal_id: TerminalId, seq: u64 }
```

`seq` is the monotonic per-Terminal sequence number from
`TERMINAL_OUTPUT` (§8.1). An ack is cumulative: acknowledging
`seq = N` implies all prior `TERMINAL_OUTPUT`s for that Terminal up
to and including `N` have been applied.

The server tracks per-client `last_acked_seq` per Terminal. When
`pending_unacked_bytes` (or equivalently the count of unacked
`TERMINAL_OUTPUT` emissions) for a Terminal exceeds a configurable
`flow_control_threshold` (default: 32 unacked emissions, per-server
configurable, never disable-able), the server:

1. Stops sending live `TERMINAL_OUTPUT` for that Terminal to that client.
2. Drops the queued byte backlog for that Terminal / client.
3. Emits a single `TERMINAL_SNAPSHOT` (§8.4) synthesized from the
   server's canonical Terminal — `vt_replay_bytes` reproduces the
   current grid on a fresh client Terminal.
4. Resumes live `TERMINAL_OUTPUT` from the post-snapshot byte stream.
   The next `seq` after the snapshot establishes a fresh base
   (§13); clients MUST NOT assume `seq` continuity across the
   snapshot boundary.

This is the playbook Mosh uses, generalized to per-Terminal streams.
It ensures a slow client cannot block the server, and the worst-case
catch-up cost is one snapshot's worth of synthesized VT bytes, not an
unbounded queue of accumulated PTY output.

Scrollback that scrolls off during a backpressure-induced snapshot is
**not** retransmitted to the lagging client; clients that require
gap-free scrollback during heavy output SHOULD configure their server
with a higher `flow_control_threshold` or accept snapshot-driven
truncation. Servers MAY include bounded scrollback in
`TERMINAL_SNAPSHOT.scrollback_bytes` if configured to do so on
backpressure (implementation-defined; not normative).

### 12.3 Per-client isolation

Each connected client has its own outbound queue. A wedged client whose
queue exceeds its bound is forcibly disconnected with
`DETACHED { reason: PROTOCOL_ERROR }`. Other clients are unaffected.

---

## 13. State replay on attach

When a client sends `ATTACH`, the server's response sequence is:

1. `ATTACHED { snapshot, initial_client_id }` — a metadata-only
   snapshot of the consumer's tier-visible state: the set of
   Terminals (L1) the client is wired to receive, and, if L2/L3 are
   in the negotiated tier set, the set of Collections and the
   relevant metadata-key inventory. This step carries no Terminal
   content.
2. For each Terminal the client is attached to, one
   `TERMINAL_SNAPSHOT { terminal_id, cols, rows, vt_replay_bytes, scrollback_bytes? }`
   per §8.4. The client applies `scrollback_bytes` (if present) and
   then `vt_replay_bytes` to a fresh `libghostty_vt::Terminal` of
   the declared dimensions; the client's local Terminal is then in
   sync with the server's canonical Terminal for that Terminal.
3. Subsequent `TERMINAL_OUTPUT { terminal_id, seq, bytes }` messages
   flow live, continuing the per-Terminal VT byte stream from where
   the snapshot left off.

The per-Terminal `seq` numbering used by `TERMINAL_OUTPUT` resumes
from the server's chosen base after snapshot emission; clients MUST
treat the first `TERMINAL_OUTPUT` after a `TERMINAL_SNAPSHOT` as
authoritative for the sequence base and MUST NOT assume `seq`
continuity across the snapshot boundary. See [ADR-0013] for the
bytes-on-wire rationale.

```
ATTACH {
    target: AttachTarget,
    viewport: { cols: u16, rows: u16, pixel_w: optional<u16>, pixel_h: optional<u16> },
    request_scrollback: bool,
    scrollback_limit_lines: u32,
}

AttachTarget = tagged_union {
    LAST,                            // most-recently-used target
    BY_NAME(str),                    // L2 collection name (if L2 in tier set);
                                     //   else implementation-defined
    BY_COLLECTION_ID(CollectionId),  // L2 only
    BY_TERMINAL_ID(TerminalId),      // L1: attach to one Terminal directly
    CREATE_IF_MISSING { name: str, command: optional<list<str>>, cwd: optional<str> },
}

ATTACHED {
    snapshot: SubstrateSnapshot,
    initial_client_id: ClientId,
}

// Tier-conditional contents:
//   - L1-only client: `terminals` populated; `collections` empty; no metadata.
//   - L1+L2:          `terminals` and `collections` populated.
//   - L1+L2+L3:       all three populated (metadata listing only — values
//                     fetched on demand via GET_METADATA).
SubstrateSnapshot {
    terminals:   list<TerminalInfo>,
    collections: list<CollectionInfo>,         // empty if L2 not negotiated
    metadata_keys: list<MetadataKey>,          // empty if L3 not negotiated
}
```

This is the protocol's killer feature: a client reconnecting after
hours of detached work receives the **full state** of every Terminal
it is wired to, including scrollback up to the configured limit.
tmux loses scrollback on detach; phux does not.

> **Wire-bytes note.** v0.1.0-draft.6 declared `ATTACHED.snapshot` as
> a `SessionSnapshot` carrying sessions/windows/panes/layouts/focus.
> The on-wire blob field-IDs and shape are unchanged in this revision
> — the rename from `SessionSnapshot` to `SubstrateSnapshot` and from
> session/window/pane content to Terminal/Collection content is a
> doc rename only. Where v0.1.0-draft.6 placed session/window
> identifiers, this revision places `TerminalId` and (optionally)
> `CollectionId`. The TUI-convention vocabulary (focused session,
> focused window, focused pane, layout tree) moves to L3 metadata
> per §17.

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
    OUT_OF_TIER          = 5,   // a message arrived from a tier outside
                                //   the negotiated `HELLO.layers` set

    NOT_ATTACHED         = 100,
    ALREADY_ATTACHED     = 101,
    COLLECTION_NOT_FOUND = 102,  // renamed from SESSION_NOT_FOUND; same byte
    METADATA_KEY_NOT_FOUND = 103, // renamed from WINDOW_NOT_FOUND; same byte
                                  //   (WindowId is no longer a wire concept)
    TERMINAL_NOT_FOUND   = 104,  // renamed from PANE_NOT_FOUND per ADR-0016
    CLIENT_NOT_FOUND     = 105,
    UNSUPPORTED_SATELLITE_ROUTE = 106,

    INVALID_COMMAND      = 200,
    PERMISSION_DENIED    = 201,
    RESOURCE_EXHAUSTED   = 202,
    UNSAFE_PASTE         = 203,

    INTERNAL_ERROR       = 65535,
}
```

The numeric discriminants for codes 102, 103, 104 are preserved
across the rename so the wire bytes are unchanged. Decoders MUST
accept the byte values; the name change is editorial.

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

Conformance is **per-tier** per
[ADR-0015](./ADR/0015-protocol-layering.md). An implementation
declares the tiers it speaks via `HELLO.layers` (§6.1) and must
satisfy the conformance requirements for each declared tier, plus
the protocol-meta requirements common to all consumers.

### 16.0 Common requirements (all consumers)

Every conforming consumer:

1. Frames every message per §5.
2. Performs the §6.1 HELLO handshake with `versions` consistent with
   §6 ordering and `version` selection, and a non-empty `layers`
   bit-field with the `L1` bit set.
3. Tolerates unknown messages by logging and dropping them (§6).
4. Tolerates unknown trailing fields per the encoding rules
   (Appendix A).
5. Implements protocol-meta messages:
   `HELLO`, `HELLO_OK`, `ATTACH`, `ATTACHED`, `DETACH`, `DETACHED`,
   `PING`, `PONG`, `ERROR`, `COMMAND`, `COMMAND_RESULT`.

### 16.1 L1 conformance (REQUIRED — Terminal substrate)

Every conforming consumer additionally implements:

- **Terminal content:** `TERMINAL_OUTPUT`, `TERMINAL_SNAPSHOT`,
  `TERMINAL_RESIZED`, `FRAME_ACK`.
- **Terminal lifecycle:** `TERMINAL_OPENED`, `TERMINAL_CLOSED`.
- **Structured events:** `TERMINAL_EVENT`, `BELL`. (`ALERT` is
  RECOMMENDED.)
- **Input:** `INPUT_KEY`, `INPUT_PASTE`, `VIEWPORT_RESIZE`.
  (`INPUT_MOUSE`, `INPUT_FOCUS`, `INPUT_RAW` are RECOMMENDED.)
- **L1 commands:** `SPAWN`, `ATTACH_TERMINAL`, `DETACH_TERMINAL`,
  `KILL_TERMINAL`, `RESIZE_TERMINAL`.

A pure L1 consumer (an agent, a recorder, a CI orchestrator) sets
`HELLO.layers = { L1 }`. The server MUST omit all L2 and L3 messages
to that consumer. The consumer MUST NOT send L2 or L3 messages.

### 16.2 L1+L3 conformance (RECOMMENDED for GUIs and shared TUIs)

A consumer that additionally declares `L3` in `HELLO.layers` MUST
implement, in addition to §16.0 and §16.1:

- **Metadata commands:** `GET_METADATA`, `SET_METADATA`,
  `DELETE_METADATA`, `LIST_METADATA`.
- **Metadata events:** `METADATA_CHANGED { scope, key }` and an
  appropriate `SUBSCRIBE_METADATA` subscription mechanism.

The server MUST implement L3 storage scoped by `MetadataScope`
(§11.L3). Values are opaque bytes; the server enforces nothing
beyond size limits.

### 16.3 L1+L2+L3 conformance (REQUIRED for the reference TUI)

A consumer that additionally declares `L2` MUST implement, in
addition to §16.0, §16.1, and (typically) §16.2:

- **Collection lifecycle commands:** `CREATE_COLLECTION`,
  `RENAME_COLLECTION`, `KILL_COLLECTION`, `LIST_COLLECTIONS`,
  `ADD_TERMINAL_TO_COLLECTION`, `REMOVE_TERMINAL_FROM_COLLECTION`.
- **Collection events:** `COLLECTION_OPENED`, `COLLECTION_CLOSED`,
  `COLLECTION_RENAMED`, `COLLECTION_MEMBERSHIP_CHANGED`.
- **The atomic-kill invariant:** `KILL_COLLECTION` MUST cause the
  server to kill every member Terminal before emitting
  `COLLECTION_CLOSED`. Clients observe a flurry of `TERMINAL_CLOSED`
  followed by `COLLECTION_CLOSED`.

The L2 wire discriminants are TBD (allocated in v0.2 per §7.3). An
L1+L2+L3 consumer that ships against a server that does not
advertise `L2` in `HELLO_OK.server_caps.layers` MUST fall back to
L1-only operation or terminate the attach with
`DETACHED { reason: PROTOCOL_ERROR }`.

### 16.4 Out-of-tier messages

A peer receiving a message from a tier outside the negotiated
intersection MUST send `ERROR { code: OUT_OF_TIER }` and SHOULD
close the connection with `DETACHED { reason: PROTOCOL_ERROR }`.

### 16.5 Test suite

The reference test suite for this specification will live at
`crates/phux-protocol/tests/` and at `tests/conformance/` in the
implementation repository. Per-tier conformance suites are tracked
separately.

---

## 17. TUI consumer conventions (non-normative)

This section is **non-normative**. It documents how the reference
TUI uses L3 metadata to maintain its session / window / pane /
layout / focus vocabulary on top of the L1+L2+L3 substrate. Other
consumers MAY ignore this section entirely; a conforming consumer
need not implement, recognize, or even acknowledge these
conventions.

Per [ADR-0017](./ADR/0017-tui-not-protocol-privileged.md), the
reference TUI is one consumer among several. The vocabulary in this
section — **window**, **pane**, **layout tree**, **focus**,
**session-as-presented-by-the-TUI** — is the TUI's product shape,
not a wire concept. It exists here so that an alternative TUI
implementation can shadow the reference TUI by reading and writing
the same metadata keys.

DESIGN.md is the more detailed home for this vocabulary. The schema
below is the seam between the two documents.

### 17.1 Where TUI state lives

Every piece of TUI state is an L3 metadata key. The TUI reads on
attach, watches for `METADATA_CHANGED`, and writes on user action.
The server does not interpret values; it is opaque storage.

Keys are versioned (`/v1`) so future schemas can co-exist with old
clients. Values are CBOR-encoded structured data unless noted.

### 17.2 `phux.tui.layout/v1` — the layout tree

Scoped to a Collection. Contains the binary-split layout tree the
reference TUI paints. The schema (one Collection's "session" in
tmux vocabulary):

```
Layout = {
    windows: list<Window>,
    focused_window_index: u32,
}

Window = {
    name: str,
    root: LayoutNode,
    focused_terminal: TerminalId,
}

LayoutNode = tagged_union {
    LEAF  { terminal_id: TerminalId, weight: u16 },
    SPLIT { direction: SplitDirection,
            children: list<LayoutNode>,
            weights: list<u16> },
    TABBED { children: list<LayoutNode>, active: u32 },  // reserved
}

SplitDirection = enum { HORIZONTAL = 0, VERTICAL = 1 }
```

The binary-split-not-n-ary decision from
[ADR-0012](./ADR/0012-binary-split-tree-layout.md) continues to
apply *to this layout schema* — i.e. to the TUI — not to the wire.

### 17.3 `phux.tui.window_order/v1` — window order

Scoped to a Collection. A `list<u32>` of stable window indices in
the consumer's preferred display order. The TUI uses this to drive
tab-bar ordering.

### 17.4 `phux.tui.focus/v1` — focus pointer

Scoped per-client (Global key namespaced by client UUID, since the
server does not expose `ClientId` as a metadata scope). Records
which Terminal the TUI's local user is currently aiming input at:

```
Focus = {
    collection_id: CollectionId,
    window_index:  u32,
    terminal_id:   TerminalId,
}
```

This is **per-client** state. The TUI does not synchronize it
across attached clients; each attach has its own focus. The L1
`INPUT_FOCUS` message (§9.3) is unrelated — it carries host-OS
focus state into the Terminal so VT-aware programs can react.

### 17.5 What the TUI does NOT use

- **No "session" wire concept.** "Session" in tmux vocabulary is
  the TUI's name for an L2 Collection. The wire knows Collections.
- **No `WindowId` on the wire.** Window indices are positions
  within `phux.tui.layout/v1`; they have no protocol identity.
- **No `LAYOUT_CHANGED` event.** Layout changes are
  `METADATA_CHANGED { scope: Collection, key: "phux.tui.layout/v1" }`.
  Subscribers re-fetch the value to learn the new tree.
- **No `FOCUS_CHANGED` event.** Focus changes are
  `METADATA_CHANGED` on the per-client focus key.
- **No `WINDOW_OPENED` / `WINDOW_CLOSED` / `WINDOW_RENAMED`
  events.** Window lifecycle is "I edited `phux.tui.layout/v1`";
  observers see `METADATA_CHANGED`.

### 17.6 Alternative consumers

A native GUI consumer mounting L3 MAY (and SHOULD) use its **own**
metadata keys with a different prefix (e.g. `app.foo.layout/v1`)
rather than reusing the TUI's schema. Sharing schema across
consumers is opt-in, not the default; the wire enforces no
agreement. An agent SDK consumer typically declares `HELLO.layers
= { L1 }` and ignores §17 entirely.

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
[ADR-0013], Terminal content is now a VT byte stream and `DiffOp` no
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
| 0.1.0-draft.6 | 2026-05-26 | Editorial: §7.1 / §7.2 message catalogs grow a `Tier` column mapping each message to its [ADR-0015](./ADR/0015-protocol-layering.md) layer (`proto` / `L1` / `L2` / `tui→L3` / `cmd`); legend and tier-notes added. Previews the layered restructure that will rename `PANE_*` → `TERMINAL_*` ([ADR-0016](./ADR/0016-terminal-id-as-wire-primary.md)) and demote `WINDOW_*` / `LAYOUT_CHANGED` / `FOCUS_CHANGED` out of the wire ([ADR-0017](./ADR/0017-tui-not-protocol-privileged.md)). No bytes changed. |
| 0.1.0-draft.7 | 2026-05-26 | L1 vocabulary cascade Wave C (phux-vp0.2). §7 catalog reorganized by tier (proto / L1 / L2 / L3); §7.L1 messages renamed `PANE_*` → `TERMINAL_*` and `pane_id` → `terminal_id` per ADR-0016 (wire bytes unchanged). §7.3/§7.4 declare L2 (Collections) and L3 (Metadata) as reserved tiers with TBD discriminants. §6.1 HELLO gains `layers: bitset<Layer>` inside `ClientCapabilities` / `ServerCapabilities` (Appendix A field-tag extensibility keeps the wire compatible). §10 collapses Sessions/Windows/Panes/Layout/Focus into §10.1 Terminal lifecycle and §10.2 Viewport resize; the demoted TUI vocabulary lands non-normative in new §17. §6.2 reclaims the `CC_FRONTEND` capability slot per ADR-0017. §14 renames `PANE_NOT_FOUND` → `TERMINAL_NOT_FOUND` (numeric discriminant 104 preserved). §16 conformance restructured per-tier (16.0 common, 16.1 L1, 16.2 L1+L3, 16.3 L1+L2+L3). No wire bytes changed; no version bump. |
