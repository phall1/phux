---
audience: humans, agents
stability: evolving
last-reviewed: 2026-06-07
---

# When to use phux (and when not to)

**TL;DR.** phux is a terminal multiplexer whose differentiator is the wire,
not the splits: a terminal is an object other programs can attach to, and the
headline case is a human and their agents driving the *same* live terminal.
If you want that, phux is for you today. If you want a battle-hardened local
multiplexer with a decade of muscle memory, you mostly don't need it yet.
Find your row below.

## Find yourself

| You are | phux? | Why |
|---|---|---|
| A human who wants their agent to *see and drive the same terminal they do* | **Yes — this is the point** | One server, many consumers; the agent attaches to your live pane, reads its grid, and types into it. |
| An agent author who wants structured, scriptable terminal control | **Yes** | `ls`/`snapshot`/`send-keys`/`run`/`wait`/`watch` with `--json`, plus `phux-mcp`. The CLI + JSON schema is the contract. |
| A tmux user who wants a modern, protocol-honest multiplexer | **Yes, with eyes open** | Attach/detach, splits, status bar, keybindings work. It's v0.1; expect rough edges and missing conveniences. |
| Someone on one SSH session who just wants splits and persistence | **Probably not yet** | tmux already does this well and phux adds no wire advantage for a single local user. Revisit when you want remoting or agents. |
| A fleet operator who wants to drive terminals across machines | **Not yet — addressed for, not wired** | The wire already speaks `SATELLITE{host, id}`; nothing routes it. That's the v0.2 arc. |

## The honest gaps

phux is v0.1. The wire, the reference TUI's attach/detach/multi-pane, and the
full modern-protocol passthrough are solid and won't move under you. The
headless verbs and `phux-mcp` are real and tested but the API may still wiggle
before 1.0. Cross-machine routing, a native GUI consumer, a typed Rust SDK
crate, and predictive local echo are designed and addressed-for but **not
wired yet** — if a capability isn't in the "works today" lists in the
[README](../README.md#what-actually-works-today), treat it as a promise, not a
feature.

## Go deeper

- The mental model: [`docs/CONCEPTS.md`](./CONCEPTS.md)
- Driving phux from an agent: [`docs/consumers/agents.md`](./consumers/agents.md) · [`docs/consumers/mcp.md`](./consumers/mcp.md)
- Why it's built on a shared engine: [ADR-0030](../ADR/0030-engine-delegated-wire-and-projection-consumers.md)
- How it sits next to tmux: [ADR-0009](../ADR/0009-phux-vs-mux-positioning.md)
- Where it's going: [`docs/vision.md`](./vision.md)
