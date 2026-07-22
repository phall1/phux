# phux CI dashboard

Generated 2026-07-22T19:32:03Z by the ci-metrics workflow. Do not edit —
every table is re-rendered from `runs/*.ndjson` on each update.
Machine rollup: [`site/summary.json`](site/summary.json), rendered live at
<https://phux.phall.io/ci>.

## Workflows, last 30 days

| workflow | runs | success | median | p95 | runner minutes |
|---|---:|---:|---:|---:|---:|
| ci | 153 | 62% | 13m37s | 17m31s | 1951 |
| stress | 14 | 71% | 20m45s | 23m45s | 202 |
| observatory | 7 | 86% | 12m25s | 12m42s | 168 |
| release-please | 25 | 100% | 42s | 54s | 50 |
| conventional-commits | 141 | 86% | 16s | 20s | 26 |

## ci jobs, last 30 days

| job | runs | median queue | median wall | p95 wall |
|---|---:|---:|---:|---:|
| test | 152 | 2s | 13m25s | 17m20s |
| check | 150 | 2s | 2m43s | 4m36s |
| detect docs-only | 153 | 2s | 5s | 7s |

## Slowest ci steps (median, last 30 days)

| job | step | median | samples |
|---|---|---:|---:|
| test | tests (unit + e2e) | 15m27s | 18 |
| check | rust checks (fmt + clippy + doc + deny) | 2m55s | 19 |
| check | runner disk headroom | 58s | 10 |
| test | runner disk headroom | 51s | 10 |
| check | Run Swatinem/rust-cache@v2 | 18s | 20 |
| test | Run Swatinem/rust-cache@v2 | 18s | 20 |
| test | agents smoke | 12s | 13 |
| check | docs-check | 9s | 20 |
| check | Run DeterminateSystems/nix-installer-action@v22 | 8s | 20 |
| test | Run DeterminateSystems/nix-installer-action@v22 | 8s | 20 |

## Cargo phases inside the lanes (median, last 30 days)

| workflow / job | phase | median | samples |
|---|---|---:|---:|
| ci / check | clippy | 2m00s | 83 |
| ci / check | doc | 12s | 83 |
| ci / check | deny | 4s | 83 |
| ci / check | fmt | 1s | 86 |
| ci / test | unit | 14m07s | 73 |
| ci / test | e2e | 10s | 72 |
| ci / test | agents-smoke | 1s | 13 |
| observatory / timings | build-dev | 11m06s | 6 |
| observatory / timings | build-release | 5m01s | 7 |
| stress / stress | stress | 19m27s | 8 |

## Cache effectiveness (last 30 days)

| workflow / job | rust-cache hit rate | samples |
|---|---:|---:|
| ci / check | 30% | 90 |
| ci / test | 34% | 87 |
| stress / stress | 13% | 8 |

## Cold build (observatory)

### dev: 11m27s (previous: 11m22s) — 520 units at `c34009cfb`

| slowest units | wall |
|---|---:|
| `libghostty-vt-sys build script (run)` | 123.46s |
| `phux-server lib (test)` | 91.46s |
| `phux bin "phux"` | 73.96s |
| `phux-client lib (test)` | 65.86s |
| `phux-server` | 56.43s |
| `rustls` | 48.41s |
| `phux-server test "spawn_terminal" (test)` | 35.92s |
| `quinn-proto` | 35.03s |

### release: 5m07s (previous: 5m24s) — 359 units at `c34009cfb`

| slowest units | wall |
|---|---:|
| `libghostty-vt-sys build script (run)` | 158.3s |
| `phux bin "phux"` | 102.9s |
| `phux-mcp bin "phux-mcp"` | 21.12s |
| `phux-server` | 20.93s |
| `regex-automata` | 19.23s |
| `rustls` | 18.4s |
| `phux-config` | 17.64s |
| `clap_builder` | 16.19s |

## Release binary size

| binary | size | previous |
|---|---:|---:|
| `phux` | 12.9 MiB | 12.9 MiB |
| `phux-mcp` | 2.1 MiB | 2.1 MiB |

## Dependency graph

- locked packages: **432** (previous: 432) — 11 workspace members, 48 direct deps
- duplicate versions: **32** (previous: 32)
- proc-macro crates: 33; build-script crates: 67

## Slowest tests (latest instrumented run, `c34009cfb`)

| test | wall |
|---|---:|
| `phux-server::stress_resize_extremes::both_axes_shrink_storm_under_output_does_not_panic` | 880.216s |
| `phux-server::stress_output_extremes::multi_mb_no_newline_burst_does_not_panic` | 36.533s |
| `phux-server::stress_lifecycle_churn::attach_racing_pty_eof_does_not_panic` | 10.191s |
| `phux-server::stress_output_extremes::wide_combining_zwj_flood_does_not_panic` | 3.563s |
| `phux-server::stress_resize_extremes::resize_degenerate_viewports_do_not_panic` | 2.979s |
| `phux-server::stress_output_extremes::rapid_alt_screen_toggles_do_not_panic` | 0.621s |
| `phux-server::stress_attach_churn::attach_detach_churn_keeps_pane_alive` | 0.454s |
| `phux-server::stress_output_extremes::control_char_flood_does_not_panic` | 0.409s |
| `phux-server::stress_lifecycle_churn::many_concurrent_clients_attach_detach_under_output` | 0.342s |
| `phux-server::stress_spawn_kill::spawn_storm_then_kill_storm_does_not_panic` | 0.149s |

## Recent runs

| when | workflow | event | branch | result | wall | runner time |
|---|---|---|---|---|---:|---:|
| 2026-07-22 19:31 | conventional-commits | pull_request | fix/plugin-agent-bench-phux-bin | success | 20s | 16s |
| 2026-07-22 09:31 | stress | schedule | main | success | 21m01s | 20m58s |
| 2026-07-21 14:48 | conventional-commits | pull_request | feat/oss-reference-relay | success | 14s | 11s |
| 2026-07-21 14:48 | ci | pull_request | feat/oss-reference-relay | success | 18m08s | 23m10s |
| 2026-07-21 14:47 | conventional-commits | pull_request | feat/relay-alpn-dialer | success | 18s | 13s |
| 2026-07-21 14:47 | ci | pull_request | feat/relay-alpn-dialer | success | 15m50s | 19m27s |
| 2026-07-21 09:31 | stress | schedule | main | success | 20m09s | 20m06s |
| 2026-07-21 07:36 | conventional-commits | pull_request | adr-0052-connector-productizatio | success | 17s | 15s |
| 2026-07-21 07:36 | ci | pull_request | adr-0052-connector-productizatio | success | 2m35s | 4m04s |
| 2026-07-20 23:10 | stress | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-07-20 23:10 | release-please | push | main | success | 8m33s | 19m45s |
| 2026-07-20 23:10 | observatory | push | main | success | 12m42s | 24m27s |
| 2026-07-20 23:10 | ci | push | main | success | 17m45s | 22m41s |
| 2026-07-20 22:52 | conventional-commits | pull_request | release-please--branches--main-- | success | 16s | 12s |
| 2026-07-20 22:52 | ci | pull_request | release-please--branches--main-- | success | 17m52s | 22m05s |
| 2026-07-20 22:51 | conventional-commits | pull_request | release-please--branches--main-- | cancelled | 1s | 0s |
| 2026-07-20 22:51 | conventional-commits | pull_request | release-please--branches--main-- | success | 15s | 11s |
| 2026-07-20 22:51 | ci | pull_request | release-please--branches--main-- | cancelled | 42s | 1m05s |
| 2026-07-20 22:51 | release-please | push | main | success | 48s | 42s |
| 2026-07-20 22:51 | ci | push | main | success | 16m45s | 20m54s |
| 2026-07-20 10:17 | stress | schedule | main | success | 20m45s | 20m41s |
| 2026-07-20 08:57 | observatory | schedule | main | success | 12m56s | 25m23s |
| 2026-07-19 09:08 | stress | schedule | main | success | 23m51s | 23m48s |
| 2026-07-18 08:52 | stress | schedule | main | success | 5m20s | 5m17s |
| 2026-07-18 03:23 | conventional-commits | pull_request | ci/sync-install-surface-releasin | success | 18s | 15s |
| 2026-07-18 03:23 | ci | pull_request | ci/sync-install-surface-releasin | success | 18m27s | 21m43s |
| 2026-07-18 03:22 | ci | pull_request | release-please--branches--main-- | success | 17m31s | 22m48s |
| 2026-07-17 09:14 | stress | schedule | main | success | 22m37s | 22m34s |
| 2026-07-16 09:20 | stress | schedule | main | success | 23m45s | 23m42s |
| 2026-07-15 20:42 | release-please | push | main | success | 21s | 18s |

---

Query the raw store directly, e.g. every recorded ci run's wall time:

```sh
git fetch origin ci-metrics && git show origin/ci-metrics:runs/2026-07.ndjson \
  | jq -r 'select(.kind == "run" and .workflow == "ci") | [.created_at, .conclusion, .duration_s] | @tsv'
```
