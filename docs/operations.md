---
audience: contributors, agents
stability: evolving
last-reviewed: 2026-06-06
---

# Operations

**TL;DR.** How phux behaves at the seams: error typing inside the workspace and translation at the IPC boundary; the logging and introspection surface an operator drives at runtime; and where the trust boundary sits and what it does and does not cover. Each section is one seam.

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

Logs are both an operator surface and a leak surface; [ADR-0028](../ADR/0028-runtime-log-control.md) owns that decision and its slicing, and this section is the home for the facts.

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

### Sensitive data in logs

Log sinks are created with mode `0o600` (owner-only) on Unix so another user on a shared box cannot read them ([ADR-0028](../ADR/0028-runtime-log-control.md)). Input atoms are **self-narrating and redaction-safe**: `KeyEvent` and `PasteEvent` have hand-written `Debug` impls (and `InputEvent::narrate`) that report only structural facts — action, physical key, modifiers, payload *lengths* — and never the typed key text or pasted bytes. A `trace!(?input, …)` therefore records that a keystroke or paste happened, with its shape, without spilling the secret it carried.

### Crash capture

Panics are durable on both sides. The **client** panic hook logs the panic message plus a captured `std::backtrace::Backtrace` to its file sink *before* it restores the terminal (survives even though the default hook's stderr backtrace would vanish into the dead alt screen). The **server** panic hook logs task/actor panics with their backtrace through `tracing`, so a daemonized server's crash lands in the log file. Both honor `RUST_BACKTRACE` for trace verbosity.

### Reading a trace to localize lag

The hot paths carry `tracing` spans whose `CLOSE` event reports the span's duration (`time.busy`/`time.idle`), so a captured session shows where time went before a stall. The per-frame and per-tick spans are at **debug**, so the default `phux=info` filter leaves them off and effectively free; raise the level only while diagnosing.

```sh
PHUX_LOG=/tmp/phux.jsonl PHUX_LOG_FORMAT=json RUST_LOG=phux=debug phux ...
# headless repro that exercises the same server paths:
PHUX_LOG=/tmp/phux.jsonl PHUX_LOG_FORMAT=json RUST_LOG=phux=debug \
  cargo run -p phux-server --example e2e-repro
```

Two spans carry most of the signal. On the server, `synthesize_against_reference` (fields `changed_row_count`, `out_bytes`) is the per-tick CPU cost of diffing engine state for one consumer. On the client, `handle_server_frame` (grep `kind=terminal_output`) is the per-frame apply-and-paint cost; its children `vt_apply` (libghostty parse) and `paint_trigger` (render) let you attribute a client stall to parse versus paint by comparing their `time.busy`. Narrow a JSON capture to timed events with `jq -c 'select(.fields.message=="close")'`. Finer per-PTY-chunk and per-frame-emit detail is at **trace**; a wedged or leaked consumer shows as `consumer mailbox full` / `consumer mailbox closed` at debug.

Runtime introspection ships as `phux server status --json`: number of sessions / windows / terminals / clients, per-terminal refresh rate, per-client queue depth, total bytes since start. This is the substrate for any future Prometheus/OpenTelemetry exporter — phux does not ship one. Runtime per-target log-level control and a `phux logs` discovery/tail verb are designed but not built ([ADR-0028](../ADR/0028-runtime-log-control.md) Slice 2); today the dials are the `RUST_LOG` / `PHUX_LOG` / `PHUX_LOG_FORMAT` environment knobs above, set at process start.

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

### Remote consumer trust model (opt-in)

A remote consumer (the native mobile app) can attach over the network without
an SSH tunnel, behind TLS plus a bearer pairing token
([ADR-0031](../ADR/0031-remote-consumer-auth-and-encryption.md)). This is the
nearer-term, single-server path, distinct from the federation hub above.

The bind address (`PHUX_WS_ADDR`) is the toggle, so there is no remote-mode
setup friction:

- **Loopback address → plaintext, unauthenticated.** The historical
  browser-client dev path; zero config.
- **Routable address → TLS + token, auto-provisioned.** Binding off-loopback is
  treated as exposing the server: phux generates and persists a self-signed
  certificate (under the state dir) if none is configured, and reads the default
  token store. It terminates TLS and requires an `Authorization: Bearer <token>`
  in the WebSocket upgrade; a missing or unrecognized token is refused with HTTP
  401 before any phux frame is read. Plaintext never reaches a routable address.
  Tokens are minted with `phux pair`, which prints the token once alongside the
  certificate's SHA-256 fingerprint to pin out-of-band. Revoke a device by
  deleting its line from the token file (effective on server restart).

`PHUX_WS_SECURE=1` forces the secure path on a loopback address (to exercise the
remote path locally); `PHUX_WS_TLS_CERT` + `PHUX_WS_TLS_KEY` substitute an
operator-supplied certificate for the auto-generated one; `PHUX_WS_TOKENS`
overrides the token-store path.

**What this means:**
- The trust boundary widens past the OS user: an authenticated network peer is a
  first-class consumer whose proof is a bearer token over TLS. This is a larger
  attack surface than local UDS; it is off by default and engages only when the
  three variables above are all set.
- The token is a bearer credential — anyone holding it is the device until the
  token is revoked. The store is owner-only (`0o600`); the comparison is
  constant-time; tokens are 256-bit from the OS CSPRNG. A client certificate
  (mutual TLS) is the stronger v0.2 hardening recorded in ADR-0031.
- Certificate lifecycle is an operator responsibility, like socket permissions.
  With a self-signed certificate, verifying the `phux pair` fingerprint on the
  device's first connect is what closes the trust-on-first-use MITM window.

### Output mode for remote consumers

A remote phone link is high-latency and may be lossy. A remote consumer SHOULD
request `OutputMode::StateSync` ([ADR-0018](../ADR/0018-lazy-state-synchronization.md))
at HELLO rather than the default `OutputMode::Raw`: StateSync ships the minimum
VT to move the consumer's last-acked state to canonical per tick, coalescing
floods and pacing per-consumer RTT. Raw stays the default for local interactive
peers, where byte-faithful pass-through is lowest-latency on a fast link.

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
