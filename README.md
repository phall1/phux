<!--
audience: humans, contributors, agents
stability: stable
last-reviewed: 2026-06-03
-->

# phux

*A terminal multiplexer. It's pronounced however you just pronounced it.*

It does roughly what tmux does. You attach, you split some panes, you
detach, your shell keeps running, you come back later and it's all still
there. So far, not a sales pitch.

Two things are a little different.

**One.** The modern terminal stuff — kitty graphics, truecolor, hyperlinks,
the works — doesn't fall apart when you detach and reattach. It just keeps
working. Boring, on purpose.

**Two.** An AI agent can drive the same terminal you're looking at. Not a
screenshot of it. Not a scrape. The actual terminal, over an actual API. You
didn't have to turn on a "robot mode," because there isn't one. There's just
terminals — and some of the things attached to them happen to be people.

That second one is doing more than it lets on. We'll get to it.

<!-- DEMO: a ~10s asciinema cast / GIF goes RIGHT HERE. Show kitty graphics +
     truecolor surviving a detach/reattach, then a `phux run`/`phux watch`
     line so the agent angle lands in the same breath. This is the single
     most important pixel on the page. Recipe and storyboard: docs/demo.md.
     When the GIF lands at docs/assets/demo.gif, replace this whole comment
     with a standard markdown image pointing at that path. -->

## Try it

```sh
brew install phall1/phux/phux
phux
```

That's the whole thing. You're attached. Detach with `Ctrl-A d`; the server
keeps your shell alive. Type `phux` again and you're right back where you were.

(No Homebrew, or you want to hack on it? [Install from source](./docs/INSTALL.md)
— it's a Nix dev shell and one `cargo run`.)

## The part the GUI doesn't show you

Everything above also works without a TTY. Same terminals, addressed by name
or id, driven from a script — or an agent — with clean JSON coming back:

```sh
phux ls --json                          # what's running
phux send-keys build:0.1 'cargo test' Enter
phux run build "cargo test"             # runs in a real pane, hands you the exit code
phux wait build --until "0 failed"      # blocks until the output shows up, exits 0
phux watch --json work:1.0 | jq .       # live events: bells, titles, idle, lifecycle
```

Point an MCP-speaking agent at it instead and it gets the same surface as tools
— `phux-mcp` exposes the core verbs over JSON-RPC stdio. The agent isn't peeking
at a human's session through a keyhole. It's a first-class user with the same keys.

## Why it's built this way

Old multiplexers were built for one person at one keyboard, because that was
the only kind of user there was. Fair enough at the time.

Now some of the users are programs. A program doesn't want a screen to read
pixels off of — it wants to start a thing, find out when it's done, and read
the exit code. The funny part: if you build the terminal so a program can use
it cleanly, the human gets a better deal too. Nothing re-interprets your bytes
in the middle, so nothing gets mangled on the way through.

So phux makes the **terminal** the thing — not the "session," not the
"window." A human's TUI and an agent's API are just two ways to hold the same
object, and neither one is the real customer.

> Under the hood: the same terminal engine ([libghostty][lghv]) runs on both
> ends of the wire, so bytes pass straight through instead of getting re-parsed
> and degraded. That's the unglamorous reason the fancy stuff survives a
> reattach. You don't have to care about it. It just means things work.

The longer version, with the boxes-and-arrows: [`docs/CONCEPTS.md`](./docs/CONCEPTS.md).

[lghv]: https://github.com/Uzaaft/libghostty-rs

## What actually works today

It's v0.1. Calibrate accordingly, then get pleasantly surprised.

**Solid, won't move under you:**
- The TUI — attach / detach / re-attach, multi-pane splits, status bar,
  keybindings, multiple clients on one session
- Full modern-protocol passthrough (Kitty keyboard, truecolor, OSC 8, OSC 133,
  images), because the parser is identical on both ends
- The wire itself, version-negotiated. `phux-protocol` is the one crate on
  crates.io.

**Real and tested, but the API may still wiggle before 1.0:**
- The headless verbs above: `ls`, `snapshot`, `send-keys`, `run`, `wait`,
  `watch`, `new`, `kill`, `rename`, `config`
- `phux-mcp` — the same surface as MCP tools

**Designed, addressed-for, not wired yet:**
- Driving terminals across machines. The wire already speaks
  `SATELLITE{host, id}`; nothing routes it yet. v0.2.
- A native GUI consumer, a typed Rust SDK crate, predictive local echo.

If it's not in one of those first two lists, it's a promise, not a feature.
We try to keep that line honest.

## Where to go from here

| You want to | Read |
|---|---|
| Get it on your machine | [`docs/INSTALL.md`](./docs/INSTALL.md) |
| First session, start to finish | [`docs/QUICKSTART.md`](./docs/QUICKSTART.md) |
| Actually understand it | [`docs/CONCEPTS.md`](./docs/CONCEPTS.md) |
| Drive it from an agent | [`docs/consumers/agents.md`](./docs/consumers/agents.md) · [`docs/consumers/mcp.md`](./docs/consumers/mcp.md) |
| Customize keys and config | [`docs/CONFIG.md`](./docs/CONFIG.md) |
| Read the wire spec | [`docs/spec/`](./docs/spec/) |
| See how it's built | [`docs/architecture/`](./docs/architecture/) |
| Read where it's going | [`docs/vision.md`](./docs/vision.md) |
| See past decisions | [`ADR/README.md`](./ADR/README.md) |
| Build it with us | [`CONTRIBUTING.md`](./CONTRIBUTING.md) |

## Crates

| Crate | Does |
|---|---|
| `phux` | The binary: `attach` / `server` plus the headless verbs |
| `phux-protocol` | Wire types, codec, version negotiation. The only one meant for publishing |
| `phux-core` | Domain types: in-process terminal / collection registries |
| `phux-server` | The daemon: per-terminal actor, PTY supervision, output fanout |
| `phux-client-core` | Renderer + protocol client, ratatui-free (the boundary is compiler-enforced) |
| `phux-client` | The TUI chrome (ratatui) over `phux-client-core` |
| `phux-config` | TOML config schema + status widget contract |
| `phux-mcp` | The agent surface as MCP tools, over JSON-RPC stdio |

## Things phux deliberately won't do

Each of these is a "no" that keeps the thing honest, not a feature we forgot:

- **No embedded scripting language.** Commands are typed messages. Logic that
  wants a runtime can shell out to one.
- **No plugin host.** Hooks are typed events. A plugin contract, if it ever
  shows up, comes after we know what's actually pluggable.
- **No copy-mode reinvention.** Selection belongs to your terminal. phux owns
  exactly one primitive nobody else provides: literal search over scrollback.
- **No homegrown crypto.** SSH and Unix-socket permissions are the trust model.
- **No format-template DSL.** The status bar takes typed widgets, not a printf
  dialect we'd have to maintain until the heat death of the universe.

Full reasoning: [`CONTRIBUTING.md`](./CONTRIBUTING.md).

## License

Dual-licensed under [MIT](./LICENSE-MIT) or [Apache-2.0](./LICENSE-APACHE), your
call.
