# phux CI dashboard

Generated 2026-07-26T09:26:07Z by the ci-metrics workflow. Do not edit —
every table is re-rendered from `runs/*.ndjson` on each update.
Machine rollup: [`site/summary.json`](site/summary.json), rendered live at
<https://phux.phall.io/ci>.

## Workflows, last 30 days

| workflow | runs | success | median | p95 | runner minutes |
|---|---:|---:|---:|---:|---:|
| ci | 228 | 59% | 13m34s | 18m11s | 2715 |
| stress | 21 | 62% | 18m05s | 23m45s | 264 |
| observatory | 10 | 80% | 12m07s | 12m56s | 215 |
| release-please | 43 | 98% | 44s | 54s | 79 |
| conventional-commits | 209 | 82% | 16s | 21s | 43 |

## ci jobs, last 30 days

| job | runs | median queue | median wall | p95 wall |
|---|---:|---:|---:|---:|
| test | 225 | 2s | 13m25s | 17m49s |
| check | 223 | 2s | 2m52s | 5m05s |
| detect docs-only | 228 | 2s | 5s | 7s |

## Slowest ci steps (median, last 30 days)

| job | step | median | samples |
|---|---|---:|---:|
| test | tests (unit + e2e) | 13m09s | 14 |
| check | rust checks (fmt + clippy + doc + deny) | 1m31s | 14 |
| check | runner disk headroom | 1m08s | 14 |
| test | runner disk headroom | 58s | 14 |
| test | Run Swatinem/rust-cache@v2 | 24s | 14 |
| check | Run Swatinem/rust-cache@v2 | 21s | 14 |
| test | agents smoke | 12s | 14 |
| check | docs-check | 10s | 14 |
| check | Run DeterminateSystems/nix-installer-action@v22 | 8s | 14 |
| test | Run DeterminateSystems/nix-installer-action@v22 | 8s | 14 |

## Cargo phases inside the lanes (median, last 30 days)

| workflow / job | phase | median | samples |
|---|---|---:|---:|
| ci / check | clippy | 1m53s | 119 |
| ci / check | doc | 11s | 119 |
| ci / check | deny | 3s | 119 |
| ci / check | fmt | 2s | 122 |
| ci / test | unit | 13m24s | 109 |
| ci / test | e2e | 10s | 108 |
| ci / test | agents-smoke | 1s | 49 |
| observatory / timings | build-dev | 11m06s | 8 |
| observatory / timings | build-release | 5m01s | 9 |
| stress / stress | stress | 19m05s | 12 |

## Cache effectiveness (last 30 days)

| workflow / job | rust-cache hit rate | samples |
|---|---:|---:|
| ci / check | 34% | 129 |
| ci / test | 39% | 127 |
| stress / stress | 17% | 12 |

## Cold build (observatory)

### dev: 11m44s (previous: 11m01s) — 528 units at `0ae92367a`

| slowest units | wall |
|---|---:|
| `libghostty-vt-sys build script (run)` | 122.95s |
| `phux-server lib (test)` | 99.24s |
| `phux bin "phux"` | 81.19s |
| `phux-client lib (test)` | 65.43s |
| `phux-server` | 57.01s |
| `rustls` | 47.45s |
| `phux-server test "spawn_terminal" (test)` | 35.61s |
| `phux-server test "hub_relay_federation" (test)` | 34.74s |

### release: 5m13s (previous: 4m10s) — 362 units at `0ae92367a`

| slowest units | wall |
|---|---:|
| `libghostty-vt-sys build script (run)` | 148.24s |
| `phux bin "phux"` | 117.54s |
| `phux-server` | 24.08s |
| `phux-mcp bin "phux-mcp"` | 22.58s |
| `phux-config` | 22.25s |
| `regex-automata` | 19.92s |
| `clap_builder` | 17.54s |
| `rustls` | 14.46s |

## Release binary size

| binary | size | previous |
|---|---:|---:|
| `phux` | 13.6 MiB | 12.8 MiB |
| `phux-mcp` | 2.1 MiB | 2.1 MiB |

## Dependency graph

- locked packages: **435** (previous: 432) — 12 workspace members, 50 direct deps
- duplicate versions: **32** (previous: 32)
- proc-macro crates: 33; build-script crates: 67

## Slowest tests (latest instrumented run, `b7ffd459d`)

| test | wall |
|---|---:|
| `phux-server::stress_output_extremes::multi_mb_no_newline_burst_does_not_panic` | 18.000s |
| `phux-server::stress_lifecycle_churn::attach_racing_pty_eof_does_not_panic` | 10.197s |
| `phux-server::stress_output_extremes::control_char_flood_does_not_panic` | 0.518s |
| `phux-server::stress_attach_churn::attach_detach_churn_keeps_pane_alive` | 0.472s |
| `phux-server::stress_lifecycle_churn::many_concurrent_clients_attach_detach_under_output` | 0.389s |

## Recent runs

| when | workflow | event | branch | result | wall | runner time |
|---|---|---|---|---|---:|---:|
| 2026-07-26 09:19 | stress | schedule | main | failure | 6m52s | 6m49s |
| 2026-07-26 03:53 | ci | pull_request | release-please--branches--main-- | skipped | 2s | 0s |
| 2026-07-26 03:53 | conventional-commits | pull_request | release-please--branches--main-- | success | 15s | 12s |
| 2026-07-26 03:52 | conventional-commits | pull_request | release-please--branches--main-- | success | 19s | 15s |
| 2026-07-26 03:52 | conventional-commits | pull_request | release-please--branches--main-- | cancelled | 2s | 0s |
| 2026-07-26 03:52 | ci | pull_request | release-please--branches--main-- | skipped | 10s | 0s |
| 2026-07-26 03:52 | release-please | push | main | success | 45s | 41s |
| 2026-07-26 03:52 | ci | push | main | success | 15m32s | 19m50s |
| 2026-07-26 03:52 | conventional-commits | pull_request | work/version-negotiation | success | 16s | 13s |
| 2026-07-26 03:52 | ci | pull_request | work/version-negotiation | success | 15m48s | 20m12s |
| 2026-07-26 02:55 | release-please | push | main | success | 28s | 23s |
| 2026-07-26 02:55 | ci | push | main | success | 16m27s | 19m37s |
| 2026-07-26 02:54 | conventional-commits | pull_request | test/put-file-e2e | success | 19s | 14s |
| 2026-07-26 02:54 | ci | pull_request | test/put-file-e2e | success | 14m30s | 18m09s |
| 2026-07-26 02:16 | ci | pull_request | release-please--branches--main-- | skipped | 2s | 0s |
| 2026-07-26 02:16 | conventional-commits | pull_request | release-please--branches--main-- | success | 20s | 16s |
| 2026-07-26 02:16 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-07-26 02:16 | conventional-commits | pull_request | release-please--branches--main-- | cancelled | 1s | 1s |
| 2026-07-26 02:16 | conventional-commits | pull_request | release-please--branches--main-- | success | 15s | 11s |
| 2026-07-26 02:15 | release-please | push | main | success | 41s | 35s |
| 2026-07-26 02:15 | ci | push | main | success | 15m23s | 18m51s |
| 2026-07-26 02:15 | conventional-commits | pull_request | feat/put-file | success | 14s | 11s |
| 2026-07-26 02:15 | ci | pull_request | feat/put-file | success | 16m21s | 19m52s |
| 2026-07-26 01:46 | ci | pull_request | release-please--branches--main-- | skipped | 2s | 0s |
| 2026-07-26 01:46 | conventional-commits | pull_request | release-please--branches--main-- | success | 15s | 11s |
| 2026-07-26 01:45 | conventional-commits | pull_request | release-please--branches--main-- | cancelled | 2s | 1s |
| 2026-07-26 01:45 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-07-26 01:45 | conventional-commits | pull_request | release-please--branches--main-- | success | 15s | 11s |
| 2026-07-26 01:45 | release-please | push | main | success | 47s | 40s |
| 2026-07-26 01:45 | ci | push | main | success | 13m54s | 16m48s |

---

Query the raw store directly, e.g. every recorded ci run's wall time:

```sh
git fetch origin ci-metrics && git show origin/ci-metrics:runs/2026-07.ndjson \
  | jq -r 'select(.kind == "run" and .workflow == "ci") | [.created_at, .conclusion, .duration_s] | @tsv'
```
