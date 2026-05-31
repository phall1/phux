---
audience: contributors, agents
stability: evolving
last-reviewed: 2026-05-28
---

# Operations

**TL;DR.** How phux behaves at the seams: error typing inside the workspace and translation at the IPC boundary; available logs and runtime introspection; where the trust boundary sits. Each section focuses on a single seam.

---

## Error model

Each library crate defines its own error type with `thiserror`. The binary crate uses `anyhow` at the top level only — never inside library code.

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

Errors crossing the IPC boundary translate to `ERROR` messages (see [`spec/proto.md`](./spec/proto.md)) with a `code: ErrorCode` and `message: str`. The server's IPC layer maps internal Rust errors to wire `ErrorCode`.

## Logging and observability

`tracing` is the structured logging substrate, bootstrapped in `phux_server::telemetry`. Two entry points share one layer builder:

- **Server / foreground** (`phux server`, one-shot control verbs, any `--json` path) — `telemetry::init()`. Always logs human-or-JSON text to **stderr**; stdout is reserved for protocol/PTY traffic.
- **Client / TUI** (`phux attach`, naked `phux`, `phux new` without `--json`) — `telemetry::init_client()`. Logs to a **file only**: the attach loop owns the alt screen, so a stray log line corrupts the display.

Both fmt layers emit span-close timing (`FmtSpan::CLOSE`), so any `#[instrument]` span reports elapsed duration at close.

### Environment knobs

| Variable | Effect |
|---|---|
| `RUST_LOG` | Filter directives. Default `phux=info,warn`. |
| `PHUX_LOG=<path>` | Write logs to `<path>` via non-blocking file writer. Server tees to this file *in addition to* stderr; client writes here *instead of* its per-pid default. Parent directory created if missing. |
| `PHUX_LOG_FORMAT=text\|json` | `text` (default): human single-line layer. `json`: one JSON object per line for `jq`/`grep`. Applies to both stderr and file sinks. |

The **client default log path** (when `PHUX_LOG` is unset) is `$XDG_STATE_HOME/phux/client-<pid>.log` (falls back to `$HOME/.local/state/phux/`). The pid scope keeps concurrent clients from interleaving. Level defaults to `phux=info,warn`, so crashes and warnings are always captured without flooding the file.

The non-blocking file writer offloads I/O to a background thread; its `WorkerGuard` is held for the lifetime of `main` and flushes on exit.

### Crash capture

Panics are durable on both sides. The **client** panic hook logs the panic message plus a captured `std::backtrace::Backtrace` to its file sink *before* it restores the terminal (survives even though the default hook's stderr backtrace would vanish into the dead alt screen). The **server** panic hook logs task/actor panics with their backtrace through `tracing`, so a daemonized server's crash lands in the log file. Both honor `RUST_BACKTRACE` for trace verbosity.

### Reading a trace to localize lag

The hot paths carry `tracing` spans whose `CLOSE` event reports the span's duration (`time.busy`/`time.idle`). A captured session shows where time went before a stall. Per-frame / per-tick spans are at **debug** so the default `phux=info` filter leaves them disabled and effectively free; turn them up only while diagnosing.

Capture a session:

```sh
PHUX_LOG=/tmp/phux.jsonl PHUX_LOG_FORMAT=json RUST_LOG=phux=debug phux ...
# headless repro that exercises the same server paths:
PHUX_LOG=/tmp/phux.jsonl PHUX_LOG_FORMAT=json RUST_LOG=phux=debug \
  cargo run -p phux-server --example e2e-repro
```

Spans to grep for (use `jq -c 'select(.fields.message=="close")'` to narrow to timed events):

| Span (`span.name`) | Side | Level | Key fields | Localizes |
|---|---|---|---|---|
| `tick_emit` | server | debug | `consumer_count`, `dirty`, `emitted`, `total_out_bytes` | per-tick fan-out cost + volume shipped |
| `synthesize` | server | debug | `client_id`, `wire_terminal_id` | per-consumer diff (parent of row walk) |
| `synthesize_against_reference` | server | debug | `changed_row_count`, `out_bytes` | per-tick CPU sink — **the** server lag signal |
| `handle_attach` | server | info | `client_id`, `target`, `cols`, `rows` | attach-handshake latency |
| `handle_command` | server | info | `client_id`, `request_id`, `kind` | control-command latency |
| `handle_server_frame` | client | debug | `kind`, `terminal_id`, `seq`, `bytes` | per-frame client apply+paint cost — **the** client lag signal |
| `attach_handshake` | client | info | `target` | client-side end-to-end attach latency |

Per-PTY-chunk volume (`vt_write`) and per-frame emit detail are at **trace** (`RUST_LOG=phux=trace`) — useful for "what was the PTY doing right before the stall," off by default. A wedged or leaked consumer shows as `consumer mailbox full` / `consumer mailbox closed` at debug.

Example close line (server, full-screen repaint): `synthesize_against_reference` span with `time.busy:586µs`, `changed_row_count:39`, `out_bytes:4359`, nested under `synthesize{client_id, wire_terminal_id}` and `tick_emit{consumer_count, dirty}` — i.e. "this tick painted 39 changed rows / 4359 bytes for that consumer in 586µs."

Runtime introspection ships as `phux server status --json`: number of sessions / windows / terminals / clients, per-terminal refresh rate, per-client queue depth, total bytes since start. This is the substrate for any future Prometheus/OpenTelemetry exporter — phux does not ship one.

## Security model and trust boundaries

**Design assumption:** This is not a security-hardened system for hostile environments. It is suitable for trusted networks and multi-user boxes where Unix permissions are enforced by the kernel.

The trust boundary is the operating system user. A phux server trusts every process running as the same UID that can connect to its Unix socket.

### Local trust model (single-machine)

The Unix socket lives in `$XDG_RUNTIME_DIR/phux/` (typically `/run/user/$UID/` on Linux, or `/var/folders/.../T/` on macOS), created with parent directory mode `0o700` (user-only). The OS kernel enforces this boundary at the filesystem level; the socket inherits the parent directory's permissions.

**What this means:**
- Another user on the same machine MAY NOT connect to the socket (kernel-enforced).
- If the parent directory or socket permissions are misconfigured (e.g., accidentally mode `0o777`), the security boundary is breached. **Administrators MUST validate socket permissions in deployment; phux does not re-check at runtime.**
- The process file descriptor table (`/proc/<pid>/fd/<socket-fd>` on Linux) is not readable by other UIDs, so the socket endpoint cannot be enumerated across user boundaries.

### Federation trust model (v0.1+, forward-compatible)

**v0.1 (current):** Remote attach uses SSH-tunneled `phux server --stdio`, delegating all authentication and encryption to SSH. Wire bytes flow plaintext through the tunnel; SSH provides the trust envelope.

**v0.2+ (future, wire-compatible):** Satellites are phux servers on other machines. The hub authenticates consumers and routes terminal sessions to satellite servers via the `Transport` trait ([ADR-0007](../ADR/0007-mosh-class-transport-and-satellites.md)).

Future transports:
- **SSH:** Reuses established SSH auth; inherits SSH's trust model.
- **QUIC (future):** Certificate-based (mutual TLS, future); no encryption on the wire yet.

### Known limitations

- **No encryption on local UDS:** Contents flow plaintext through the socket. Roadmap does not include local TLS; if confidentiality is required, delegate to the transport layer (SSH, VPN).
- **Scrollback unencrypted:** Terminal history is stored in the libghostty grid in RAM, unencrypted. A memory dump can recover it.
- **No per-command encryption:** Control messages and terminal output are structured but unencrypted on the wire.
- **No audit logging:** phux does not log which user accessed which terminal or when. Can be added as future hooks.
- **SSH is the trust boundary for remote attach (v0.1):** phux does not perform additional authentication over SSH; it delegates entirely to SSH key management and host verification.

### What you DO get

- **Kernel-enforced permission boundary:** On Linux and macOS, the OS prevents other users from connecting to your socket.
- **No privilege escalation surface:** The server runs as your user (not setuid/setgid). A compromised terminal cannot elevate to other UIDs.
- **No arbitrary-code-execution surface on the wire:** The wire carries structured commands (key, mouse, paste, focus events), not arbitrary scripts. The server does not `eval` or execute user input — it routes it to PTYs managed by the OS.
- **Process isolation via OS:** Each terminal's PTY is managed by the kernel; one terminal's PTY cannot directly access another terminal's memory or file descriptors.
