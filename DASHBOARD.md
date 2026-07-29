# phux CI dashboard

Generated 2026-07-29T10:04:46Z by the ci-metrics workflow. Do not edit —
every table is re-rendered from `runs/*.ndjson` on each update.
Machine rollup: [`site/summary.json`](site/summary.json), rendered live at
<https://phux.phall.io/ci>.

## Workflows, last 30 days

| workflow | runs | success | median | p95 | runner minutes |
|---|---:|---:|---:|---:|---:|
| ci | 260 | 59% | 13m37s | 18m24s | 3140 |
| observatory | 16 | 88% | 12m07s | 12m56s | 360 |
| stress | 30 | 47% | 5m20s | 22m37s | 295 |
| release-please | 53 | 98% | 45s | 7m42s | 143 |
| conventional-commits | 232 | 82% | 16s | 22s | 49 |

## ci jobs, last 30 days

| job | runs | median queue | median wall | p95 wall |
|---|---:|---:|---:|---:|
| test | 257 | 2s | 13m27s | 17m49s |
| check | 255 | 2s | 3m16s | 5m12s |
| detect docs-only | 260 | 2s | 5s | 8s |

## Slowest ci steps (median, last 30 days)

| job | step | median | samples |
|---|---|---:|---:|
| test | tests (unit + e2e) | 14m32s | 17 |
| check | rust checks (fmt + clippy + doc + deny) | 2m47s | 17 |
| test | runner disk headroom | 38s | 18 |
| check | runner disk headroom | 29s | 17 |
| check | Run Swatinem/rust-cache@v2 | 18s | 17 |
| test | Run Swatinem/rust-cache@v2 | 17s | 18 |
| check | docs-check | 13s | 17 |
| test | agents smoke | 12s | 17 |
| check | Run DeterminateSystems/nix-installer-action@v22 | 9s | 17 |
| test | Run DeterminateSystems/nix-installer-action@v22 | 9s | 18 |
| check | e2e lane coverage | 5s | 16 |
| check | formula-check | 5s | 16 |

## Cargo phases inside the lanes (median, last 30 days)

| workflow / job | phase | median | samples |
|---|---|---:|---:|
| ci / check | clippy | 1m55s | 140 |
| ci / check | doc | 11s | 140 |
| ci / check | deny | 4s | 139 |
| ci / check | fmt | 2s | 143 |
| ci / test | unit | 13m24s | 128 |
| ci / test | e2e | 10s | 127 |
| ci / test | agents-smoke | 1s | 68 |
| observatory / timings | build-dev | 11m06s | 14 |
| observatory / timings | build-release | 5m11s | 15 |
| stress / stress | stress | 19m05s | 15 |

## Cache effectiveness (last 30 days)

| workflow / job | rust-cache hit rate | samples |
|---|---:|---:|
| ci / check | 31% | 150 |
| ci / test | 35% | 148 |
| stress / stress | 13% | 15 |

## Cold build (observatory)

### dev: 11m24s (previous: 7m50s) — 541 units at `02dd58643`

| slowest units | wall |
|---|---:|
| `libghostty-vt-sys build script (run)` | 131.05s |
| `phux-client lib (test)` | 92.59s |
| `phux bin "phux"` | 90.2s |
| `phux-server` | 82.24s |
| `phux-server lib (test)` | 50.6s |
| `rustls` | 50.38s |
| `phux bin "phux" (test)` | 45.07s |
| `phux-config` | 34.65s |

### release: 4m45s (previous: 5m25s) — 365 units at `02dd58643`

| slowest units | wall |
|---|---:|
| `libghostty-vt-sys build script (run)` | 121.34s |
| `phux bin "phux"` | 118.25s |
| `phux-server` | 23.48s |
| `phux-config` | 21.09s |
| `phux-mcp bin "phux-mcp"` | 20.13s |
| `regex-automata` | 18.52s |
| `rustls` | 12.94s |
| `clap_builder` | 12.29s |

## Release binary size

| binary | size | previous |
|---|---:|---:|
| `phux` | 14.6 MiB | 14.5 MiB |
| `phux-mcp` | 2.1 MiB | 2.1 MiB |

## Dependency graph

- locked packages: **438** (previous: 438) — 13 workspace members, 51 direct deps
- duplicate versions: **32** (previous: 32)
- proc-macro crates: 33; build-script crates: 67

## Slowest tests (latest instrumented run, `04b289c1e`)

| test | wall |
|---|---:|
| `phux-server::stress_resize_extremes::both_axes_shrink_storm_under_output_does_not_panic` | 914.658s |
| `phux-server::stress_output_extremes::multi_mb_no_newline_burst_does_not_panic` | 39.436s |
| `phux-server::stress_lifecycle_churn::attach_racing_pty_eof_does_not_panic` | 10.197s |
| `phux-server::stress_output_extremes::wide_combining_zwj_flood_does_not_panic` | 3.808s |
| `phux-server::stress_resize_extremes::resize_degenerate_viewports_do_not_panic` | 3.442s |
| `phux-server::stress_attach_churn::attach_detach_churn_keeps_pane_alive` | 0.449s |
| `phux-server::stress_output_extremes::control_char_flood_does_not_panic` | 0.421s |
| `phux-server::stress_lifecycle_churn::many_concurrent_clients_attach_detach_under_output` | 0.349s |
| `phux-server::stress_output_extremes::rapid_alt_screen_toggles_do_not_panic` | 0.348s |
| `phux-server::stress_spawn_kill::spawn_storm_then_kill_storm_does_not_panic` | 0.152s |

## Recent runs

| when | workflow | event | branch | result | wall | runner time |
|---|---|---|---|---|---:|---:|
| 2026-07-29 09:42 | stress | schedule | main | success | 22m14s | 22m04s |
| 2026-07-29 09:37 | conventional-commits | pull_request | release-please--branches--main-- | success | 16s | 14s |
| 2026-07-29 09:37 | ci | pull_request | release-please--branches--main-- | skipped | 7s | 0s |
| 2026-07-29 09:36 | ci | pull_request | release-please--branches--main-- | skipped | 2s | 0s |
| 2026-07-29 09:36 | conventional-commits | pull_request | release-please--branches--main-- | success | 18s | 14s |
| 2026-07-29 09:36 | stress | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-07-29 09:36 | release-please | push | main | success | 1m00s | 41s |
| 2026-07-29 09:15 | conventional-commits | pull_request | fix/snapshot-mouse-modes | success | 17s | 14s |
| 2026-07-29 09:15 | ci | pull_request | fix/snapshot-mouse-modes | success | 20m30s | 24m41s |
| 2026-07-28 09:39 | stress | schedule | main | failure | 2m39s | 2m35s |
| 2026-07-27 20:38 | stress | pull_request | release-please--branches--main-- | skipped | 11s | 0s |
| 2026-07-27 20:37 | release-please | push | main | success | 8m03s | 20m11s |
| 2026-07-27 20:37 | observatory | push | main | success | 12m46s | 23m55s |
| 2026-07-27 20:37 | ci | push | main | success | 17m26s | 22m12s |
| 2026-07-27 20:20 | ci | pull_request | release-please--branches--main-- | success | 17m18s | 22m03s |
| 2026-07-27 20:19 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-07-27 20:19 | conventional-commits | pull_request | release-please--branches--main-- | success | 18s | 14s |
| 2026-07-27 20:19 | stress | pull_request | release-please--branches--main-- | skipped | 2s | 0s |
| 2026-07-27 20:19 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-07-27 20:19 | conventional-commits | pull_request | release-please--branches--main-- | success | 16s | 13s |
| 2026-07-27 20:18 | release-please | push | main | success | 50s | 44s |
| 2026-07-27 20:18 | ci | push | main | success | 15m37s | 20m37s |
| 2026-07-27 20:03 | conventional-commits | pull_request | ephemeral-lifetime-and-playback | success | 15s | 13s |
| 2026-07-27 20:03 | ci | pull_request | ephemeral-lifetime-and-playback | success | 15m53s | 19m34s |
| 2026-07-27 13:53 | stress | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-07-27 13:53 | release-please | push | main | success | 8m35s | 20m28s |
| 2026-07-27 13:53 | observatory | push | main | success | 11m08s | 21m07s |
| 2026-07-27 13:53 | ci | push | main | success | 24m26s | 22m26s |
| 2026-07-27 13:36 | ci | pull_request | release-please--branches--main-- | success | 16m23s | 18m03s |
| 2026-07-27 13:36 | conventional-commits | pull_request | release-please--branches--main-- | success | 48s | 11s |

---

Query the raw store directly, e.g. every recorded ci run's wall time:

```sh
git fetch origin ci-metrics && git show origin/ci-metrics:runs/2026-07.ndjson \
  | jq -r 'select(.kind == "run" and .workflow == "ci") | [.created_at, .conclusion, .duration_s] | @tsv'
```
