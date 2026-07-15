# phux CI dashboard

Generated 2026-07-15T03:34:58Z by the ci-metrics workflow. Do not edit —
every table is re-rendered from `runs/*.ndjson` on each update.
Machine rollup: [`site/summary.json`](site/summary.json), rendered live at
<https://phux.phall.io/ci>.

## Workflows, last 30 days

| workflow | runs | success | median | p95 | runner minutes |
|---|---:|---:|---:|---:|---:|
| ci | 3 | 0% | 3m29s | 3m29s | 21 |
| conventional-commits | 3 | 100% | 17s | 17s | 1 |
| release-please | 1 | 100% | 21s | 21s | 0 |
| stress | 1 | 0% | 1s | 1s | -0 |

## ci jobs, last 30 days

| job | runs | median queue | median wall | p95 wall |
|---|---:|---:|---:|---:|
| check | 3 | 2s | 3m17s | 3m17s |
| test | 3 | 2s | 3m17s | 3m17s |
| detect docs-only | 3 | 3s | 6s | 6s |

## Slowest ci steps (median, last 30 days)

| job | step | median | samples |
|---|---|---:|---:|
| check | rust checks (fmt + clippy + doc + deny) | 3m30s | 1 |
| test | Run Swatinem/rust-cache@v2 | 18s | 2 |
| check | Run Swatinem/rust-cache@v2 | 17s | 2 |
| check | docs-check | 9s | 1 |
| test | Run DeterminateSystems/nix-installer-action@v22 | 8s | 2 |
| check | Run DeterminateSystems/nix-installer-action@v22 | 7s | 2 |
| check | Run cachix/cachix-action@v15 | 5s | 1 |
| test | Run cachix/cachix-action@v15 | 5s | 1 |

## Cargo phases inside the lanes (median, last 30 days)

| workflow / job | phase | median | samples |
|---|---|---:|---:|

## Cache effectiveness (last 30 days)

| workflow / job | rust-cache hit rate | samples |
|---|---:|---:|

## Recent runs

| when | workflow | event | branch | result | wall | runner time |
|---|---|---|---|---|---:|---:|
| 2026-07-15 03:22 | conventional-commits | pull_request | fix/mouse-encoder-size-and-scrol | success | 19s | 17s |
| 2026-07-15 03:13 | conventional-commits | pull_request | release-please--branches--main-- | success | 17s | 12s |
| 2026-07-15 03:12 | conventional-commits | pull_request | ci/node24-actions | success | 17s | 13s |
| 2026-07-15 03:11 | release-please | push | main | success | 21s | 18s |
| 2026-07-15 03:09 | ci | pull_request | ci/node24-actions | cancelled | 3m29s | 6m40s |
| 2026-07-15 03:03 | ci | pull_request | release-please--branches--main-- | cancelled | 9m58s | 14m14s |
| 2026-07-15 03:03 | stress | pull_request | release-please--branches--main-- | skipped | 1s | -8s |
| 2026-07-15 03:03 | ci | pull_request | release-please--branches--main-- | cancelled | 28s | 32s |

---

Query the raw store directly, e.g. every recorded ci run's wall time:

```sh
git fetch origin ci-metrics && git show origin/ci-metrics:runs/2026-07.ndjson \
  | jq -r 'select(.kind == "run" and .workflow == "ci") | [.created_at, .conclusion, .duration_s] | @tsv'
```
