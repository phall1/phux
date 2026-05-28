#!/usr/bin/env bash
# tui-probe.sh — black-box test harness for the phux reference TUI.
#
# Runs `phux attach` *inside an isolated tmux pane*, then uses tmux's
# send-keys / capture-pane / display-message to drive phux and observe
# exactly what it paints — including the host cursor position, which is
# the thing the phux-gxy cursor bug corrupts. tmux here is a stand-in
# for a real interactive terminal that we can scriptably inspect.
#
# Everything is isolated: a dedicated tmux server socket (-L phux-probe)
# and a dedicated phux UDS, so this never touches your live sessions.
#
# Usage: scripts/tui-probe.sh [COLS] [ROWS]
set -uo pipefail

COLS="${1:-80}"
ROWS="${2:-24}"

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PHUX_BIN="$REPO/target/debug/phux"
TMUX="tmux -L phux-probe"
PHUX_SOCK="/tmp/phux-probe/phux.sock"
SESSION="probe"
LOG="/tmp/phux-probe.log"

note() { printf '\n=== %s ===\n' "$*"; }
cursor() { $TMUX display-message -p -t "$SESSION" '#{cursor_x},#{cursor_y}'; }
screen() { $TMUX capture-pane -p -t "$SESSION"; }

cleanup() {
  $TMUX kill-server 2>/dev/null
  pkill -f "phux server --socket $PHUX_SOCK" 2>/dev/null
  rm -f "$PHUX_SOCK"
}
trap cleanup EXIT

# --- fresh state -----------------------------------------------------
cleanup
mkdir -p "$(dirname "$PHUX_SOCK")"
rm -f "$LOG"

# --- launch phux inside an isolated tmux pane sized COLSxROWS ---------
$TMUX new-session -d -s "$SESSION" -x "$COLS" -y "$ROWS" \
  "RUST_LOG=trace,phux_client=trace '$PHUX_BIN' attach --socket '$PHUX_SOCK' '$SESSION' 2>'$LOG'"

# give the server auto-spawn + attach + first paint time to settle
sleep 2.5

note "host terminal is ${COLS}x${ROWS} (bottom-right cursor would be $((COLS-1)),$((ROWS-1)))"

note "AFTER ATTACH — cursor (x,y)"
cursor

note "AFTER ATTACH — screen"
screen

# --- type something so the focused pane has real content -------------
$TMUX send-keys -t "$SESSION" "echo hello-from-pane-one" Enter
sleep 0.8
note "AFTER TYPING in pane 1 — cursor (x,y)"
cursor
note "AFTER TYPING in pane 1 — screen"
screen

# --- split (prefix C-a then '|') -------------------------------------
$TMUX send-keys -t "$SESSION" C-a
sleep 0.3
$TMUX send-keys -t "$SESSION" "|"
sleep 1.0
note "AFTER SPLIT (C-a |) — cursor (x,y)"
cursor
note "AFTER SPLIT (C-a |) — screen"
screen

# --- detach cleanly (C-a d) ------------------------------------------
$TMUX send-keys -t "$SESSION" C-a
sleep 0.2
$TMUX send-keys -t "$SESSION" "d"
sleep 0.5

note "phux client log tail ($LOG)"
tail -40 "$LOG" 2>/dev/null

note "done"
