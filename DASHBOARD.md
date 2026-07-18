# phux CI dashboard

Generated 2026-07-18T03:24:27Z by the ci-metrics workflow. Do not edit —
every table is re-rendered from `runs/*.ndjson` on each update.
Machine rollup: [`site/summary.json`](site/summary.json), rendered live at
<https://phux.phall.io/ci>.

## Workflows, last 30 days

| workflow | runs | success | median | p95 | runner minutes |
|---|---:|---:|---:|---:|---:|
| ci | 144 | 60% | 13m31s | 17m15s | 1793 |
| observatory | 5 | 80% | 11m56s | 12m25s | 118 |
| stress | 8 | 63% | 21m11s | 22m37s | 111 |
| release-please | 23 | 100% | 42s | 52s | 29 |
| conventional-commits | 134 | 86% | 15s | 20s | 25 |

## ci jobs, last 30 days

| job | runs | median queue | median wall | p95 wall |
|---|---:|---:|---:|---:|
| test | 143 | 2s | 13m21s | 17m02s |
| check | 141 | 2s | 2m40s | 4m32s |
| detect docs-only | 144 | 2s | 5s | 7s |

## Slowest ci steps (median, last 30 days)

| job | step | median | samples |
|---|---|---:|---:|
| test | tests (unit + e2e) | 15m27s | 16 |
| check | rust checks (fmt + clippy + doc + deny) | 2m58s | 18 |
| check | Run Swatinem/rust-cache@v2 | 19s | 19 |
| check | runner disk headroom | 19s | 2 |
| test | Run Swatinem/rust-cache@v2 | 18s | 19 |
| test | runner disk headroom | 18s | 2 |
| test | agents smoke | 12s | 6 |
| check | docs-check | 9s | 17 |
| check | Run DeterminateSystems/nix-installer-action@v22 | 8s | 19 |
| test | Run DeterminateSystems/nix-installer-action@v22 | 8s | 19 |

## Cargo phases inside the lanes (median, last 30 days)

| workflow / job | phase | median | samples |
|---|---|---:|---:|
| ci / check | clippy | 1m59s | 76 |
| ci / check | doc | 13s | 76 |
| ci / check | deny | 4s | 76 |
| ci / check | fmt | 1s | 79 |
| ci / test | unit | 14m03s | 66 |
| ci / test | e2e | 9s | 65 |
| ci / test | agents-smoke | 1s | 6 |
| observatory / timings | build-dev | 10m47s | 4 |
| observatory / timings | build-release | 5m00s | 5 |
| stress / stress | stress | 21m02s | 3 |

## Cache effectiveness (last 30 days)

| workflow / job | rust-cache hit rate | samples |
|---|---:|---:|
| ci / check | 29% | 82 |
| ci / test | 34% | 79 |
| stress / stress | 0% | 3 |

## Cold build (observatory)

### dev: 11m28s (previous: 11m06s) — 520 units at `5b4cd3856`

| slowest units | wall |
|---|---:|
| `libghostty-vt-sys build script (run)` | 113.73s |
| `phux-server lib (test)` | 93.4s |
| `phux bin "phux"` | 76.2s |
| `phux-client lib (test)` | 68.39s |
| `phux-server` | 56.21s |
| `rustls` | 54.73s |
| `phux-server test "spawn_terminal" (test)` | 35.15s |
| `phux-server test "hub_relay_federation" (test)` | 34.92s |

### release: 5m11s (previous: 5m01s) — 359 units at `5b4cd3856`

| slowest units | wall |
|---|---:|
| `libghostty-vt-sys build script (run)` | 152.65s |
| `phux bin "phux"` | 109.69s |
| `regex-automata` | 26.09s |
| `phux-server` | 23.96s |
| `phux-mcp bin "phux-mcp"` | 22.59s |
| `phux-config` | 17.72s |
| `rustls` | 17.24s |
| `quinn-proto` | 13.33s |

## Release binary size

| binary | size | previous |
|---|---:|---:|
| `phux` | 12.8 MiB | 12.8 MiB |
| `phux-mcp` | 2.1 MiB | 2.1 MiB |

## Dependency graph

- locked packages: **432** (previous: 431) — 11 workspace members, 48 direct deps
- duplicate versions: **32** (previous: 32)
- proc-macro crates: 33; build-script crates: 67

## Slowest tests (latest instrumented run, `220695682`)

| test | wall |
|---|---:|
| `phux-server::stress_resize_extremes::both_axes_shrink_storm_under_output_does_not_panic` | 960.092s |
| `phux-server::stress_output_extremes::multi_mb_no_newline_burst_does_not_panic` | 36.579s |
| `phux-server::stress_lifecycle_churn::attach_racing_pty_eof_does_not_panic` | 10.196s |
| `phux-server::stress_output_extremes::wide_combining_zwj_flood_does_not_panic` | 3.703s |
| `phux-server::stress_resize_extremes::resize_degenerate_viewports_do_not_panic` | 3.064s |
| `phux-server::stress_output_extremes::control_char_flood_does_not_panic` | 0.785s |
| `phux-server::stress_attach_churn::attach_detach_churn_keeps_pane_alive` | 0.450s |
| `phux-server::stress_lifecycle_churn::many_concurrent_clients_attach_detach_under_output` | 0.345s |
| `phux-server::stress_output_extremes::rapid_alt_screen_toggles_do_not_panic` | 0.339s |
| `phux-server::stress_spawn_kill::spawn_storm_then_kill_storm_does_not_panic` | 0.152s |

## Recent runs

| when | workflow | event | branch | result | wall | runner time |
|---|---|---|---|---|---:|---:|
| 2026-07-18 03:23 | conventional-commits | pull_request | ci/sync-install-surface-releasin | success | 18s | 15s |
| 2026-07-17 09:14 | stress | schedule | main | success | 22m37s | 22m34s |
| 2026-07-16 09:20 | stress | schedule | main | success | 23m45s | 23m42s |
| 2026-07-15 20:42 | release-please | push | main | success | 21s | 18s |
| 2026-07-15 20:42 | ci | push | main | success | 15m45s | 20m43s |
| 2026-07-15 20:24 | conventional-commits | pull_request | ci/runner-disk-headroom | success | 19s | 14s |
| 2026-07-15 20:24 | ci | pull_request | ci/runner-disk-headroom | success | 16m52s | 20m45s |
| 2026-07-15 20:22 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-07-15 20:22 | conventional-commits | pull_request | release-please--branches--main-- | success | 13s | 10s |
| 2026-07-15 20:22 | conventional-commits | pull_request | release-please--branches--main-- | success | 18s | 14s |
| 2026-07-15 20:22 | conventional-commits | pull_request | release-please--branches--main-- | cancelled | 2s | 1s |
| 2026-07-15 20:22 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-07-15 20:21 | release-please | push | main | success | 45s | 38s |
| 2026-07-15 20:21 | ci | push | main | success | 13m34s | 17m20s |
| 2026-07-15 20:04 | conventional-commits | pull_request | train/wave2-2026-07-15 | success | 16s | 12s |
| 2026-07-15 20:04 | ci | pull_request | train/wave2-2026-07-15 | success | 15m56s | 19m33s |
| 2026-07-15 19:54 | ci | pull_request | release-please--branches--main-- | skipped | 2s | 0s |
| 2026-07-15 19:54 | conventional-commits | pull_request | release-please--branches--main-- | success | 17s | 13s |
| 2026-07-15 19:53 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-07-15 19:53 | conventional-commits | pull_request | release-please--branches--main-- | success | 14s | 10s |
| 2026-07-15 19:53 | conventional-commits | pull_request | release-please--branches--main-- | cancelled | 2s | 0s |
| 2026-07-15 19:53 | release-please | push | main | success | 44s | 39s |
| 2026-07-15 19:53 | ci | push | main | success | 16m29s | 20m17s |
| 2026-07-15 19:46 | conventional-commits | pull_request | train/wave2-2026-07-15 | success | 16s | 12s |
| 2026-07-15 19:46 | ci | pull_request | train/wave2-2026-07-15 | success | 15m10s | 17m47s |
| 2026-07-15 19:37 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-07-15 19:37 | conventional-commits | pull_request | release-please--branches--main-- | success | 17s | 12s |
| 2026-07-15 19:37 | conventional-commits | pull_request | release-please--branches--main-- | cancelled | 2s | 1s |
| 2026-07-15 19:37 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-07-15 19:37 | conventional-commits | pull_request | release-please--branches--main-- | success | 15s | 11s |

---

Query the raw store directly, e.g. every recorded ci run's wall time:

```sh
git fetch origin ci-metrics && git show origin/ci-metrics:runs/2026-07.ndjson \
  | jq -r 'select(.kind == "run" and .workflow == "ci") | [.created_at, .conclusion, .duration_s] | @tsv'
```
