---
audience: contributors, agents
stability: evolving
last-reviewed: 2026-05-30
---

# Releasing

**TL;DR.** Two independent distribution channels. The `phux` binary
ships as prebuilt tarballs via a `v*` tag → GitHub release → Homebrew
tap (`phall1/homebrew-phux`), with `v0.0.1` seeded as a Linux x86_64
artifact. The `phux-protocol` library ships separately to crates.io by
manual dispatch. `cargo install phux` is unsupported because the
binary/internal crates are not publishable and `libghostty-vt` is
git-pinned.

## What ships where

| Artifact | Channel | Mechanism |
|---|---|---|
| `phux`, `phux-mcp` binaries | Homebrew + GitHub release | [`release.yml`](../.github/workflows/release.yml) on a `v*` tag |
| `phux-protocol` crate | crates.io | [`publish-crate.yml`](../.github/workflows/publish-crate.yml), manual dispatch |

Every other crate (`phux`, `phux-core`, `phux-server`, `phux-client`,
`phux-config`, `phux-mcp`) is `publish = false`: binary or internal-only.
The binary also carries the workspace's git-pinned `libghostty-vt`, which
crates.io rejects, so the installable CLI ships through release artifacts
and Homebrew instead of `cargo install phux`.

## Versioning

The workspace shares one `version` in the root `Cargo.toml`
(`[workspace.package]`), and the internal `[workspace.dependencies]`
pins must match it — bump both or the workspace fails to resolve.
`phux-protocol` overrides with its own version line because it cuts
crates.io releases on its own cadence.

To bump the binary release version:

1. Edit `[workspace.package].version` in the root `Cargo.toml`.
2. Edit the five internal `phux-* = { ..., version = "X.Y.Z" }`
   requirements in `[workspace.dependencies]` to match (leave
   `phux-protocol` at its own version).
3. `nix develop -c cargo build` to refresh `Cargo.lock`, commit.

## Cutting a binary release

```sh
git tag v0.1.0
git push origin v0.1.0
```

`release.yml` then builds `phux` + `phux-mcp` for
`aarch64-apple-darwin`, `x86_64-apple-darwin`, and
`x86_64-unknown-linux-gnu`, packages
`phux-<tag>-<target>.tar.gz` + `.sha256`, creates the GitHub release, and
— if the `HOMEBREW_TAP_DEPLOY_KEY` secret is set — regenerates and pushes
`Formula/phux.rb` to the tap. macOS builds run inside `nix develop`; Linux
builds use rustup plus setup-zig on Ubuntu so the ELF does not record a
`/nix/store` dynamic loader. `v0.0.1` was seeded with a Linux x86_64 tarball
plus checksum, but that first artifact is Nix-linked and not portable; do not
point installers or the tap at it.

Seed a host-only release locally without CI:

```sh
nix develop -c cargo build --release --bin phux --bin phux-mcp
just dist v0.0.2                       # -> dist/phux-v0.0.2-<host>.tar.gz (+ .sha256)
gh release create v0.0.2 dist/*        # attach the tarball + checksum
```

### Required secret

`HOMEBREW_TAP_DEPLOY_KEY` — the **private** half of an SSH key whose
public half is a write-enabled deploy key on `phall1/homebrew-phux`.
Without it the release still publishes; only the automatic formula bump
is skipped (a warning annotation is emitted). The formula itself is
produced by [`scripts/gen-formula.sh`](../scripts/gen-formula.sh), which
emits a block only for the targets that actually built — so a
partial-matrix release still yields an installable formula.

### Curl installer contract

The curl installer is a convenience layer over GitHub release artifacts. The
unversioned command is user-facing only after a post-`v0.0.1` release is the
latest GitHub release:

```sh
curl -fsSL https://raw.githubusercontent.com/phall1/phux/main/scripts/install.sh | bash
```

Keep it aligned with the release layout above. It should download the target
tarball and `.sha256` sidecar from the selected release, verify the checksum
before unpacking, and install `phux` + `phux-mcp` into
`${PHUX_INSTALL_DIR:-$HOME/.local/bin}`. With no `--version`, it resolves the
latest GitHub release. Keep the explicit `v0.0.1` Linux refusal until a newer
portable Linux release is published, and keep install docs version-pinned or
source-first until latest no longer resolves to `v0.0.1`.

### CPU baseline caveat

`libghostty-vt`'s `build.rs` lets zig auto-detect the host CPU for
native builds, so `x86_64` artifacts may carry instructions specific to
the runner generation and can `SIGILL` on older hardware.
`aarch64-apple-darwin` has a uniform baseline and is unaffected. Pinning
an `x86_64` baseline through `libghostty-vt`'s build is future work.

## Publishing phux-protocol to crates.io

Publishing is irreversible — versions cannot be reused and the name
cannot be reclaimed — so this is never automatic.

1. Settle `docs/spec/` + the `phux-protocol` version (see
   [`CONTRIBUTING.md`](../CONTRIBUTING.md)).
2. Dry-run locally: `just publish-protocol-dry` (packages + verifies;
   the default feature set has no git deps, so it builds clean).
3. Authenticate: `cargo login` once on the publishing machine, or set
   the `CARGO_REGISTRY_TOKEN` secret for the workflow.
4. Publish: `just publish-protocol`, or dispatch `publish-crate.yml`
   with `dry_run: false`.

The `server` feature's optional `libghostty-vt` resolves to the
crates.io release (`>= 0.1.1`) for external consumers; verify that
release is API-compatible with the git rev pinned in `Cargo.toml`
before relying on the `server` feature downstream.

Do not publish the binary crate or internal workspace crates as part of
this workflow. For users, the idiomatic crates.io command is
`cargo add phux-protocol`; `cargo install phux` remains unsupported until
the binary crate is intentionally made publishable and its git-pinned
terminal dependency is removed or replaced.

## Installing from the tap

```sh
brew install phall1/phux/phux
```
