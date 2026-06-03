---
audience: humans, contributors, agents, consumers
stability: stable
last-reviewed: 2026-06-03
---

# Concepts

**TL;DR.** The thing phux manages is the *terminal* — spawned, observed, driven, persisted, addressable across hosts — not the session or the pane. A human's TUI and an agent's API are two consumers of the same terminal, and neither is privileged. Sessions, windows, and panes are just how the TUI arranges terminals for a person. The wire is layered (L1/L2/L3) so a consumer subscribes to only what it needs; identity is federation-ready from the first byte.

---

## Terminals, not panes

A terminal: runs a process, parses bytes into a grid, accepts structured input, reports events (title, cwd, command lifecycle, hyperlinks, bells). That object is the whole model. A person looking at it and a program driving it are holding the same thing two different ways — that equivalence is the point, not a feature bolted on.

Sessions, windows, panes, splits — the entire tmux vocabulary — live in the TUI layer. Not on the wire. Not load-bearing for agents or control planes.

Why? The consumer category expanded beyond humans:
- Humans want windows and splits (the TUI layer handles this)
- Agents want to spawn, observe, tear down terminals
- CI fleets want event streams
- Control planes want addressable resources across machines

The terminal is what everyone needs. Everything else is optional.

The reference TUI ships in-tree because the substrate is only real if someone rides it.

---

## libghostty is the foundation

Both ends run `libghostty_vt::Terminal`: server (canonical state), client (local mirror for rendering). The same parser on both ends; nothing re-encodes in the middle.

Modern terminal protocols (Kitty keyboard, true colour, OSC 8, OSC 133, images, pixel-precision mouse) pass through losslessly. Older multiplexers re-parse VT mid-path and degrade fidelity. phux structurally cannot.

See [ADR-0004 (libghostty-vt as the canonical grid)](../ADR/0004-libghostty-vt-as-grid.md), [ADR-0008 (use libghostty types directly)](../ADR/0008-use-libghostty-types-directly.md), and [ADR-0013 (libghostty bytes on the wire)](../ADR/0013-libghostty-bytes-on-wire.md).

### In practice

- **Asymmetric wire.** Server→client: VT bytes. Client→server: structured events (key, mouse, focus, paste).
- **Input types are libghostty's.** Direct re-export. No parallel phux enum.
- **New libghostty features auto-light.** Next pin bump, both client and server get them.

---

## The wire is layered

The wire splits into tiers, declared at HELLO. See [ADR-0015 (protocol layering)](../ADR/0015-protocol-layering.md) and [`spec/`](./spec/).

| Tier | Concept | Carries |
|---|---|---|
| **L1** | Terminal | PTY + libghostty Terminal. Bytes-out, structured input, snapshot, resize, close, bell, events (title, cwd, command lifecycle, hyperlinks). |
| **L2** | Collection | Named bundle of Terminals with kill semantics. Optional; L1-only deployments skip it. |
| **L3** | Metadata | Opaque key-value pairs (Terminal, Collection, global scope). Consumers store conventions (TUI layout, window names). Server stores; does not interpret. |

Each layer references only lower layers. Consumers declare which tiers they speak at HELLO; the server omits unneeded messages.

Example: an agent SDK speaks L1 alone. It never sees "session" or "window" (TUI L3 metadata). A GUI might speak L1 + its own L3 and ignore the TUI's. No consumer is privileged on the wire.

---

## Identity is federation-ready

Every terminal:

```
TerminalId = LOCAL     { id: u32 }
           | SATELLITE { host: String, id: u32 }
```

v0.1 constructs `LOCAL` only. The wire accepts both from byte zero. Write `SATELLITE{host: "prod-box-3", id: 42}` today; v0.1 rejects it gracefully; v0.2 routes it. Same command, same wire bytes.

This is intentional forward compatibility. phux is a control plane from the first byte, not a single-machine tool with remote attach bolted on later.

See [ADR-0007](../ADR/0007-mosh-class-transport-and-satellites.md) and [ADR-0016](../ADR/0016-terminal-id-as-wire-primary.md).

---

## Comparison to other tools

### Traditional multiplexers

| Dimension | phux | tmux | zellij | screen |
|---|---|---|---|---|
| Modern terminal protocol support[^1] | ✓ | ◐ | ◐ | ✗ |
| Federation-ready addressing[^2] | ✓ | ✗ | ✗ | ✗ |
| libghostty canonical parser | ✓ | ✗ | ✗ | ✗ |
| Structured input types | ✓ | ✗ | ✓ | ✗ |
| End-to-end passthrough (no re-parse) | ✓ | ✗ | ✗ | ✗ |
| Production-proven maturity | ✗ | ✓ | ◐ | ✓ |

**What's different:** phux parses once (libghostty), routes VT bytes losslessly, and bakes federated addressing into the wire from day 1. tmux and zellij re-parse in-path and bolt on remote attach via SSH. screen is the oldest and simplest.

### Agent-focused tools

| Dimension | phux | zmx | cmux | rmux |
|---|---|---|---|---|
| Agent SDK or programmatic API | ✓ (CLI + MCP shipped; Rust SDK planned) | ✗ | ✓ (macOS native) | ✓ (Rust async) |
| Primary use case | Human multiplexer + control plane | Minimal session persistence | Agent UI (git, PR, notifications) | Agent automation |
| Cross-platform | ✓ | ✗ (implied) | ✗ (macOS only) | ✓ |
| Maturity | Pre-release v0.1 | Minimal scope, SSH bugs | Production (20k+ stars) | Public preview (v0.3.1) |
| Wire protocol published | ✓ (phux-proto in docs/spec/) | ✗ | Proprietary | ✓ (rmux-proto crate) |

**The difference:** These are not multiplexer competitors.

- **zmx**: minimal (session persistence only), SSH support, no agent SDK
- **cmux**: macOS agent UI (Claude, Cursor), not a replacement
- **rmux**: agent SDK (Rust async), no federation

phux is both a human multiplexer and a federation-ready control plane. See [ADR-0017](../ADR/0017-tui-not-protocol-privileged.md).

[^1]: Kitty keyboard, true colour, OSC 8, OSC 133, images, pixel-precision mouse. phux passes through unchanged (libghostty parses once, bytes forward). Others re-parse mid-path and degrade fidelity.
[^2]: phux wire knows remote identity from day 1. Design in [ADR-0016 (TerminalId as wire primary)](../ADR/0016-terminal-id-as-wire-primary.md) and [ADR-0007 (Mosh-class transport and satellites)](../ADR/0007-mosh-class-transport-and-satellites.md). Traditional multiplexers treat remote attach as client-side (SSH + local connect).

---

## Consumers are plural; none are privileged

Consumers in scope:

- **Reference TUI.** Tmux-shaped: sessions, windows, panes, splits, status bar, keybindings. Speaks L1 + L2 + L3. See [`consumers/tui.md`](./consumers/tui.md).
- **Agent surface (shipped).** The headless CLI verbs (`ls`, `snapshot`, `send-keys`, `run`, `wait`, `watch`, …) and the [`phux-mcp`](./consumers/mcp.md) adapter, both over the same selector grammar and JSON shapes. L1-shaped: spawn a terminal, drive it, read events and exit codes. A typed Rust SDK crate (`phux-client-sdk`) is still planned.
- **Future:** native GUI, recorder, tmux control-mode adapter ([ADR-0010](../ADR/0010-frontend-agnostic-tmux-cc-reserved.md)).

[ADR-0017 (TUI not protocol-privileged)](../ADR/0017-tui-not-protocol-privileged.md): the reference TUI is one consumer with no protocol-level privileges. If the TUI needs a feature the wire doesn't provide, extend the spec (with an ADR), not add a TUI-shaped hook.

### TUI vocabulary mapped to substrate

| TUI | Substrate |
|---|---|
| "session" | L2 Collection (named Terminals, kill-all) |
| "window" | TUI layout tree in L3 metadata |
| "pane" | Leaf of layout tree, points to L1 Terminal |
| "split a pane" | Mutate L3 tree; spawn L1 Terminal; repaint |
| "kill pane" | L1 `KILL_TERMINAL` for leaf; consumer updates layout |
| "attach session" | L2 `ATTACH`; read L3 layout; subscribe L1 output |

The TUI's vocabulary is user-facing. The substrate's vocabulary is what the wire carries.

---

## What v0.1 looks like

- **L1 is frozen.** The Terminal substrate is specified, tested, documented. Federation forward-compat is baked in.
- **L2 is stable.** Collection lifecycle, membership, naming, kill semantics.
- **L3 exists as opaque storage.** Read / write / delete on metadata blobs. The TUI's conventions live in [`consumers/tui.md`](./consumers/tui.md) and the non-normative conventions appendix in [`spec/L3.md`](./spec/L3.md).
- **The reference TUI works** on L1 + L3 metadata: attach, detach, splits, layouts, status bar, keybindings.
- **The agent surface ships** — not as a Rust SDK crate yet, but as the headless CLI verbs and the `phux-mcp` adapter. A program can spawn a build, wait for it to settle or for output to appear, and read the exit code today. That's what makes the agent-first thesis real instead of aspirational; the typed `phux-client-sdk` crate is the convenience layer still to come.

Federation and Automation are **designed for** in v0.1 (their hooks in the wire are present; their ADRs are written) and **shipped** in v0.2. Building for now, designed for later.

For the long arc — what v0.2+ looks like, where the agent-driven world is going, what makes this not a smaller tmux — see [`vision.md`](./vision.md).

---

## Where to go next

| You want to | Read |
|---|---|
| Run it | [`QUICKSTART.md`](./QUICKSTART.md) |
| Understand the wire bytes | [`spec/README.md`](./spec/README.md) |
| Understand how the server is built | [`architecture/README.md`](./architecture/README.md) |
| Understand the TUI surface | [`consumers/tui.md`](./consumers/tui.md) |
| See why we decided X | [`../ADR/README.md`](../ADR/README.md) |
| Read the long arc | [`vision.md`](./vision.md) |
| Contribute | [`../CONTRIBUTING.md`](../CONTRIBUTING.md) |
