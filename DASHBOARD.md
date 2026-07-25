# phux CI dashboard

Generated 2026-07-25T09:22:25Z by the ci-metrics workflow. Do not edit —
every table is re-rendered from `runs/*.ndjson` on each update.
Machine rollup: [`site/summary.json`](site/summary.json), rendered live at
<https://phux.phall.io/ci>.

## Workflows, last 30 days

| workflow | runs | success | median | p95 | runner minutes |
|---|---:|---:|---:|---:|---:|
| ci | 169 | 62% | 13m37s | 17m57s | 2144 |
| stress | 20 | 65% | 18m05s | 23m45s | 257 |
| observatory | 8 | 88% | 12m07s | 12m42s | 190 |
| release-please | 29 | 100% | 43s | 7m03s | 71 |
| conventional-commits | 152 | 86% | 16s | 21s | 33 |

## ci jobs, last 30 days

| job | runs | median queue | median wall | p95 wall |
|---|---:|---:|---:|---:|
| test | 168 | 2s | 13m25s | 17m46s |
| check | 166 | 2s | 2m47s | 4m56s |
| detect docs-only | 169 | 2s | 5s | 7s |

## Slowest ci steps (median, last 30 days)

| job | step | median | samples |
|---|---|---:|---:|
| test | tests (unit + e2e) | 15m37s | 17 |
| check | rust checks (fmt + clippy + doc + deny) | 2m58s | 16 |
| check | runner disk headroom | 1m01s | 18 |
| test | runner disk headroom | 53s | 20 |
| check | Run Swatinem/rust-cache@v2 | 19s | 18 |
| test | Run Swatinem/rust-cache@v2 | 18s | 20 |
| test | agents smoke | 12s | 17 |
| check | docs-check | 10s | 18 |
| check | Run DeterminateSystems/nix-installer-action@v22 | 8s | 18 |
| test | Run DeterminateSystems/nix-installer-action@v22 | 8s | 20 |

## Cargo phases inside the lanes (median, last 30 days)

| workflow / job | phase | median | samples |
|---|---|---:|---:|
| ci / check | clippy | 1m59s | 91 |
| ci / check | doc | 12s | 91 |
| ci / check | deny | 4s | 91 |
| ci / check | fmt | 1s | 94 |
| ci / test | unit | 14m14s | 81 |
| ci / test | e2e | 10s | 80 |
| ci / test | agents-smoke | 1s | 21 |
| observatory / timings | build-dev | 11m06s | 7 |
| observatory / timings | build-release | 5m00s | 8 |
| stress / stress | stress | 19m15s | 11 |

## Cache effectiveness (last 30 days)

| workflow / job | rust-cache hit rate | samples |
|---|---:|---:|
| ci / check | 30% | 99 |
| ci / test | 31% | 97 |
| stress / stress | 18% | 11 |

## Cold build (observatory)

### dev: 11m01s (previous: 11m27s) — 520 units at `a27ecc10d`

| slowest units | wall |
|---|---:|
| `libghostty-vt-sys build script (run)` | 111.14s |
| `phux-server lib (test)` | 89.32s |
| `phux bin "phux"` | 71.94s |
| `phux-client lib (test)` | 63.33s |
| `phux-server` | 54.26s |
| `rustls` | 46.5s |
| `phux-server test "spawn_terminal" (test)` | 34.2s |
| `phux-server test "hub_relay_federation" (test)` | 33.44s |

### release: 4m10s (previous: 5m07s) — 359 units at `a27ecc10d`

| slowest units | wall |
|---|---:|
| `libghostty-vt-sys build script (run)` | 113.58s |
| `phux bin "phux"` | 95.73s |
| `phux-server` | 19.85s |
| `phux-mcp bin "phux-mcp"` | 19.15s |
| `regex-automata` | 16.16s |
| `phux-config` | 15.04s |
| `rustls` | 13.25s |
| `tracing-subscriber` | 9.55s |

## Release binary size

| binary | size | previous |
|---|---:|---:|
| `phux` | 12.8 MiB | 12.9 MiB |
| `phux-mcp` | 2.1 MiB | 2.1 MiB |

## Dependency graph

- locked packages: **432** (previous: 432) — 11 workspace members, 48 direct deps
- duplicate versions: **32** (previous: 32)
- proc-macro crates: 33; build-script crates: 67

## Slowest tests (latest instrumented run, `f1a1306a1`)

| test | wall |
|---|---:|
| `phux-server::stress_resize_extremes::both_axes_shrink_storm_under_output_does_not_panic` | 846.454s |
| `phux-server::stress_output_extremes::multi_mb_no_newline_burst_does_not_panic` | 37.544s |
| `phux-server::stress_lifecycle_churn::attach_racing_pty_eof_does_not_panic` | 10.196s |
| `phux-server::stress_output_extremes::wide_combining_zwj_flood_does_not_panic` | 4.456s |
| `phux-server::stress_resize_extremes::resize_degenerate_viewports_do_not_panic` | 3.329s |
| `phux-server::stress_attach_churn::attach_detach_churn_keeps_pane_alive` | 0.464s |
| `phux-server::stress_output_extremes::control_char_flood_does_not_panic` | 0.441s |
| `phux-server::stress_lifecycle_churn::many_concurrent_clients_attach_detach_under_output` | 0.361s |
| `phux-server::stress_output_extremes::rapid_alt_screen_toggles_do_not_panic` | 0.358s |
| `phux-server::stress_spawn_kill::spawn_storm_then_kill_storm_does_not_panic` | 0.141s |

## Recent runs

| when | workflow | event | branch | result | wall | runner time |
|---|---|---|---|---|---:|---:|
| 2026-07-25 09:04 | stress | schedule | main | success | 18m05s | 18m01s |
| 2026-07-25 01:40 | ci | pull_request | release-please--branches--main-- | skipped | 2s | 0s |
| 2026-07-25 01:40 | conventional-commits | pull_request | release-please--branches--main-- | success | 19s | 14s |
| 2026-07-25 01:40 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-07-25 01:40 | conventional-commits | pull_request | release-please--branches--main-- | cancelled | 1s | 1s |
| 2026-07-25 01:40 | conventional-commits | pull_request | release-please--branches--main-- | success | 21s | 16s |
| 2026-07-25 01:39 | release-please | push | main | success | 39s | 35s |
| 2026-07-25 01:39 | ci | push | main | success | 18m24s | 24m18s |
| 2026-07-25 01:22 | conventional-commits | pull_request | feat/acknowledged-input | success | 25s | 21s |
| 2026-07-25 01:22 | ci | pull_request | feat/acknowledged-input | success | 17m14s | 22m23s |
| 2026-07-24 10:59 | ci | pull_request | release-please--branches--main-- | skipped | 2s | 0s |
| 2026-07-24 10:59 | conventional-commits | pull_request | release-please--branches--main-- | success | 5m04s | 4m45s |
| 2026-07-24 10:58 | stress | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-07-24 10:58 | ci | pull_request | release-please--branches--main-- | skipped | 10s | 0s |
| 2026-07-24 10:58 | conventional-commits | pull_request | release-please--branches--main-- | cancelled | 40s | 36s |
| 2026-07-24 10:58 | release-please | push | main | success | 51s | 43s |
| 2026-07-24 10:58 | ci | push | main | success | 17m33s | 21m19s |
| 2026-07-24 10:55 | conventional-commits | pull_request | adr-0052-connector-productizatio | success | 16s | 14s |
| 2026-07-24 10:55 | ci | pull_request | adr-0052-connector-productizatio | success | 2m49s | 3m50s |
| 2026-07-24 10:55 | conventional-commits | pull_request | feat/relay-alpn-dialer | success | 13s | 11s |
| 2026-07-24 10:55 | ci | pull_request | feat/relay-alpn-dialer | success | 18m24s | 22m51s |
| 2026-07-24 10:52 | conventional-commits | pull_request | feat/relay-alpn-dialer | success | 20s | 15s |
| 2026-07-24 10:52 | ci | pull_request | feat/relay-alpn-dialer | cancelled | 3m27s | 6m18s |
| 2026-07-24 09:25 | stress | schedule | main | success | 20m44s | 20m41s |
| 2026-07-23 09:28 | stress | schedule | main | success | 16m36s | 16m33s |
| 2026-07-22 20:10 | stress | pull_request | release-please--branches--main-- | skipped | 8s | 0s |
| 2026-07-22 20:10 | release-please | push | main | success | 7m36s | 19m14s |
| 2026-07-22 20:10 | observatory | push | main | success | 12m07s | 22m13s |
| 2026-07-22 20:10 | ci | push | main | success | 19m37s | 24m52s |
| 2026-07-22 19:50 | ci | pull_request | release-please--branches--main-- | success | 18m59s | 24m44s |

---

Query the raw store directly, e.g. every recorded ci run's wall time:

```sh
git fetch origin ci-metrics && git show origin/ci-metrics:runs/2026-07.ndjson \
  | jq -r 'select(.kind == "run" and .workflow == "ci") | [.created_at, .conclusion, .duration_s] | @tsv'
```
