---
audience: consumers, contributors, agents
stability: evolving
last-reviewed: 2026-06-06
---

# The phux client library

**TL;DR.** The phux SDK is `phux-client`: a Rust library crate over the
`phux-protocol` wire codec. It is not a separate crate, not gRPC, and not
unbuilt — it exists today and is what `phux-mcp` is built from. It targets
L1 (the terminal substrate) and follows the carry-your-own-engine projection
pattern from ADR-0030: a consumer that wants structured terminal state runs
the engine and reads it locally. This file explains the crate's place among
the consumer surfaces and where to read its code-level types.

---

## What it is

There is no `phux-client-sdk` crate and no gRPC service. The library a
program links to drive phux is `phux-client`, and it already ships: it is the
crate the CLI agent verbs and the [MCP adapter](./mcp.md) are both written
against. `phux-client` wraps the `phux-protocol` codec and exposes the same
resolution, snapshot, run, and wait functions the CLI surfaces as
subcommands.

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

`phux-client` also carries a typed `Agent` handle (`Agent::connect_uds`,
`run`, `wait_for_prompt`, `get_state`) for programs that want a Rust API
rather than shelling out to the CLI. It is L1-shaped: no `Session`, no
`Window`, no `Pane`. The structured shapes it returns are the same ones the
[CLI agent surface](./agents.md) documents (`ScreenState`, `RunResult`,
`WaitOutcome`), which are the stable agent contract.

Divergence, marked honestly: parts of the `Agent` handle are designed, not
built. `subscribe_events` and `send_signal` are present in the type surface
but currently stubs, and the push-event story they front (`watch`) is the one
documented in [`agents.md`](./agents.md). Treat the handle's event and signal
methods as a direction, not shipped behavior; the CLI verbs and their JSON
shapes are the contract programs should depend on now.

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
