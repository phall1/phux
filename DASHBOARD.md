# phux CI dashboard

Generated 2026-08-08T07:58:15Z by the ci-metrics workflow. Do not edit —
every table is re-rendered from `runs/*.ndjson` on each update.
Machine rollup: [`site/summary.json`](site/summary.json), rendered live at
<https://phux.phall.io/ci>.

## Workflows, last 30 days

Success is over **concluded** runs only. Skipped (draft PRs, which the
workflow deliberately does not run) and cancelled (superseded pushes)
runs never reached a verdict and are counted under "not run".

| workflow | runs | concluded | success | main | not run | median | p95 | runner minutes |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| ci | 483 | 280 | 91% | 98% (101) | 203 | 10m24s | 18m11s | 5223 |
| observatory | 32 | 32 | 94% | 94% (32) | 0 | 12m23s | 13m14s | 747 |
| stress | 54 | 27 | 78% | 78% (27) | 27 | 11s | 22m15s | 373 |
| release-please | 103 | 103 | 97% | 97% (103) | 0 | 48s | 7m47s | 280 |
| conventional-commits | 433 | 360 | 96% | -- | 73 | 17s | 25s | 89 |
| release | 3 | 3 | 100% | 100% (2) | 0 | 8m02s | 8m02s | 61 |

## ci jobs, last 30 days

| job | runs | median queue | median wall | p95 wall |
|---|---:|---:|---:|---:|
| test | 473 | 2s | 10m13s | 17m24s |
| check | 475 | 2s | 2m33s | 5m31s |
| detect docs-only | 478 | 2s | 5s | 9s |

## Slowest ci steps (median, last 30 days)

| job | step | median | samples |
|---|---|---:|---:|
| test | tests (unit + e2e) | 9m45s | 12 |
| check | rust checks (fmt + clippy + doc + deny) | 4m33s | 14 |
| test | Run Swatinem/rust-cache@v2 | 15s | 17 |
| check | Run Swatinem/rust-cache@v2 | 14s | 17 |
| check | docs-check | 11s | 15 |
| test | agents smoke | 10s | 12 |
| test | runner disk headroom | 8s | 17 |
| check | Run DeterminateSystems/nix-installer-action@v22 | 7s | 17 |
| check | runner disk headroom | 7s | 17 |
| test | Run DeterminateSystems/nix-installer-action@v22 | 7s | 17 |

## Cargo phases inside the lanes (median, last 30 days)

| workflow / job | phase | median | samples |
|---|---|---:|---:|
| ci / check | clippy | 1m51s | 266 |
| ci / check | doc | 10s | 266 |
| ci / check | deny | 3s | 264 |
| ci / check | fmt | 2s | 275 |
| ci / test | unit | 11m40s | 246 |
| ci / test | e2e | 19s | 236 |
| ci / test | agents-smoke | 1s | 172 |
| observatory / timings | build-dev | 10m47s | 30 |
| observatory / timings | build-release | 5m33s | 31 |
| stress / stress | stress | 7m04s | 25 |

## Cache effectiveness (last 30 days)

| workflow / job | rust-cache hit rate | samples |
|---|---:|---:|
| ci / check | 42% | 284 |
| ci / test | 42% | 283 |
| stress / stress | 12% | 25 |

## Cold build (observatory)

### dev: 7m11s (previous: 9m48s) — 555 units at `d8fe06c6f`

| slowest units | wall |
|---|---:|
| `libghostty-vt-sys build script (run)` | 200.26s |
| `phux-server lib (test)` | 101.5s |
| `phux bin "phux"` | 73.83s |
| `phux-server` | 69.82s |
| `phux bin "phux" (test)` | 42.99s |
| `phux-client lib (test)` | 39.15s |
| `phux-server-testkit` | 25.33s |
| `phux-config` | 24.89s |

### release: 6m20s (previous: 6m51s) — 368 units at `d8fe06c6f`

| slowest units | wall |
|---|---:|
| `libghostty-vt-sys build script (run)` | 176.47s |
| `phux bin "phux"` | 153.84s |
| `phux-server` | 31.73s |
| `phux-mcp bin "phux-mcp"` | 23.89s |
| `phux-config` | 22.22s |
| `phux-server-testkit` | 13.14s |
| `phux-client` | 9.9s |
| `regex-automata` | 9.01s |

## Release binary size

| binary | size | previous |
|---|---:|---:|
| `phux` | 13.5 MiB | 16.4 MiB |
| `phux-mcp` | 1.7 MiB | 2.1 MiB |

## Dependency graph

- locked packages: **441** (previous: 441) — 15 workspace members, 53 direct deps
- duplicate versions: **33** (previous: 33)
- proc-macro crates: 33; build-script crates: 67

## Slowest tests (latest instrumented run, `d8fe06c6f`)

| test | wall |
|---|---:|
| `phux-server::stress_lifecycle_churn::attach_racing_pty_eof_does_not_panic` | 10.175s |
| `phux-server::stress_attach_churn::attach_detach_churn_keeps_pane_alive` | 0.339s |
| `phux-server::stress_spawn_kill::spawn_storm_then_kill_storm_does_not_panic` | 0.126s |
| `phux-server::perf_bursty_output::synthesize_against_reference_alloc_bounded_under_full_churn` | 0.072s |
| `phux-server::stress_resize_storm::resize_storm_converges_to_final_geometry` | 0.049s |
| `phux-server::stress_output_extremes::multi_mb_no_newline_burst_does_not_panic` | 0.046s |
| `phux-server::stress_resize_extremes::resize_degenerate_viewports_do_not_panic` | 0.038s |
| `phux-server::stress_lifecycle_churn::many_concurrent_clients_attach_detach_under_output` | 0.038s |
| `phux-server::stress_output_extremes::control_char_flood_does_not_panic` | 0.034s |
| `phux-server::stress_output_extremes::wide_combining_zwj_flood_does_not_panic` | 0.033s |

## Recent runs

| when | workflow | event | branch | result | wall | runner time |
|---|---|---|---|---|---:|---:|
| 2026-08-08 07:52 | stress | schedule | main | success | 5m35s | 5m32s |
| 2026-08-08 05:55 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-08-08 05:55 | conventional-commits | pull_request | release-please--branches--main-- | success | 19s | 17s |
| 2026-08-08 05:54 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-08-08 05:54 | conventional-commits | pull_request | release-please--branches--main-- | success | 22s | 19s |
| 2026-08-08 05:54 | release-please | push | main | success | 51s | 39s |
| 2026-08-08 05:54 | ci | push | main | success | 11m09s | 16m58s |
| 2026-08-08 05:54 | observatory | push | main | success | 12m47s | 22m11s |
| 2026-08-07 13:59 | conventional-commits | pull_request | rc/one-zero-candidate-pass | success | 18s | 14s |
| 2026-08-07 13:59 | ci | pull_request | rc/one-zero-candidate-pass | success | 11m46s | 16m55s |
| 2026-08-07 13:45 | ci | pull_request | rc/one-zero-candidate-pass | failure | 9m22s | 15m12s |
| 2026-08-07 13:45 | conventional-commits | pull_request | rc/one-zero-candidate-pass | success | 14s | 12s |
| 2026-08-07 13:30 | ci | pull_request | rc/one-zero-candidate-pass | failure | 8m59s | 14m55s |
| 2026-08-07 13:30 | conventional-commits | pull_request | rc/one-zero-candidate-pass | success | 21s | 17s |
| 2026-08-07 13:26 | conventional-commits | pull_request | rc/one-zero-candidate-pass | success | 15s | 12s |
| 2026-08-07 13:13 | conventional-commits | pull_request | rc/one-zero-candidate-pass | failure | 17s | 13s |
| 2026-08-07 13:13 | ci | pull_request | rc/one-zero-candidate-pass | failure | 9m22s | 10m20s |
| 2026-08-07 13:10 | conventional-commits | pull_request | rc/one-zero-candidate-pass | failure | 21s | 18s |
| 2026-08-07 13:10 | ci | pull_request | rc/one-zero-candidate-pass | cancelled | 2m32s | 3m50s |
| 2026-08-07 08:18 | stress | schedule | main | success | 6m33s | 6m29s |
| 2026-08-07 07:08 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-08-07 07:08 | conventional-commits | pull_request | release-please--branches--main-- | success | 29s | 26s |
| 2026-08-07 07:07 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-08-07 07:07 | stress | pull_request | release-please--branches--main-- | skipped | 8s | 0s |
| 2026-08-07 07:07 | conventional-commits | pull_request | release-please--branches--main-- | success | 20s | 17s |
| 2026-08-07 07:07 | ci | push | main | success | 10m52s | 16m33s |
| 2026-08-07 07:07 | release-please | push | main | success | 42s | 35s |
| 2026-08-07 06:51 | conventional-commits | pull_request | docs-ios-consumer-contract | success | 14s | 11s |
| 2026-08-07 06:51 | ci | pull_request | docs-ios-consumer-contract | success | 13m15s | 18m24s |
| 2026-08-06 09:43 | stress | schedule | main | success | 6m49s | 6m45s |

---

Query the raw store directly, e.g. every recorded ci run's wall time:

```sh
git fetch origin ci-metrics && git show origin/ci-metrics:runs/2026-08.ndjson \
  | jq -r 'select(.kind == "run" and .workflow == "ci") | [.created_at, .conclusion, .duration_s] | @tsv'
```
