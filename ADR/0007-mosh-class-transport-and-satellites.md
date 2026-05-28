---
audience: contributors
stability: stable
last-reviewed: 2026-05-28
---

# 0007 — Mosh-class transport semantics and satellite forward-compat

**TL;DR.** Mosh is decomposed: snapshot-on-attach is adopted via byte replay, predictive echo is adopted as a client feature, SSP is rejected in favor of QUIC for v0.2+. Transport is a trait so v0.1's Unix socket and a future QUIC impl share one boundary. SessionId (and every other identity) carries a `{LOCAL, SATELLITE}` tag from day one so hub-and-spoke federation drops in without a wire break.

> **Post-ADR-0013 amendment (2026-05-25):** ADR-0013 supersedes
> ADR-0002 — pane content now ships as VT bytes (`PANE_OUTPUT`), not
> structured cell diffs. The Mosh-decomposition table below has been
> updated inline; the rest of this ADR (Transport trait, URI-shaped
> SessionId, hub-and-spoke satellites, forward-compat invariants)
> stands as-is. Satellite relaying is in fact *simpler* under ADR-0013
> because the hub forwards opaque byte payloads instead of having to
> understand cell structure.
>
> Predictive local echo prose: pre-ADR-0013 the client maintained a
> "diff mirror" that the prediction overlay sat on top of. Under
> ADR-0013 the client maintains a libghostty `Terminal` directly;
> predictive echo speculatively `vt_write`s encoded keystrokes into a
> shadow terminal (or overlay), reconciles when authoritative
> `PANE_OUTPUT` bytes arrive. The UX guarantee is unchanged; the
> substrate is libghostty, not a phux-defined mirror.

Status: Accepted (forward-compat)
Date: 2026-05-25

> **Update 2026-05-26:** [ADR-0015](./0015-protocol-layering.md)
> §"Cross-cutting: Federation" generalizes the `{LOCAL, SATELLITE}`
> tagged-union shape introduced here on `SessionId` to *every* protocol
> identity uniformly — `TerminalId` ([ADR-0016](./0016-terminal-id-as-wire-primary.md)),
> `CollectionId`, `SessionId`. v0.1 servers construct `LOCAL` only;
> v0.1 decoders MUST accept `SATELLITE` and respond
> `ERROR { code: UnsupportedSatelliteRoute }` when not configured as a
> federation hub. The §"Sessions are URI-shaped" decision below is
> preserved and broadened: it is now an *identity* invariant, not a
> `SessionId`-specific one.
>
> Additionally, [ADR-0013](./0013-libghostty-bytes-on-wire.md) renamed
> the per-pane content frame and snapshot frame to `TERMINAL_OUTPUT`
> and `TERMINAL_SNAPSHOT`; the inline references to `PANE_OUTPUT` /
> `PANE_SNAPSHOT` below should be read with that substitution.

## Context

Two requests have arrived for what look like distinct features but
share a common architectural backbone:

1. **Satellite/federation.** A user with several hosts (laptop, devbox,
   ephemeral sandboxes, agent VMs) wants one phux-server to act as a
   hub over the others, exposing remote sessions as if they were local.
2. **Mosh-class transport.** A user wants the responsiveness of Mosh —
   roaming across networks, sub-second reconnect, instant local echo
   over high-latency links.

Both reduce to the same architectural question: what is the shape of the
network plane below the wire protocol? If we answer that wrong now, both
features become refactors later. If we answer it right and leave the
door open, both become additive v0.2 work.

This ADR is **not** an instruction to implement satellites or QUIC in
v0.1. It is a record of the design decision so v0.1 code does not
preclude either.

## Decision

### 1. Mosh is decomposed, not adopted

"Support Mosh" is rejected as an ambiguous goal. Mosh bundles three
ideas; we treat them separately.

| Mosh innovation              | Our treatment                                    | Where it lives                              |
|------------------------------|--------------------------------------------------|---------------------------------------------|
| Authoritative server state synthesized on attach | **Adopted via byte-replay snapshots** per ADR-0013. The server walks its libghostty `Terminal` grid to emit a VT byte sequence that catches a new client up — exactly Mosh's snapshot trick, mapped onto our bytes-on-the-wire shape. | `phux-protocol::wire::frame::PaneOutput` (and `PaneSnapshot`) |
| Predictive local echo        | **Adopt as client feature.** Transport-agnostic. | `phux-client`                               |
| UDP State Sync Protocol (SSP) | **Reject. Use QUIC instead** for v0.2+.         | `phux-server::transport`                    |

Reasoning:

- **Authoritative server + byte-replay snapshots already exist.**
  SPEC §8 is the canonical statement; ADR-0013 records the
  bytes-on-the-wire shape that makes Mosh-style snapshot synthesis
  the natural attach path.
- **Predictive echo is a client concern**, not a transport concern.
  The client speculatively `vt_write`s keystrokes (encoded via
  libghostty's encoders + the client's best guess at mode) into a
  shadow `Terminal` or directly into the rendered one with a
  predicted-cells overlay, then reconciles when the authoritative
  `PANE_OUTPUT` arrives — diff-of-grids via `grid_ref()`. It works
  over any transport — Unix socket, TCP, QUIC.
- **QUIC strictly dominates SSP for our use case.** QUIC gives us
  connection migration (roaming), 0-RTT resumption (sub-second
  reconnect), TLS encryption, and congestion control — all the UX
  properties of SSP. SSP's unreliable+resync model is only a win when
  the stream is large and lossy; our protocol ships small ordered
  VT byte frames (ADR-0013) and structured input frames, for which
  reliable+ordered is correct. Reimplementing SSP
  (~1500 LoC of UDP framing, OCB-AES, ack windows, roaming) buys us
  nothing that `quinn` doesn't already provide.

Mosh **wire-compatibility** (i.e. `mosh-client` attaching to phux) is
explicitly out of scope. Doing so would constrain our protocol
evolution to Mosh's framing, with the only practical benefit being
attachment from environments that ship mosh-client but not phux-client
(notably iOS apps like Blink). Revisit only if that use case becomes
concrete.

### 2. Transport is a trait

The wire codec sits behind an `async Transport` trait on both server
and client. The trait surface is roughly:

```rust
trait Transport: AsyncRead + AsyncWrite + Send {
    fn peer_identity(&self) -> PeerIdentity;
    fn supports_migration(&self) -> bool;       // QUIC: true; Unix sock: false
    async fn on_path_change(&mut self) -> ...;  // hook for roaming-aware clients
}
```

v0.1 ships one implementation: `UnixSocketTransport`. v0.2+ adds
`QuicTransport` (via `quinn`) and `SshStdioTransport` (for satellite
hops over existing SSH paths). Domain logic in `phux-server` and
`phux-client` MUST NOT depend on any concrete transport type.

### 3. Sessions are URI-shaped

`SessionId` is a tagged union, not an opaque `u32`. v0.1 only ever
constructs the `Local` variant, but the wire format reserves space for
satellite-routed sessions from day one. (Per the 2026-05-26 update at
the top of this ADR, every protocol identity — `TerminalId`,
`CollectionId`, `SessionId` — carries this tag uniformly under
ADR-0015.)

```
SessionUri = tagged_union {
    LOCAL     { id: u32 },                          // v0.1 default
    SATELLITE { host: str, id: u32 },               // v0.2+
}
```

This is the single hardest thing to retrofit. Without URI-shaped IDs,
adding satellites later means rewriting every reference to a session
in the codebase and on the wire. The v0.1 cost is one byte of tag per
session reference plus a one-time match arm in code; the v0.2 dividend
is "satellites just slot in."

### 4. Satellite topology is hub-and-spoke

When satellites land in v0.2+, the architecture is hub-and-spoke, not
mesh:

```
client ──wire──> hub-server ──wire──> satellite-server
                     │
                     └──wire──> satellite-server
```

- The client attaches to exactly one phux-server (the hub).
- The hub server can declare other phux-servers as satellites and
  re-export their sessions via the SATELLITE variant of SessionUri.
- Satellites do not know about each other or about the hub's other
  satellites.
- The hub relays frames bidirectionally; it does NOT re-encode VT
  bytes. Predictive echo, FRAME_ACK, and snapshot fallback all
  function across the hub.
- Direct client → satellite paths (NAT-permitting shortcuts) are not
  precluded but are not v0.2 either.

Discovery and auth for v0.2: manual config (`phux satellite add devbox
ssh://devbox`), SSH-key-derived identities, no central registry.

## Must-not-preclude invariants for v0.1

Anything that violates these is a v0.1 bug, even though satellites
aren't shipping.

1. **Transport-agnostic domain.** No `phux-server` or `phux-client`
   module names a concrete transport. All I/O goes through the
   `Transport` trait. Even the Unix socket impl lives behind it.
2. **URI-shaped SessionId on the wire.** `SessionId` encodes as
   `tagged_union { LOCAL(u32), SATELLITE { host: str, id: u32 } }`
   from day one. v0.1 only ever constructs and accepts `LOCAL`, but
   the decoder MUST accept `SATELLITE` and return a clean
   `UnsupportedSatelliteRoute` error rather than rejecting at the
   frame layer.
3. **Per-pane encoder isolation.** Mouse, key, focus, paste encoders
   are per-pane (ADR-0006) and per-satellite when satellites land.
   No shared global encoder state.
4. **FRAME_ACK is on the protocol, not the transport.** The
   predictive-echo confirmation signal is part of SPEC §6, not a QUIC
   feature. Predictive echo must work over Unix sockets in v0.1, even
   if the latency benefit only matters over QUIC.

## Consequences

- The wire format has tag bytes on `SessionId` for v0.1 work that
  satellites won't use. Cost: one byte per session reference per
  message. Acceptable.
- v0.1 ships with one transport (Unix socket). Adding QUIC and
  SSH-stdio in v0.2 is purely additive.
- v0.1 client gets predictive echo as a first-class feature, not an
  afterthought, because deferring it until QUIC ships would push the
  most visible Mosh-class UX out to v0.3.
- We are explicitly accepting that some users will want mosh
  wire-compat for iOS/embedded; we are not building it.

## Related

- ADR-0013 — libghostty bytes on the wire (supersedes ADR-0002).
  The bytes-on-wire shape is what makes Mosh-style snapshot synthesis
  on attach the natural code path. Hub-and-spoke satellite relaying
  is also simpler under 0013 — the hub forwards opaque byte payloads
  instead of having to understand cell structure.
- ADR-0002 — diff-based protocol (superseded; retained as historical
  context for the bet that earlier framing of "Mosh-class semantics
  via cell diffs" rested on).
- ADR-0006 — input mirrors libghostty (encoder locality, which the hub
  inherits).
- SPEC §6 — FRAME_ACK and snapshot fallback (the protocol substrate
  predictive echo depends on).
- SPEC §10.1 — sessions and windows (SessionUri form).
