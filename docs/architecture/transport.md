---
audience: contributors, agents
stability: evolving
last-reviewed: 2026-06-06
---

# Transport abstraction

**TL;DR.** The wire codec sits behind an async `Transport` trait on both
ends, so the same framing rides any byte stream. Two implementations exist
today: a Unix-domain-socket transport for local server/client links and a
WebSocket transport that carries the identical codec to browser consumers.
QUIC and SSH-stdio are designed as additive transports, not yet built. No
domain module names a concrete transport type; all I/O goes through the
trait.

---

The wire codec sits behind an `async Transport` trait on both server and
client. No domain module in `phux-server` or `phux-client` names a concrete
transport type; all I/O goes through the trait, which keeps new transports
additive rather than invasive.

## Implementations that exist today

- **`UnixSocketTransport`** — the local server/client link. This is the
  default path for a server and the clients attached to it on the same host.
- **WebSocket transport** — carries the same wire codec to browser
  consumers. `phux-web` ([the web consumer](../consumers/web.md), per
  ADR-0025) speaks the exact framing over WebSocket and projects engine
  state locally; the bytes on the wire are identical to the UDS path, only
  the byte stream underneath differs.

Both run the same codec. A consumer that can frame the codec over a stream
is a peer regardless of which stream it uses.

## Transports designed but not built

ADR-0007 (Mosh-class transport and satellites) keeps two further
transports purely additive — designed, not built:

- **QUIC transport** (via `quinn`) — connection migration, 0-RTT
  resumption, and TLS, which would give the UX properties of Mosh without
  reimplementing SSP.
- **SSH-stdio transport** — frames the wire codec over a child SSH
  process's stdin/stdout, intended for hub servers reaching satellites over
  existing SSH paths.

See ADR-0007 for the full forward-compat constraints (URI-shaped session
IDs, hub-and-spoke satellite topology, per-pane encoder isolation).
