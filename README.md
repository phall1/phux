<!--
audience: humans, contributors, agents
stability: stable
last-reviewed: 2026-06-03
-->

# phux

**A terminal multiplexer that doesn't re-parse your terminal.**

tmux and screen sit in the middle of the wire and re-interpret every byte your
programs emit — which is why kitty graphics, undercurl, pixel-precision mouse,
and half of OSC degrade or vanish across a detach/reattach. phux runs the *same*
VT parser ([libghostty][lghv]) on **both ends** of the connection, so modern
terminal protocols survive a remote attach losslessly. Structurally — not "we
patched the common cases."

And the thing it multiplexes isn't a *session* or a *pane*. It's a **terminal**:
a first-class object you can spawn, observe, drive, and address — by hand from a
tmux-shaped TUI, or headless from a script or an agent over a typed wire, on this
box or across a fleet.

```sh
phux                                  # attach (auto-starts a server) — the tmux-shaped TUI
phux ls --json                        # list sessions, machine-readable
phux send-keys build:0.1 'cargo test' Enter
phux run build "cargo test"           # run in a real pane, get the exit code back
phux wait build --until "0 failed"    # block until output matches, then exit 0
phux watch --json work:1.0 | jq .     # live event stream: bells, titles, dirty/idle, lifecycle
```

<!-- DEMO: drop a ~10s asciinema cast / GIF here — kitty graphics + truecolor
     surviving a detach/reattach that tmux would smear. This is the single
     highest-leverage asset on the page; nothing converts this audience like
     seeing the thing render. -->

## What actually works today

Two surfaces ride the same source-of-truth libghostty `Terminal`:

- **The TUI** — auto-attach/detach/re-attach, multi-pane splits, status bar,
  keybindings, multi-client attach. Full modern-protocol passthrough (Kitty
  keyboard, truecolor, OSC 8 hyperlinks, OSC 133, images) because the parser is
  identical on both ends.
- **The headless / agent surface** — every command above is real and tested.
  A selector grammar (`name`, `name:window.pane`, `@id`, `.` focused, `=`
  last-focused) addresses any pane; reads are side-effect-free (no attach, no
  resize) and take `--json`. Same surface is exposed over MCP by the `phux-mcp`
  crate — six tools, JSON-RPC over stdio, no protocol privilege. **This is the
  part you point an agent at.**

Both are real binaries you can run now, not a roadmap. The honest line on what's
stable vs. still moving is in [Status](#status).

## The idea

Two decisions do all the work.

**The same parser on both ends.** The server's libghostty `Terminal` is
canonical; the client's is a local mirror for rendering. Nothing in the middle
re-parses VT — so new terminal features light up on the next libghostty bump, on
both ends, for free. Older multiplexers re-parse mid-path and degrade fidelity.
phux structurally can't.

**The terminal is the unit, not the session.** Sessions, windows, panes, splits
— the whole tmux vocabulary — live in the TUI's metadata layer, never on the
wire. An agent speaks to *terminals* and never hears the word "window." That's
why a non-human consumer is a first-class citizen instead of a screen-scraping
hack bolted onto a tool built for one human at a keyboard.

The wire is layered — L1 Terminal / L2 Collection / L3 Metadata — and consumers
declare which tiers they speak so the server omits the rest. Identity is
federation-ready from byte zero: `TerminalId` is `LOCAL{id} | SATELLITE{host,id}`.
v0.1 constructs `LOCAL`; the wire already accepts `SATELLITE`; v0.2 routes it;
the bytes never change. Full mental model: [`docs/CONCEPTS.md`](./docs/CONCEPTS.md).

[lghv]: https://github.com/Uzaaft/libghostty-rs

## Install

Homebrew (macOS, Linux x86_64):

```sh
brew install phall1/phux/phux
```

From source — the toolchain is Nix-pinned (Rust 1.90 + Zig for libghostty's
build), so the dev shell is the supported path:

```sh
nix develop          # or: direnv allow
cargo run --bin phux # auto-spawns a server and attaches
```

Detach with the default prefix, `Ctrl-A d`. Full walk-through:
[`docs/QUICKSTART.md`](./docs/QUICKSTART.md).

## Status

**Stable, shipped — L1 terminal control plane:**
- TUI: attach / detach / re-attach, multi-pane splits, status bar, keybindings,
  multi-client attach
- libghostty wire protocol (VT bytes server→client, structured input
  client→server), version-negotiated. `phux-protocol` is the only crate
  published to crates.io.

**Shipped and tested, API still moving (pre-1.0):**
- Headless verbs: `ls`, `snapshot`, `send-keys`, `run`, `wait`, `watch`, `new`,
  `kill`, `rename`, `config` — selector grammar + `--json`
- `phux-mcp`: MCP adapter exposing six tools over JSON-RPC stdio
- L2 agent protocol layer (collections, terminal-state reads, event subscriptions)

**Not yet — addressing is in the wire, routing is not:**
- Control-plane routing across satellites (`SATELLITE{host,id}` ids are accepted
  today, not yet routed)
- Native GUI consumer over libghostty's surface API

Building against it now? L1 is the part that won't move under you. Contributor
roadmap + constraints: [`CONTRIBUTING.md`](./CONTRIBUTING.md).

## Where to go next

| You want to | Read |
|---|---|
| Understand the model | [`docs/CONCEPTS.md`](./docs/CONCEPTS.md) |
| Run it | [`docs/QUICKSTART.md`](./docs/QUICKSTART.md) |
| Customize config and keybindings | [`docs/CONFIG.md`](./docs/CONFIG.md) |
| Drive it from an agent | [`docs/consumers/agents.md`](./docs/consumers/agents.md) · [`docs/consumers/mcp.md`](./docs/consumers/mcp.md) |
| Read the wire spec | [`docs/spec/`](./docs/spec/) |
| Understand how it's built | [`docs/architecture/`](./docs/architecture/) |
| Read the TUI surface | [`docs/consumers/tui.md`](./docs/consumers/tui.md) |
| Read the long arc | [`docs/vision.md`](./docs/vision.md) |
| See past decisions | [`ADR/README.md`](./ADR/README.md) |
| Contribute | [`CONTRIBUTING.md`](./CONTRIBUTING.md) |

The doc system itself is defined in [`docs/CONVENTIONS.md`](./docs/CONVENTIONS.md)
— frontmatter schema, TL;DR rule, ADR template, CI gates.

## Crates

| Crate | Purpose |
|---|---|
| `phux` | Binary; `attach`/`server` + the headless verbs |
| `phux-protocol` | Wire types, codec, version negotiation. The only crate intended for publication |
| `phux-core` | Domain types: in-process terminal / collection registries |
| `phux-server` | Daemon: per-terminal actor, PTY supervision, output fanout |
| `phux-client-core` | Renderer + protocol client, ratatui-free (the boundary is compiler-enforced) |
| `phux-client` | TUI chrome (ratatui) over `phux-client-core` |
| `phux-config` | TOML config schema + status widget contract |
| `phux-mcp` | MCP adapter: the agent CLI surface over JSON-RPC stdio |

Future, not yet started: `phux-client-gui` (native GUI consumer over libghostty's
surface API).

## Non-goals

Each of these is a "no" that keeps the substrate honest, not a feature deferred:

- **No embedded scripting language.** Commands are typed IPC messages. Logic that
  wants a runtime can shell out to one.
- **No plugin host.** Hooks are typed events. A plugin contract, if it ever lands,
  comes after we know what is genuinely pluggable.
- **No copy-mode reinvention.** Selection and extraction belong to libghostty and
  the host terminal. phux owns exactly one primitive libghostty doesn't provide:
  literal search over scrollback.
- **No homegrown crypto.** SSH and Unix-socket permissions are the trust model.
- **No format-template DSL.** The status bar takes typed widgets, not a printf
  dialect to maintain forever.

Full rationale in [`CONTRIBUTING.md`](./CONTRIBUTING.md).

## License

Dual-licensed under [MIT](./LICENSE-MIT) or [Apache-2.0](./LICENSE-APACHE) at your
option.
