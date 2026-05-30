<!--
audience: humans, contributors, agents
stability: stable
last-reviewed: 2026-05-28
-->

# phux

**A libghostty-backed terminal control plane.** Spawn, observe,
control, persist, and address terminals — locally or across a fleet —
with a tmux-shaped TUI riding on top as one consumer among several.

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

phux aims to be [**smol**][smol]:

> - Write programs that solve a well-defined problem.
> - Write programs that behave the way most users expect them to behave.
> - Write programs that a single person can maintain.
> - Write programs that compose with other smol tools.
> - Write programs that can be finished.

The well-defined problem: *spawn, observe, control, persist, and
address libghostty terminals — locally or across a fleet — with
conformance tiers a consumer can target without inheriting everything
else.* The reference TUI proves the substrate is real. The substrate
is what makes phux not-tmux.

[smol]: https://smol.tauri.app/

## Status

**Pre-alpha. Spec first, code second.**

Working today:

- Auto-attach to a single session; detach; re-attach
- Multi-pane splits, kill, focus, click-to-focus
- Status bar with typed widgets; keybindings (prefix + global chords); help overlay
- Multi-client attach to the same session
- Full bytes-on-wire pass-through (Kitty keyboard, OSC 8, OSC 133, true colour, images)

Not yet wired: most of L2 Collection lifecycle, most L3 metadata
commands, federation routing, the agent SDK, predictive local echo,
most of the `phux <subcommand>` CLI surface. See
[`docs/QUICKSTART.md`](./docs/QUICKSTART.md) for the full state.

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

No embedded scripting language. No plugin host. No copy-mode
reinvention. No homegrown crypto. No format-template DSL. Full list
with rationale in [`CONTRIBUTING.md`](./CONTRIBUTING.md).

## License

Dual-licensed under [MIT](./LICENSE-MIT) or
[Apache-2.0](./LICENSE-APACHE) at your option.
