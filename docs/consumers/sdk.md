---
audience: consumers, contributors, agents
stability: evolving
last-reviewed: 2026-05-28
---

# The phux agent SDK

**TL;DR.** Forward-looking stub. The agent SDK
(`phux-client-sdk`) will be a small Rust crate giving a program a
typed handle to spawn, observe, and drive Terminals over the phux
wire. L1 only — no sessions, no windows, no layout. Not yet
implemented; ships in the v0.1 milestone alongside the substrate cut.

---

## Why this exists

phux's bet is that the unit of work is the terminal, not the session
or the pane (see [`../CONCEPTS.md`](../CONCEPTS.md)). The agent SDK is
how that bet becomes consumer-real instead of marketing-real.

A consumer that speaks only L1 — spawn, observe, drive, lifecycle
events — proves the substrate is the substrate. If the protocol
needs concepts beyond L1 to be useful to an agent, then "L1 alone is
useful" was a story we told ourselves. The SDK is the falsifier.

## Shape (planned)

```rust
// Sketch — not yet implemented.
let phux = phux_client_sdk::connect_local().await?;
let term = phux.spawn(Spawn {
    command: "cargo build --release",
    cwd: ".",
    env: env_for_build(),
}).await?;

while let Some(event) = term.events().next().await {
    match event {
        Event::CommandEnd { exit_code, .. } => break,
        Event::Output { bytes, .. } => log_output(bytes),
        _ => {}
    }
}

let _ = term.kill().await;
```

Concrete: a `Phux` connect handle, a `Terminal` handle per managed
terminal, async event streams keyed by the L1 message catalog. The
type surface is L1-shaped. No `Session`. No `Window`. No `Pane`.

## Status

Not yet started. Tracked under the v0.1 substrate cut. Filed as a
future epic in `bd`; search `phux-client-sdk` for the ticket once it
exists.

## Where to read

- The L1 message catalog the SDK speaks:
  [`../spec/L1.md`](../spec/L1.md)
- The conformance tier the SDK targets:
  [`../spec/proto.md`](../spec/proto.md) (conformance section)
- The framing — why the SDK is the substrate's falsifier:
  [`../CONCEPTS.md`](../CONCEPTS.md) and
  [`../vision.md`](../vision.md)
