---
audience: humans, contributors
stability: stable
last-reviewed: 2026-06-03
---

# Install

**TL;DR.** Homebrew on macOS and Linux x86_64 is the one-liner. From source is
a Nix dev shell plus one `cargo run` — that's the path if you're hacking on it
or you're on a platform the bottle doesn't cover. There is no `cargo install
phux` yet (only `phux-protocol` is published); the binary ships via brew and
source for now.

---

## Homebrew (the easy one)

macOS and Linux, x86_64:

```sh
brew install phall1/phux/phux
phux            # attached. that's it.
```

`phux` with no arguments auto-spawns a server and attaches to it. Detach with
`Ctrl-A d`; run `phux` again to come back.

Apple Silicon and Linux aarch64 build fine from source today; a bottle for them
is on the list, not in the tap yet.

## From source

You need the toolchain phux is pinned to. The supported way to get it is the Nix
dev shell, because it pins everything — including the Zig compiler libghostty's
build wants — to versions known to work.

```sh
git clone https://github.com/phall1/phux
cd phux
nix develop          # or `direnv allow` once, then it loads on cd
cargo run --bin phux # auto-spawns a server and attaches
```

That's a working phux. For the guided first session — splits, config,
detach/reattach — see [`QUICKSTART.md`](./QUICKSTART.md).

### Off Nix

If you'd rather not use Nix, you're signing up to match the pins by hand. As of
this writing:

- Rust **1.90**
- Zig **0.15** (`zig_0_15` — libghostty-vt's build invokes it)
- `cargo-nextest`, `cargo-deny` on `PATH` if you want to run the gates

The Nix flake (`flake.nix`) is the source of truth for exact versions; when in
doubt, read it rather than this paragraph.

### Verify the build

```sh
just check           # quick type-check across the workspace
just ci              # the full bar: fmt-check + lint + test + deny + doc
```

`just ci` is what CI runs and what a PR has to pass. If it's green, you're good.

## Drive it from an agent

The agent surface ships with the same binary — nothing extra to install. The
MCP adapter is its own binary in the workspace:

```sh
cargo run --bin phux-mcp     # JSON-RPC over stdio; wire it into your MCP client
```

Tool catalog and JSON contracts: [`consumers/mcp.md`](./consumers/mcp.md). The
plain-CLI version of the same surface: [`consumers/agents.md`](./consumers/agents.md).

## Platform support, honestly

| Platform | Status |
|---|---|
| macOS (Apple Silicon) | Source: yes. Bottle: not yet. |
| macOS (x86_64) | Brew + source |
| Linux x86_64 | Brew + source |
| Linux aarch64 | Source: yes. Bottle: not yet. |
| Windows | No. Not on the near roadmap. |

phux is v0.1. The install story gets shorter from here, not longer.
