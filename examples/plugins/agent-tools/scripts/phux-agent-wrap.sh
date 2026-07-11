#!/bin/sh
#
# phux-agent-wrap.sh (phux-r82.11) — make a terminal agent self-identify.
#
# Wrap a real agent command so that, the moment it launches inside a
# phux pane, the pane gets a first-class `phux.agent/v1` L3 record
# (ADR-0040) instead of relying on the OSC-title substring heuristic.
# The record carries the agent's name and kind, so the TUI sidebar and
# any fleet view show a declared identity that a plain `claude`/`codex`
# session never announces.
#
# Usage:
#   phux-agent-wrap.sh [--name NAME] [--kind KIND] [--state STATE]
#                      [--target TARGET] -- command [arg...]
#
# Everything after `--` is the real agent argv, run in the foreground so
# its TTY / job-control semantics are untouched. On start the wrapper
# writes the record; on exit (normal, signal, or agent failure) it clears
# it via a trap, so an un-launched pane never shows a stale agent.
#
# Design constraints (see phux CLAUDE.md / AGENTS.md):
#   - POSIX sh, no bashisms, no new dependencies.
#   - No shell injection: every value is passed as its own quoted argv
#     element to `phux`; nothing is ever routed through `eval` or `sh -c`.
#   - Best-effort identity: if `phux` is missing or no server is up, the
#     record write fails silently and the agent still launches. Losing the
#     sidebar label must never stop the agent from running.
#
# We deliberately do NOT `exec` the agent: a trap on EXIT cannot fire
# after `exec` replaces this process, and clearing the record on exit is
# the whole point of the trap. Running the agent as a foreground child and
# forwarding its exit status is the only way to guarantee cleanup.
#
# Overrides (env):
#   PHUX_AGENT_PHUX_BIN / PHUX_BIN  path to the `phux` binary (default `phux`)
#   PHUX_AGENT_NAME                 default --name
#   PHUX_AGENT_KIND                 default --kind
#   PHUX_AGENT_STATE                default --state (see note below)
#   PHUX_AGENT_TARGET               default --target (pane selector)
#
# State note: the wrapper only observes the agent's launch/exit boundary,
# so it does NOT continuously feed a working/blocked lifecycle state. It
# sets name+kind (the high-value, always-honest part) and leaves state
# unset (== unknown) unless you pass --state / PHUX_AGENT_STATE. A live
# state feed would need a separate signal source updating the same record
# — see the README for what that would take.

set -eu

phux_bin=${PHUX_AGENT_PHUX_BIN:-${PHUX_BIN:-phux}}
agent_name=${PHUX_AGENT_NAME:-}
agent_kind=${PHUX_AGENT_KIND:-}
agent_state=${PHUX_AGENT_STATE:-}
agent_target=${PHUX_AGENT_TARGET:-}

usage() {
  printf 'usage: %s [--name NAME] [--kind KIND] [--state STATE] [--target TARGET] -- command [arg...]\n' "$0" >&2
}

need_value() {
  # $1 flag name, $2 remaining arg count
  if [ "$2" -lt 2 ]; then
    printf '%s: %s requires a value\n' "$0" "$1" >&2
    exit 2
  fi
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    --name) need_value "$1" "$#"; agent_name=$2; shift 2 ;;
    --name=*) agent_name=${1#--name=}; shift ;;
    --kind) need_value "$1" "$#"; agent_kind=$2; shift 2 ;;
    --kind=*) agent_kind=${1#--kind=}; shift ;;
    --state) need_value "$1" "$#"; agent_state=$2; shift 2 ;;
    --state=*) agent_state=${1#--state=}; shift ;;
    --target) need_value "$1" "$#"; agent_target=$2; shift 2 ;;
    --target=*) agent_target=${1#--target=}; shift ;;
    --) shift; break ;;
    -*) usage; exit 2 ;;
    *) break ;;
  esac
done

if [ "$#" -eq 0 ]; then
  usage
  exit 2
fi

# Fall back to the launched command's basename as the agent name, so the
# wrapper is still useful when invoked with a bare `-- command`.
if [ -z "$agent_name" ]; then
  agent_name=$(basename -- "$1")
fi

# Run `phux` with the given argv, best-effort: never let a missing binary
# or absent server abort the agent launch or the cleanup. Positional
# params here are local to the function, so the caller's agent argv ($@)
# is preserved across these calls.
run_phux() {
  "$phux_bin" "$@" >/dev/null 2>&1 || true
}

set_record() {
  set -- agent set
  if [ -n "$agent_target" ]; then
    set -- "$@" "$agent_target"
  fi
  set -- "$@" --name "$agent_name"
  if [ -n "$agent_kind" ]; then
    set -- "$@" --kind "$agent_kind"
  fi
  if [ -n "$agent_state" ]; then
    set -- "$@" --state "$agent_state"
  fi
  run_phux "$@"
}

# Invoked indirectly through the EXIT trap below.
# shellcheck disable=SC2329
clear_record() {
  set -- agent clear
  if [ -n "$agent_target" ]; then
    set -- "$@" "$agent_target"
  fi
  run_phux "$@"
}

# Clear on any exit path. Signal traps re-raise through `exit`, which then
# runs the EXIT trap exactly once.
trap 'clear_record' EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
trap 'exit 129' HUP

set_record

status=0
"$@" || status=$?
exit "$status"
