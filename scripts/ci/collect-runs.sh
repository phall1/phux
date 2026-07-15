#!/usr/bin/env bash
# Record completed workflow runs into the ci-metrics data store (an NDJSON
# tree, normally a checkout of the `ci-metrics` branch). For each run not yet
# recorded this appends:
#   * one kind:"run" record — per-job and per-step wall/queue times scraped
#     from the Actions API (nothing in the lanes needs instrumenting for it)
#   * every record from the run's uploaded `ci-metrics-*` artifacts (the
#     kind:"phases"/"build-timings"/"deps"/"binary-size" records written by
#     the other scripts in scripts/ci/)
#
# IDEMPOTENT AND SELF-HEALING by design: it sweeps the last --window
# completed runs and diffs against what is already recorded, so a collector
# invocation that gets cancelled (concurrency-queue collapse) or errors is
# simply caught up by the next one. Never assume one invocation per run.
#
# Artifact ingestion is hardened — artifacts ride in from PR forks, so lines
# are only accepted when they parse as a JSON object with a string .kind, and
# per-artifact size/line caps apply. Records are data, never executed.
#
# Usage: GH_REPO=owner/repo collect-runs.sh --out <dir> [--window N]
# Needs: gh (authenticated), jq, unzip.
set -euo pipefail

out_dir=""
window=40
while [ $# -gt 0 ]; do
    case "$1" in
    --out) out_dir="$2"; shift 2 ;;
    --window) window="$2"; shift 2 ;;
    *) echo "unknown arg: $1" >&2; exit 2 ;;
    esac
done
: "${out_dir:?--out <dir> required}"
: "${GH_REPO:?GH_REPO owner/repo required}"
mkdir -p "$out_dir/runs"

# Keys (run_id:attempt) already recorded.
recorded=$(cat "$out_dir"/runs/*.ndjson 2>/dev/null \
    | jq -r 'select(.kind? == "run") | "\(.run_id):\(.run_attempt)"' | sort -u || true)

runs=$(gh api "repos/${GH_REPO}/actions/runs?status=completed&per_page=${window}" \
    --jq '[.workflow_runs[] | select(.name != "ci-metrics")]')

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
new_count=0

while IFS= read -r run; do
    id=$(jq -r '.id' <<<"$run")
    attempt=$(jq -r '.run_attempt' <<<"$run")
    if grep -qxF "${id}:${attempt}" <<<"$recorded"; then
        continue
    fi
    month=$(jq -r '.created_at[:7]' <<<"$run")
    shard="$out_dir/runs/${month}.ndjson"

    jobs=$(gh api "repos/${GH_REPO}/actions/runs/${id}/jobs?per_page=100" \
        --jq '.jobs' 2>/dev/null || echo '[]')

    jq -c --argjson jobs "$jobs" '
        def secs(a; b): if a and b then
            # Skipped jobs carry degenerate timestamps; clamp instead of
            # letting negative wall times poison the medians.
            ([(b | fromdateiso8601) - (a | fromdateiso8601), 0] | max)
            else null end;
        {schema: 1, kind: "run",
         workflow: .name, run_id: .id, run_attempt: .run_attempt,
         event: .event, branch: .head_branch, sha: .head_sha,
         actor: .actor.login, conclusion: .conclusion,
         created_at: .created_at,
         duration_s: secs(.run_started_at; .updated_at),
         jobs: [$jobs[] | {name, conclusion,
             queued_s: secs(.created_at; .started_at),
             duration_s: secs(.started_at; .completed_at),
             steps: [.steps[]? | {name, conclusion,
                 seconds: secs(.started_at; .completed_at)}]}]}' \
        <<<"$run" >>"$shard"

    # Fold in the run's own metrics artifacts (phase timers, timings, deps...).
    artifacts=$(gh api "repos/${GH_REPO}/actions/runs/${id}/artifacts" \
        --jq '[.artifacts[] | select((.name | startswith("ci-metrics")) and
              (.expired | not) and .size_in_bytes < 10485760)]' 2>/dev/null || echo '[]')
    created_at=$(jq -r '.created_at' <<<"$run")
    while IFS= read -r aid; do
        [ -n "$aid" ] || continue
        rm -rf "$tmp/a"; mkdir -p "$tmp/a"
        if gh api "repos/${GH_REPO}/actions/artifacts/${aid}/zip" >"$tmp/a.zip" 2>/dev/null \
            && unzip -oq "$tmp/a.zip" -d "$tmp/a" 2>/dev/null; then
            find "$tmp/a" -name records.ndjson -print0 | while IFS= read -r -d '' f; do
                head -200 "$f" | jq -c --argjson id "$id" --argjson attempt "$attempt" \
                    --arg created "$created_at" \
                    'select(type == "object" and (.kind? | type == "string"))
                     | .run_id = $id | .run_attempt = $attempt
                     | .created_at = $created' \
                    >>"$shard" 2>/dev/null || true
            done
        fi
    done < <(jq -r '.[].id' <<<"$artifacts")

    new_count=$((new_count + 1))
    echo "recorded: $(jq -r '"\(.name) #\(.run_number) (\(.event), \(.conclusion))"' <<<"$run")"
done < <(jq -c '.[]' <<<"$runs")

echo "collect-runs: ${new_count} new run(s) recorded into ${out_dir}/runs/"
