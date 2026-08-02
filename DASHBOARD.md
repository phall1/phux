# phux CI dashboard

Generated 2026-08-02T12:29:58Z by the ci-metrics workflow. Do not edit —
every table is re-rendered from `runs/*.ndjson` on each update.
Machine rollup: [`site/summary.json`](site/summary.json), rendered live at
<https://phux.phall.io/ci>.

## Workflows, last 30 days

| workflow | runs | success | median | p95 | runner minutes |
|---|---:|---:|---:|---:|---:|
| ci | 353 | 56% | 12m54s | 18m24s | 4070 |
| observatory | 18 | 89% | 12m23s | 13m00s | 410 |
| stress | 36 | 42% | 5m20s | 22m37s | 334 |
| release-please | 75 | 99% | 45s | 7m37s | 174 |
| conventional-commits | 318 | 80% | 16s | 24s | 65 |

## ci jobs, last 30 days

| job | runs | median queue | median wall | p95 wall |
|---|---:|---:|---:|---:|
| test | 347 | 2s | 12m30s | 17m38s |
| check | 345 | 2s | 2m39s | 5m05s |
| detect docs-only | 348 | 2s | 5s | 8s |

## Slowest ci steps (median, last 30 days)

| job | step | median | samples |
|---|---|---:|---:|
| test | tests (unit + e2e) | 10m09s | 15 |
| check | rust checks (fmt + clippy + doc + deny) | 1m21s | 15 |
| test | Run Swatinem/rust-cache@v2 | 18s | 17 |
| check | Run Swatinem/rust-cache@v2 | 17s | 17 |
| test | agents smoke | 11s | 15 |
| check | docs-check | 10s | 13 |
| check | Run DeterminateSystems/nix-installer-action@v22 | 7s | 17 |
| check | runner disk headroom | 6s | 17 |
| test | Run DeterminateSystems/nix-installer-action@v22 | 6s | 17 |

## Cargo phases inside the lanes (median, last 30 days)

| workflow / job | phase | median | samples |
|---|---|---:|---:|
| ci / check | clippy | 1m39s | 192 |
| ci / check | doc | 10s | 192 |
| ci / check | deny | 3s | 191 |
| ci / check | fmt | 2s | 197 |
| ci / test | unit | 12m32s | 178 |
| ci / test | e2e | 11s | 175 |
| ci / test | agents-smoke | 1s | 115 |
| observatory / timings | build-dev | 11m06s | 16 |
| observatory / timings | build-release | 5m13s | 17 |
| stress / stress | stress | 17m27s | 19 |

## Cache effectiveness (last 30 days)

| workflow / job | rust-cache hit rate | samples |
|---|---:|---:|
| ci / check | 43% | 205 |
| ci / test | 45% | 203 |
| stress / stress | 16% | 19 |

## Cold build (observatory)

### dev: 11m02s (previous: 11m45s) — 543 units at `6c7d65e8e`

| slowest units | wall |
|---|---:|
| `libghostty-vt-sys build script (run)` | 121.35s |
| `phux bin "phux"` | 90.05s |
| `phux-client lib (test)` | 88.53s |
| `phux-server` | 77.84s |
| `phux-server lib (test)` | 59.62s |
| `rustls` | 45.79s |
| `phux bin "phux" (test)` | 44.64s |
| `phux-config` | 34.72s |

### release: 5m34s (previous: 5m35s) — 366 units at `6c7d65e8e`

| slowest units | wall |
|---|---:|
| `libghostty-vt-sys build script (run)` | 147.37s |
| `phux bin "phux"` | 135.53s |
| `phux-server` | 28.16s |
| `phux-config` | 24.01s |
| `phux-mcp bin "phux-mcp"` | 22.81s |
| `regex-automata` | 21.31s |
| `rustls` | 14.95s |
| `clap_builder` | 13.03s |

## Release binary size

| binary | size | previous |
|---|---:|---:|
| `phux` | 14.8 MiB | 14.7 MiB |
| `phux-mcp` | 2.1 MiB | 2.1 MiB |

## Dependency graph

- locked packages: **438** (previous: 438) — 13 workspace members, 52 direct deps
- duplicate versions: **32** (previous: 32)
- proc-macro crates: 33; build-script crates: 67

## Slowest tests (latest instrumented run, `b734b7007`)

| test | wall |
|---|---:|
| `phux-server::perf_bursty_output::synthesize_against_reference_alloc_bounded_under_full_churn` | 111.320s |
| `phux-server::stress_output_extremes::multi_mb_no_newline_burst_does_not_panic` | 17.664s |
| `phux-server::stress_lifecycle_churn::attach_racing_pty_eof_does_not_panic` | 10.192s |
| `phux-server::stress_attach_churn::attach_detach_churn_keeps_pane_alive` | 0.461s |
| `phux-server::stress_output_extremes::control_char_flood_does_not_panic` | 0.448s |
| `phux-server::stress_lifecycle_churn::many_concurrent_clients_attach_detach_under_output` | 0.386s |

## Recent runs

| when | workflow | event | branch | result | wall | runner time |
|---|---|---|---|---|---:|---:|
| 2026-08-02 12:29 | conventional-commits | pull_request | phux-cull | failure | 19s | 9s |
| 2026-08-02 09:13 | stress | schedule | main | failure | 8m38s | 8m33s |
| 2026-08-02 08:50 | conventional-commits | pull_request | feat/phux-do1-relay-end-to-end | success | 14s | 11s |
| 2026-08-02 08:50 | ci | pull_request | feat/phux-do1-relay-end-to-end | success | 11m30s | 13m50s |
| 2026-08-02 08:26 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-08-02 08:26 | conventional-commits | pull_request | release-please--branches--main-- | success | 18s | 15s |
| 2026-08-02 08:25 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-08-02 08:25 | conventional-commits | pull_request | release-please--branches--main-- | cancelled | 2s | 0s |
| 2026-08-02 08:25 | conventional-commits | pull_request | release-please--branches--main-- | success | 16s | 11s |
| 2026-08-02 08:25 | release-please | push | main | success | 44s | 39s |
| 2026-08-02 08:25 | ci | push | main | success | 12m20s | 14m36s |
| 2026-08-02 08:18 | conventional-commits | pull_request | docs/phux-bd3-relay-spec-addendu | success | 19s | 14s |
| 2026-08-02 08:18 | ci | pull_request | docs/phux-bd3-relay-spec-addendu | success | 2m06s | 2m43s |
| 2026-08-02 08:14 | conventional-commits | pull_request | feat/ux-wave-5 | success | 19s | 14s |
| 2026-08-02 08:14 | ci | pull_request | feat/ux-wave-5 | success | 10m55s | 13m14s |
| 2026-08-02 07:30 | conventional-commits | pull_request | feat/ux-wave-5 | success | 23s | 19s |
| 2026-08-02 07:30 | ci | pull_request | feat/ux-wave-5 | failure | 11m22s | 13m56s |
| 2026-08-02 06:52 | conventional-commits | pull_request | feat/ux-wave-5 | failure | 14s | 12s |
| 2026-08-02 06:49 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-08-02 06:49 | conventional-commits | pull_request | release-please--branches--main-- | success | 18s | 15s |
| 2026-08-02 06:48 | conventional-commits | pull_request | release-please--branches--main-- | cancelled | 2s | 1s |
| 2026-08-02 06:48 | ci | pull_request | release-please--branches--main-- | skipped | 10s | 0s |
| 2026-08-02 06:48 | conventional-commits | pull_request | release-please--branches--main-- | success | 15s | 12s |
| 2026-08-02 06:48 | release-please | push | main | success | 45s | 38s |
| 2026-08-02 06:48 | ci | push | main | failure | 16m18s | 13m35s |
| 2026-08-02 06:48 | conventional-commits | pull_request | feat/ux-wave-5 | failure | 20s | 17s |
| 2026-08-02 06:48 | ci | pull_request | feat/ux-wave-5 | success | 10m59s | 13m34s |
| 2026-08-02 06:47 | conventional-commits | pull_request | fix/adr-0066-collision | success | 17s | 13s |
| 2026-08-02 06:47 | ci | pull_request | fix/adr-0066-collision | failure | 10m50s | 12m43s |
| 2026-08-02 06:44 | ci | pull_request | release-please--branches--main-- | skipped | 2s | 0s |

---

Query the raw store directly, e.g. every recorded ci run's wall time:

```sh
git fetch origin ci-metrics && git show origin/ci-metrics:runs/2026-08.ndjson \
  | jq -r 'select(.kind == "run" and .workflow == "ci") | [.created_at, .conclusion, .duration_s] | @tsv'
```
