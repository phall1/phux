#!/usr/bin/env bash
# agent-asked-capture.sh -- clean-room evidence harness for agent ASKED states.
#
# Launches installed agent CLIs inside isolated phux panes and records only
# phux-owned evidence: watch events, snapshots, version metadata, and command
# availability. Generated evidence is intentionally local scratch data.
#
# Usage:
#   scripts/agent-asked-capture.sh
#   scripts/agent-asked-capture.sh --prompt "Ask me before continuing"
#   PHUX=/path/to/phux scripts/agent-asked-capture.sh --out /tmp/corpus
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
STAMP="$(date -u +%Y%m%dT%H%M%SZ)"
OUT_DIR="${PHUX_ASK_CAPTURE_OUT:-$ROOT/.omo/evidence/agent-asked-capture/$STAMP}"
AGENTS_TEXT="${PHUX_ASK_CAPTURE_AGENTS:-claude codex pi}"
DWELL_SECS="${PHUX_ASK_CAPTURE_DWELL:-8}"
PROMPT="${PHUX_ASK_CAPTURE_PROMPT:-}"
PHUX="${PHUX:-}"

usage() {
  cat <<'EOF'
Usage: scripts/agent-asked-capture.sh [options]

Options:
  --agents "claude codex pi"  Space-separated candidate agent commands.
  --dwell SECS                Seconds to observe each launched agent.
  --out DIR                   Evidence output directory.
  --prompt TEXT               Optional text to send to each agent, followed by Enter.
  --phux PATH                 phux binary to use instead of PATH/local debug build.
  -h, --help                  Show this help.

Exit status:
  0  At least two agent CLIs were installed and exercised.
  1  Harness ran, but fewer than two agents were exercised.
  2  Harness setup failed.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --agents)
      AGENTS_TEXT="${2:-}"
      shift 2
      ;;
    --dwell)
      DWELL_SECS="${2:-}"
      shift 2
      ;;
    --out)
      OUT_DIR="${2:-}"
      shift 2
      ;;
    --prompt)
      PROMPT="${2:-}"
      shift 2
      ;;
    --phux)
      PHUX="${2:-}"
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "error: unknown option: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

if ! [[ "$DWELL_SECS" =~ ^[0-9]+$ ]] || [[ "$DWELL_SECS" -lt 1 ]]; then
  echo "error: --dwell must be a positive integer" >&2
  exit 2
fi

if ! command -v python3 >/dev/null 2>&1; then
  echo "error: python3 is required to write metadata JSON" >&2
  exit 2
fi

if [[ -z "$PHUX" ]]; then
  if command -v phux >/dev/null 2>&1; then
    PHUX="$(command -v phux)"
  elif [[ -x "$ROOT/target/debug/phux" ]]; then
    PHUX="$ROOT/target/debug/phux"
  else
    echo "agent-asked-capture: building phux (debug)..." >&2
    cargo build -p phux >&2
    PHUX="$ROOT/target/debug/phux"
  fi
fi
if [[ ! -x "$PHUX" ]]; then
  echo "error: phux binary not found or not executable: $PHUX" >&2
  exit 2
fi

IFS=' ' read -r -a AGENTS <<< "$AGENTS_TEXT"
if [[ "${#AGENTS[@]}" -eq 0 ]]; then
  echo "error: no agents requested" >&2
  exit 2
fi

mkdir -p "$OUT_DIR"
TMP_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/phux-asked-capture.XXXXXX")"
SOCK="$TMP_ROOT/phux.sock"
CAPTURE_CWD="$TMP_ROOT/workspace"
SERVER_LOG="$OUT_DIR/server.log"
SERVER_PID=""
WATCH_PIDS=()

cleanup() {
  for pid in "${WATCH_PIDS[@]}"; do
    kill "$pid" 2>/dev/null || true
    wait "$pid" 2>/dev/null || true
  done
  if [[ -n "$SERVER_PID" ]]; then
    kill "$SERVER_PID" 2>/dev/null || true
    wait "$SERVER_PID" 2>/dev/null || true
  fi
  rm -rf "$TMP_ROOT"
}
trap cleanup EXIT

wait_for_file() {
  local path="$1"
  local tries="${2:-200}"
  for _ in $(seq 1 "$tries"); do
    [[ -e "$path" ]] && return 0
    sleep 0.05
  done
  return 1
}

write_metadata() {
  local path="$1"
  local agent="$2"
  local status="$3"
  local command_path="$4"
  local session="$5"
  local notes="$6"
  AGENT_NAME="$agent" \
    AGENT_STATUS="$status" \
    AGENT_COMMAND_PATH="$command_path" \
    AGENT_SESSION="$session" \
    AGENT_NOTES="$notes" \
    AGENT_DWELL_SECS="$DWELL_SECS" \
    AGENT_PROMPT_SET="$([[ -n "$PROMPT" ]] && echo true || echo false)" \
    PHUX_BIN="$PHUX" \
    PHUX_SOCKET="$SOCK" \
    PHUX_CAPTURE_CWD="$CAPTURE_CWD" \
    python3 - "$path" <<'PY'
import json
import os
import sys
from datetime import datetime, timezone

payload = {
    "schema": "phux.agent_asked_capture.v1",
    "captured_at": datetime.now(timezone.utc).isoformat(),
    "agent": os.environ["AGENT_NAME"],
    "status": os.environ["AGENT_STATUS"],
    "command_path": os.environ["AGENT_COMMAND_PATH"] or None,
    "session": os.environ["AGENT_SESSION"] or None,
    "dwell_seconds": int(os.environ["AGENT_DWELL_SECS"]),
    "prompt_was_sent": os.environ["AGENT_PROMPT_SET"] == "true",
    "phux_binary": os.environ["PHUX_BIN"],
    "phux_socket": os.environ["PHUX_SOCKET"],
    "capture_cwd": os.environ["PHUX_CAPTURE_CWD"],
    "notes": os.environ["AGENT_NOTES"],
}
with open(sys.argv[1], "w", encoding="utf-8") as f:
    json.dump(payload, f, indent=2, sort_keys=True)
    f.write("\n")
PY
}

capture_snapshot() {
  local session="$1"
  local out_prefix="$2"
  "$PHUX" snapshot --socket "$SOCK" --json --scrollback=200 "$session" \
    >"$out_prefix.json" 2>"$out_prefix.stderr" || true
  "$PHUX" snapshot --socket "$SOCK" --scrollback=40 "$session" \
    >"$out_prefix.txt" 2>>"$out_prefix.stderr" || true
}

version_probe() {
  local bin="$1"
  local out_dir="$2"
  if "$bin" --version >"$out_dir/version.stdout" 2>"$out_dir/version.stderr"; then
    printf '0' >"$out_dir/version.status"
  else
    printf '%s' "$?" >"$out_dir/version.status"
  fi
}

cat >"$OUT_DIR/CLEAN_ROOM_NOTES.md" <<'EOF'
# Clean-room notes

This directory was produced by phux's own `scripts/agent-asked-capture.sh`.
It records observed behavior from locally installed agent CLIs using only
phux-owned surfaces:

- `phux watch --json` for title, bell, dirty, idle, and asked events.
- `phux snapshot` for visible grid rows and scrollback.
- direct `<agent> --version` stdout/stderr for availability metadata.

No herdr source files, manifests, regular expressions, hook scripts, or
detector assets are read or copied by this harness. The output is empirical
evidence for future phux-owned detector design, not an authoritative passive
detector.
EOF

mkdir -p "$CAPTURE_CWD"
echo "agent-asked-capture: output $OUT_DIR" >&2
"$PHUX" server --socket "$SOCK" --session asked-seed >"$SERVER_LOG" 2>&1 &
SERVER_PID=$!
if ! wait_for_file "$SOCK" 400; then
  echo "error: server did not create socket; see $SERVER_LOG" >&2
  exit 2
fi

exercised=0
missing=0
printf 'agent\tstatus\tsession\tpath\n' >"$OUT_DIR/summary.tsv"

for agent in "${AGENTS[@]}"; do
  [[ -n "$agent" ]] || continue
  safe_agent="${agent//[^A-Za-z0-9_.-]/_}"
  agent_dir="$OUT_DIR/$safe_agent"
  mkdir -p "$agent_dir"

  bin="$(command -v "$agent" 2>/dev/null || true)"
  if [[ -z "$bin" ]]; then
    missing=$((missing + 1))
    write_metadata "$agent_dir/metadata.json" "$agent" "missing" "" "" "command not found on PATH"
    printf '%s\tmissing\t\t\n' "$agent" >>"$OUT_DIR/summary.tsv"
    continue
  fi

  version_probe "$bin" "$agent_dir"

  session="asked-${safe_agent}-$$"
  ready="$TMP_ROOT/$safe_agent.ready"
  go="$TMP_ROOT/$safe_agent.go"
  rm -f "$ready" "$go"

  if ! "$PHUX" new --json --socket "$SOCK" --cwd "$CAPTURE_CWD" -s "$session" -- \
    /bin/sh -c 'printf ready > "$1"; while [ ! -f "$2" ]; do sleep 0.05; done; shift 2; exec "$@"' \
    sh "$ready" "$go" "$bin" >"$agent_dir/new.json" 2>"$agent_dir/new.stderr"; then
    write_metadata "$agent_dir/metadata.json" "$agent" "create_failed" "$bin" "$session" "phux new failed"
    printf '%s\tcreate_failed\t%s\t%s\n' "$agent" "$session" "$bin" >>"$OUT_DIR/summary.tsv"
    continue
  fi
  if ! wait_for_file "$ready" 200; then
    write_metadata "$agent_dir/metadata.json" "$agent" "gate_timeout" "$bin" "$session" "gate wrapper did not become ready"
    printf '%s\tgate_timeout\t%s\t%s\n' "$agent" "$session" "$bin" >>"$OUT_DIR/summary.tsv"
    "$PHUX" kill --socket "$SOCK" "$session" >/dev/null 2>&1 || true
    continue
  fi

  "$PHUX" watch --json --socket "$SOCK" "$session" >"$agent_dir/watch.jsonl" 2>"$agent_dir/watch.stderr" &
  watch_pid=$!
  WATCH_PIDS+=("$watch_pid")
  sleep 0.2
  touch "$go"

  capture_snapshot "$session" "$agent_dir/snapshot-start"
  if [[ -n "$PROMPT" ]]; then
    "$PHUX" send-keys --socket "$SOCK" "$session" "$PROMPT" Enter \
      >"$agent_dir/send-keys.stdout" 2>"$agent_dir/send-keys.stderr" || true
  fi
  sleep "$DWELL_SECS"
  capture_snapshot "$session" "$agent_dir/snapshot-end"

  kill "$watch_pid" 2>/dev/null || true
  wait "$watch_pid" 2>/dev/null || true
  "$PHUX" kill --socket "$SOCK" "$session" >"$agent_dir/kill.stdout" 2>"$agent_dir/kill.stderr" || true

  write_metadata "$agent_dir/metadata.json" "$agent" "exercised" "$bin" "$session" "captured watch events and snapshots"
  printf '%s\texercised\t%s\t%s\n' "$agent" "$session" "$bin" >>"$OUT_DIR/summary.tsv"
  exercised=$((exercised + 1))
done

{
  printf 'Output: %s\n' "$OUT_DIR"
  printf 'Exercised: %s\n' "$exercised"
  printf 'Missing: %s\n' "$missing"
  printf 'Requested agents: %s\n' "$AGENTS_TEXT"
  if [[ -n "$PROMPT" ]]; then
    printf 'Prompt: provided\n'
  else
    printf 'Prompt: not provided\n'
  fi
} >"$OUT_DIR/INCOMPLETE_COVERAGE.txt"

if [[ "$exercised" -lt 2 ]]; then
  echo "agent-asked-capture: incomplete coverage: exercised $exercised of ${#AGENTS[@]} requested agents" >&2
  echo "agent-asked-capture: see $OUT_DIR/INCOMPLETE_COVERAGE.txt" >&2
  exit 1
fi

rm -f "$OUT_DIR/INCOMPLETE_COVERAGE.txt"
echo "agent-asked-capture: exercised $exercised agents; evidence in $OUT_DIR"
