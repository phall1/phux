<!--
audience: humans, contributors, agents
stability: stable
last-reviewed: 2026-05-28
-->

# phux

**A libghostty-backed terminal control plane.** Spawn, observe,
control, persist, and address terminals — locally or across a fleet —
with a tmux-shaped TUI riding on top as one consumer among several.

The gap it fills, in the words of libghostty's author:

> "Need to replace tmux with a libghostty-based multiplexer so it can
> understand KIP."
> — Mitchell Hashimoto

## What it is

The unit of work is the **terminal**, not the session or the pane.
Both ends of the wire run [`libghostty_vt::Terminal`][lghv]: the
server's is the canonical state for a managed terminal; the client's
is a local mirror used for rendering. Nothing in the middle re-parses
VT. Kitty keyboard, true colour, OSC 8, OSC 133, images — they all
pass through end-to-end because the parser is identical on both ends.

The wire is layered. Consumers declare which tiers they speak and the
server omits messages from layers they don't subscribe to:

- **L1 — Terminal.** PTY, bytes-out, structured input, snapshot, lifecycle, events.
- **L2 — Collection.** Named lifecycle bundle of Terminals. Optional.
- **L3 — Metadata.** Opaque KV the server stores but doesn't interpret. Where the TUI keeps its layout, window names, focus pointer.

Identity is federation-ready from the first byte. `TerminalId` is a
`LOCAL { id } | SATELLITE { host, id }` tagged union. Day-1 servers
construct `LOCAL` only; day-N hubs route `SATELLITE` ids to satellites
on other machines. The wire bytes don't change.

[lghv]: https://github.com/Uzaaft/libghostty-rs

## Why it looks different from tmux underneath

- **No re-parse in the middle.** libghostty parses on both ends.
  Modern terminal protocols pass through unchanged.
- **The terminal is the substrate, not the pane.** Sessions, windows,
  panes, splits live in L3 metadata, not on the wire — an agent SDK
  never hears them.
- **Federation is in the addressing.** Local UDS and the remote hub
  speak the same wire. Not a single-machine tool that one day might
  support remote attach.
- **No consumer is privileged.** The reference TUI ships in-tree
  because the substrate is only real if a real consumer rides it.

For the full mental model, read [`docs/CONCEPTS.md`](./docs/CONCEPTS.md).
For the long arc, read [`docs/vision.md`](./docs/vision.md).

## Philosophy

phux solves one problem and refuses to grow a second: spawn, observe,
control, persist, and address libghostty terminals — locally or across
a fleet — over a layered wire whose tiers a consumer can target
without inheriting everything else. The terminal is the substrate; a
consumer is anything that rides it. The reference TUI is the proof that
the substrate is real, not the product the substrate exists to serve.
Everything that doesn't make terminals more spawnable, observable, or
addressable is out of scope on purpose — see Non-goals below.

## When to use phux

**You are a tmux user wanting a modern replacement:**
✅ Yes. The TUI is tmux-shaped and the underlying substrate is stronger: libghostty instead of VT re-parse, federation-ready addressing, and a clean wire contract for agents. Tradeoff: L2 Collection (named sessions) and full CLI surface land in v0.2. Read [`docs/QUICKSTART.md`](./docs/QUICKSTART.md) to get started.

**You are SSHing into one box and want an ephemeral multiplexer:**
⚠️ Maybe not yet. phux assumes a persistent server-per-user; one-off SSH + detach-on-disconnect works but isn't a first-class use case. tmux remains the practical choice for "I just logged in and want to split panes."

**You are an agent (Claude, Cursor, etc.) orchestrating terminals:**
✅ Yes, eventually. L1's wire protocol is stable and publishable today; the agent SDK is coming in v0.2. For now, use L1 directly — see [`CONTRIBUTING.md`](./CONTRIBUTING.md) §"Building on phux" for examples. Your code will work unchanged when the SDK ships.

**You are a fleet orchestrator managing terminals across 10+ machines:**
✅ Yes, this is the 10-year vision. Federation (v0.2+) isn't here yet, but the addressing is ready from day 1. `SATELLITE { host, id }` TerminalIds already live on the wire. Start building on L1 now; your code will route end-to-end when satellites land.

**You want to script and automate everything:**
✅ Yes. phux refuses an embedded scripting language (see Non-goals), so you shell out to a real runtime. Typed L1 wire messages are far better than string-scraping terminal output. See [`docs/spec/TUTORIAL.md`](./docs/spec/TUTORIAL.md) for wire examples.

**You want tmux key bindings unchanged or a drop-in replacement:**
⚠️ We are not tmux. We will be better in some places and different in others. Read [`docs/consumers/tui.md`](./docs/consumers/tui.md) to learn the surface before committing.

**Not sure which box you check?** Read [`docs/CONCEPTS.md`](./docs/CONCEPTS.md) — it explains the design philosophy and mental model. That will tell you if this is aligned with how you think about terminals.

## Status

**v0.1: L1 Terminal substrate, stable and shipped.**

**L1 — Terminal control plane (stable, shipped today):**
- Single-session auto-attach, detach, re-attach
- Multi-pane splits with full terminal protocol support (Kitty keyboard, OSC 8, images)
- Status bar, keybindings, multi-client attach
- libghostty-backed wire protocol (VT bytes server→client, structured input client→server)

**L2 & L3 — Collections and metadata (roadmapped, spec'd, wire hooks in place):**
- Named session lifecycle (CREATE_SESSION, KILL_COLLECTION)
- TUI layout persistence, window management, metadata storage
- Full `phux` CLI surface (new, ls, kill, rename, send-keys, run, snapshot, wait)

**Federation and agent SDK (v0.2+, addressing ready from day 1):**
- Control plane routing across satellites (SATELLITE{host, id} TerminalIds already accepted)
- Agent SDK with structured L1 interface (wire hooks in place, implementation pending)
- Predictive local echo and lazy state sync

The L1 substrate is the part worth building against now. For contributors,
[`CONTRIBUTING.md`](./CONTRIBUTING.md) has the exact roadmap and constraints.

## Install

Prebuilt binaries via Homebrew (macOS arm64/x86_64, Linux x86_64):

```sh
brew install phall1/phux/phux
```

To hack on it instead, build from source — see Quickstart below. The
binary is not on crates.io (it needs zig to build `libghostty-vt`); only
the [`phux-protocol`](https://crates.io/crates/phux-protocol) wire crate
publishes there. Release mechanics live in
[`docs/RELEASING.md`](./docs/RELEASING.md).

## Quickstart

```sh
nix develop                # or direnv allow once
just ci                    # the bar — fmt-check + lint + test + deny + doc
cargo run --bin phux       # auto-spawns a server and attaches
```

Detach with the default prefix (`Ctrl-A d`). Walk-through in
[`docs/QUICKSTART.md`](./docs/QUICKSTART.md).

## Where to go next

| You want to | Read |
|---|---|
| Understand the model | [`docs/CONCEPTS.md`](./docs/CONCEPTS.md) |
| Run it | [`docs/QUICKSTART.md`](./docs/QUICKSTART.md) |
| Customize config and keybindings | [`docs/CONFIG.md`](./docs/CONFIG.md) |
| Read the wire spec | [`docs/spec/`](./docs/spec/) |
| Understand how it's built | [`docs/architecture/`](./docs/architecture/) |
| Read the TUI surface | [`docs/consumers/tui.md`](./docs/consumers/tui.md) |
| Read the long arc | [`docs/vision.md`](./docs/vision.md) |
| See past decisions | [`ADR/README.md`](./ADR/README.md) |
| Contribute | [`CONTRIBUTING.md`](./CONTRIBUTING.md) |

The doc system itself is defined in
[`docs/CONVENTIONS.md`](./docs/CONVENTIONS.md) — frontmatter schema,
TL;DR rule, ADR template, CI gates.

## Crates

| Crate | Purpose |
|---|---|
| `phux` | Binary; `attach` and `server` subcommands today |
| `phux-protocol` | Wire types, codec, version negotiation. The only crate intended for publication |
| `phux-core` | Domain types: in-process terminal / collection registries |
| `phux-server` | Daemon: per-terminal actor, PTY supervision, output fanout |
| `phux-client` | TUI client: local libghostty Terminal + RenderState redraw + ratatui chrome |
| `phux-config` | TOML config schema + status widget contract |

Future, not yet started: `phux-client-sdk` (L1-only typed Rust handle
for agents) and `phux-client-gui` (native GUI consumer over
libghostty's surface API).

## Non-goals

Each of these is a "no" that keeps the substrate honest, not a feature
deferred:

- **No embedded scripting language.** Commands are typed IPC messages.
  Logic that wants a runtime can shell out to one.
- **No plugin host.** Hooks are typed events. A plugin contract, if it
  ever lands, comes after we know what is genuinely pluggable.
- **No copy-mode reinvention.** Selection and extraction belong to
  libghostty and the host terminal. phux owns exactly one primitive
  libghostty doesn't provide: literal search over scrollback.
- **No homegrown crypto.** SSH and Unix-socket permissions are the
  trust model.
- **No format-template DSL.** The status bar takes typed widgets, not a
  printf dialect to maintain forever.

Full rationale in [`CONTRIBUTING.md`](./CONTRIBUTING.md).

## License

Dual-licensed under [MIT](./LICENSE-MIT) or
[Apache-2.0](./LICENSE-APACHE) at your option.
