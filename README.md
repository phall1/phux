<!--
audience: humans, contributors, agents
stability: stable
last-reviewed: 2026-06-17
-->

<div align="center">

<!-- LOGO: drop a wordmark/logo asset here once one exists, e.g.
     <img src="docs/assets/logo.svg" alt="phux" width="320"> -->

# phux

**the tmux job, done — a terminal is an object on a wire**

<!-- BADGES: only badges that resolve today live here. The rest are
     placeholders until the asset/tag exists. -->
[![CI](https://github.com/phall1/phux/actions/workflows/ci.yml/badge.svg)](https://github.com/phall1/phux/actions/workflows/ci.yml)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)
<!-- TODO badges (do not enable until they resolve):
     - crates.io: needs the first `phux-protocol` publish
     - version/release: needs a v0.0.x tag populating GitHub Releases
     - Homebrew: needs the first bottle in the tap
-->

[Concepts](./docs/CONCEPTS.md) ·
[Quickstart](./docs/QUICKSTART.md) ·
[Install](./docs/INSTALL.md) ·
[Agents](./docs/consumers/agents.md) ·
[Spec](./docs/spec/) ·
[Architecture](./docs/architecture/) ·
[Contributing](./CONTRIBUTING.md)

</div>

<!-- DEMO: a ~10s asciinema cast / GIF goes here. Show kitty graphics +
     truecolor surviving a detach/reattach, then a `phux run` / `phux watch`
     line so the agent angle lands in the same breath. Recipe: docs/demo.md.
     Replace this comment with the image once docs/assets/demo.gif exists. -->

phux is a terminal multiplexer. You attach, split panes, detach, and come back
later to find your shells still running — the tmux job, done. The difference is
underneath: in phux a terminal is a first-class object on a wire, not a screen
trapped behind one process. The same live terminal can be held by more than one
thing at once — your TUI, a GUI, an AI agent — each reading and writing the real
terminal rather than a screenshot of it.

## Why it's different

Two consequences fall out of that, and they're the reason to use phux.

**Modern terminals stay modern across a reattach.** Kitty graphics, truecolor,
hyperlinks, the modern keyboard protocol — they keep working after you detach
and reattach, because phux never re-parses your bytes in the middle. Most
multiplexers degrade or drop these the moment they sit between you and your
terminal. The same terminal engine ([libghostty][lghv]) runs on both ends of the
wire, so bytes pass straight through instead of being re-parsed and degraded.

**An agent is a first-class user, not a guest.** An AI agent can drive the same
terminal you're looking at, over the wire, with the same authority you have.
There is no "agent mode" to enable — there are terminals, and some of the things
attached to them are people.

## Install

phux is v0.0.x. Pick the tier that's honest about your machine.

**From source — works today.**

```sh
git clone https://github.com/phall1/phux
cd phux
nix develop          # pins the toolchain, including the zig libghostty needs
cargo run --bin phux
```

You're attached. Detach with `Ctrl-A d` — the server keeps your shells alive —
and run `phux` again to come back to exactly where you were. Off-Nix pins and the
agent binaries are in [INSTALL.md](./docs/INSTALL.md).

**Prebuilt binary — coming.** A download-and-run binary needs a `v0.0.x` tag
cut to populate GitHub Releases first; until that lands, build from source.

**Homebrew — coming.** A tap exists; the first bottles haven't shipped. Until
they do, the source build above is the install path.

There is no `cargo install phux` yet — `phux-protocol`'s first crates.io publish
is still pending.

## The wire, without a TTY

Everything above also works headless. The same terminals, addressed by name or
id, driven from a script — or an agent — with JSON coming back:

```sh
phux ls --json                          # what's running
phux send-keys build:0.1 'cargo test' Enter
phux run build "cargo test"             # runs in a real pane, returns the exit code
phux wait build --until "0 failed"      # blocks until the output appears, then exits 0
phux watch --json work:1.0 | jq .       # live events: bells, titles, idle, lifecycle
```

Point an MCP agent at it and it gets the same surface as a set of tools:
`phux-mcp` exposes the core verbs over JSON-RPC stdio. The agent holds the
terminal the same way you do — same object, same keys.

## How it works

A terminal multiplexer is the right shape for one person at one keyboard. Now
some of the users are programs, and a program does not want a screen to read
pixels off of — it wants to start something, learn when it finished, and read
the exit code. Build the terminal so a program can use it cleanly and the human
gets the better terminal too: nothing reinterprets the bytes in the middle, so
nothing gets mangled on the way through.

So phux makes the **terminal** the unit — not the session, not the window. A
human's TUI and an agent's API are two ways to hold the same object; neither is
the privileged one. The longer version, with the diagrams:
[`docs/CONCEPTS.md`](./docs/CONCEPTS.md).

[lghv]: https://github.com/Uzaaft/libghostty-rs

## Status

The line between what's solid and what's still a promise is kept honest here:

**Stable — won't move under you**
- The TUI: attach / detach / reattach, multi-pane splits, status bar,
  keybindings, multiple clients on one session
- Full modern-protocol passthrough (Kitty keyboard, truecolor, OSC 8, OSC 133,
  images) — the parser is identical on both ends
- The wire, version-negotiated. `phux-protocol` is the crate boundary; its
  first crates.io publish is pending.

**Real and tested — the API may still move before 1.0**
- The headless verbs: `ls`, `snapshot`, `send-keys`, `run`, `wait`, `watch`,
  `new`, `kill`, `rename`, `config`
- `phux-mcp` — the same surface as MCP tools

**Designed and addressed-for — not wired yet**
- Driving terminals across machines. The wire already carries
  `SATELLITE{host, id}`; nothing routes it yet. v0.2.
- A native GUI consumer, a typed Rust SDK crate, predictive local echo.

Anything not in the first two lists is a direction, not a feature.

Not sure phux fits? [When to use phux (and when not to)](./docs/when-to-use.md)
sorts the common cases and says which are shipped and which are still a promise.

## Where to go from here

| You want to | Read |
|---|---|
| Decide if it's for you | [`docs/when-to-use.md`](./docs/when-to-use.md) |
| Get it on your machine | [`docs/INSTALL.md`](./docs/INSTALL.md) |
| First session, start to finish | [`docs/QUICKSTART.md`](./docs/QUICKSTART.md) |
| Understand the model | [`docs/CONCEPTS.md`](./docs/CONCEPTS.md) |
| Drive it from an agent | [`docs/consumers/agents.md`](./docs/consumers/agents.md) · [`docs/consumers/mcp.md`](./docs/consumers/mcp.md) |
| Customize keys and config | [`docs/CONFIG.md`](./docs/CONFIG.md) |
| Read the wire spec | [`docs/spec/`](./docs/spec/) |
| See how it's built | [`docs/architecture/`](./docs/architecture/) |
| Read where it's going | [`docs/vision.md`](./docs/vision.md) |
| See the decisions | [`ADR/README.md`](./ADR/README.md) |
| Build it with us | [`CONTRIBUTING.md`](./CONTRIBUTING.md) |

## Crates

| Crate | Does |
|---|---|
| `phux` | The binary: `attach` / `server` plus the headless verbs |
| `phux-protocol` | Wire types, codec, version negotiation. The one crate meant for publishing |
| `phux-core` | Domain types: in-process terminal / collection registries |
| `phux-server` | The daemon: per-terminal actor, PTY supervision, output fanout |
| `phux-client-core` | Renderer + protocol client, ratatui-free (the boundary is compiler-enforced) |
| `phux-client` | The TUI chrome (ratatui) over `phux-client-core` |
| `phux-config` | TOML config schema + status widget contract |
| `phux-mcp` | The agent surface as MCP tools, over JSON-RPC stdio |

## What phux deliberately won't do

Each of these is a "no" that keeps the model honest, not a gap:

- **No embedded scripting language.** Commands are typed messages. Logic that
  wants a runtime can shell out to one.
- **No plugin host.** Hooks are typed events. A plugin contract, if it ever
  arrives, comes after we know what's actually pluggable.
- **No copy-mode reinvention.** Selection belongs to your terminal. phux owns
  one primitive nobody else provides: literal search over scrollback.
- **No homegrown crypto.** SSH and Unix-socket permissions are the trust model.
- **No format-template DSL.** The status bar takes typed widgets, not a printf
  dialect we'd have to maintain forever.

Full reasoning: [`CONTRIBUTING.md`](./CONTRIBUTING.md).

## License

Dual-licensed under [MIT](./LICENSE-MIT) or [Apache-2.0](./LICENSE-APACHE).
