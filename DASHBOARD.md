# phux CI dashboard

Generated 2026-08-03T04:57:36Z by the ci-metrics workflow. Do not edit —
every table is re-rendered from `runs/*.ndjson` on each update.
Machine rollup: [`site/summary.json`](site/summary.json), rendered live at
<https://phux.phall.io/ci>.

## Workflows, last 30 days

| workflow | runs | success | median | p95 | runner minutes |
|---|---:|---:|---:|---:|---:|
| ci | 426 | 54% | 11m16s | 18m14s | 4657 |
| observatory | 24 | 92% | 12m23s | 12m56s | 559 |
| stress | 42 | 36% | 11s | 22m15s | 334 |
| release-please | 91 | 97% | 47s | 7m37s | 206 |
| conventional-commits | 389 | 80% | 16s | 25s | 79 |
| release | 3 | 100% | 8m02s | 8m02s | 61 |

## ci jobs, last 30 days

| job | runs | median queue | median wall | p95 wall |
|---|---:|---:|---:|---:|
| test | 419 | 2s | 10m57s | 17m30s |
| check | 418 | 2s | 2m33s | 5m05s |
| detect docs-only | 421 | 2s | 5s | 8s |

## Slowest ci steps (median, last 30 days)

| job | step | median | samples |
|---|---|---:|---:|
| test | tests (unit + e2e) | 9m18s | 10 |
| check | rust checks (fmt + clippy + doc + deny) | 1m18s | 13 |
| test | Run Swatinem/rust-cache@v2 | 20s | 13 |
| check | Run Swatinem/rust-cache@v2 | 19s | 14 |
| check | docs-check | 11s | 13 |
| test | agents smoke | 11s | 10 |
| check | runner disk headroom | 7s | 14 |
| test | Run DeterminateSystems/nix-installer-action@v22 | 7s | 13 |
| test | runner disk headroom | 7s | 13 |
| check | Run DeterminateSystems/nix-installer-action@v22 | 6s | 14 |

## Cargo phases inside the lanes (median, last 30 days)

| workflow / job | phase | median | samples |
|---|---|---:|---:|
| ci / check | clippy | 1m38s | 232 |
| ci / check | doc | 10s | 232 |
| ci / check | deny | 3s | 230 |
| ci / check | fmt | 2s | 238 |
| ci / test | unit | 12m04s | 213 |
| ci / test | e2e | 12s | 209 |
| ci / test | agents-smoke | 1s | 149 |
| observatory / timings | build-dev | 11m06s | 22 |
| observatory / timings | build-release | 5m24s | 23 |
| stress / stress | stress | 17m27s | 19 |

## Cache effectiveness (last 30 days)

| workflow / job | rust-cache hit rate | samples |
|---|---:|---:|
| ci / check | 46% | 246 |
| ci / test | 46% | 244 |
| stress / stress | 16% | 19 |

## Cold build (observatory)

### dev: 11m30s (previous: 11m28s) — 538 units at `55956b722`

| slowest units | wall |
|---|---:|
| `libghostty-vt-sys build script (run)` | 188.21s |
| `phux-server lib (test)` | 109.98s |
| `phux bin "phux"` | 102.67s |
| `phux-server` | 86.28s |
| `phux-client lib (test)` | 85.76s |
| `phux bin "phux" (test)` | 57.57s |
| `phux-config` | 37.86s |
| `phux-server test "command_dispatch" (test)` | 32.0s |

### release: 6m12s (previous: 6m10s) — 366 units at `55956b722`

| slowest units | wall |
|---|---:|
| `libghostty-vt-sys build script (run)` | 181.07s |
| `phux bin "phux"` | 139.54s |
| `phux-server` | 26.64s |
| `phux-config` | 25.04s |
| `phux-mcp bin "phux-mcp"` | 23.36s |
| `rustls` | 15.03s |
| `regex-automata` | 12.42s |
| `phux-client` | 12.31s |

## Release binary size

| binary | size | previous |
|---|---:|---:|
| `phux` | 15.5 MiB | 15.4 MiB |
| `phux-mcp` | 2.1 MiB | 2.1 MiB |

## Dependency graph

- locked packages: **438** (previous: 438) — 13 workspace members, 52 direct deps
- duplicate versions: **32** (previous: 32)
- proc-macro crates: 33; build-script crates: 67

## Slowest tests (latest instrumented run, `5f2f7b555`)

| test | wall |
|---|---:|
| `phux-server::terminal_actor::tests::resize_desync_then_both_shrink_does_not_overflow` | 3.982s |
| `phux-record::golden_cast::golden_cast_renders_gif_and_apng_and_frame_counts_agree` | 3.979s |
| `phux-server::terminal_actor::tests::xtwinops_size_queries_answered_from_resized_geometry` | 1.459s |
| `phux-server::phux_3uv_acked_incremental::acked_incremental_converges_and_seq_is_monotonic` | 1.312s |
| `phux-server::agent_detect::a_plain_shell_pane_never_gets_an_agent_record` | 1.215s |
| `phux-server::agent_detect::deleting_the_record_hands_it_back_to_the_detector` | 1.115s |
| `phux-server::agent_detect::an_identity_only_set_gets_its_state_filled_in_by_the_detector` | 1.114s |
| `phux-server::server_idle_exit::connecting_disarms_the_idle_clock` | 1.064s |
| `phux-server::server_self_exit::server_without_clients_does_not_self_exit_on_seed_pane_death` | 1.013s |
| `phux::config_plugin_actions::config_run_timeout_returns_125_and_json_timeout` | 1.011s |

## Recent runs

| when | workflow | event | branch | result | wall | runner time |
|---|---|---|---|---|---:|---:|
| 2026-08-03 04:56 | conventional-commits | pull_request | feat/ux-followups | success | 18s | 14s |
| 2026-08-03 04:53 | conventional-commits | pull_request | feat/negotiated-libghostty-codec | success | 26s | 17s |
| 2026-08-03 04:51 | ci | pull_request | feat/negotiated-libghostty-codec | skipped | 0s | 0s |
| 2026-08-03 04:51 | conventional-commits | pull_request | feat/negotiated-libghostty-codec | success | 20s | 15s |
| 2026-08-03 04:47 | conventional-commits | pull_request | perf/ci-extract-server-testkit | success | 23s | 13s |
| 2026-08-03 04:47 | ci | pull_request | perf/ci-extract-server-testkit | success | 9m23s | 14m09s |
| 2026-08-03 04:42 | ci | pull_request | feat/negotiated-libghostty-codec | skipped | 1s | 0s |
| 2026-08-03 04:42 | conventional-commits | pull_request | feat/negotiated-libghostty-codec | success | 17s | 13s |
| 2026-08-03 04:08 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-08-03 04:08 | conventional-commits | pull_request | release-please--branches--main-- | success | 20s | 12s |
| 2026-08-03 04:08 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-08-03 04:08 | conventional-commits | pull_request | release-please--branches--main-- | cancelled | 1s | 1s |
| 2026-08-03 04:08 | conventional-commits | pull_request | release-please--branches--main-- | success | 18s | 14s |
| 2026-08-03 04:07 | release-please | push | main | success | 48s | 43s |
| 2026-08-03 04:07 | ci | push | main | success | 10m39s | 13m11s |
| 2026-08-03 03:57 | conventional-commits | pull_request | feat/ux-wave-13 | success | 14s | 11s |
| 2026-08-03 03:57 | ci | pull_request | feat/ux-wave-13 | success | 10m00s | 12m18s |
| 2026-08-03 03:55 | conventional-commits | pull_request | feat/negotiated-libghostty-codec | success | 19s | 14s |
| 2026-08-03 03:55 | ci | pull_request | feat/negotiated-libghostty-codec | cancelled | 26m48s | 31m22s |
| 2026-08-03 03:44 | ci | pull_request | feat/negotiated-libghostty-codec | cancelled | 11m23s | 15m56s |
| 2026-08-03 03:43 | ci | pull_request | feat/negotiated-libghostty-codec | skipped | 1s | 0s |
| 2026-08-03 03:43 | conventional-commits | pull_request | feat/negotiated-libghostty-codec | success | 18s | 16s |
| 2026-08-03 03:36 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-08-03 03:36 | conventional-commits | pull_request | release-please--branches--main-- | success | 15s | 12s |
| 2026-08-03 03:36 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-08-03 03:36 | conventional-commits | pull_request | release-please--branches--main-- | cancelled | 2s | 0s |
| 2026-08-03 03:36 | conventional-commits | pull_request | release-please--branches--main-- | success | 15s | 10s |
| 2026-08-03 03:35 | release-please | push | main | success | 48s | 36s |
| 2026-08-03 03:35 | ci | push | main | success | 9m46s | 12m12s |
| 2026-08-03 03:24 | conventional-commits | pull_request | fix/release-portability-gate | success | 14s | 11s |

---

Query the raw store directly, e.g. every recorded ci run's wall time:

```sh
git fetch origin ci-metrics && git show origin/ci-metrics:runs/2026-08.ndjson \
  | jq -r 'select(.kind == "run" and .workflow == "ci") | [.created_at, .conclusion, .duration_s] | @tsv'
```
