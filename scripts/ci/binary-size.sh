#!/usr/bin/env bash
# Release binary size observability: raw bytes per shipped binary plus a
# by-crate attribution from cargo-bloat. Emits one NDJSON record (kind:
# "binary-size") and a markdown block. Run after a release build so
# cargo-bloat reuses the artifacts instead of rebuilding.
#
# cargo-bloat's JSON output is best-effort: if its interface changes the
# record simply carries an empty `bloat_crates` rather than failing the lane
# (sizes come from the filesystem, not from bloat).
#
# Usage: binary-size.sh [<out.ndjson>]
set -euo pipefail

out="${1:-${PHUX_METRICS_DIR:-target/ci-metrics}/records.ndjson}"
mkdir -p "$(dirname "$out")"

bins='[]'
for bin in target/release/phux target/release/phux-mcp; do
    [ -f "$bin" ] || continue
    if size=$(stat -c %s "$bin" 2>/dev/null) || size=$(stat -f %z "$bin"); then
        bins=$(jq -c --arg name "$(basename "$bin")" --argjson bytes "$size" \
            '. + [{name: $name, bytes: $bytes}]' <<<"$bins")
    fi
done

bloat='[]'
if command -v cargo-bloat >/dev/null 2>&1; then
    raw=$(cargo bloat --release --bin phux --crates -n 15 --message-format json 2>/dev/null || true)
    if [ -n "$raw" ] && jq -e '.crates' <<<"$raw" >/dev/null 2>&1; then
        bloat=$(jq -c '[.crates[] | {name, bytes: .size}]' <<<"$raw")
    fi
fi

record=$(jq -cn \
    --argjson bins "$bins" \
    --argjson bloat "$bloat" \
    --arg workflow "${GITHUB_WORKFLOW:-local}" \
    --arg run_id "${GITHUB_RUN_ID:-0}" \
    --arg attempt "${GITHUB_RUN_ATTEMPT:-1}" \
    --arg sha "${GITHUB_SHA:-}" \
    --arg branch "${GITHUB_REF_NAME:-}" \
    --arg event "${GITHUB_EVENT_NAME:-}" \
    '{schema: 1, kind: "binary-size",
      workflow: $workflow, run_id: ($run_id | tonumber),
      run_attempt: ($attempt | tonumber),
      sha: $sha, branch: $branch, event: $event,
      bins: $bins, bloat_crates: $bloat}')
printf '%s\n' "$record" >>"$out"

{
    echo "### release binary size"
    echo
    echo "| binary | size |"
    echo "|---|---:|"
    jq -r '.bins[] | "| `\(.name)` | \(.bytes / 1024 / 1024 * 100 | round / 100) MiB |"' <<<"$record"
    if jq -e '.bloat_crates | length > 0' <<<"$record" >/dev/null; then
        echo
        echo "| crate (bloat attribution, .text of phux) | size |"
        echo "|---|---:|"
        jq -r '.bloat_crates[] | "| `\(.name)` | \(.bytes / 1024 | round) KiB |"' <<<"$record"
    fi
    echo
} >>"${GITHUB_STEP_SUMMARY:-/dev/stdout}"
