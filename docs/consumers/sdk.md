---
audience: consumers, contributors, agents
stability: evolving
last-reviewed: 2026-06-06
---

# The internal Rust client library

**TL;DR.** `phux-client` is the in-tree Rust library behind the CLI and MCP
adapter. It speaks `phux-protocol` and exposes useful internal client
functions, but it is unpublished and its typed `Agent` facade is incomplete.
Treat the CLI and versioned JSON shapes as the supported programmatic surface
today, not this crate as a stable public SDK.

---

## What it is

There is no published `phux-client-sdk` crate and no gRPC service.
`phux-client` exists inside the workspace and powers the CLI agent verbs and
the [MCP adapter](./mcp.md), but downstream consumers cannot depend on a
versioned crates.io package yet. It wraps the `phux-protocol` codec and the
internal resolution, snapshot, run, and wait functions used by the binaries.

This sits at **L1** — the terminal substrate ([`../spec/L1.md`](../spec/L1.md)).
A program using it speaks terminal lifecycle, input atoms, snapshots, and
events; it does not get sessions, windows, or layout as types. Those are L3
conventions plus client logic ([`../spec/L3.md`](../spec/L3.md)), not part of
the L1 handle.

It is one consumer among peers — the reference TUI, the [web client](./web.md),
the [CLI agent surface](./agents.md), and the [MCP adapter](./mcp.md) — none
protocol-privileged ([ADR-0017](../../ADR/0017-tui-not-protocol-privileged.md)).

## How it fits the projection thesis

[ADR-0030](../../ADR/0030-engine-delegated-wire-and-projection-consumers.md)
states the wire carries opaque terminal bytes, not structured screen state.
A consumer that wants structure computes it from an engine it runs. The
reference shape for that is [phux-web](./web.md): Rust to WASM, loading
`ghostty-vt.wasm`, projecting the grid locally (ADR-0030 §4).

`phux-client` is the native-side library for the same pattern. Today it leans
on the server's engine-convenience snapshots (`GET_SCREEN` /
`GET_TERMINAL_STATE`, [`../spec/L1.md`](../spec/L1.md)) to read screen state
rather than running a local engine; those are a convenience over the shared
engine, not a normative structured wire tier, and a consumer that wants to
own its projection follows phux-web's carry-your-own-engine shape instead.
Either way the wire stays identical; only the projection differs.

## The agent handle

`phux-client` also contains a typed `Agent` handle, but it is not a complete
public facade. Command execution, event subscription, signals, attach/create,
and prompt-readiness behavior are incomplete or stubbed. Its types show the
direction of a native SDK; they are not a shipped compatibility promise.

Use the [CLI agent surface](./agents.md) or [MCP adapter](./mcp.md) today. Their
`ScreenState`, `RunResult`, and `WaitOutcome` JSON shapes are versioned, though
still pre-1.0.

## Where to read

- The structured shapes (`ScreenState`, `RunResult`, exit-code semantics):
  [`agents.md`](./agents.md).
- The thin JSON-RPC wrapper over the same `phux-client` functions:
  [`mcp.md`](./mcp.md).
- The L1 message catalog the library speaks:
  [`../spec/L1.md`](../spec/L1.md).
- The wire codec the library encodes against:
  [`../spec/appendix-encoding.md`](../spec/appendix-encoding.md).
- The projection thesis that places this crate among its peers:
  [ADR-0030](../../ADR/0030-engine-delegated-wire-and-projection-consumers.md) §4.
