#!/usr/bin/env bash
# Dependency-graph observability: counts and shapes that explain build cost
# without compiling anything. Reads `cargo metadata` (locked) and emits one
# NDJSON record (kind: "deps") plus a step-summary/stdout markdown block.
#
# Signals and why they matter for CI cost:
#   locked_packages   every crate CI may compile cold
#   duplicate crates  same crate, multiple versions — each compiles separately
#   proc_macros       compile serially early and gate everything downstream
#   build_scripts     each is a compile + an execution (libghostty-vt's zig
#                     shell-out is the workspace's dominant single cost)
#
# Usage: dep-stats.sh [<out.ndjson>]   (also runnable locally: `just dep-stats`)
set -euo pipefail

out="${1:-${PHUX_METRICS_DIR:-target/ci-metrics}/records.ndjson}"
mkdir -p "$(dirname "$out")"

meta=$(cargo metadata --format-version 1 --locked 2>/dev/null || cargo metadata --format-version 1)

record=$(jq -c \
    --arg workflow "${GITHUB_WORKFLOW:-local}" \
    --arg run_id "${GITHUB_RUN_ID:-0}" \
    --arg attempt "${GITHUB_RUN_ATTEMPT:-1}" \
    --arg sha "${GITHUB_SHA:-}" \
    --arg branch "${GITHUB_REF_NAME:-}" \
    --arg event "${GITHUB_EVENT_NAME:-}" \
    '(.workspace_members) as $ws
     | (.packages | map(select(.id as $id | $ws | index($id)))) as $members
     | ($members | map(.name)) as $member_names
     | {schema: 1, kind: "deps",
        workflow: $workflow, run_id: ($run_id | tonumber),
        run_attempt: ($attempt | tonumber),
        sha: $sha, branch: $branch, event: $event,
        workspace_members: ($members | length),
        locked_packages: (.packages | length),
        direct_deps: ([$members[].dependencies[].name] | unique
                      | map(select(. as $n | $member_names | index($n) | not))
                      | length),
        proc_macros: ([.packages[] | select(any(.targets[]; .kind | index("proc-macro")))] | length),
        build_scripts: ([.packages[] | select(any(.targets[]; .kind | index("custom-build")))] | length),
        duplicates: (.packages | map(select(.name as $n | $member_names | index($n) | not))
                     | group_by(.name) | map(select(length > 1)
                     | {name: .[0].name, versions: [.[].version]}))}' \
    <<<"$meta")
printf '%s\n' "$record" >>"$out"

{
    echo "### dependency graph"
    echo
    jq -r '"- locked packages: **\(.locked_packages)** (\(.workspace_members) workspace members, \(.direct_deps) direct deps)
- proc-macro crates: \(.proc_macros); crates with build scripts: \(.build_scripts)
- duplicate versions: **\(.duplicates | length)**"' <<<"$record"
    if jq -e '.duplicates | length > 0' <<<"$record" >/dev/null; then
        echo
        echo "| duplicated crate | versions |"
        echo "|---|---|"
        jq -r '.duplicates[] | "| `\(.name)` | \(.versions | join(", ")) |"' <<<"$record"
    fi
    echo
} >>"${GITHUB_STEP_SUMMARY:-/dev/stdout}"
