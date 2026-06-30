#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TMP="$(mktemp -d "${TMPDIR:-/tmp}/phux-formula-check.XXXXXX")"
trap 'rm -rf "$TMP"' EXIT

write_sha() {
  local tag="$1"
  local target="$2"
  local sha="$3"
  printf '%s  phux-%s-%s.tar.gz\n' "$sha" "$tag" "$target" \
    > "${TMP}/phux-${tag}-${target}.tar.gz.sha256"
}

assert_contains() {
  local file="$1"
  local needle="$2"
  if ! grep -Fq "$needle" "$file"; then
    echo "error: expected ${file} to contain: ${needle}" >&2
    sed -n '1,160p' "$file" >&2
    exit 1
  fi
}

assert_top_level_url() {
  local file="$1"
  local first_url_line
  local first_block_line
  first_url_line="$(grep -n '^  url ' "$file" | head -1 | cut -d: -f1)"
  first_block_line="$(grep -n '^  on_' "$file" | head -1 | cut -d: -f1 || true)"
  if [ -z "$first_url_line" ]; then
    echo "error: generated formula has no top-level url" >&2
    sed -n '1,160p' "$file" >&2
    exit 1
  fi
  if [ -n "$first_block_line" ] && [ "$first_url_line" -gt "$first_block_line" ]; then
    echo "error: generated formula url is only inside a platform block" >&2
    sed -n '1,160p' "$file" >&2
    exit 1
  fi
}

tag="v9.9.9"

write_sha "$tag" aarch64-apple-darwin 1111111111111111111111111111111111111111111111111111111111111111
bash "${ROOT}/scripts/gen-formula.sh" "$tag" "$TMP" "${TMP}/phux-arm.rb" >/dev/null
assert_top_level_url "${TMP}/phux-arm.rb"
assert_contains "${TMP}/phux-arm.rb" 'url "https://github.com/phall1/phux/releases/download/v9.9.9/phux-v9.9.9-aarch64-apple-darwin.tar.gz"'
assert_contains "${TMP}/phux-arm.rb" 'sha256 "1111111111111111111111111111111111111111111111111111111111111111"'

write_sha "$tag" x86_64-apple-darwin 2222222222222222222222222222222222222222222222222222222222222222
write_sha "$tag" x86_64-unknown-linux-gnu 3333333333333333333333333333333333333333333333333333333333333333
write_sha "$tag" aarch64-unknown-linux-gnu 4444444444444444444444444444444444444444444444444444444444444444
bash "${ROOT}/scripts/gen-formula.sh" "$tag" "$TMP" "${TMP}/phux-full.rb" >/dev/null
assert_top_level_url "${TMP}/phux-full.rb"
assert_contains "${TMP}/phux-full.rb" 'on_macos do'
assert_contains "${TMP}/phux-full.rb" 'on_linux do'
assert_contains "${TMP}/phux-full.rb" 'phux-v9.9.9-x86_64-apple-darwin.tar.gz'
assert_contains "${TMP}/phux-full.rb" 'phux-v9.9.9-x86_64-unknown-linux-gnu.tar.gz'
assert_contains "${TMP}/phux-full.rb" 'phux-v9.9.9-aarch64-unknown-linux-gnu.tar.gz'

if command -v ruby >/dev/null 2>&1; then
  ruby -c "${TMP}/phux-arm.rb" >/dev/null
  ruby -c "${TMP}/phux-full.rb" >/dev/null
fi

echo "formula generation ok"
