#!/usr/bin/env bash
# Integration script for the module-split swarm (phux-agbs, b7z6, vzn8, evny, hfd7, zoqq, c9xx)
#
# Usage: ./scripts/integrate-module-split.sh
#
# Assumes: agents may have committed, may have left uncommitted changes,
# or may have failed. This script audits, cleans, verifies, and gates.

set -euo pipefail

cd "$(dirname "$0")/.."

echo "=== Module Split Integration ==="
echo

# ---------------------------------------------------------------------------
# 1. Audit what we have
# ---------------------------------------------------------------------------
echo "[1/7] Git status audit..."
git status --short

echo
echo "Recent commits:"
git log --oneline -10

echo

# ---------------------------------------------------------------------------
# 2. Verify expected directory structures
# ---------------------------------------------------------------------------
echo "[2/7] Verifying expected directory structures..."

expectations=(
  "crates/phux/src/commands/mod.rs"
  "crates/phux-server/src/terminal_actor/mod.rs"
  "crates/phux-server/src/runtime/mod.rs"
  "crates/phux-server/src/state/mod.rs"
  "crates/phux-server/src/grid/mod.rs"
  "crates/phux-client-core/src/layout/mod.rs"
  "crates/phux-client-core/src/multi_pane/mod.rs"
)

missing=0
for f in "${expectations[@]}"; do
  if [[ -f "$f" ]]; then
    echo "  ✓ $f"
  else
    echo "  ✗ MISSING: $f"
    missing=$((missing + 1))
  fi
done

if [[ $missing -gt 0 ]]; then
  echo
  echo "ERROR: $missing expected files missing. Aborting."
  exit 1
fi

# Verify old monoliths are gone
monoliths=(
  "crates/phux/src/main.rs"
  "crates/phux-server/src/terminal_actor.rs"
  "crates/phux-server/src/runtime.rs"
  "crates/phux-server/src/state.rs"
  "crates/phux-server/src/grid.rs"
  "crates/phux-client-core/src/layout.rs"
  "crates/phux-client-core/src/multi_pane.rs"
)

# Note: main.rs should still exist but be tiny; the others should be deleted.
for f in "${monoliths[@]:1}"; do
  if [[ -f "$f" ]]; then
    echo "  ✗ STALE MONOLITH STILL EXISTS: $f"
    missing=$((missing + 1))
  fi
done

if [[ $missing -gt 0 ]]; then
  echo
  echo "ERROR: stale monoliths still present. Aborting."
  exit 1
fi

echo

# ---------------------------------------------------------------------------
# 3. Clean up any non-agent changes that leaked into the tree
# ---------------------------------------------------------------------------
echo "[3/7] Cleaning non-agent changes..."

# Reset known auto-generated files that we never want to commit
if git diff --name-only | grep -qE '^\.beads/interactions\.jsonl$'; then
  git checkout -- .beads/interactions.jsonl 2>/dev/null || true
fi
if git diff --name-only | grep -qE '^\.claude/scheduled_tasks\.lock$'; then
  git checkout -- .claude/scheduled_tasks.lock 2>/dev/null || true
fi

# If there's still unstaged stuff, report it
if [[ -n $(git status --short) ]]; then
  echo "  Uncommitted changes remain:"
  git status --short
  echo
  read -p "Stage all remaining changes and commit as 'integrate module-split swarm'? [y/N] " -n 1 -r
  echo
  if [[ $REPLY =~ ^[Yy]$ ]]; then
    git add -A
    git commit -m "refactor: integrate module-split swarm (phux-agbs b7z6 vzn8 evny hfd7 zoqq c9xx)"
  else
    echo "Aborting. Please review working tree manually."
    exit 1
  fi
else
  echo "  Working tree clean."
fi

echo

# ---------------------------------------------------------------------------
# 4. Fast-compile check
# ---------------------------------------------------------------------------
echo "[4/7] cargo check --workspace ..."
cargo check --workspace

echo

# ---------------------------------------------------------------------------
# 5. Full doc gate (catches intra-doc link errors)
# ---------------------------------------------------------------------------
echo "[5/7] cargo doc --workspace (deny warnings) ..."
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps

echo

# ---------------------------------------------------------------------------
# 6. Clippy gate
# ---------------------------------------------------------------------------
echo "[6/7] cargo clippy --workspace --all-targets ..."
cargo clippy --workspace --all-targets -- -D warnings

echo

# ---------------------------------------------------------------------------
# 7. Final report
# ---------------------------------------------------------------------------
echo "[7/7] Integration complete."
echo
git log --oneline -10
echo
echo "Ready to push. Run: git pull --rebase && git push"
