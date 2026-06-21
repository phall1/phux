---
audience: contributors, agents
stability: evolving
last-reviewed: 2026-05-30
---

# Releasing

**TL;DR.** Prefer the `release` workflow's manual button. It creates or
verifies the `v*` tag, publishes GitHub release tarballs, refreshes the
Homebrew tap (`phall1/homebrew-phux`), and can publish `phux-protocol` to
crates.io when explicitly confirmed. `cargo install phux` is unsupported
because the binary/internal crates are not publishable.

## What ships where

| Artifact | Channel | Mechanism |
|---|---|---|
| `phux`, `phux-mcp` binaries | Homebrew + GitHub release | [`release.yml`](../.github/workflows/release.yml), manual dispatch |
| `phux-protocol` crate | crates.io | [`release.yml`](../.github/workflows/release.yml) with `publish_protocol`; [`publish-crate.yml`](../.github/workflows/publish-crate.yml) is a crate-only fallback |

Every other crate (`phux`, `phux-core`, `phux-server`, `phux-client`,
`phux-config`, `phux-mcp`) is `publish = false`: binary or internal-only.
The installable CLI ships through release artifacts and Homebrew instead of
`cargo install phux`.

## Versioning

The workspace shares one `version` in the root `Cargo.toml`
(`[workspace.package]`). All in-repo crates inherit it with
`version.workspace = true`, and internal workspace dependencies use path-only
requirements so release bumps do not require duplicate manifest edits.
`phux-protocol` still ships to crates.io by manual dispatch, but its package
version is the same root workspace version for the checkout being published.

To bump the binary release version:

1. Edit `[workspace.package].version` in the root `Cargo.toml`.
2. `cargo check --workspace` to refresh `Cargo.lock`.
3. `just release-check vX.Y.Z` to verify the tag and resolved package versions
   agree.
4. Commit the bump.

## Cutting a full release

Use GitHub Actions:

1. Open **Actions → release → Run workflow**.
2. Select the default branch.
3. Enter `tag` as `vX.Y.Z`.
4. Leave `publish_protocol` off for a binary/Homebrew-only release, or enable
   it to publish `phux-protocol` to crates.io.
5. If `publish_protocol` is enabled, type `publish phux-protocol` in
   `crates_io_confirm`.
6. Run the workflow.

The workflow validates that `vX.Y.Z` matches Cargo's resolved workspace
versions. If the tag is missing, it creates it from the default branch. If the
tag already exists, it must point at the same commit or the workflow fails
before building. After that it builds `phux` + `phux-mcp`, creates/updates the
GitHub release, updates the Homebrew tap when `HOMEBREW_TAP_DEPLOY_KEY` is
configured, and publishes `phux-protocol` only when the crates.io confirmation
input is present.

The manual workflow is the release entrypoint. Running these locally is now
only a preflight, not the publish trigger:

```sh
just release-check v0.1.0
```

`release.yml` then builds `phux` + `phux-mcp` for
`aarch64-apple-darwin`, `x86_64-apple-darwin`, and
`x86_64-unknown-linux-gnu`, packages
`phux-<tag>-<target>.tar.gz` + `.sha256`, creates the GitHub release, and
— if the `HOMEBREW_TAP_DEPLOY_KEY` secret is set — regenerates and pushes
`Formula/phux.rb` to the tap. macOS builds run inside `nix develop`; Linux
builds use rustup plus setup-zig on Ubuntu so the ELF does not record a
`/nix/store` dynamic loader. The workflow runs `just release-check <tag>` before
building, so a pushed tag that does not match Cargo's resolved package versions
fails before any artifact or tap update is published. `v0.0.1` was seeded with a
Linux x86_64 tarball plus checksum, but that first artifact is Nix-linked and
not portable; do not point installers or the tap at it.

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
cannot be reclaimed — so this still requires an explicit confirmation input.
For a normal release, use the `release` workflow's `publish_protocol` toggle
and `crates_io_confirm` phrase.

The crate-only fallback remains:

1. Settle `docs/spec/` + the `phux-protocol` version (see
   [`CONTRIBUTING.md`](../CONTRIBUTING.md)).
2. Dry-run locally: `just publish-protocol-dry` (packages + verifies;
   the default feature set has no git deps, so it builds clean).
3. Authenticate: `cargo login` once on the publishing machine, or set
   the `CARGO_REGISTRY_TOKEN` secret for the workflow.
4. Publish: `just publish-protocol`, or dispatch `publish-crate.yml`
   with `tag: vX.Y.Z` and `dry_run: false`.

The `server` feature's optional `libghostty-vt` resolves to the
crates.io release (`>= 0.2.0`) for external consumers; verify that
release is API-compatible with the workspace dependency before relying on
the `server` feature downstream.

Do not publish the binary crate or internal workspace crates as part of
this workflow. For users, the idiomatic crates.io command is
`cargo add phux-protocol`; `cargo install phux` remains unsupported until
the binary crate and its internal dependencies are intentionally made
publishable.

## Installing from the tap

```sh
brew install phall1/phux/phux
```
