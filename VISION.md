# phux — Vision

phux is a **libghostty-backed terminal control plane.** The primary noun
is the terminal: spawned, observed, controlled, persisted, moved across
hosts, attached by N consumers — humans, agents, recorders, debuggers,
schedulers. The reference consumer is a TUI in the shape of tmux. The
substrate is the point.

This document is the long arc. It states where the project is going.
The normative protocol lives in [`SPEC.md`](./SPEC.md). The decisions
that close off design space live in [`ADR/`](./ADR/). The product
surfaces (TUI, SDK, CLI) live in their own design docs. This file is
read first.

---

## The arc

A terminal is a long-lived stateful thing. It runs a process, parses
the bytes that process emits into a grid, accepts structured input
events, and reports notable structural events back to whoever is
listening. That is the unit of work. Sessions, windows, panes, splits,
status bars — the entire tmux vocabulary — are *one* useful way to
present collections of terminals to a human on a screen. They are not
the unit of work.

phux's bet is that, in the agent era, the unit of work matters more
than the presentation. An agent does not want sessions or windows; it
wants a terminal it can spawn, observe, drive, and tear down. A CI box
hosting forty ephemeral shells for a fleet of agents does not want a
status bar. A control plane managing terminals across satellites does
not care about pane focus. They want the terminal, with its events, on
a stable, portable, network-friendly substrate.

The same substrate carries the tmux-shaped TUI without strain, because
the only thing the TUI does that the agent doesn't is *arrange* and
*decorate* terminals. The arrangement is a small piece of state the
TUI maintains; the decoration is chrome the TUI renders. Neither is
load-bearing for the substrate.

---

## Why now

Two things changed.

**libghostty exists.** A bytes-in / structure-out terminal emulator
that both the server and a client can run, identically, with no
re-parsing in between. Modern terminal protocols — the Kitty keyboard
protocol, true colour, OSC 8 hyperlinks, OSC 133 prompt boundaries,
image protocols, mouse pixel-precision — pass through end-to-end
because libghostty parses on both ends and nothing in the middle
re-encodes them. tmux, screen, zellij, all built before libghostty,
re-parse VT in the middle of the path and degrade these features as a
matter of architecture. phux structurally cannot.

**Agents arrived.** Programs that drive terminals — Claude Code,
Cursor's agent, anything orchestrating a developer workflow — are now
a primary consumer category, alongside humans. They want primitives,
not opinions. They want to know when a command started and finished
and what its exit code was, not to scrape a grid. They want to spawn
a terminal on a remote box and observe it from a control plane, not to
SSH into a tmux session by name. The existing multiplexers are not
built for this, and the gap is widening.

phux is what falls out of taking both of those seriously at once.

---

## The shape

Three layers on the wire, two orthogonal axes, one client-side surface.

### Three wire layers

| Layer | Concept | What it carries |
|---|---|---|
| **L1** | Terminal | A PTY + libghostty `Terminal` + observable bytes out + structured input + snapshot + resize + close + a stream of structured terminal events (title, cwd, command-start / command-end, hyperlinks, progress, bell, clipboard). |
| **L2** | Collection | A named lifecycle bundle of Terminals. Killing the Collection kills its members. Optional: a server may decline to mount the Collection service for L1-only deployments. |
| **L3** | Metadata | Opaque key-value pairs scoped to Terminal, Collection, or global. Used by consumers to agree on conventions (a TUI's layout tree, window names, ordering). The server stores; the server does not interpret. |

Each layer references only layers below it. A consumer declares which
layers it speaks at HELLO time. Conformance is per-tier. An L1-only
consumer is conforming; the server omits messages from layers it
doesn't subscribe to.

### Two orthogonal axes

- **Federation.** Addressing scheme that lets any identity be local
  or remote-routed. A `TerminalId` carries a tag — `LOCAL` or
  `SATELLITE { host, id }` — and the wire stays the same whether the
  Terminal lives on this server or on a satellite the hub routes to.
  Day-1 servers construct `LOCAL` only; day-N hubs route to satellites.
  See [ADR-0007](./ADR/0007-mosh-class-transport-and-satellites.md)
  for the satellite roadmap.

- **Automation.** Server-side rules that subscribe to L1 events and
  fire actions ("when this Terminal's command exits non-zero, run
  hook X"). An optional service, useful to agents and humans alike.

### Client-side surface

Everything else lives client-side: the TUI's layout interpretation,
status bar, keybinding dispatch, predictive echo, rendering policy.
The wire is silent about how a consumer arranges or decorates the
Terminals it observes. A native GUI client implements its own thing
in the same protocol.

---

## Distributed by design

phux is not a single-machine product that one day might support
remote attach. It is a control plane from day one. The local Unix
socket and the federated hub are *the same wire*. Identity is
portable across hosts because federation is in the addressing scheme,
not bolted on later.

A satellite is a phux server running on another machine. A hub
federates them: an authenticated consumer connecting to the hub can
list Terminals on any satellite, spawn new Terminals on any satellite,
observe and drive them. The transport between hub and satellite is
whatever works — SSH-stdio first, QUIC eventually. The wire is
oblivious.

This shape is the load-bearing answer to "what is this for, beyond a
better tmux." A fleet of agents working on a fleet of cloud boxes
needs exactly this: terminals as first-class addressable resources,
accessible from one place, observable in real time, persistent across
disconnect. tmux cannot become this without throwing its wire away.
phux's wire is already pointed at it.

---

## Two consumer surfaces

### The reference TUI

The shape users expect from a multiplexer. Sessions, windows, panes,
splits, status bar, keybindings, prefix table. The user-facing
vocabulary is tmux's because it's what people know. Under the hood,
"session" maps to a phux Collection; "window" maps to a layout tree
the TUI stores in L3 metadata; "pane" maps to a Terminal that appears
as a leaf of that layout tree. The TUI is one consumer among several;
nothing in the wire privileges it.

### The agent SDK

A small Rust crate (`phux-client-sdk`) giving a program a typed handle
to spawn, observe, and drive Terminals over the wire. L1 only. No
sessions, no windows, no layout. The agent's universe is *terminals
and events*: it spawns a build, waits for the OSC 133 command-end
event, reads the exit code, kills the terminal, moves on.

A future `phux` CLI grows the same primitives for shell use: `phux
spawn`, `phux observe`, `phux exec`. JSON-over-HTTP shows up if
non-Rust agents become a real consumer category.

---

## What the v0.1 milestone looks like

The substrate cut, with the TUI riding on top.

- **L1 is stable.** The Terminal protocol is frozen: messages, events,
  semantics, conformance. Specified, tested, documented. Federation
  forward-compat baked in (`TerminalId` carries the satellite tag,
  even if no satellites exist yet).
- **L2 is stable.** Collection lifecycle, membership, naming, kill
  semantics.
- **L3 exists as opaque storage.** Read/write/delete on metadata
  blobs. No conventions are normative; we ship the TUI's conventions
  as documentation in the TUI's design doc.
- **The reference TUI works** on L1 + L3 metadata for layout. Tmux-
  shaped: attach, detach, splits, layouts, status bar, keybindings,
  predictive local echo.
- **The agent SDK ships** as a thin L1-only wrapper. Examples that
  spawn a build, wait for it to finish, read its exit code. This is
  what makes the agent-first thesis real, not aspirational.

Federation and Automation are designed for in v0.1 (their hooks in
the wire are present; their ADRs are written) and shipped in v0.2.
Building for now, designed for later.

---

## What this is and isn't

phux **is** a libghostty-backed terminal control plane and the
reference consumer that proves it. The well-defined problem is:
*spawn, observe, control, persist, and address libghostty terminals,
locally or across a fleet, with conformance tiers a consumer can
target without inheriting everything else.*

phux **isn't**, on purpose:

- An embedded scripting language. Commands are typed wire messages.
- A plugin host. Extensions are consumer-side; agents already cover
  the "I want phux to do something programmatic" case structurally.
- A homegrown selection engine. Selection is a libghostty feature;
  the TUI surfaces it.
- A copy-mode reimplementation. Modern terminals do this well.
- A homegrown crypto layer. Transport (Unix socket perms, SSH, future
  QUIC TLS) carries authentication and confidentiality.
- A format-template DSL or status-bar mini-language. Status widgets
  are typed; arbitrary logic lives in widget binaries the TUI runs.

These were already the project's no-list under the tmux-replacement
framing. They survive the reframe because they were always about
keeping the substrate honest, not about being a smaller tmux.
