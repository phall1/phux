#!/usr/bin/env bash
# check-ratatui-boundary.sh
#
# CI guard for epic phux-5ke. ratatui is the chrome-layer toolkit in
# phux-client; the architectural invariant is that ratatui imports live
# *only* under `crates/phux-client/src/render/`. Pane-interior code paths
# (attach loop, render mirror, predict, layout math) stay ratatui-free so
# libghostty owns the hot path unmodified.
#
# This script greps for `use ratatui` / `ratatui::` in any phux-client
# source file outside `render/` and fails if it finds one.

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SRC="$ROOT/crates/phux-client/src"

if [[ ! -d "$SRC" ]]; then
    echo "error: $SRC not found (run from repo root)" >&2
    exit 2
fi

# Match `use ratatui` or `ratatui::` not preceded by an identifier char,
# so we catch `use ratatui::Frame;`, `ratatui::widgets::Block`, etc.,
# without matching hypothetical identifiers like `my_ratatui::`.
PATTERN='(^|[^a-zA-Z_])use ratatui|ratatui::'

# Collect violations: any matching line whose file path is NOT under render/.
violations="$(grep -rEn "$PATTERN" "$SRC" 2>/dev/null \
    | grep -v "^$SRC/render/" \
    || true)"

if [[ -n "$violations" ]]; then
    echo "error: ratatui imports found outside crates/phux-client/src/render/:" >&2
    echo "$violations" >&2
    echo "" >&2
    echo "ratatui is the chrome-layer toolkit (epic phux-5ke). Pane-interior" >&2
    echo "code paths must stay ratatui-free; libghostty owns those cells." >&2
    exit 1
fi

echo "ratatui boundary OK"
