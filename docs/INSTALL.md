---
audience: humans, contributors
stability: stable
last-reviewed: 2026-06-09
---

# Install

**TL;DR.** Source builds are the only install path guaranteed to work while the latest GitHub release is the seeded `v0.0.1`. Homebrew becomes the primary binary path after a post-`v0.0.1` Formula lands. The curl installer is a convenience wrapper around portable GitHub release tarballs and their `.sha256` sidecars. crates.io is for `phux-protocol`; `cargo install phux` is unsupported while the binary/internal crates are unpublished.

---

## Homebrew

Once `Formula/phux.rb` is published in
[`phall1/homebrew-phux`](https://github.com/phall1/homebrew-phux), install with:

```sh
brew install phall1/phux/phux
```

Use this path first on supported Homebrew platforms after a post-`v0.0.1`
Formula lands. If the Formula has not reached the tap for your target yet,
use a source build.

## Curl installer

The installer is a convenience wrapper over the same GitHub release assets.
Use it once a post-`v0.0.1` release is available:

```sh
curl -fsSL https://raw.githubusercontent.com/phall1/phux/main/scripts/install.sh | bash
```

It verifies the release `.sha256` sidecar before unpacking and installs
`phux` and `phux-mcp` into `${PHUX_INSTALL_DIR:-$HOME/.local/bin}`. Set
`PHUX_INSTALL_DIR` to choose a different bin directory. With no `--version`, it
uses the latest GitHub release; latest is currently `v0.0.1`, and the installer
refuses that release because it is not a portable binary release.

To install a specific future tag before it is latest:

```sh
curl -fsSL https://raw.githubusercontent.com/phall1/phux/main/scripts/install.sh | bash -s -- --version v0.0.2
```

## Release tarball

Release tags include target-specific tarballs and checksum sidecars:

```sh
tag=v0.1.0
target=aarch64-apple-darwin
base="https://github.com/phall1/phux/releases/download/${tag}"
curl -LO "${base}/phux-${tag}-${target}.tar.gz"
curl -LO "${base}/phux-${tag}-${target}.tar.gz.sha256"
shasum -a 256 -c "phux-${tag}-${target}.tar.gz.sha256"
tar -xzf "phux-${tag}-${target}.tar.gz"
```

Put the extracted `phux` and `phux-mcp` binaries somewhere on `PATH`. Avoid the
seeded `v0.0.1` Linux tarball outside Nix environments; it was built with a
Nix-store dynamic loader and is not portable.

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

## crates.io

crates.io is for the wire library, not for installing the `phux` binary:

```sh
cargo add phux-protocol
```

`cargo install phux` is not supported yet. The binary crate and internal
workspace crates are `publish = false`; install the CLI through Homebrew,
the curl installer, release tarballs, or a source build.

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
| macOS (Apple Silicon) | Source: yes. Homebrew Formula/artifact pending. |
| macOS (x86_64) | Source: yes. Homebrew Formula/artifact pending. |
| Linux x86_64 | Source: yes. Portable release artifacts are built by the tag workflow after `v0.0.1`. |
| Linux aarch64 | Source: yes. Release artifact pending. |
| Windows | No. Not on the near roadmap. |
