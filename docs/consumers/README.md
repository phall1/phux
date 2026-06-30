---
audience: consumers, contributors, agents
stability: evolving
last-reviewed: 2026-06-06
---

# docs/consumers/

**TL;DR.** One file per consumer surface that rides on the phux wire. The
surfaces are peers: a reference TUI, a reference browser projection, an agent
surface, an MCP adapter, and the client library they share. No consumer is
protocol-privileged; each speaks a subset of the wire spec and projects what
it needs locally. This index lists them and points at the owning doc for each.

---

## The peer principle

No consumer is protocol-privileged. The TUI, the web client, and the agent
surface are peers over one wire, and the structured views each one shows
(screen state, panes, layouts, run-and-wait results) are computed locally
from the shared engine, not transmitted as a wire tier. This is stated once
here; the per-consumer docs link it rather than restating it. See
[ADR-0017](../../ADR/0017-tui-not-protocol-privileged.md) (the TUI gets no
protocol-level standing) and
[ADR-0030](../../ADR/0030-engine-delegated-wire-and-projection-consumers.md)
(every structured surface is a consumer-side projection of the shared engine).

If a consumer needs behavior the wire does not provide, the answer is to
extend the spec with an ADR, not to add a consumer-shaped hook. The reference
pattern for a consumer that wants structure is to carry its own engine and
project locally, the way the web client does.

## Files

| File | Owns |
|---|---|
| [tui.md](./tui.md) | Reference TUI, the adoption wedge: CLI, keybinds, status bar, layout, hooks, recording. |
| [web.md](./web.md) | Reference projection consumer: Rust-to-WASM browser client that carries its own engine over the WebSocket wire codec. |
| [agents.md](./agents.md) | Agent surface: the CLI verb set, public agent state, asks, workspace save/restore, and versioned JSON contracts. (See [`../../AGENTS.md`](../../AGENTS.md) for universal agent substrate instructions.) |
| [mcp.md](./mcp.md) | MCP adapter: a JSON-RPC stdio tool surface over the agent verbs, `phux_ask`, and plugin workspace profile discovery. |
| [sdk.md](./sdk.md) | The `phux-client` library crate over the `phux-protocol` wire codec, shared by the surfaces above. |

Future consumers — a native GUI, a recorder, a tmux-CC adapter — get their
own files here when they materialize. Each file's frontmatter declares its
own `stability`; a shipped surface is `stable`, a forward-looking sketch is
`evolving`. Today the consumer surfaces are real but still pre-1.0, so most
files remain marked `evolving`.
