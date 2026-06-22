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

# Fast e2e lane — gates every PR (the `e2e` step in ci.yml). Covers the
# headless agent-surface contract (`run_wait_e2e`) plus the wall-clock perf
# gates (`perf_latency`, `perf_colored_output`). These spin a real server +
# PTY, so they are `#[ignore]`d out of the default `just test` pool and run
# serially with `--retries=2`: serial removes the CPU contention that makes
# a fresh attach handshake / snapshot render miss `WIRE_RECV_TIMEOUT`, and
# the retries absorb residual environment-driven flakes (mirroring the
# reconnect override in .config/nextest.toml). Finishes in minutes — fast
# enough to block a PR on.

# Fast e2e lane (run_wait_e2e + perf gates) — gates every PR.
e2e:
    cargo nextest run -p phux --test run_wait_e2e --run-ignored all \
      --test-threads=1 --retries=2
    cargo nextest run -p phux-server --run-ignored ignored-only \
      --test-threads=1 --retries=2 \
      --test perf_latency --test perf_colored_output

# Heavy stress/flywheel lane — runs OFF the PR critical path (the `stress`
# GitHub workflow: post-merge on `main` + nightly). Resize/output/lifecycle
# storms that hammer a real server + PTY. They are CPU-starvation-sensitive:
# the server is one current-thread runtime, and on a 2-core runner the
# output-flood-vs-resize-reflow feedback loop balloons a sub-second test
# into minutes (e.g. both_axes_shrink_storm_under_output: ~0.3s on a
# multi-core box, ~13 min on a 2-core runner). That cost is pure CPU
# starvation, not a code defect — so these run where they don't block a PR,
# never as a `just ci` gate. Run locally any time (one binary at a time,
# they pass reliably).

# Heavy stress storms — off the PR path (post-merge + nightly stress.yml).
stress:
    cargo nextest run -p phux-server --run-ignored ignored-only \
      --test-threads=1 --retries=2 \
      --test stress_resize_storm --test stress_resize_extremes \
      --test stress_attach_churn --test stress_lifecycle_churn \
      --test stress_output_extremes --test stress_spawn_kill

# Spins a real `phux` server + session, drives a scripted scenario (heavy
# colored output, a 2nd client attach, a resize storm, an input line) and
# writes screen snapshots + a summary to /tmp/phux-repro-<ts>/ for
# inspection. See crates/phux-server/examples/e2e-repro.rs.
#
# One-command real-server repro of a lag/crash edge case.
e2e-repro:
    cargo run -p phux-server --example e2e-repro

# Capture a REAL traced client session for the debugging flywheel. Attaches
# with JSON tracing to a timestamped log, then prints the path to hand off
# for analysis. Reproduce the lag/crash during the session (and a crash's
# backtrace lands in the same log), then detach. An auto-spawned server
# inherits the same tracing env, so the log holds both sides (filter by the
# `target` field: phux_client::* vs phux_server::*).
#   just trace-attach                 # session "default"
#   just trace-attach work            # a named session
#   just trace-attach work phux=trace # crank the level
trace-attach session="default" level="phux=debug":
    #!/usr/bin/env bash
    set -euo pipefail
    log="/tmp/phux-trace-$(date +%s).json"
    echo "[trace] -> $log  (PHUX_LOG_FORMAT=json, RUST_LOG={{level}}); reproduce the issue, then detach"
    PHUX_LOG="$log" PHUX_LOG_FORMAT=json RUST_LOG="{{level}}" cargo run -q -p phux -- attach {{session}} || true
    echo "[trace] session ended -> hand off this file: $log"
    echo "[trace] quick peek at the slowest renders:"
    jq -rc 'select(.fields.message=="close" and (.span.name|test("render|handle_server_frame|synthesize|tick_emit"))) | [.fields["time.busy"], .span.name, (.span.changed_row_count//.span.out_bytes//"")] | @tsv' "$log" 2>/dev/null | sort -h | tail -15 || true

# Smoke-test the examples/agents/ scripts against a throwaway server, so
# they cannot rot silently against CLI changes (phux-wiv). Builds `phux`
# once, pins SHELL=/bin/sh for a banner-free seed pane (no p10k/direnv
# noise in snapshots), then runs every example and fails on any non-zero
# exit. Like `e2e` it spawns real PTY-backed servers, so it stays OUT of
# the parallel `ci` pool and runs on demand or as its own CI step.
examples-smoke:
    bash scripts/examples-smoke.sh

# Run the checked-in plugin package through the same discover/validate/run
# sequence documented in examples/plugins/agent-tools/README.md.
plugin-demo:
    XDG_CONFIG_HOME="{{justfile_directory()}}/examples/plugins/agent-tools/config" cargo run -q -p phux -- config plugins
    XDG_CONFIG_HOME="{{justfile_directory()}}/examples/plugins/agent-tools/config" cargo run -q -p phux -- config plugins --json
    XDG_CONFIG_HOME="{{justfile_directory()}}/examples/plugins/agent-tools/config" cargo run -q -p phux -- config run com.phux.demo.agent-tools inspect
    XDG_CONFIG_HOME="{{justfile_directory()}}/examples/plugins/agent-tools/config" cargo run -q -p phux -- config run com.phux.demo.agent-tools inspect --json

# Lint shell scripts with shellcheck (the harness, the boundary/docs
# guards, and the examples). Provided by the dev shell. Gates at
# `warning` severity: the examples carry deliberate `info`-level nits
# (sourced libs shellcheck can't follow, single-quoted heredoc-ish
# program strings) that are correct as written. On-demand, not in `ci`.
shellcheck:
    shellcheck --severity=warning scripts/*.sh examples/agents/*.sh examples/plugins/*/scripts/*.sh

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

# Doc system gates: frontmatter, TL;DR, dead links, ADR status, spec version.
# See docs/CONVENTIONS.md.
docs-check:
    bash scripts/check-docs.sh

# Everything CI must pass.
#
# The ratatui-confinement boundary (ADR-0020) used to be a grep guard
# (`check-ratatui-boundary`); phux-0fv replaced it with a crate split, so
# `cargo build`/`lint` now enforce it structurally — `phux-client-core` has
# no `ratatui` dependency.
ci: fmt-check lint docs-check test deny doc
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

# Check that a release tag matches the resolved Cargo package versions before
# cutting/pushing the tag.
release-check TAG:
    bash scripts/check-release-version.sh {{TAG}}

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

# --- Build observability ---------------------------------------------------
# Three lenses on "why is the build this slow / this big". Honest caveat:
# the dominant COLD-build cost is libghostty-vt's zig blob (a build.rs
# shell-out to zig), which none of these three see — they profile the Rust
# side. For the zig cost, lean on the CPU-keyed CI cache and not rebuilding
# per-worktree. These tools find the *Rust* wins: critical-path crates,
# monomorphization bloat, and binary size.

# Cargo's built-in compile-time report -> target/cargo-timings/. Shows the
# per-crate timeline, the critical path (the longest dependency chain
# gating everything else), and the codegen-vs-frontend split. Pass extra
# args to change profile:
#   just timings                 # debug, --all-targets (dev iteration cost)
#   just timings --release       # the release/LTO timeline
# CAVEAT: a WARM build's timeline is near-empty because cached crates don't
# recompile. For a true cold picture, `cargo clean` first — or use the
# `build-timings` GitHub workflow, which always builds cold.

# HTML compile-time report (critical path, codegen vs frontend).
timings *ARGS:
    cargo build --workspace --all-targets --timings {{ARGS}}
    @echo "report -> target/cargo-timings/cargo-timing.html"

# LLVM IR lines emitted per (generic) function for one crate — the
# monomorphization-bloat view. A helper instantiated for hundreds of type
# combinations shows up at the top; the fix is usually `#[inline(never)]`
# or pulling the type-independent body into a non-generic fn. Reads the
# `llvm-tools-preview` component (pinned in rust-toolchain.toml).
#   just llvm-lines                     # phux-protocol lib (default)
#   just llvm-lines phux-server         # another crate's lib
#   just llvm-lines phux --bin phux     # a specific binary target

# Per-function LLVM IR line counts (monomorphization bloat) for one crate.
llvm-lines PKG='phux-protocol' *ARGS:
    cargo llvm-lines -p {{PKG}} {{ARGS}}

# Attribute release binary size. Defaults to a by-crate breakdown of the
# `phux` binary; pass args for the per-function view or another target:
#   just bloat                   # size by crate (the phux binary)
#   just bloat -n 30             # top 30 individual functions
#   just bloat --bin phux-mcp    # a different binary

# Attribute release binary size by crate (or per-fn with args).
bloat *ARGS:
    cargo bloat --release --bin phux {{ if ARGS == "" { "--crates" } else { ARGS } }}
