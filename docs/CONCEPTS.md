---
audience: humans, contributors, agents, consumers
stability: stable
last-reviewed: 2026-05-28
---

# Concepts

**TL;DR.** phux is a libghostty-backed terminal control plane. The
unit of work is the *terminal* — spawned, observed, controlled,
persisted, addressable across hosts. Sessions, windows, and panes are
one consumer's way to arrange terminals on screen. The wire is layered
so that a consumer speaks only the tiers it needs and federation is
in the addressing, not bolted on.

---

## The unit of work is the terminal

A terminal is a long-lived stateful thing. It runs a process, parses
the bytes that process emits into a grid, accepts structured input
events, and reports notable structural events back to whoever is
listening — title changes, working directory, command start, command
end, hyperlinks, bells.

That is the unit of work in phux. Everything else — sessions, windows,
panes, splits, status bars, the entire tmux vocabulary — is *one* way
to arrange and decorate terminals for a human staring at a screen.
Useful, but not load-bearing.

Why frame it this way: because the consumer category has widened. A
human at a workstation wants the tmux experience. An agent driving a
build wants a terminal it can spawn, observe, drive, and tear down.
A CI box hosting forty ephemeral shells for a fleet of agents wants
forty terminals with their events on a stable wire — it does not want
a status bar. A control plane managing terminals across satellites
wants them as first-class addressable resources. The terminal is the
greatest common factor.

The reference TUI ships in this repo because the substrate is only
real if a real consumer rides it. The substrate is the point.

---

## libghostty is the foundation

Both ends of the phux wire run `libghostty_vt::Terminal`. The server's
is the canonical state for a managed terminal; the client's is a local
mirror used for rendering.

This is structural, not incidental. Modern terminal protocols — Kitty
keyboard, true colour, OSC 8 hyperlinks, OSC 133 prompt boundaries,
image protocols, mouse pixel-precision — pass through end-to-end
because libghostty parses on both ends and nothing in the middle
re-encodes them. Multiplexers built before libghostty (tmux, screen,
zellij) re-parse VT in the middle of the path and degrade these
features as a matter of architecture. phux structurally cannot.

The decision and its consequences are recorded in
[ADR-0004 (libghostty-vt as the canonical grid)](../ADR/0004-libghostty-vt-as-grid.md),
[ADR-0008 (use libghostty types directly)](../ADR/0008-use-libghostty-types-directly.md),
and [ADR-0013 (libghostty bytes on the wire)](../ADR/0013-libghostty-bytes-on-wire.md).

### What this means in practice

- **The wire is asymmetric.** Server→client *terminal content* is **VT
  bytes** forwarded from the PTY (after per-client capability
  rewriting). Client→server *input* is **structured key, mouse, focus,
  and paste events** built from libghostty's own atoms. The server
  encodes input back to VT bytes per-PTY using libghostty's encoders.
- **Input types are libghostty's types.** phux re-exports
  `libghostty_vt::key::{Key, Action, Mods}`,
  `libghostty_vt::mouse::{Action, Button}`, etc., directly. There is
  no parallel phux input enum to keep in sync.
- **A new libghostty feature lights up automatically.** When
  libghostty learns a new escape sequence or input atom, both the
  server and the client gain support the moment the pin advances.
  There is no phux-side bridge to update.

---

## The wire is layered

phux's wire is split into tiers, declared at HELLO time, with strict
conformance per tier. See
[ADR-0015 (protocol layering)](../ADR/0015-protocol-layering.md) for
the canonical statement; the normative bytes are in
[`spec/`](./spec/).

| Tier | Concept | Carries |
|---|---|---|
| **L1** | Terminal | A PTY + libghostty `Terminal`. Bytes-out (`TERMINAL_OUTPUT`), structured input, snapshot, resize, close, bell, structured terminal events (title, cwd, command-start/end, hyperlinks, progress). |
| **L2** | Collection | A named lifecycle bundle of Terminals. Killing the Collection kills its members. A server may decline to mount L2 for L1-only deployments. |
| **L3** | Metadata | Opaque key-value pairs scoped to Terminal, Collection, or global. Consumers store their own conventions here (a TUI's layout tree, window names, ordering). The server stores; the server does not interpret. |

Each layer references only layers below it. A consumer **declares
which layers it speaks** at HELLO and the server omits messages from
layers the consumer didn't subscribe to.

Why this matters: an agent SDK speaks L1 alone. It never sees "session"
or "window" because those concepts live in the TUI's L3 metadata. A
native GUI consumer might speak L1 plus its own L3 conventions and
still ignore the TUI's. The wire doesn't privilege any consumer's
arrangement.

### What's on the wire vs in the consumer

| Concept | Owner |
|---|---|
| Terminal — PTY, bytes, input, events | L1 (wire) |
| Collection — named bundle, lifecycle | L2 (wire) |
| Metadata blob — opaque KV | L3 (wire) |
| The TUI's layout tree (binary split tree) | TUI consumer; stored as L3 metadata under key `phux.tui.layout/v1` |
| The TUI's window ordering, focus pointer | TUI consumer; L3 metadata |
| Status bar, keybindings, hooks | TUI consumer; local config |
| Predictive local echo | Any consumer; client-side, transport-agnostic |

If you find yourself wondering "should X be on the wire" — the test
is: does *every* plausible consumer need it? If only the TUI needs it,
it's a TUI feature stored in L3, not a wire message.

---

## Identity is federation-ready

A `TerminalId` is a tagged union:

```
TerminalId = LOCAL     { id: u32 }
           | SATELLITE { host: str, id: u32 }
```

Day-1 servers construct `LOCAL` only. Day-N hubs route `SATELLITE` ids
to satellite servers running on other machines. The wire bytes are the
same in both cases. v0.1 servers MUST accept SATELLITE-tagged ids and
reply `UnsupportedSatelliteRoute` if they're not a federation hub —
this is forward-compat baked in from day one.

See [ADR-0007 (Mosh-class transport and satellites)](../ADR/0007-mosh-class-transport-and-satellites.md)
and [ADR-0016 (TerminalId as the wire primary)](../ADR/0016-terminal-id-as-wire-primary.md).

Why this matters: phux is not a single-machine tool that one day might
support remote attach. It is a control plane from the first byte. A
fleet of agents working on a fleet of cloud boxes is the load-bearing
use case — terminals as addressable resources, accessible from one
place, observable in real time, persistent across disconnect. tmux
cannot become this without throwing its wire away. phux's wire is
already pointed at it.

The transport between hub and satellites is whatever works — SSH-stdio
first, QUIC eventually — behind a `Transport` trait. Domain code never
names a concrete transport. See
[`architecture/transport.md`](./architecture/transport.md).

---

## Consumers are plural; none are privileged

Consumers in scope:

- **The reference TUI.** Tmux-shaped: sessions, windows, panes,
  splits, status bar, keybindings, prefix table. Speaks L1 + L2 + L3.
  Lives in [`consumers/tui.md`](./consumers/tui.md).
- **The agent SDK** (`phux-client-sdk`, planned). A typed Rust handle
  to spawn, observe, and drive Terminals. L1 only. No sessions, no
  windows, no layout.
  [`consumers/sdk.md`](./consumers/sdk.md) when it ships.
- **Future:** a native GUI consumer over libghostty's surface API; a
  recorder; a tmux control-mode adapter
  ([ADR-0010](../ADR/0010-frontend-agnostic-tmux-cc-reserved.md)).

[ADR-0017 (TUI not protocol-privileged)](../ADR/0017-tui-not-protocol-privileged.md)
states the conformance rule: the reference TUI is one consumer among
several with no protocol-level privileges. If the TUI needs a behavior
the wire doesn't provide, the answer is to extend the spec (with an
ADR) — not to add a TUI-shaped hook.

### What the TUI's vocabulary maps to

When a TUI user says one of these, they mean:

| TUI says | Phux substrate is |
|---|---|
| "session" | An L2 Collection (named bundle of Terminals, kill-all semantics) |
| "window" | A layout tree the TUI stores in L3 metadata; not on the wire |
| "pane" | A leaf of that layout tree — pointing at a Terminal (L1) |
| "split a pane" | Mutate the L3 layout tree; spawn a new Terminal; client repaints |
| "kill the pane" | Send L1 `KILL_TERMINAL` for the leaf's terminal; consumer updates its layout |
| "attach to a session" | L2 `ATTACH` to the Collection; consumer reads its L3 layout and subscribes to L1 output for each Terminal |

The TUI's vocabulary is what users expect; the substrate's vocabulary
is what the wire actually carries.

---

## What v0.1 looks like

- **L1 is frozen.** The Terminal substrate is specified, tested,
  documented. Federation forward-compat is baked in.
- **L2 is stable.** Collection lifecycle, membership, naming, kill
  semantics.
- **L3 exists as opaque storage.** Read / write / delete on metadata
  blobs. The TUI's conventions live in
  [`consumers/tui.md`](./consumers/tui.md) and the non-normative
  conventions appendix in [`spec/L3.md`](./spec/L3.md).
- **The reference TUI works** on L1 + L3 metadata: attach, detach,
  splits, layouts, status bar, keybindings.
- **The agent SDK ships** as a thin L1-only wrapper, with examples
  that spawn a build, wait for OSC 133 command-end, read exit code.
  This is what makes the agent-first thesis real instead of
  aspirational.

Federation and Automation are **designed for** in v0.1 (their hooks in
the wire are present; their ADRs are written) and **shipped** in v0.2.
Building for now, designed for later.

For the long arc — what v0.2+ looks like, where the agent-driven world
is going, what makes this not a smaller tmux — see
[`vision.md`](./vision.md).

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
