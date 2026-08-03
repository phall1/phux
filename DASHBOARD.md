# phux CI dashboard

Generated 2026-08-03T04:18:34Z by the ci-metrics workflow. Do not edit —
every table is re-rendered from `runs/*.ndjson` on each update.
Machine rollup: [`site/summary.json`](site/summary.json), rendered live at
<https://phux.phall.io/ci>.

## Workflows, last 30 days

| workflow | runs | success | median | p95 | runner minutes |
|---|---:|---:|---:|---:|---:|
| ci | 422 | 55% | 11m19s | 18m11s | 4612 |
| observatory | 24 | 92% | 12m23s | 12m56s | 559 |
| stress | 42 | 36% | 11s | 22m15s | 334 |
| release-please | 91 | 97% | 47s | 7m37s | 206 |
| conventional-commits | 384 | 79% | 16s | 24s | 77 |
| release | 3 | 100% | 8m02s | 8m02s | 61 |

## ci jobs, last 30 days

| job | runs | median queue | median wall | p95 wall |
|---|---:|---:|---:|---:|
| test | 415 | 2s | 10m58s | 17m24s |
| check | 414 | 2s | 2m33s | 5m03s |
| detect docs-only | 417 | 2s | 5s | 8s |

## Slowest ci steps (median, last 30 days)

| job | step | median | samples |
|---|---|---:|---:|
| test | tests (unit + e2e) | 9m26s | 12 |
| check | rust checks (fmt + clippy + doc + deny) | 1m17s | 12 |
| check | Run Swatinem/rust-cache@v2 | 20s | 13 |
| test | Run Swatinem/rust-cache@v2 | 20s | 14 |
| check | docs-check | 11s | 12 |
| test | agents smoke | 11s | 12 |
| check | runner disk headroom | 8s | 13 |
| check | Run DeterminateSystems/nix-installer-action@v22 | 7s | 13 |
| test | Run DeterminateSystems/nix-installer-action@v22 | 7s | 14 |
| test | runner disk headroom | 7s | 14 |

## Cargo phases inside the lanes (median, last 30 days)

| workflow / job | phase | median | samples |
|---|---|---:|---:|
| ci / check | clippy | 1m35s | 230 |
| ci / check | doc | 10s | 230 |
| ci / check | deny | 3s | 228 |
| ci / check | fmt | 2s | 236 |
| ci / test | unit | 12m04s | 212 |
| ci / test | e2e | 11s | 208 |
| ci / test | agents-smoke | 1s | 148 |
| observatory / timings | build-dev | 11m06s | 22 |
| observatory / timings | build-release | 5m24s | 23 |
| stress / stress | stress | 17m27s | 19 |

## Cache effectiveness (last 30 days)

| workflow / job | rust-cache hit rate | samples |
|---|---:|---:|
| ci / check | 46% | 244 |
| ci / test | 46% | 242 |
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

## Slowest tests (latest instrumented run, `86c8ac725`)

| test | wall |
|---|---:|
| `phux-record::golden_cast::golden_cast_renders_gif_and_apng_and_frame_counts_agree` | 3.997s |
| `phux-server::terminal_actor::tests::resize_desync_then_both_shrink_does_not_overflow` | 3.985s |
| `phux-server::terminal_actor::tests::xtwinops_size_queries_answered_from_resized_geometry` | 1.458s |
| `phux-server::phux_3uv_acked_incremental::acked_incremental_converges_and_seq_is_monotonic` | 1.314s |
| `phux-server::agent_detect::a_plain_shell_pane_never_gets_an_agent_record` | 1.217s |
| `phux-server::server_idle_exit::connecting_disarms_the_idle_clock` | 1.183s |
| `phux-server::agent_detect::an_identity_only_set_gets_its_state_filled_in_by_the_detector` | 1.118s |
| `phux-server::agent_detect::deleting_the_record_hands_it_back_to_the_detector` | 1.115s |
| `phux-server::server_self_exit::server_without_clients_does_not_self_exit_on_seed_pane_death` | 1.013s |
| `phux::config_plugin_actions::config_run_timeout_returns_125_and_json_timeout` | 1.010s |

## Recent runs

| when | workflow | event | branch | result | wall | runner time |
|---|---|---|---|---|---:|---:|
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
| 2026-08-03 03:24 | ci | pull_request | fix/release-portability-gate | success | 11m27s | 13m26s |
| 2026-08-03 03:22 | release | workflow_dispatch | main | success | 8m02s | 20m01s |
| 2026-08-03 03:21 | conventional-commits | pull_request | feat/negotiated-libghostty-codec | success | 21s | 17s |
| 2026-08-03 03:20 | ci | pull_request | feat/negotiated-libghostty-codec | skipped | 1s | 0s |
| 2026-08-03 03:20 | conventional-commits | pull_request | feat/negotiated-libghostty-codec | success | 19s | 15s |
| 2026-08-03 03:18 | conventional-commits | pull_request | fix/release-portability-gate | success | 18s | 15s |
| 2026-08-03 03:18 | ci | pull_request | fix/release-portability-gate | cancelled | 5m47s | 8m05s |
| 2026-08-03 03:17 | conventional-commits | pull_request | feat/negotiated-libghostty-codec | success | 25s | 15s |
| 2026-08-03 03:17 | conventional-commits | pull_request | feat/negotiated-libghostty-codec | success | 16s | 13s |

---

Query the raw store directly, e.g. every recorded ci run's wall time:

```sh
git fetch origin ci-metrics && git show origin/ci-metrics:runs/2026-08.ndjson \
  | jq -r 'select(.kind == "run" and .workflow == "ci") | [.created_at, .conclusion, .duration_s] | @tsv'
```
