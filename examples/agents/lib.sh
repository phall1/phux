# shellcheck shell=bash
#
# Shared setup for the examples/agents/ scripts.
#
# Sourced, not executed. It does three things every example needs:
#
#   1. Locate a `phux` binary (an installed one on PATH, else a freshly
#      built debug binary from this checkout).
#   2. Stand up a throwaway server on a private socket under a temp dir,
#      so an example never touches the user's real phux server.
#   3. Register cleanup so the server and temp dir go away on exit.
#
# Every example sources this, then drives `$PHUX --socket "$PHUX_SOCKET"`.
# An agent in production does NOT need any of this scaffolding: it just
# runs `phux <verb>` against the user's one-per-user server. The socket
# plumbing here exists only to keep the examples hermetic and runnable
# in CI without disturbing anything.

set -euo pipefail

# --- Locate the phux binary -------------------------------------------------
#
# Prefer an installed `phux`. Otherwise fall back to a debug build in this
# repo, building it on demand. Override with PHUX=/path/to/phux.
if [[ -n "${PHUX:-}" ]]; then
    :
elif command -v phux >/dev/null 2>&1; then
    PHUX="$(command -v phux)"
else
    repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
    candidate="$repo_root/target/debug/phux"
    if [[ ! -x "$candidate" ]]; then
        echo "examples/agents: building phux (one-time, may be slow)..." >&2
        # The dev shell provides zig for libghostty-vt's build.
        ( cd "$repo_root" && nix develop -c cargo build -p phux ) >&2
    fi
    PHUX="$candidate"
fi
export PHUX

# --- Private, throwaway server ----------------------------------------------
#
# A real agent uses the user's one-per-user server and never sets --socket.
# The examples isolate themselves on a temp socket so running one can't
# stomp on a session a human is using.
PHUX_TMPDIR="$(mktemp -d "${TMPDIR:-/tmp}/phux-example.XXXXXX")"
export PHUX_SOCKET="$PHUX_TMPDIR/phux.sock"
PHUX_SESSION="${PHUX_SESSION:-demo}"
export PHUX_SESSION

_phux_server_pid=""

# Start the throwaway server and block until its socket is bound.
phux_start_server() {
    "$PHUX" server --session "$PHUX_SESSION" --socket "$PHUX_SOCKET" \
        >"$PHUX_TMPDIR/server.log" 2>&1 &
    _phux_server_pid=$!
    # The server binds in sub-ms on a healthy host; poll briefly so the
    # example doesn't race the bind. Give up after ~5s.
    for _ in $(seq 1 200); do
        [[ -S "$PHUX_SOCKET" ]] && return 0
        sleep 0.025
    done
    echo "examples/agents: server did not bind $PHUX_SOCKET" >&2
    cat "$PHUX_TMPDIR/server.log" >&2 || true
    return 1
}

# Cleanup: kill the server, remove the temp dir. Registered as an EXIT trap.
phux_cleanup() {
    [[ -n "$_phux_server_pid" ]] && kill "$_phux_server_pid" 2>/dev/null || true
    rm -rf "$PHUX_TMPDIR" 2>/dev/null || true
}
trap phux_cleanup EXIT

# A thin wrapper so example bodies read like the commands an agent runs,
# minus the --socket every line would otherwise carry.
#
# `--socket` is a PER-SUBCOMMAND flag, so it goes after the verb, not
# before it (`phux ls --socket P`, not `phux --socket P ls`). And it must
# precede a verb's trailing positional args (`send-keys`/`run`'s keys and
# command, `wait`/`run`'s --until/--timeout), or clap swallows it. This
# wrapper inserts `--socket PATH` right after the first token (the verb)
# to satisfy both rules.
phux() {
    local verb="$1"
    shift
    "$PHUX" "$verb" --socket "$PHUX_SOCKET" "$@"
}

# Pretty section header, so script output is self-narrating when run.
section() {
    printf '\n=== %s ===\n' "$*"
}
