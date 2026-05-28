---
audience: humans, contributors
stability: stable
last-reviewed: 2026-05-28
---

# Quickstart

**TL;DR.** Drop into the Nix-pinned dev shell, run `just ci` to verify
the toolchain, then `cargo run` to spawn a server and attach. phux is
pre-alpha — what works today is single-pane attach with multi-pane
splits, keybindings, status bar, and config loading. Most lifecycle
and federation surface is not yet wired.

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
`$XDG_RUNTIME_DIR/phux/phux.sock`, a single PTY-backed terminal is
spawned (your `$SHELL`), and the client attaches and starts rendering.
Detach with the default prefix (`Ctrl-A d`). Re-running `cargo run
--bin phux` re-attaches to the same session.

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

## What doesn't yet

- The full subcommand set (`phux new`, `phux ls`, `phux kill`) — only
  `phux` (naked, auto-attach) and `phux server` ship today.
- Most L2 Collection lifecycle and L3 metadata commands.
- Federation routing (satellites, hubs).
- The agent SDK.
- Predictive local echo (designed for; gated on a transport whose
  RTT actually needs it).

Open tickets in the `bd` tracker — run `bd ready` from any phux
checkout to see them.

## Next

- Conceptual model → [CONCEPTS.md](./CONCEPTS.md)
- Wire bytes → [`spec/`](./spec/)
- Contributing → [`../CONTRIBUTING.md`](../CONTRIBUTING.md)
