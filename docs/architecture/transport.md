---
audience: contributors, agents
stability: evolving
last-reviewed: 2026-05-27
---

# Transport abstraction

**TL;DR.** The wire codec sits behind an async `Transport` trait on both
ends. v0.1 ships only `UnixSocketTransport`, but no domain module names
a concrete transport — that's load-bearing for the QUIC and SSH-stdio
implementations that ADR-0007 keeps purely additive.

---

The wire codec sits behind an `async Transport` trait on both server and
client. v0.1 ships exactly one implementation — `UnixSocketTransport` —
but no domain module in `phux-server` or `phux-client` names a concrete
transport type. All I/O goes through the trait.

This is a load-bearing invariant for ADR-0007 (Mosh-class transport and
satellite forward-compat). It exists to keep two v0.2+ features purely
additive:

- **QUIC transport** (via `quinn`) — provides connection migration,
  0-RTT resumption, and TLS, giving us the UX properties of Mosh
  without reimplementing SSP.
- **SSH-stdio transport** — frames the wire codec over a child SSH
  process's stdin/stdout, used by hub servers to reach satellites over
  existing SSH paths.

See ADR-0007 for the full forward-compat invariants (URI-shaped
session IDs, hub-and-spoke satellite topology, per-pane encoder
isolation).
