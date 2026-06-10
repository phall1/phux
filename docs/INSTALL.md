---
audience: humans, contributors
stability: stable
last-reviewed: 2026-06-09
---

# Install

**TL;DR.** Source is the install path today. The Nix dev shell pins the full toolchain — including the Zig compiler libghostty's build needs — so the build is reproducible on any supported platform. A Homebrew tap ([`phall1/homebrew-phux`](https://github.com/phall1/homebrew-phux)) exists and the release pipeline targets it; the first bottles have not shipped yet. There is no `cargo install phux`; only `phux-protocol` is published, and the binary ships via source (and brew, once bottles land).

---

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

`phux` with no arguments auto-spawns a server and attaches to it. Detach with
`Ctrl-A d`; run `phux` again to re-attach.

## Homebrew

Not live yet. The tap is [`phall1/homebrew-phux`](https://github.com/phall1/homebrew-phux)
and the release workflow regenerates `Formula/phux.rb` on each `v*` tag; the
first release with bottles is tracked in [`RELEASING.md`](./RELEASING.md). Once
it ships, the install is:

```sh
brew install phall1/phux/phux
```

Until then, build from source.

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
| macOS (x86_64) | Source: yes. Bottle: not yet. |
| Linux x86_64 | Source: yes. Bottle: not yet. |
| Linux aarch64 | Source: yes. Bottle: not yet. |
| Windows | No. Not on the near roadmap. |
