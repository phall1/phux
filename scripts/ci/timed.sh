# shellcheck shell=bash
# Phase timer for CI lanes. Source this, then wrap each phase:
#
#   source scripts/ci/timed.sh
#   timed fmt    cargo fmt --check
#   timed doc    env RUSTDOCFLAGS='-D warnings' cargo doc --no-deps
#
# Each phase runs inside a GitHub Actions ::group:: and appends one line to
# $PHUX_METRICS_DIR/phases.ndjson: {"name","seconds","exit"}. A failing
# phase still gets recorded (the summary step runs `if: always()`), then the
# failure propagates so the lane aborts exactly as it did before timing.
#
# Commands with environment prefixes go through `env` (see the doc example):
# `timed` execs its arguments directly, it is not a shell re-parse.

PHUX_METRICS_DIR="${PHUX_METRICS_DIR:-target/ci-metrics}"
mkdir -p "$PHUX_METRICS_DIR"

timed() {
    local name="$1"
    shift
    local start end rc=0
    echo "::group::${name}"
    start=$(date +%s)
    "$@" || rc=$?
    end=$(date +%s)
    echo "::endgroup::"
    printf '{"name":"%s","seconds":%d,"exit":%d}\n' \
        "$name" "$((end - start))" "$rc" >>"$PHUX_METRICS_DIR/phases.ndjson"
    if [ "$rc" -ne 0 ]; then
        echo "::error::phase '${name}' failed after $((end - start))s (exit ${rc})"
    fi
    return "$rc"
}
