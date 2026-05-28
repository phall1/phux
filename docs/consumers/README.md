---
audience: consumers, contributors, agents
stability: stable
last-reviewed: 2026-05-28
---

# docs/consumers/

**TL;DR.** One file per consumer surface that rides on the phux wire.
The reference TUI is the consumer most users meet first; the agent
SDK is the consumer that proves the substrate is consumer-shaped, not
TUI-shaped. Every consumer here speaks some subset of the tiers
defined in [`../spec/`](../spec/) and has no protocol-level
privileges ([ADR-0017](../../ADR/0017-tui-not-protocol-privileged.md)).

---

## Files

| File | Owns |
|---|---|
| [tui.md](./tui.md) | Reference TUI: CLI, keybinds, status bar, layout, hooks, recording |
| [sdk.md](./sdk.md) | Agent SDK shape (forward-looking; not yet shipped) |

Future consumers — a native GUI, a recorder, a tmux-CC adapter — get
their own files here when they materialize. Each file's frontmatter
declares its `stability`: a shipped surface is `stable`; a
forward-looking sketch is `evolving`.

## Conformance, not chrome

The reason consumers live in their own directory is that **none of
them are privileged**. The TUI happens to be in-tree and shipping
first; it is not "the phux UI" any more than the SDK is "the phux
API." Each consumer speaks an L-tier subset of the wire spec and
nothing more. If a consumer needs behavior the wire doesn't provide,
the answer is to extend the spec (with an ADR) — not to add a
consumer-shaped hook.
