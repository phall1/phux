---
audience: humans, contributors, agents, consumers
stability: stable
last-reviewed: 2026-05-28
---

# Concepts

**TL;DR.** phux is a libghostty-backed terminal control plane. The unit of work is the *terminal* — spawned, observed, controlled, persisted, addressable across hosts. Sessions, windows, and panes are one consumer's way to arrange terminals on screen. The wire is layered so that a consumer speaks only the tiers it needs and federation is in the addressing, not bolted on.

---

## The unit of work is the terminal

A terminal is stateful: runs a process, parses bytes into a grid, accepts structured input, reports events (title, cwd, command lifecycle, hyperlinks, bells).

Everything else — sessions, windows, panes, splits, status bars — is one way to arrange terminals on screen. Not load-bearing.

Why frame it this way? The consumer category widened:
- Humans want the tmux experience.
- Agents want to spawn, observe, and tear down terminals.
- CI fleets want stable event streams.
- Control planes want addressable resources.

The terminal is the greatest common factor.

The reference TUI ships in-tree because the substrate is only real if a real consumer rides it. The substrate is the point.

---

## libghostty is the foundation

Both ends run `libghostty_vt::Terminal`: server (canonical state), client (local mirror for rendering). The same parser on both ends; nothing re-encodes in the middle.

Modern terminal protocols (Kitty keyboard, true colour, OSC 8, OSC 133, images, pixel-precision mouse) pass through losslessly. Older multiplexers re-parse VT mid-path and degrade fidelity. phux structurally cannot.

See [ADR-0004 (libghostty-vt as the canonical grid)](../ADR/0004-libghostty-vt-as-grid.md), [ADR-0008 (use libghostty types directly)](../ADR/0008-use-libghostty-types-directly.md), and [ADR-0013 (libghostty bytes on the wire)](../ADR/0013-libghostty-bytes-on-wire.md).

### Practical consequences

- **Asymmetric wire.** Server→client: VT bytes (capability-rewritten per client). Client→server: structured events (key, mouse, focus, paste) from libghostty atoms.
- **Input types are libghostty's.** Direct re-export: `libghostty_vt::key::{Key, Action, Mods}`, `libghostty_vt::mouse::{Action, Button}`. No parallel phux enum.
- **New libghostty features auto-light.** New escape sequences or input atoms land in both client and server at the next pin. No phux bridge needed.

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

### Wire vs consumer boundary

| Concept | Owner |
|---|---|
| Terminal — PTY, bytes, input, events | L1 (wire) |
| Collection — named bundle, lifecycle | L2 (wire) |
| Metadata blob — opaque KV | L3 (wire) |
| TUI layout tree | TUI consumer; L3 metadata under `phux.tui.layout/v1` |
| TUI window ordering, focus | TUI consumer; L3 metadata |
| Status bar, keybindings, hooks | TUI consumer; local config |
| Predictive local echo | Client-side, transport-agnostic |

Test: does *every* plausible consumer need it? If only the TUI needs it, it's TUI local config or L3 metadata, not a wire message.

---

## Identity is federation-ready

Every terminal is addressed by a `TerminalId` tagged union:

```
TerminalId = LOCAL     { id: u32 }
           | SATELLITE { host: String, id: u32 }
```

v0.1 constructs `LOCAL` only. The wire accepts both from byte zero. A client can write `SATELLITE{host: "prod-box-3", id: 42}` today. v0.1 rejects it with `UnsupportedSatelliteRoute` (not a crash). When v0.2 lands with satellite routing, the wire bytes are identical. The same command that got rejected in v0.1 just works in v0.2. Forward compatibility, in place before satellites exist.

**Unlike tmux:** tmux SSH-attach is client-side (connect to remote, spawn local client). A fleet control plane managing terminals across machines must speak different protocols to each server. phux's wire knows about remote identity from day 1. A single control plane addresses terminals uniformly across all machines; addressing is in L1, the terminal substrate.

phux is a control plane from the first byte, not a single-machine tool with remote attach bolted on. The load-bearing case: a fleet of agents working across cloud boxes with terminals as addressable, persistent, observable resources.

See [ADR-0007 (Mosh-class transport and satellites)](../ADR/0007-mosh-class-transport-and-satellites.md) and [ADR-0016 (TerminalId as wire primary)](../ADR/0016-terminal-id-as-wire-primary.md). Transport (SSH, QUIC, etc.) is pluggable via `Transport` trait; see [`architecture/transport.md`](./architecture/transport.md).

---

## Comparison to other multiplexers

| Feature | phux | tmux | zellij | screen | zmx | rmux | cmux |
|---|---|---|---|---|---|---|---|
| Modern terminal protocol support[^1] | ✓ | ◐ | ◐ | ✗ | ✗ | ✗ | ✗ |
| Federation-ready addressing[^2] | ✓ | ✗ | ✗ | ✗ | ✗ | ✗ | ✗ |
| libghostty canonical parser | ✓ | ✗ | ✗ | ✗ | ✗ | ✗ | ✗ |
| Structured input types | ✓ | ✗ | ✓ | ✗ | ✗ | ✗ | ✗ |
| End-to-end passthrough (no re-parse) | ✓ | ✗ | ✗ | ✗ | ✗ | ✗ | ✗ |
| Agent/programmatic first-class | ✓ | ◐ | ◐ | ✗ | ✗ | ✗ | ✓ |
| Production-proven maturity | ✗ | ✓ | ◐ | ✓ | ✗ | ✗ | ✗ |

[^1]: Kitty keyboard, true colour, OSC 8, OSC 133, images, pixel-precision mouse. phux passes through unchanged (libghostty parses once, bytes forward). Others re-parse mid-path and degrade fidelity.
[^2]: phux wire knows remote identity from day 1. Others treat remote attach as client-side (SSH + local connect). phux can build a fleet control plane with uniform addressing from the start.

---

## Consumers are plural; none are privileged

Consumers in scope:

- **Reference TUI.** Tmux-shaped: sessions, windows, panes, splits, status bar, keybindings. Speaks L1 + L2 + L3. See [`consumers/tui.md`](./consumers/tui.md).
- **Agent SDK** (`phux-client-sdk`, planned). Typed Rust handle to spawn, observe, drive Terminals. L1 only.
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
- **The agent SDK ships** as a thin L1-only wrapper, with examples that spawn a build, wait for OSC 133 command-end, read exit code. This is what makes the agent-first thesis real instead of aspirational.

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
