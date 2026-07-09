#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

failures=0

require_fixed() {
  local file="$1"
  local needle="$2"
  if ! grep -Fq -- "$needle" "$ROOT/$file"; then
    printf 'missing: %s: %s\n' "$file" "$needle" >&2
    failures=$((failures + 1))
  fi
}

forbid_fixed() {
  local file="$1"
  local needle="$2"
  if grep -Fq -- "$needle" "$ROOT/$file"; then
    printf 'stale claim: %s: %s\n' "$file" "$needle" >&2
    failures=$((failures + 1))
  fi
}

require_regex() {
  local file="$1"
  local regex="$2"
  if ! grep -Eq -- "$regex" "$ROOT/$file"; then
    printf 'missing regex: %s: %s\n' "$file" "$regex" >&2
    failures=$((failures + 1))
  fi
}

require_fixed README.md "Install and run"
require_fixed README.md "brew install phall1/phux/phux"
require_fixed README.md "nix develop -c cargo install --locked --path crates/phux"
require_fixed README.md "nix develop -c cargo install --locked --path crates/phux-mcp"
require_fixed README.md "v0.0.3"
require_fixed README.md "Supported install channels"
require_fixed README.md "cargo install phux is unsupported"
require_fixed README.md "Windows is not supported"
require_fixed README.md "First run: persistent session + agent loop"
require_fixed README.md "macOS arm64, Linux x86_64, and Linux arm64"
require_fixed README.md "first portable public release"
forbid_fixed README.md "macOS x86_64"

require_fixed docs/INSTALL.md "Use Homebrew on supported Homebrew platforms"
require_fixed docs/INSTALL.md "Supported install channels"
require_fixed docs/INSTALL.md "Homebrew"
require_fixed docs/INSTALL.md "Curl installer"
require_fixed docs/INSTALL.md "Release tarball"
require_fixed docs/INSTALL.md "From source"
require_fixed docs/INSTALL.md "nix develop -c cargo install --locked --path crates/phux"
require_fixed docs/INSTALL.md "nix develop -c cargo install --locked --path crates/phux-mcp"
require_fixed docs/INSTALL.md "portable public release"
require_fixed docs/INSTALL.md "phux-mcp is bundled"
require_fixed docs/INSTALL.md "cargo install phux is unsupported"
require_fixed docs/INSTALL.md "Windows is not supported"
require_fixed docs/INSTALL.md "First run: persistent session + agent loop"
require_fixed docs/INSTALL.md "verifies the release .sha256 sidecar before unpacking"
require_fixed docs/INSTALL.md "| macOS (x86_64) | Source: yes. No official release artifact. |"
require_fixed docs/INSTALL.md "| Linux aarch64 | Curl/tarball: yes. Homebrew: yes where Linuxbrew supports the host. Source: yes. |"

require_fixed docs/RELEASING.md "phux and phux-mcp artifacts"
require_fixed docs/RELEASING.md "cargo install phux is unsupported"
require_fixed docs/RELEASING.md "Windows is not supported"
require_fixed docs/RELEASING.md "Release cockpit"
require_fixed docs/RELEASING.md "just release-preflight vX.Y.Z"
require_fixed docs/RELEASING.md "publish_protocol=true"
require_fixed docs/RELEASING.md "CARGO_REGISTRY_TOKEN"
require_fixed docs/RELEASING.md "portable public release"
require_fixed docs/RELEASING.md 'explicit `v0.0.1` refusal'
require_fixed docs/RELEASING.md "aarch64-apple-darwin"
require_fixed docs/RELEASING.md "x86_64-unknown-linux-gnu"
require_fixed docs/RELEASING.md "aarch64-unknown-linux-gnu"

require_fixed scripts/install.sh 'download "$sha_url" "$sha_path"'
require_fixed scripts/install.sh 'sha256sum -c "$(basename "$sha_path")"'
require_fixed scripts/install.sh 'shasum -a 256 -c "$(basename "$sha_path")"'
require_fixed scripts/install.sh '"${stage_name}/phux-mcp"'
require_fixed scripts/install.sh 'cp -f "${stage_dir}/phux-mcp" "${install_dir}/phux-mcp"'

require_fixed justfile "release-preflight TAG:"
require_fixed justfile "release-preflight-fast TAG:"
require_fixed scripts/release-preflight.sh "cargo publish --dry-run --allow-dirty -p phux-protocol"

require_fixed scripts/gen-formula.sh 'bin.install "phux-mcp"'
require_fixed scripts/gen-formula.sh 'assert_match version.to_s, shell_output("#{bin}/phux --version 2>&1")'
require_fixed .github/workflows/release.yml 'cargo +1.90.0 build --release --bin phux --bin phux-mcp'
require_fixed .github/workflows/release.yml 'cp -f target/release/phux target/release/phux-mcp'
require_fixed .github/workflows/release.yml 'target: aarch64-apple-darwin'
require_fixed .github/workflows/release.yml 'target: x86_64-unknown-linux-gnu'
require_fixed .github/workflows/release.yml 'target: aarch64-unknown-linux-gnu'
require_regex .github/workflows/release.yml 'test -x .*phux-mcp|command -v .*phux-mcp|./phux-mcp --'

if [ "$failures" -ne 0 ]; then
  printf 'install surface check failed: %d missing contract item(s)\n' "$failures" >&2
  exit 1
fi

echo "install surface check passed"
