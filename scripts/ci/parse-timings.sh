#!/usr/bin/env bash
# Distill cargo's --timings HTML report into one NDJSON record (kind:
# "build-timings") plus a step-summary table. The HTML embeds the raw data as
# `const UNIT_DATA = [...]` (pretty-printed, closing `];` on its own line)
# and the total wall as `DURATION = N;` — this extracts both, so nothing here
# recompiles anything.
#
# Unit semantics (cargo 1.90):
#   target " build script (run)"  build.rs execution — libghostty-vt's zig
#                                 blob is the dominant one in this workspace
#   target " bin \"x\"" / tests   terminal units: codegen + LINK wall lives here
#   rmeta_time                    frontend time; duration - rmeta_time is the
#                                 codegen tail for pipelined lib units
#
# Usage: parse-timings.sh <cargo-timing.html> <profile-label> [<out.ndjson>]
#   Appends the record to <out.ndjson> (default $PHUX_METRICS_DIR/records.ndjson)
#   and writes markdown to $GITHUB_STEP_SUMMARY when set.
set -euo pipefail

html="${1:?usage: parse-timings.sh <cargo-timing.html> <profile> [out]}"
profile="${2:?profile label required (dev|release)}"
out="${3:-${PHUX_METRICS_DIR:-target/ci-metrics}/records.ndjson}"
mkdir -p "$(dirname "$out")"

units=$(sed -n '/^const UNIT_DATA = \[$/,/^\];$/p' "$html" | sed '1s/.*/[/; $s/;$//')
total=$(sed -n 's/^DURATION = \(.*\);$/\1/p;/^DURATION = /q' "$html")
if [ -z "$units" ] || ! jq -e 'type == "array"' <<<"$units" >/dev/null 2>&1; then
    echo "::error::could not extract UNIT_DATA from ${html} — cargo's report format changed; update scripts/ci/parse-timings.sh" >&2
    exit 1
fi

record=$(jq -c \
    --arg profile "$profile" \
    --argjson total "${total:-0}" \
    --arg workflow "${GITHUB_WORKFLOW:-local}" \
    --arg run_id "${GITHUB_RUN_ID:-0}" \
    --arg attempt "${GITHUB_RUN_ATTEMPT:-1}" \
    --arg sha "${GITHUB_SHA:-}" \
    --arg branch "${GITHUB_REF_NAME:-}" \
    --arg event "${GITHUB_EVENT_NAME:-}" \
    '{schema: 1, kind: "build-timings",
      workflow: $workflow, run_id: ($run_id | tonumber),
      run_attempt: ($attempt | tonumber),
      sha: $sha, branch: $branch, event: $event,
      profile: $profile,
      total_wall_s: $total,
      units: length,
      cpu_seconds: ([.[].duration] | add | . * 100 | round / 100),
      build_script_runs: ([.[] | select(.mode == "run-custom-build")
          | {name, seconds: .duration}] | sort_by(-.seconds) | .[:10]),
      terminal_units: ([.[] | select(.target | test("bin |test |example "))
          | {name: (.name + .target), seconds: .duration}]
          | sort_by(-.seconds) | .[:10]),
      slowest_units: ([.[] | {name: (.name + .target), seconds: .duration,
          frontend_s: .rmeta_time}] | sort_by(-.seconds) | .[:20])}' \
    <<<"$units")
printf '%s\n' "$record" >>"$out"

{
    echo "### cold build timings — ${profile}"
    echo
    jq -r '"- total wall: **\(.total_wall_s)s** across \(.units) units (\(.cpu_seconds)s of unit time)"' <<<"$record"
    echo
    echo "| slowest units | wall | frontend |"
    echo "|---|---:|---:|"
    jq -r '.slowest_units[] | "| `\(.name)` | \(.seconds)s | \(.frontend_s // "-")\(if .frontend_s then "s" else "" end) |"' <<<"$record"
    echo
    echo "| build-script executions (zig lives here) | wall |"
    echo "|---|---:|"
    jq -r '.build_script_runs[] | "| `\(.name)` | \(.seconds)s |"' <<<"$record"
    echo
    echo "| terminal units (codegen + link) | wall |"
    echo "|---|---:|"
    jq -r '.terminal_units[] | "| `\(.name)` | \(.seconds)s |"' <<<"$record"
    echo
} >>"${GITHUB_STEP_SUMMARY:-/dev/stdout}"
