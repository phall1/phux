---
audience: humans, contributors
stability: evolving
last-reviewed: 2026-06-06
---

# Quickstart

**TL;DR.** This doc owns the setup path: drop into the Nix-pinned dev shell,
run `just ci` to verify the toolchain, then `cargo run --bin phux` to spawn a
server and attach. It also catalogs the TUI, agent, MCP, plugin, and workspace
surfaces that run today versus the ones that are designed but not built.

---

## Prerequisites

- macOS or Linux
- [Nix with flakes](https://nixos.org/download.html), or, off-Nix: Rust 1.90, `zig` 0.15, `cargo-nextest`, and `cargo-deny` on your PATH

## Drop into the dev shell

```sh
nix develop          # one-shot
# or
direnv allow         # once; then automatic on cd
```

The shell pins Rust 1.90, `zig_0_15` (libghostty-vt's build needs it), `nextest`, `deny`, `watch`, `insta`, `mutants`, and `just`.

## Verify

```sh
just check           # quick type-check across the workspace
just ci              # full CI: fmt-check + lint + test + deny + doc
```

`just ci` is the bar. PRs do not land until it is green.

## Run it

```sh
cargo run --bin phux           # auto-spawns a server, then attaches
```

Behind that one command: a `phux server` daemon binds to `$XDG_RUNTIME_DIR/phux/phux.sock`, spawns one PTY-backed terminal running your `$SHELL`, and the client attaches and renders it. Detach with the default prefix (`Ctrl-A d`); the server keeps running. Run `cargo run --bin phux` again to re-attach where you left off.

## What works today

Interactive TUI:

- Attach and detach to a single auto-spawned session, with multiple clients on the same session.
- Split panes (horizontal and vertical) and kill panes.
- A status bar with widgets; the typed widget contract is in [`consumers/tui.md`](./consumers/tui.md).
- Prefix-aware help hints in the default status bar (`Ctrl-A ?`, `Ctrl-A :`,
  `Ctrl-A [`), so first-run discovery is visible without opening docs.
- Keybindings via a TOML config that layers over the shipped `default.toml`, with prefix-table and global chord resolution.
- A help overlay (`prefix ?`).
- Terminal content as bytes on the wire with structured input: Kitty keyboard, OSC 8, OSC 133, true colour, and image protocols pass through to the engine on both ends.

Headless verbs you can run without a TTY:

```
attach   server   ls   new   kill   rename
snapshot send-keys wait watch run    config
ask      agent     plugin workspace
```

Read verbs take `--json` for a machine-readable shape, and every verb addresses panes by the same selector grammar the TUI uses. This is the surface a script or an agent drives; the per-verb catalog and JSON contracts live in [`consumers/agents.md`](./consumers/agents.md).

There is also an MCP adapter, `phux-mcp`, exposing the core verbs as JSON-RPC
tools plus `phux_ask` and plugin workspace profile discovery — see
[`consumers/mcp.md`](./consumers/mcp.md).

Remote attach for a phone or another native client uses the same server:

```sh
phux pair
phux server --listen 0.0.0.0:8787 --quic 0.0.0.0:8788
phux attach --ws wss://HOST:8787 --token HEX --cert-fingerprint FP
phux attach --quic HOST:8788 --token HEX --cert-fingerprint FP
```

WebSocket/TCP is the fallback for networks that block UDP; QUIC is the roaming
path when UDP is available.

Try the headless side once a session is up:

```sh
phux ls --json                       # list sessions
phux run . "echo hello && exit 3"    # run in the focused pane, get exit code 3 back
phux watch --json .                  # stream live events; Ctrl-C to stop
```

Try the agent workbench pieces:

```sh
phux agent list --json
phux agent explain .
XDG_CONFIG_HOME="$PWD/examples/plugins/agent-tools/config" \
  phux config run com.phux.demo.agent-tools smoke-integrations
```

`phux agent` reports explainable public state (`working`, `blocked`, `idle`,
`done`, or `unknown`) with confidence and evidence sources. The checked-in
agent-tools package demonstrates external Codex and Claude Code integration
records without requiring private credentials.

## What does not work yet

- **`split` and `detach` as CLI subcommands.** Splitting and detaching exist inside the interactive TUI, but they are not headless verbs. Tracked as bead phux-99te.
- **Federation routing.** The wire accepts `SATELLITE { host, id }`, but nothing routes it to a remote host. See the maturity note in [`CONCEPTS.md`](./CONCEPTS.md).
- **Predictive local echo.** Designed, gated on a transport whose round-trip actually needs it.
- **Live PTY resurrection through workspace restore.** `workspace restore`
  recreates sessions and seed processes from a typed archive. Live handoff
  across a server re-exec is the `upgrade` path.

The wire encoding is positional (big-endian, length-prefixed) today; a move to field-tagged TLV is deferred future work (beads phux-ktte, relates phux-i58). The normative codec statement is in [`spec/appendix-encoding.md`](./spec/appendix-encoding.md).

For where phux sits on the maturity curve overall, and which behaviors are spec-first rather than shipped, [`CONCEPTS.md`](./CONCEPTS.md) owns that picture.

## Next

You have a server running and a terminal attached. Where you go next depends on what you came for:

- To understand why the wire is shaped this way → [`CONCEPTS.md`](./CONCEPTS.md)
- To read the actual bytes → [`spec/README.md`](./spec/README.md)
- To drive it from an agent → [`consumers/agents.md`](./consumers/agents.md)
- To build something on it, or fix something in it → [`../CONTRIBUTING.md`](../CONTRIBUTING.md)
