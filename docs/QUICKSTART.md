---
audience: humans, contributors
stability: stable
last-reviewed: 2026-06-03
---

# Quickstart

**TL;DR.** Drop into the Nix-pinned dev shell, run `just ci` to verify
the toolchain, then `cargo run` to spawn a server and attach. phux is
v0.1 — what works today is the TUI (attach/detach, multi-pane splits,
keybindings, status bar, config) plus the headless verbs you can script
or point an agent at (`ls`, `run`, `wait`, `watch`, `send-keys`,
`snapshot`, …). Federation routing is the main thing still on the wire
but not yet wired.

---

## Prerequisites

- macOS or Linux
- [Nix with flakes](https://nixos.org/download.html) (or, off-Nix:
  Rust 1.90, `zig` 0.15, `cargo-nextest`, `cargo-deny` on your PATH)

## Drop into the dev shell

```sh
nix develop          # one-shot
# or
direnv allow         # once; then automatic on cd
```

The shell pins Rust 1.90, `zig_0_15` (libghostty-vt's build needs it),
`nextest`, `deny`, `watch`, `insta`, `mutants`, and `just`.

## Verify

```sh
just check           # quick type-check across the workspace
just ci              # full CI: fmt-check + lint + test + deny + doc
```

`just ci` is the bar. PRs do not land until it's green.

## Run it

```sh
cargo run --bin phux           # auto-spawns a server, then attaches
```

Behind that single command: a `phux server` daemon binds to
`$XDG_RUNTIME_DIR/phux/phux.sock`, spawns one PTY-backed terminal
(your `$SHELL`), and the client attaches and starts rendering. Detach
with the default prefix (`Ctrl-A d`); the server keeps running. Run
`cargo run --bin phux` again and you re-attach to the same session,
right where you left it.

## What works today

- **Attach / detach** to a single auto-spawned session.
- **Split panes** (horizontal and vertical), kill panes.
- **Status bar** with widgets (time, session name); typed widget
  contract documented in [`consumers/tui.md`](./consumers/tui.md).
- **Keybindings** via a TOML config that layers over the shipped
  `default.toml`. Prefix-table + global chord resolution.
- **Help overlay** (`prefix ?`).
- **Multi-client attach** to the same session.
- **Bytes-on-wire terminal content**, structured input — full Kitty
  keyboard, OSC 8, OSC 133, true colour, image protocols pass through.
- **Headless verbs** you can run without a TTY: `ls`, `snapshot`,
  `send-keys`, `run`, `wait`, `watch`, `new`, `kill`, `rename`,
  `config`. Each addresses panes by the same selector grammar the TUI
  uses; reads take `--json`. This is the surface a script — or an agent
  — drives. See [`consumers/agents.md`](./consumers/agents.md).
- **MCP adapter** (`phux-mcp`): the same six core verbs as JSON-RPC
  tools. See [`consumers/mcp.md`](./consumers/mcp.md).

Try the headless side once you have a session up:

```sh
phux ls --json                       # list sessions
phux run . "echo hello && exit 3"    # run in the focused pane, get exit code 3 back
phux watch --json .                  # stream live events; Ctrl-C to stop
```

## What doesn't yet

- **Federation routing** (satellites, hubs). The wire already accepts
  `SATELLITE{host, id}`; nothing routes it. v0.2.
- **The typed Rust SDK crate** (`phux-client-sdk`) — the CLI and MCP
  surfaces cover agent use today; the crate is convenience on top.
- **Predictive local echo** (designed for; gated on a transport whose
  RTT actually needs it).

Each of these is spec'd before it's built, so the wire hooks are
already there. Run `bd ready` from any phux checkout to see the open
tickets.

## Next

You have a server running and a terminal attached. Where you go from
here depends on what you came for:

- To understand why the wire is shaped this way → [CONCEPTS.md](./CONCEPTS.md)
- To read the actual bytes → [`spec/`](./spec/)
- To build something on it, or fix something in it → [`../CONTRIBUTING.md`](../CONTRIBUTING.md)
