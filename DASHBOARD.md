# phux CI dashboard

Generated 2026-07-15T03:36:13Z by the ci-metrics workflow. Do not edit —
every table is re-rendered from `runs/*.ndjson` on each update.
Machine rollup: [`site/summary.json`](site/summary.json), rendered live at
<https://phux.phall.io/ci>.

## Workflows, last 30 days

| workflow | runs | success | median | p95 | runner minutes |
|---|---:|---:|---:|---:|---:|
| ci | 5 | 80% | 14m05s | 16m03s | 77 |
| conventional-commits | 5 | 100% | 17s | 19s | 1 |
| release-please | 2 | 100% | 17s | 17s | 1 |

## ci jobs, last 30 days

| job | runs | median queue | median wall | p95 wall |
|---|---:|---:|---:|---:|
| test | 5 | 2s | 13m32s | 15m54s |
| check | 5 | 2s | 2m37s | 3m17s |
| detect docs-only | 5 | 2s | 6s | 6s |

## Slowest ci steps (median, last 30 days)

| job | step | median | samples |
|---|---|---:|---:|
| test | tests (unit + e2e) | 12m40s | 4 |
| check | rust checks (fmt + clippy + doc + deny) | 1m42s | 4 |
| check | Run Swatinem/rust-cache@v2 | 22s | 5 |
| test | Run Swatinem/rust-cache@v2 | 18s | 5 |
| check | docs-check | 9s | 4 |
| check | Run DeterminateSystems/nix-installer-action@v22 | 8s | 5 |
| test | Run DeterminateSystems/nix-installer-action@v22 | 8s | 5 |

## Cargo phases inside the lanes (median, last 30 days)

| workflow / job | phase | median | samples |
|---|---|---:|---:|

## Cache effectiveness (last 30 days)

| workflow / job | rust-cache hit rate | samples |
|---|---:|---:|

## Recent runs

| when | workflow | event | branch | result | wall | runner time |
|---|---|---|---|---|---:|---:|
| 2026-07-15 03:30 | conventional-commits | pull_request | feat/ci-observability | success | 19s | 16s |
| 2026-07-15 03:29 | conventional-commits | pull_request | ci/draft-release-prs | success | 16s | 12s |
| 2026-07-15 03:26 | release-please | push | main | success | 17s | 14s |
| 2026-07-15 03:22 | conventional-commits | pull_request | fix/mouse-encoder-size-and-scrol | success | 19s | 17s |
| 2026-07-15 03:22 | ci | pull_request | fix/mouse-encoder-size-and-scrol | success | 12m25s | 14m53s |
| 2026-07-15 03:13 | conventional-commits | pull_request | release-please--branches--main-- | success | 17s | 12s |
| 2026-07-15 03:13 | ci | pull_request | release-please--branches--main-- | success | 16m29s | 20m14s |
| 2026-07-15 03:12 | conventional-commits | pull_request | ci/node24-actions | success | 17s | 13s |
| 2026-07-15 03:12 | ci | pull_request | ci/node24-actions | success | 14m05s | 16m15s |
| 2026-07-15 03:11 | release-please | push | main | success | 21s | 18s |
| 2026-07-15 03:11 | ci | push | main | success | 16m03s | 18m36s |
| 2026-07-15 03:09 | ci | pull_request | ci/node24-actions | cancelled | 3m29s | 6m40s |

---

Query the raw store directly, e.g. every recorded ci run's wall time:

```sh
git fetch origin ci-metrics && git show origin/ci-metrics:runs/2026-07.ndjson \
  | jq -r 'select(.kind == "run" and .workflow == "ci") | [.created_at, .conclusion, .duration_s] | @tsv'
```
