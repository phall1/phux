# 0007 — Mosh-class transport semantics and satellite forward-compat

Status: Accepted (forward-compat invariants); implementation deferred to v0.2+.
Date: 2026-05-25

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

| Mosh innovation              | Our treatment                                    | Where it lives        |
|------------------------------|--------------------------------------------------|-----------------------|
| Cell-level state + diffs     | **Already done.** This is the bet of ADR-0002.  | `phux-protocol::diff` |
| Predictive local echo        | **Adopt as client feature.** Transport-agnostic. | `phux-client`         |
| UDP State Sync Protocol (SSP) | **Reject. Use QUIC instead** for v0.2+.         | `phux-server::transport` |

Reasoning:

- **Cell diffs already exist.** SPEC §8 is the canonical statement.
- **Predictive echo is a client concern**, not a transport concern.
  The client maintains a local prediction overlay against its diff
  mirror, dim-renders predicted cells, and reconciles on server
  `FRAME_ACK` (SPEC §6). It works over any transport — Unix socket,
  TCP, QUIC.
- **QUIC strictly dominates SSP for our use case.** QUIC gives us
  connection migration (roaming), 0-RTT resumption (sub-second
  reconnect), TLS encryption, and congestion control — all the UX
  properties of SSP. SSP's unreliable+resync model is only a win when
  the stream is large and lossy; our protocol ships small ordered cell
  diffs, for which reliable+ordered is correct. Reimplementing SSP
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
satellite-routed sessions from day one.

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

- ADR-0002 — diff-based protocol (the bet that makes Mosh-class
  semantics possible).
- ADR-0006 — input mirrors libghostty (encoder locality, which the hub
  inherits).
- SPEC §6 — FRAME_ACK and snapshot fallback (the protocol substrate
  predictive echo depends on).
- SPEC §10.1 — sessions and windows (SessionUri form).
