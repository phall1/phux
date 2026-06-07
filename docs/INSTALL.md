---
audience: humans, contributors
stability: stable
last-reviewed: 2026-06-06
---

# Install

**TL;DR.** Homebrew on macOS and Linux x86_64 is the one-liner for the binary. Building from source — including any platform the bottle doesn't cover yet — uses the Nix dev shell, which QUICKSTART.md owns. There is no `cargo install phux` yet; only `phux-protocol` is published, and the binary ships via brew and source.

---

## Homebrew

macOS and Linux, x86_64:

```sh
brew install phall1/phux/phux
phux            # auto-spawns a server and attaches
```

`phux` with no arguments auto-spawns a server and attaches to it. Detach with
`Ctrl-A d`; run `phux` again to re-attach.

Apple Silicon and Linux aarch64 build from source today; a bottle for them is
tracked, not in the tap yet (see the matrix below).

## From source

Building from source uses the Nix dev shell, which pins the toolchain —
including the Zig compiler libghostty's build needs — to known-good versions.
The setup block (dev shell, off-Nix pins, `just ci`) lives in
[`QUICKSTART.md`](./QUICKSTART.md); follow it there rather than duplicating it
here. The short version:

```sh
git clone https://github.com/phall1/phux
cd phux
nix develop          # or `direnv allow` once, then it loads on cd
cargo run --bin phux # auto-spawns a server and attaches
```

## Drive it from an agent

The agent surface ships with the same binary — nothing extra to install. The
MCP adapter is its own binary in the workspace:

```sh
cargo run --bin phux-mcp     # JSON-RPC over stdio; wire it into your MCP client
```

Tool catalog and JSON contracts: [`consumers/mcp.md`](./consumers/mcp.md). The
plain-CLI version of the same surface: [`consumers/agents.md`](./consumers/agents.md).

## Platform support

| Platform | Status |
|---|---|
| macOS (Apple Silicon) | Source: yes. Bottle: not yet. |
| macOS (x86_64) | Brew + source |
| Linux x86_64 | Brew + source |
| Linux aarch64 | Source: yes. Bottle: not yet. |
| Windows | No. Not on the near roadmap. |
