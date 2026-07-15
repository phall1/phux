#!/usr/bin/env bash
# examples-smoke.sh
#
# Smoke test for examples/agents/ (bead phux-wiv). Those scripts drive the
# real `phux` binary end to end but carry no CI/lint gate, so they rot
# silently against CLI changes. This harness runs each one against a
# throwaway server and fails if any exits non-zero, turning "the examples
# still work" into a checkable invariant.
#
# Two things this harness controls that an ad-hoc run does not:
#
#   * Seed shell. The server spawns the pane's shell from $SHELL
#     (terminal_actor::default_shell_command). A developer's login shell
#     emits p10k / direnv / instant-prompt banners that pollute snapshots
#     and make `wait --until` matches flaky. We pin SHELL=/bin/sh: POSIX,
#     no rc files, no banner noise. The examples already wrap their
#     interactive programs in `sh -c` for this exact portability reason.
#
#   * Binary. We build `phux` once up front and hand it to every example
#     via $PHUX, so the per-example "build if missing" path in lib.sh /
#     agent_loop.py never fires mid-run (which would interleave a slow
#     build with example output and muddy a failure).
#
# This is intentionally NOT wired into `just ci`. Each example spawns a
# real PTY-backed server; run under the full parallel pool those spawns
# starve and socket binds trip — the same reason the e2e tests are
# `#[ignore]`d (see the `e2e` recipe). Run it on demand, or as its own CI
# step, never inside the parallel test pool.
#
# Usage:  bash scripts/examples-smoke.sh
#         PHUX=/path/to/phux bash scripts/examples-smoke.sh   # skip the build

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
EXAMPLES="$ROOT/examples/agents"

if [[ ! -d "$EXAMPLES" ]]; then
    echo "error: $EXAMPLES not found (run from repo root)" >&2
    exit 2
fi

# --- Pinned, banner-free seed shell -----------------------------------------
# Exported so the server the examples spawn inherits it. /bin/sh reads no
# interactive rc files, so snapshots carry only the example's own output.
export SHELL=/bin/sh

# --- Build the binary once --------------------------------------------------
# Honour a caller-supplied $PHUX; otherwise build a debug binary and pin it
# so no example triggers its own build partway through the run.
if [[ -z "${PHUX:-}" ]]; then
    echo "examples-smoke: building phux (debug)..." >&2
    cargo build -p phux >&2
    PHUX="$ROOT/target/debug/phux"
fi
if [[ ! -x "$PHUX" ]]; then
    echo "error: phux binary not found or not executable: $PHUX" >&2
    exit 2
fi
export PHUX

# --- Warm up the binary before the timed examples ---------------------------
# Each example's lib.sh polls for the server socket with a fixed ~5s deadline
# (which this harness must not edit — examples/ is the surface under test). On
# a loaded host the FIRST server spawn pays a one-time cold cost (page-in,
# dynamic-link resolution, libghostty init) that can blow that deadline and
# fail an otherwise-healthy example. Pay that cost here, outside any example's
# window: page in the binary, then run one full throwaway server cycle with a
# generous wait. Best-effort — a warmup hiccup must not fail the run, so this
# never trips `set -e`.
"$PHUX" --help >/dev/null 2>&1 || true
_warm_dir="$(mktemp -d "${TMPDIR:-/tmp}/phux-smoke-warm.XXXXXX")"
_warm_sock="$_warm_dir/warm.sock"
"$PHUX" server --session warm --socket "$_warm_sock" \
    >"$_warm_dir/server.log" 2>&1 &
_warm_pid=$!
for _ in $(seq 1 800); do
    [[ -S "$_warm_sock" ]] && break
    sleep 0.025
done
kill "$_warm_pid" 2>/dev/null || true
wait "$_warm_pid" 2>/dev/null || true
rm -rf "$_warm_dir" 2>/dev/null || true

# --- Run every example ------------------------------------------------------
# Each script and agent_loop.py stands up its own throwaway server on a
# private socket (see examples/agents/lib.sh), so they are independent and
# leave no state behind. A non-zero exit from any one fails the harness.
failed=0
run_example() {
    local label="$1"
    shift
    printf '\n########## %s ##########\n' "$label"
    if "$@"; then
        printf '########## %s: OK ##########\n' "$label"
    else
        local rc=$?
        printf '########## %s: FAILED (exit %d) ##########\n' "$label" "$rc" >&2
        failed=1
    fi
}

for script in "$EXAMPLES"/*.sh; do
    [[ "$(basename "$script")" == "lib.sh" ]] && continue
    run_example "$(basename "$script")" bash "$script"
done

# The fleet example needs configured agent integrations in normal use. Its
# fake-phux test proves deterministic argv/control flow; the live test uses the
# already-built binary with an isolated real server and ordinary shell panes.
# Neither requires an external agent binary.
for test_script in "$EXAMPLES"/tests/*.sh; do
    run_example "tests/$(basename "$test_script")" bash "$test_script"
done

if command -v python3 >/dev/null 2>&1; then
    run_example "agent_loop.py" python3 "$EXAMPLES/agent_loop.py"
else
    echo "examples-smoke: python3 not found; skipping agent_loop.py" >&2
fi

if [[ "$failed" -ne 0 ]]; then
    echo "" >&2
    echo "examples-smoke: one or more examples failed (see above)." >&2
    exit 1
fi

echo ""
echo "examples-smoke: all examples ran clean."
