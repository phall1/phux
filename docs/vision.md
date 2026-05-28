---
audience: humans, contributors, agents
stability: evolving
last-reviewed: 2026-05-28
---

# Vision

**TL;DR.** The long arc. Where phux is going beyond the v0.1
substrate cut: agents as a primary consumer category, federation as
the default deployment shape, lazy state synchronization as the wire's
destination semantics. For the present-tense model — what phux is
today — read [`CONCEPTS.md`](./CONCEPTS.md).

---

## Why now

Two things changed.

**libghostty exists.** A bytes-in / structure-out terminal emulator
that both the server and a client can run, identically, with no
re-parsing in between. Modern terminal protocols — the Kitty keyboard
protocol, true colour, OSC 8 hyperlinks, OSC 133 prompt boundaries,
image protocols, mouse pixel-precision — pass through end-to-end
because libghostty parses on both ends. tmux, screen, zellij — all
built before libghostty — re-parse VT in the middle of the path and
degrade these features as a matter of architecture. phux structurally
cannot.

**Agents arrived.** Programs that drive terminals — Claude Code,
Cursor's agent, anything orchestrating a developer workflow — are now
a primary consumer category, alongside humans. They want primitives,
not opinions. They want to know when a command started and finished
and what its exit code was, not to scrape a grid. They want to spawn
a terminal on a remote box and observe it from a control plane, not
to SSH into a tmux session by name. The existing multiplexers are not
built for this, and the gap is widening.

phux is what falls out of taking both of those seriously at once.

---

## Distributed by design

phux is not a single-machine tool that one day might support remote
attach. It is a control plane from day one. The local Unix socket and
the federated hub are *the same wire*. Identity is portable across
hosts because federation is in the addressing scheme, not bolted on
later.

A satellite is a phux server running on another machine. A hub
federates them: an authenticated consumer connecting to the hub can
list Terminals on any satellite, spawn new Terminals on any
satellite, observe and drive them. The transport between hub and
satellite is whatever works — SSH-stdio first, QUIC eventually. The
wire is oblivious.

This shape is the load-bearing answer to "what is this for, beyond a
better tmux." A fleet of agents working on a fleet of cloud boxes
needs exactly this: terminals as first-class addressable resources,
accessible from one place, observable in real time, persistent across
disconnect. tmux cannot become this without throwing its wire away.
phux's wire is already pointed at it.

## Lazy state synchronization as the wire's destination

The long-arc wire semantics that make federation scale across lossy
and high-latency links is **lazy state synchronization** of
libghostty Terminal state — Mosh's State Synchronization Protocol
composed with libghostty as the state model
([ADR-0018](../ADR/0018-lazy-state-synchronization.md)). Today's
pass-through bytes ([ADR-0013](../ADR/0013-libghostty-bytes-on-wire.md))
is the degenerate case of that scheme; the destination shape is
structurally identical on the wire, with the server synthesizing
minimum-VT transitions per consumer per tick once the per-consumer
RenderState lifecycle lands.

The research note (now archived) captures the algorithm composition:
[`../research/archive/2026-05-26-state-sync-algorithm.md`](../research/archive/2026-05-26-state-sync-algorithm.md).

---

## Two consumer surfaces, both on the arc

### The reference TUI

The shape users expect from a multiplexer. Sessions, windows, panes,
splits, status bar, keybindings, prefix table. The user-facing
vocabulary is tmux's because it's what people know.
[`consumers/tui.md`](./consumers/tui.md) is the surface doc.

What's on the arc for the TUI past v0.1: command-mode (`:command`
prompt akin to vim), session/window/pane pickers as overlays, a tab
strip if multi-window juggling becomes painful enough,
prefix-discoverable hooks. None of these touch the wire — they're
chrome the TUI grows over its layered substrate.

### The agent SDK

A small Rust crate (`phux-client-sdk`) giving a program a typed
handle to spawn, observe, and drive Terminals over the wire. L1 only.
No sessions, no windows, no layout. The agent's universe is
*terminals and events*: spawn a build, wait for the OSC 133
command-end event, read the exit code, kill the terminal, move on.

A future `phux` CLI grows the same primitives for shell use — `phux
spawn`, `phux observe`, `phux exec`. JSON-over-HTTP shows up if
non-Rust agents become a real consumer category.

[`consumers/sdk.md`](./consumers/sdk.md) is the surface doc (currently
a stub).

---

## Milestones

- **v0.1 — substrate cut.** L1 frozen, L2 stable, L3 as opaque
  storage. Reference TUI works on L1 + L3 for layout. Federation hooks
  are baked into the wire but not exercised. Agent SDK ships as an
  L1-only wrapper.
- **v0.2 — federation real.** Hubs route to satellites. QUIC
  transport. Lazy state sync replaces pass-through bytes per ADR-0018.
- **v0.x and beyond — second consumer.** A native GUI consumer
  proves the substrate isn't TUI-shaped. A recorder proves it isn't
  consumer-shaped. A tmux control-mode adapter
  ([ADR-0010](../ADR/0010-frontend-agnostic-tmux-cc-reserved.md))
  proves it isn't phux-shaped.

Building for now, designed for later.

---

## What phux is, on purpose, not

The no-list survived two reframes — first from "better tmux" to
"libghostty multiplexer," then from "multiplexer" to "terminal
control plane." It survives because each item is about keeping the
substrate honest, not about being a smaller anything. The full list
with rationale lives in [`../CONTRIBUTING.md`](../CONTRIBUTING.md);
the headlines:

- No embedded scripting language.
- No plugin host.
- No copy-mode reinvention.
- No homegrown crypto.
- No format-template DSL.
