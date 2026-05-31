# phux developer commands.
# Run `just` (no args) to list them.

default:
    @just --list

# Scaffold a commented starter config into a worktree-local XDG dir
# (./.phux-xdg) so you can test config changes without touching your real
# ~/.config/phux. Re-run freely: `phux config init` refuses to clobber.
# Inspect the result with: XDG_CONFIG_HOME="$PWD/.phux-xdg" phux config show
scaffold-config:
    XDG_CONFIG_HOME="{{justfile_directory()}}/.phux-xdg" cargo run -q -p phux -- config init

# Quick type-check across the workspace.
check:
    cargo check --workspace --all-targets

# Build all crates (debug).
build:
    cargo build --workspace --all-targets

# Release build with full LTO.
build-release:
    cargo build --workspace --release

# Format every Rust file in place.
fmt:
    cargo fmt --all

# CI-style format check — fails if anything is dirty.
fmt-check:
    cargo fmt --all -- --check

# Clippy with warnings denied. The bar.
lint:
    cargo clippy --workspace --all-targets --all-features -- -D warnings

# Run tests via nextest (parallel, sane output).
test:
    cargo nextest run --workspace --all-features

# They are `#[ignore]`d so the default `test`/`ci` run stays deterministic
# — in the full parallel pool the server spawns starve and the socket-bind
# wait trips. Run on demand (they pass reliably one binary at a time, ~2s).
# CI may invoke this as a separate step.
#
# NOTE: the in-process e2e flywheel suite (the `E2eBuilder` harness, the
# resize-storm / attach-churn stress tests, and the wall-clock
# `perf_latency` gate in crates/phux-server/tests/) runs in the normal
# `just test` / `just ci` pool — it is fast and does not spawn subprocesses.
#
# Binary-level e2e tests that spawn real PTY-backed `phux server` subprocesses.
e2e:
    cargo nextest run -p phux --test run_wait_e2e --run-ignored all

# Spins a real `phux` server + session, drives a scripted scenario (heavy
# colored output, a 2nd client attach, a resize storm, an input line) and
# writes screen snapshots + a summary to /tmp/phux-repro-<ts>/ for
# inspection. See crates/phux-server/examples/e2e-repro.rs.
#
# One-command real-server repro of a lag/crash edge case.
e2e-repro:
    cargo run -p phux-server --example e2e-repro

# Smoke-test the examples/agents/ scripts against a throwaway server, so
# they cannot rot silently against CLI changes (phux-wiv). Builds `phux`
# once, pins SHELL=/bin/sh for a banner-free seed pane (no p10k/direnv
# noise in snapshots), then runs every example and fails on any non-zero
# exit. Like `e2e` it spawns real PTY-backed servers, so it stays OUT of
# the parallel `ci` pool and runs on demand or as its own CI step.
examples-smoke:
    bash scripts/examples-smoke.sh

# Lint shell scripts with shellcheck (the harness, the boundary/docs
# guards, and the examples). Provided by the dev shell. Gates at
# `warning` severity: the examples carry deliberate `info`-level nits
# (sourced libs shellcheck can't follow, single-quoted heredoc-ish
# program strings) that are correct as written. On-demand, not in `ci`.
shellcheck:
    shellcheck --severity=warning scripts/*.sh examples/agents/*.sh

# Stable-cargo test for environments without nextest.
test-cargo:
    cargo test --workspace --all-features

# Dependency hygiene: licenses, advisories, bans.
deny:
    cargo deny check

# Build rustdoc with warnings denied — mirrors the CI `doc` gate.
doc:
    RUSTDOCFLAGS='-D warnings' cargo doc --workspace --no-deps --all-features

# Watch loop — re-check + test on every save.
watch:
    cargo watch -x check -x 'nextest run --workspace'

# Boundary guard: ratatui imports must stay under phux-client/src/render/.
# See epic phux-5ke and ARCHITECTURE.md.
check-ratatui-boundary:
    bash scripts/check-ratatui-boundary.sh

# Doc system gates: frontmatter, TL;DR, dead links, ADR status, spec version.
# See docs/CONVENTIONS.md.
docs-check:
    bash scripts/check-docs.sh

# Everything CI must pass.
ci: fmt-check lint check-ratatui-boundary docs-check test deny doc
    @echo "ok"

# Print the toolchain we are pinned to.
toolchain:
    @rustc --version
    @cargo --version

# Package the host-target release binaries into a tarball matching the
# release workflow's naming (phux-<tag>-<target>.tar.gz) under dist/. Used
# to seed the first Homebrew release locally; CI does this per-target on a
# `v*` tag. Pass the tag, e.g. `just dist v0.0.1`.
dist TAG:
    bash scripts/dist.sh {{TAG}}

# Dry-run the crates.io publish of phux-protocol (package + verify, no
# upload). The only publishable crate. Mirrors the publish-crate workflow.
publish-protocol-dry:
    cargo publish --dry-run -p phux-protocol

# Publish phux-protocol to crates.io. IRREVERSIBLE. Requires `cargo login`
# (or CARGO_REGISTRY_TOKEN). Run `just publish-protocol-dry` first.
publish-protocol:
    cargo publish -p phux-protocol

# Builds the `profiling` profile (release codegen + line-table debug
# info) then records a Firefox Profiler JSON at target/samply-profile.json.
# Default subcommand is `server`; pass any other subcommand + args:
#
#   just profile                 # records `phux server`
#   just profile attach default  # records `phux attach default`
#
# samply is not a workspace dep — install with `cargo install samply`.

# CPU-profile the phux binary with samply.
profile *ARGS:
    @if ! command -v samply >/dev/null 2>&1; then \
        echo "error: samply not found on PATH." >&2; \
        echo "  install it with:  cargo install samply" >&2; \
        echo "  (samply is intentionally not a workspace dep; it is a host tool)" >&2; \
        exit 127; \
    fi
    cargo build --profile profiling --bin phux
    @echo ""
    @echo "Recording profile -> target/samply-profile.json"
    @echo "  Stop the profiled process (Ctrl-C) to finalize the recording."
    @echo ""
    samply record --output target/samply-profile.json -- target/profiling/phux {{ if ARGS == "" { "server" } else { ARGS } }}
    @echo ""
    @echo "Profile written to target/samply-profile.json"
    @echo "  View it with:  samply load target/samply-profile.json"
    @echo "  (opens https://profiler.firefox.com in your browser)"
