---
audience: contributors, agents
stability: evolving
last-reviewed: 2026-05-28
---

# Operations

**TL;DR.** How phux behaves at the seams: how errors are typed inside
the workspace and translated at the IPC boundary; what logs and
runtime introspection are available; where the trust boundary sits.
Cross-cuts the architecture; each section below stays focused on a
single seam so additions don't churn the doc.

---

## Error model

Each library crate defines its own error type with `thiserror`. The
binary crate uses `anyhow` at the top level only — never inside
library code.

```rust
// crates/phux-server/src/error.rs
#[derive(Debug, thiserror::Error)]
pub enum ServerError {
    #[error("protocol: {0}")]
    Protocol(#[from] phux_protocol::ProtocolError),
    #[error("pty: {0}")]
    Pty(#[from] PtyError),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    // ...
}
```

Errors that cross the IPC boundary are translated to `ERROR` messages
(see [`spec/proto.md`](./spec/proto.md)) with a `code: ErrorCode` and
a `message: str`. The mapping from internal Rust errors to wire
`ErrorCode` is the responsibility of the server's IPC layer.

## Logging and observability

`tracing` is the structured logging substrate. Server logs go to
`~/.local/state/phux/log/server.log`, rotated daily via
`tracing-appender`'s file rolling writer. Filter precedence:

1. Config file (`log_filter = "phux=info,phux_server=debug"`).
2. `PHUX_LOG` environment variable (overrides config).
3. Default: `phux=info`.

Spans by convention:

- `attach` (client_id, session_id) — wraps an attachment for its
  lifetime.
- `pane` (terminal_id) — wraps PTY pump and `TERMINAL_OUTPUT` fanout
  per terminal.
- `command` (request_id, kind) — wraps a `COMMAND` dispatch.

Runtime introspection ships as `phux server status --json`: number of
sessions / windows / terminals / clients, per-terminal refresh rate,
per-client queue depth, total bytes since start. This is the
substrate for any future Prometheus/OpenTelemetry exporter — phux
does not ship one.

## Security model

The trust boundary is the operating system user. A phux server trusts
every process running as the same UID that can connect to its Unix
socket.

- **Local.** Unix socket under `$XDG_RUNTIME_DIR/phux/` (or
  `/tmp/phux-$UID/`), with the parent directory created mode `0o700`.
  The OS enforces the trust boundary on the directory; the socket
  inherits that boundary.
- **Remote (v0.1).** SSH-tunneled `phux server --stdio` over
  `ssh host`. Authentication is SSH's problem.
- **Remote (v0.2+).** QUIC transport behind the `Transport` trait
  ([ADR-0007](../ADR/0007-mosh-class-transport-and-satellites.md));
  the wire bytes don't change.

phux itself does no authentication and no encryption. Crypto in
multiplexers is a tarpit; we delegate. See
[`architecture/transport.md`](./architecture/transport.md) for the
trait shape that makes additional transports purely additive.
