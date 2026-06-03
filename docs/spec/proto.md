---
audience: consumers, contributors, agents
stability: stable
last-reviewed: 2026-06-03
---

# proto — connection lifecycle, framing, and protocol meta

**TL;DR.** The protocol-meta tier. Every consumer that completes a
HELLO speaks this surface: transport assumptions, length-prefixed
framing, version and capability negotiation, lifecycle frames
(DETACH / SUBSCRIBE / PING), per-Terminal flow control, structured
errors, security delegation to the transport, and the per-tier
conformance contract.

---

## Conventions

Throughout the spec:

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

The key words "MUST", "MUST NOT", "REQUIRED", "SHALL", "SHALL NOT",
"SHOULD", "SHOULD NOT", "RECOMMENDED", "MAY", and "OPTIONAL" in this
document are to be interpreted as described in [RFC 2119].

[LEB128]: https://en.wikipedia.org/wiki/LEB128
[RFC 2119]: https://datatracker.ietf.org/doc/html/rfc2119

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
  §5 Version negotiation and [L1.md](./L1.md)).
- **Client → Server (input events):** structured `KeyEvent`,
  `MouseEvent`, `FocusEvent`, paste, and viewport messages — never raw
  VT bytes ([input.md](./input.md)).

A `libghostty_vt::Terminal` runs on **both** ends. The server's
Terminal is the canonical state (authoritative grid, scrollback,
cursor, modes). The client parses the received VT bytes into its own
local Terminal for rendering. Cell data, cursor position, and Terminal
modes are queried out of libghostty's `Terminal` API on each end; they
are not separate wire concepts.

This is the protocol's defining trait. Everything else follows from
it. See [ADR-0013] for the design rationale.

The protocol is organized into three layers per
[ADR-0015](../../ADR/0015-protocol-layering.md): **L1** (Terminal substrate,
MUST), **L2** (Collection lifecycle, OPTIONAL service), **L3**
(Metadata storage, OPTIONAL service). The Terminal is the wire's
primary identity ([ADR-0016](../../ADR/0016-terminal-id-as-wire-primary.md));
session-window-pane-layout-focus vocabulary is a convention of the
reference TUI consumer, not a wire concept
([ADR-0017](../../ADR/0017-tui-not-protocol-privileged.md)). See those
ADRs for the rationale that shapes this document.

[ADR-0013]: ../../ADR/0013-libghostty-bytes-on-wire.md

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
| **Tier** | A conformance layer: L1, L2, or L3 (message catalog and §10 Conformance below). |
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
- `type` is the message discriminant. See the per-tier message catalogs
  in [L1.md](./L1.md), [L2.md](./L2.md), [L3.md](./L3.md), and the
  proto-tier catalog in §6.3 below.
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
declares which conformance tiers (§10 Conformance) each side speaks. Per
[ADR-0015](../../ADR/0015-protocol-layering.md) §"Conformance tiers":

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
    layers: bitset<Layer>,         // tiers the client speaks (§10; ADR-0015)
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
    layers: bitset<Layer>,         // tiers the server implements (§10; ADR-0015)
}
```

The reference HELLO codec encodes `ClientCapabilities` as additive
trailing positional bytes after the original version tuple. The current
order is `color`, `layers`, `images`, `kbd_protocols`, `hyperlinks`.
Decoders MUST accept every prefix of this sequence and apply defaults for
missing trailing bytes; future fields append after `hyperlinks` until the
phux-i58 TLV migration replaces this legacy positional shape.

The `CC_FRONTEND` bit on `features` is **reclaimed** per
[ADR-0017](../../ADR/0017-tui-not-protocol-privileged.md). Earlier drafts
reserved it for a server that could "speak tmux control mode as an
alternative frontend." Under ADR-0017 the reference TUI has no
protocol-level privilege, and `tmux control mode` (when added) is one
L1/L2/L3 consumer among several — no capability bit required.
Decoders MUST ignore the slot.

Servers MUST adapt outbound `TERMINAL_OUTPUT` (see [L1.md §state
synchronization](./L1.md)) byte streams to each
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

## 7. Message catalog (proto tier)

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
[ADR-0015](../../ADR/0015-protocol-layering.md):

- **proto** — protocol meta (lifecycle, flow control, errors).
  Required of every consumer that completes a HELLO. Not tier-
  specific. Defined here.
- **L1** — Terminal substrate. Every conforming consumer speaks L1
  (§10). Carries `TerminalId` per
  [ADR-0016](../../ADR/0016-terminal-id-as-wire-primary.md). See
  [L1.md](./L1.md).
- **L2** — Collection lifecycle. Optional service. See
  [L2.md](./L2.md).
- **L3** — Metadata storage. Optional service. See [L3.md](./L3.md).
- **cmd** — typed command messages. Carry an L1/L2/L3 payload
  depending on the variant (see each tier's commands section).

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

[`phux_protocol::wire::frame::FrameKind`]: ../../crates/phux-protocol/src/wire/frame.rs

### 7.1 proto frames — connection lifecycle and flow control

| ID    | Direction | Name              | Reference          | Status    |
|-------|-----------|-------------------|--------------------|-----------|
| 0x01  | C → S     | `HELLO`           | §6.1               | shipped   |
| 0x02  | C → S     | `ATTACH`          | [L1.md §replay](./L1.md) | shipped |
| 0x03  | C → S     | `DETACH`          | §7.2               | shipped   |
| 0x21  | C → S     | `FRAME_ACK`       | §8                 | shipped   |
| 0x31  | C → S     | `COMMAND`         | [L1.md §5](./L1.md)| shipped   |
| 0x40  | C → S     | `SUBSCRIBE`       | §7.3               | spec-only |
| 0x7F  | C → S     | `PING`            | §7.4               | shipped   |
| 0x80  | S → C     | `HELLO_OK`        | §6.1               | shipped   |
| 0x81  | S → C     | `ATTACHED`        | [L1.md §replay](./L1.md) | shipped |
| 0x82  | S → C     | `DETACHED`        | §7.2               | shipped   |
| 0xC1  | S → C     | `ERROR`           | §9                 | shipped   |
| 0xC2  | S → C     | `COMMAND_RESULT`  | [L1.md §5](./L1.md)| shipped   |
| 0xFF  | S → C     | `PONG`            | §7.4               | shipped   |

The `COMMAND` / `COMMAND_RESULT` envelope (§5, allocated 0.2.0-draft.5
per [ADR-0021](../../ADR/0021-control-plane-commands.md)) round-trips
through the codec. v0.1 wires the `KILL_TERMINAL` (tag 0x03) and
`GET_STATE` (tag 0x05) commands, plus the appended agent-surface and
control commands `GET_SCREEN` (tag 0x07), `ROUTE_INPUT` (tag 0x08),
`CREATE_SESSION` (tag 0x09), and `KILL_COLLECTION` (tag 0x0a); the
remaining §5.1 catalog entries are reserved and decode as
`UnknownEnumValue` until allocated. `CREATE_SESSION` is a full-fledged
L1 command (create a session without attaching, ADR-0021 §3), not
deferred to L2; `KILL_COLLECTION` is its teardown counterpart, destroying
a named session in one round-trip (ADR-0021 §3, phux-h9s).

### 7.2 DETACH / DETACHED

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

### 7.3 SUBSCRIBE

Reserved for opting in/out of notification streams (e.g. only the focused
client should receive `BELL` for inactive Terminals). Format defined in
v0.2.

### 7.4 PING / PONG

```
PING { nonce: u64 }
PONG { nonce: u64 }
```

A peer receiving `PING` MUST respond with `PONG` carrying the same nonce
within a reasonable interval. PING/PONG is liveness only — clients and
servers MAY use it for keepalive; absence of pongs SHOULD NOT be
interpreted as anything other than a transport failure.

---

## 8. Flow control

### 8.1 Output pacing

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

### 8.2 Per-Terminal acknowledgement

Clients acknowledge `TERMINAL_OUTPUT` emissions they have processed
(applied to their local libghostty `Terminal`):

```
FRAME_ACK { terminal_id: TerminalId, seq: u64 }
```

`seq` is the monotonic per-Terminal sequence number from
`TERMINAL_OUTPUT` (see [L1.md §frame model](./L1.md)). An ack is
cumulative: acknowledging
`seq = N` implies all prior `TERMINAL_OUTPUT`s for that Terminal up
to and including `N` have been applied.

The server tracks per-client `last_acked_seq` per Terminal. When
`pending_unacked_bytes` (or equivalently the count of unacked
`TERMINAL_OUTPUT` emissions) for a Terminal exceeds a configurable
`flow_control_threshold` (default: 32 unacked emissions, per-server
configurable, never disable-able), the server:

1. Stops sending live `TERMINAL_OUTPUT` for that Terminal to that client.
2. Drops the queued byte backlog for that Terminal / client.
3. Emits a single `TERMINAL_SNAPSHOT` (see [L1.md §snapshots](./L1.md))
   synthesized from the server's canonical Terminal — `vt_replay_bytes`
   reproduces the current grid on a fresh client Terminal.
4. Resumes live `TERMINAL_OUTPUT` from the post-snapshot byte stream.
   The next `seq` after the snapshot establishes a fresh base
   (see [L1.md §state replay on attach](./L1.md)); clients MUST NOT
   assume `seq` continuity across the snapshot boundary.

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

### 8.3 Per-client isolation

Each connected client has its own outbound queue. A wedged client whose
queue exceeds its bound is forcibly disconnected with
`DETACHED { reason: PROTOCOL_ERROR }`. Other clients are unaffected.

---

## 9. Errors

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

## 10. Security

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

## 11. Conformance

Conformance is **per-tier** per
[ADR-0015](../../ADR/0015-protocol-layering.md). An implementation
declares the tiers it speaks via `HELLO.layers` (§6.1) and must
satisfy the conformance requirements for each declared tier, plus
the protocol-meta requirements common to all consumers.

### 11.1 Common requirements (all consumers)

Every conforming consumer:

1. Frames every message per §5.
2. Performs the §6.1 HELLO handshake with `versions` consistent with
   §6 ordering and `version` selection, and a non-empty `layers`
   bit-field with the `L1` bit set.
3. Tolerates unknown messages by logging and dropping them (§6).
4. Tolerates unknown trailing fields per the encoding rules
   ([appendix-encoding.md](./appendix-encoding.md)).
5. Implements protocol-meta messages:
   `HELLO`, `HELLO_OK`, `ATTACH`, `ATTACHED`, `DETACH`, `DETACHED`,
   `PING`, `PONG`, `ERROR`, `COMMAND`, `COMMAND_RESULT`.

### 11.2 L1 conformance (REQUIRED — Terminal substrate)

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

See [L1.md](./L1.md) and [input.md](./input.md) for the frame
definitions.

### 11.3 L1+L3 conformance (RECOMMENDED for GUIs and shared TUIs)

A consumer that additionally declares `L3` in `HELLO.layers` MUST
implement, in addition to §11.1 and §11.2:

- **Metadata commands:** `GET_METADATA`, `SET_METADATA`,
  `DELETE_METADATA`, `LIST_METADATA`.
- **Metadata events:** `METADATA_CHANGED { scope, key }` and an
  appropriate `SUBSCRIBE_METADATA` subscription mechanism.

The server MUST implement L3 storage scoped by `MetadataScope`
(see [L3.md](./L3.md)). Values are opaque bytes; the server enforces
nothing beyond size limits.

### 11.4 L1+L2+L3 conformance (REQUIRED for the reference TUI)

A consumer that additionally declares `L2` MUST implement, in
addition to §11.1, §11.2, and (typically) §11.3:

- **Collection lifecycle commands:** `CREATE_COLLECTION`,
  `RENAME_COLLECTION`, `KILL_COLLECTION`, `LIST_COLLECTIONS`,
  `ADD_TERMINAL_TO_COLLECTION`, `REMOVE_TERMINAL_FROM_COLLECTION`.
- **Collection events:** `COLLECTION_OPENED`, `COLLECTION_CLOSED`,
  `COLLECTION_RENAMED`, `COLLECTION_MEMBERSHIP_CHANGED`.
- **The atomic-kill invariant:** `KILL_COLLECTION` MUST cause the
  server to kill every member Terminal before emitting
  `COLLECTION_CLOSED`. Clients observe a flurry of `TERMINAL_CLOSED`
  followed by `COLLECTION_CLOSED`.

The L2 wire discriminants are TBD (allocated in v0.2; see
[L2.md](./L2.md)). An L1+L2+L3 consumer that ships against a server
that does not advertise `L2` in `HELLO_OK.server_caps.layers` MUST
fall back to L1-only operation or terminate the attach with
`DETACHED { reason: PROTOCOL_ERROR }`.

### 11.5 Out-of-tier messages

A peer receiving a message from a tier outside the negotiated
intersection MUST send `ERROR { code: OUT_OF_TIER }` and SHOULD
close the connection with `DETACHED { reason: PROTOCOL_ERROR }`.

### 11.6 Test suite

The reference test suite for this specification will live at
`crates/phux-protocol/tests/` and at `tests/conformance/` in the
implementation repository. Per-tier conformance suites are tracked
separately.
