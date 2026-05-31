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

`tracing` is the structured logging substrate, bootstrapped in
`phux_server::telemetry`. Two entry points share one layer builder:

- **Server / foreground** (`phux server`, one-shot control verbs, any
  `--json` path) — `telemetry::init()`. Always logs human-or-JSON text to
  **stderr**; the binary's stdout is reserved for protocol/PTY traffic.
- **Client / TUI** (`phux attach`, naked `phux`, `phux new` without
  `--json`) — `telemetry::init_client()`. Logs to a **file only**, never
  stdout/stderr: the attach loop owns the alt screen, so a stray log line
  would corrupt the display.

Both fmt layers emit span-close timing (`FmtSpan::CLOSE`), so any
`#[instrument]` span reports its elapsed duration when it closes.

### Environment knobs

| Variable | Effect |
|---|---|
| `RUST_LOG` | Filter directives. Default `phux=info,warn`. |
| `PHUX_LOG=<path>` | Write logs to `<path>` via a non-blocking file writer. The **server** tees to this file *in addition to* stderr; the **client** writes here *instead of* its per-pid default. The parent directory is created if missing. |
| `PHUX_LOG_FORMAT=text\|json` | `text` (default) is the human single-line layer; `json` emits one JSON object per line for `jq`/`grep`. Applies to both stderr and file sinks. |

The **client default log path** (when `PHUX_LOG` is unset) is
`$XDG_STATE_HOME/phux/client-<pid>.log` (falling back to
`$HOME/.local/state/phux/` when `XDG_STATE_HOME` is unset). The pid scope
keeps concurrent clients from interleaving and makes "which log is this
crash in?" answerable from the client's own pid. The level defaults to
`phux=info,warn`, so crashes and warnings are always captured without
flooding the file.

The non-blocking file writer offloads I/O to a background thread; its
`WorkerGuard` is held for the lifetime of `main` and flushes on exit.

### Crash capture

Panics are durable on both sides. The **client** panic hook logs the
panic message plus a captured `std::backtrace::Backtrace` to its file
sink *before* it restores the terminal — so a crash survives even though
the default hook's stderr backtrace would otherwise vanish into the dead
alt screen. The **server** panic hook (installed by `telemetry::init()`)
logs task/actor panics with their backtrace through `tracing`, so a
daemonized server's crash lands in the log file. Both honor
`RUST_BACKTRACE` for the trace's verbosity.

### Reading a trace to localize lag

The hot paths carry `tracing` spans whose `CLOSE` event reports the span's
duration (`time.busy`/`time.idle`), so a captured session shows where time
went before a stall. Per-frame / per-tick spans are at **debug** so the
default `phux=info` filter leaves them disabled and effectively free; turn
them up only while diagnosing.

Capture a session:

```sh
PHUX_LOG=/tmp/phux.jsonl PHUX_LOG_FORMAT=json RUST_LOG=phux=debug phux ...
# headless repro that exercises the same server paths:
PHUX_LOG=/tmp/phux.jsonl PHUX_LOG_FORMAT=json RUST_LOG=phux=debug \
  cargo run -p phux-server --example e2e-repro
```

Spans to grep for, by signal (`jq -c 'select(.fields.message=="close")'`
narrows to the timed events):

| Span (`span.name`) | Side | Level | Key fields | Localizes |
|---|---|---|---|---|
| `tick_emit` | server | debug | `consumer_count`, `dirty`, `emitted`, `total_out_bytes` | per-tick fan-out cost + how much was shipped |
| `synthesize` | server | debug | `client_id`, `wire_terminal_id` | per-consumer diff (parent of the row walk) |
| `synthesize_against_reference` | server | debug | `changed_row_count`, `out_bytes` | the per-tick CPU sink — its duration is **the** server lag signal |
| `handle_attach` | server | info | `client_id`, `target`, `cols`, `rows` | attach-handshake latency |
| `handle_command` | server | info | `client_id`, `request_id`, `kind` | control-command latency |
| `handle_server_frame` | client | debug | `kind`, `terminal_id`, `seq`, `bytes` | per-frame client apply+paint cost — **the** client lag signal (grep `kind=terminal_output`) |
| `attach_handshake` | client | info | `target` | client-side end-to-end attach latency |

Per-PTY-chunk volume (`vt_write`) and per-frame emit detail are at
**trace** (`RUST_LOG=phux=trace`) — useful for "what was the PTY doing
right before the stall," off by default. A wedged or leaked consumer shows
as `consumer mailbox full` / `consumer mailbox closed` at debug. Example
close line (server, full-screen repaint): a `synthesize_against_reference`
span with `time.busy:586µs`, `changed_row_count:39`, `out_bytes:4359`,
nested under `synthesize{client_id, wire_terminal_id}` and
`tick_emit{consumer_count, dirty}` — i.e. "this tick painted 39 changed
rows / 4359 bytes for that consumer in 586µs."

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
