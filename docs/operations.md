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

## Security model and trust boundaries

> **Design assumption:** This is not a security-hardened system for
> hostile environments. It is suitable for trusted networks and
> multi-user boxes where Unix permissions are enforced by the kernel.

The trust boundary is the operating system user. A phux server trusts
every process running as the same UID that can connect to its Unix
socket.

### Local trust model (single-machine)

The Unix socket lives in `$XDG_RUNTIME_DIR/phux/` (typically
`/run/user/$UID/` on Linux, or `/var/folders/.../T/` on macOS),
created with parent directory mode `0o700` (user-only). The OS kernel
enforces this boundary at the filesystem level; the socket inherits
the parent directory's permissions.

**What this means:**

- Another user on the same machine MAY NOT read or write to the socket
  (kernel-enforced).
- If the parent directory or socket permissions are misconfigured (e.g.,
  accidentally mode `0o777`), the security boundary is breached — any
  user can connect. **Administrators MUST validate socket permissions
  in deployment; phux does not re-check at runtime.**
- The process file descriptor table (`/proc/<pid>/fd/<socket-fd>` on
  Linux) is not readable by other UIDs, so the socket endpoint cannot
  be enumerated across user boundaries.

### Federation trust model (v0.1+, forward-compatible)

**v0.1 (current):** Remote attach uses SSH-tunneled `phux server
--stdio`, delegating all authentication and encryption to SSH. The wire
bytes flow plaintext through the tunnel; SSH provides the trust
envelope.

**v0.2+ (future, wire-compatible):** Satellites are phux servers on
other machines. The hub authenticates consumers and routes terminal
sessions to satellite servers via the `Transport` trait
([ADR-0007](../ADR/0007-mosh-class-transport-and-satellites.md)).
Future transports will include:

- **SSH:** Reuses established SSH auth; inherits SSH's trust model.
- **QUIC (future):** Certificate-based (mutual TLS, future); no
  encryption on the wire yet, TLS will be added later.

### Known limitations (be honest)

- **No encryption on local UDS:** Contents flow plaintext through the
  socket. Roadmap does not currently include local TLS; if a
  confidentiality requirement exists, delegate to the transport layer
  (SSH, VPN).
- **Scrollback unencrypted:** Terminal history is stored in the
  libghostty grid in RAM, unencrypted. A memory dump can recover it.
- **No per-command encryption:** Control messages and terminal output
  are structured but unencrypted on the wire.
- **No audit logging:** phux does not log which user accessed which
  terminal or when. Audit requirements can be added as future hooks.
- **SSH is the trust boundary for remote attach (v0.1):** phux does not
  perform additional authentication over SSH; it delegates entirely to
  SSH key management and host verification.

### What you DO get (security wins)

- **Kernel-enforced permission boundary:** On Linux and macOS, the OS
  prevents other users from connecting to your socket. This holds
  without phux doing anything special.
- **No privilege escalation surface:** The server runs as your user
  (not setuid/setgid). A compromised terminal cannot elevate to other
  UIDs.
- **No arbitrary-code-execution surface on the wire:** The wire carries
  structured commands (key, mouse, paste, focus events), not arbitrary
  scripts or shell commands. The server does not `eval` or execute
  user input — it routes it to PTYs managed by the OS.
- **Process isolation via OS:** Each terminal's PTY is managed by the
  kernel; one terminal's PTY cannot directly access another terminal's
  memory or file descriptors.
