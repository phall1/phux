---
audience: contributors, agents
stability: evolving
last-reviewed: 2026-05-30
---

# Releasing

**TL;DR.** One button ships the project: bump the root workspace version, run
`just release-preflight vX.Y.Z`, merge to the default branch, then dispatch
**Actions -> release -> Run workflow** with that tag. The workflow creates or
verifies the tag, publishes GitHub release tarballs, refreshes the Homebrew tap,
and can publish `phux-protocol` to crates.io when explicitly confirmed.
`cargo install phux is unsupported` because the binary/internal crates are not
publishable. Windows is not supported by this release lane.

## Release cockpit

| You want to | Do this |
|---|---|
| Prove the release is locally coherent | `just release-preflight vX.Y.Z` |
| Skip crates.io packaging during a fast/offline binary-only check | `just release-preflight-fast vX.Y.Z` |
| Ship binaries + Homebrew | Dispatch **Actions -> release** with `tag=vX.Y.Z`, `publish_protocol=false` |
| Ship binaries + Homebrew + `phux-protocol` | Dispatch **Actions -> release** with `publish_protocol=true` and `crates_io_confirm=publish phux-protocol` |
| Retry only the crates.io package | Dispatch **Actions -> publish-crate** with an existing `vX.Y.Z` tag |
| Check a suspected install-doc drift | `bash scripts/check-install-surface.sh` |

The release workflow is the source of truth. Local commands are preflight and
emergency fallback only; they should not be the normal publishing path.

## What runs when

| Flow | Trigger | What it does |
|---|---|---|
| Pull request CI | `pull_request` | Docs-only detection, docs check, fmt, clippy, rustdoc, cargo-deny, unit tests, and fast real-PTY e2e unless the change is docs-only. |
| Main CI | push to `main` | Same gates as PR CI, always full, and refreshes the warm caches. |
| Stress lane | nightly, manual, or PR label `stress` | Heavy resize/output/lifecycle storms that are useful but too slow for every PR. |
| Build timings | manual | Cold `cargo --timings` report for compile-time diagnosis. |
| Full release | manual `release` workflow | Tag create/verify, portable binary tarballs + checksums, GitHub release, Homebrew tap update, optional crates.io publish. |
| Crate retry | manual `publish-crate` workflow | `phux-protocol` package dry-run, then publish when `dry_run=false`. |

Required secrets:

| Secret | Used by | Required for |
|---|---|---|
| `HOMEBREW_TAP_DEPLOY_KEY` | `release.yml` | Automatic push to `phall1/homebrew-phux`. If absent, the GitHub release still publishes and the tap step warns/skips. |
| `CARGO_REGISTRY_TOKEN` | `release.yml`, `publish-crate.yml` | Publishing `phux-protocol` to crates.io. Not needed for binary/Homebrew-only releases. |

Post-release verification:

```sh
scripts/install.sh --dry-run --version vX.Y.Z
brew fetch --formula phall1/phux/phux
cargo search phux-protocol --limit 1
```

Use the GitHub release page to confirm that the expected target tarballs and
`.sha256` sidecars uploaded. The current release lane builds macOS arm64,
Linux x86_64, and Linux arm64.

## What ships where

| Artifact | Channel | Mechanism |
|---|---|---|
| `phux`, `phux-mcp` binaries | Homebrew + GitHub release | [`release.yml`](../.github/workflows/release.yml), manual dispatch |
| `phux-protocol` crate | crates.io | [`release.yml`](../.github/workflows/release.yml) with `publish_protocol`; [`publish-crate.yml`](../.github/workflows/publish-crate.yml) is a crate-only fallback |

Every other crate (`phux`, `phux-core`, `phux-server`, `phux-client`,
`phux-config`, `phux-mcp`) is `publish = false`: binary or internal-only.
The installable CLI ships through release artifacts and Homebrew instead of
`cargo install phux`.

Each binary release must produce `phux and phux-mcp artifacts` for every target
that publishes. The tarball layout is:

```text
phux-<tag>-<target>/
  phux
  phux-mcp
  README.md
  LICENSE-MIT
  LICENSE-APACHE
```

The workflow smoke-checks both binaries in the staging directory before it
creates `phux-<tag>-<target>.tar.gz` and the matching `.sha256` sidecar.
Homebrew installs both binaries from the same tarball.

## Versioning

The workspace shares one `version` in the root `Cargo.toml`
(`[workspace.package]`). All in-repo crates inherit it with
`version.workspace = true`, and internal workspace dependencies use path-only
requirements so release bumps do not require duplicate manifest edits.
`phux-protocol` still ships to crates.io by manual dispatch, but its package
version is the same root workspace version for the checkout being published.

To bump the release version:

1. Edit `[workspace.package].version` in the root `Cargo.toml`.
2. `cargo check --workspace` to refresh `Cargo.lock`.
3. `just release-preflight vX.Y.Z` to verify the tag, install surface, formula
   generation, and `phux-protocol` package dry-run.
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
tag already exists, the workflow reuses that tagged commit so failed releases
can be retried after workflow fixes. After that it builds `phux` + `phux-mcp`,
creates/updates the GitHub release, updates the Homebrew tap when
`HOMEBREW_TAP_DEPLOY_KEY` is configured, and publishes `phux-protocol` only
when the crates.io confirmation input is present.

The manual workflow is the release entrypoint. Running this locally is a
preflight, not the publish trigger:

```sh
just release-check v0.1.0
```

`release.yml` builds `phux` + `phux-mcp` for
`aarch64-apple-darwin`, `x86_64-unknown-linux-gnu`, and
`aarch64-unknown-linux-gnu`, packages
`phux-<tag>-<target>.tar.gz` + `.sha256`, creates the GitHub release, and
— if the `HOMEBREW_TAP_DEPLOY_KEY` secret is set — regenerates and pushes
`Formula/phux.rb` to the tap. Release builds use rustup plus setup-zig instead
of the Nix dev shell, because portable release binaries must not record
`/nix/store` dynamic library paths. The workflow checks macOS binaries with
`otool -L` before packaging. It also runs `just release-check <tag>` before
building, so a pushed tag that does not match Cargo's resolved package versions
fails before any artifact or tap update is published. `v0.0.3` is the current
portable public release. `v0.0.1` was seeded with a Linux x86_64 tarball plus
checksum, but that first artifact is Nix-linked and not portable; do not point
installers or the tap at it.

For an emergency host-only artifact, use the same dist layout locally:

```sh
cargo build --release --bin phux --bin phux-mcp
just dist vX.Y.Z                       # -> dist/phux-vX.Y.Z-<host>.tar.gz (+ .sha256)
gh release upload vX.Y.Z dist/*        # attach the tarball + checksum
```

Do not use this for normal releases. Do not run a local release build inside
`nix develop`; use a host toolchain plus Zig on `PATH` so the packaged binaries
do not link to Nix-store libraries.

### Required secret

`HOMEBREW_TAP_DEPLOY_KEY` — the **private** half of an SSH key whose
public half is a write-enabled deploy key on `phall1/homebrew-phux`.
Without it the release still publishes; only the automatic formula bump
is skipped (a warning annotation is emitted). The formula itself is
produced by [`scripts/gen-formula.sh`](../scripts/gen-formula.sh), which
emits a stable top-level URL plus overrides only for the targets that actually
built — so a partial-matrix release still yields an installable formula.

### Curl installer contract

The curl installer is a convenience layer over GitHub release artifacts. The
unversioned command is user-facing because `v0.0.3` or newer is now the latest
GitHub release:

```sh
curl -fsSL https://raw.githubusercontent.com/phall1/phux/main/scripts/install.sh | bash
```

Keep it aligned with the release layout above. It should download the target
tarball and `.sha256` sidecar from the selected release, verify the checksum
before unpacking, and install `phux` + `phux-mcp` into
`${PHUX_INSTALL_DIR:-$HOME/.local/bin}`. With no `--version`, it resolves the
current GitHub release. Keep the explicit `v0.0.1` refusal as a historical
safety guard. User-facing docs should identify `v0.0.3` or newer as the current
portable release.

### CPU baseline caveat

`libghostty-vt`'s `build.rs` lets zig auto-detect the host CPU for
native builds, so Linux artifacts may carry instructions specific to the
runner generation and can `SIGILL` on older hardware. `aarch64-apple-darwin`
has a uniform baseline and is unaffected. Pinning Linux CPU baselines through
`libghostty-vt`'s build is future work.

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
`cargo add phux-protocol`; `cargo install phux is unsupported` until
the binary crate and its internal dependencies are intentionally made
publishable.

## Installing from the tap

```sh
brew install phall1/phux/phux
```

The tap does not add Windows support; Windows is not supported here. A Windows
release would need a separate design and build lane rather than a formula tweak.
