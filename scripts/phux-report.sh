#!/usr/bin/env bash
# phux-report — file a phux bug/feedback GitHub issue with auto-attached
# context (version, session topology, the visible screen, and the phux log
# tail), with zero effort from inside a phux session.
#
# Usage:
#   phux-report [--dry-run] [note words...]
#   phux-report                 # prompts for a one-line note
#   PHUX_REPORT_DRYRUN=1 phux-report ...   # print, don't file
#
# Env:
#   PHUX_REPORT_REPO    target repo (default: phall1/phux)
#   PHUX_REPORT_LABELS  comma labels (default: dogfood)
#   PHUX_REPORT_KIND    extra label for bug|enhancement|question (optional)
#
# Best-effort: every capture is isolated; a missing tool or dead server
# degrades that section to a note, never aborts the report.
set -u

repo="${PHUX_REPORT_REPO:-phall1/phux}"
labels="${PHUX_REPORT_LABELS:-dogfood}"
[ -n "${PHUX_REPORT_KIND:-}" ] && labels="${labels},${PHUX_REPORT_KIND}"

dry=0
if [ "${1:-}" = "--dry-run" ]; then dry=1; shift; fi
[ "${PHUX_REPORT_DRYRUN:-}" = "1" ] && dry=1

note="$*"
if [ -z "$note" ] && [ -t 0 ]; then
  printf 'phux report — one line, what happened?\n> ' >&2
  IFS= read -r note || true
fi
[ -z "$note" ] && note="(no description)"

tmp="$(mktemp "${TMPDIR:-/tmp}/phux-report.XXXXXX")" || exit 1
trap 'rm -f "$tmp"' EXIT

cap() { # cap "Title" -- command...
  title="$1"; shift; [ "$1" = "--" ] && shift
  {
    printf '\n<details><summary>%s</summary>\n\n```\n' "$title"
    if out="$("$@" 2>&1)"; then printf '%s\n' "$out"; else
      printf '(capture unavailable)\n'; fi
    printf '```\n</details>\n'
  } >> "$tmp"
}

{
  printf '**Reported from phux** — %s\n\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  printf '%s\n' "$note"
} > "$tmp"

env_probe() {
  phux --version 2>/dev/null || echo "phux: (not on PATH)"
  printf 'repo HEAD: '; git -C "${PHUX_REPO_DIR:-$PWD}" rev-parse --short HEAD 2>/dev/null || echo "(not a repo)"
  uname -a
  printf 'TERM=%s  focused-pane=%s\n' "${TERM:-?}" "${PHUX_TERMINAL_ID:-?}"
}
log_tail() {
  dir="${XDG_STATE_HOME:-$HOME/.local/state}/phux"
  f="$(ls -t "$dir"/*.log 2>/dev/null | head -1)"
  [ -n "$f" ] && { echo "# $f"; tail -n 200 "$f"; } || echo "(no phux log under $dir)"
}

cap "Environment" -- env_probe
cap "Session topology (phux ls)" -- sh -c 'phux ls 2>/dev/null || phux ls --json 2>/dev/null || true'
cap "Visible screen (phux snapshot --rendered)" -- sh -c 'phux snapshot --rendered 2>/dev/null || phux snapshot 2>/dev/null || true'
cap "phux log (tail 200)" -- log_tail

title="$(printf '%s' "$note" | tr '\n' ' ' | cut -c1-100)"

if [ "$dry" = "1" ]; then
  printf '=== DRY RUN — would file to %s  labels=[%s] ===\n\n' "$repo" "$labels" >&2
  printf 'Title: %s\n\n' "$title" >&2
  cat "$tmp" >&2
  exit 0
fi

if ! command -v gh >/dev/null 2>&1; then
  echo "gh CLI not found; report saved to: $tmp" >&2; trap - EXIT; exit 1
fi
url="$(gh issue create --repo "$repo" --title "$title" --label "$labels" --body-file "$tmp" 2>&1)" \
  && printf '\n  filed: %s\n' "$url" \
  || { printf '\n  gh issue create failed:\n%s\n  body saved: %s\n' "$url" "$tmp" >&2; trap - EXIT; exit 1; }
