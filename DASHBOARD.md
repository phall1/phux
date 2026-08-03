# phux CI dashboard

Generated 2026-08-03T03:16:20Z by the ci-metrics workflow. Do not edit —
every table is re-rendered from `runs/*.ndjson` on each update.
Machine rollup: [`site/summary.json`](site/summary.json), rendered live at
<https://phux.phall.io/ci>.

## Workflows, last 30 days

| workflow | runs | success | median | p95 | runner minutes |
|---|---:|---:|---:|---:|---:|
| ci | 407 | 55% | 11m35s | 18m14s | 4510 |
| observatory | 24 | 92% | 12m23s | 12m56s | 559 |
| stress | 42 | 36% | 11s | 22m15s | 334 |
| release-please | 89 | 97% | 46s | 7m37s | 204 |
| conventional-commits | 369 | 79% | 16s | 24s | 74 |
| release | 1 | 100% | 8m11s | 8m11s | 20 |

## ci jobs, last 30 days

| job | runs | median queue | median wall | p95 wall |
|---|---:|---:|---:|---:|
| test | 400 | 2s | 11m17s | 17m30s |
| check | 399 | 2s | 2m35s | 5m05s |
| detect docs-only | 402 | 2s | 5s | 8s |

## Slowest ci steps (median, last 30 days)

| job | step | median | samples |
|---|---|---:|---:|
| test | tests (unit + e2e) | 11m06s | 14 |
| check | rust checks (fmt + clippy + doc + deny) | 1m23s | 15 |
| check | Run Swatinem/rust-cache@v2 | 19s | 15 |
| test | Run Swatinem/rust-cache@v2 | 15s | 15 |
| check | docs-check | 11s | 15 |
| test | agents smoke | 10s | 14 |
| check | runner disk headroom | 7s | 15 |
| test | runner disk headroom | 7s | 15 |
| check | Run DeterminateSystems/nix-installer-action@v22 | 6s | 15 |
| test | Run DeterminateSystems/nix-installer-action@v22 | 6s | 15 |

## Cargo phases inside the lanes (median, last 30 days)

| workflow / job | phase | median | samples |
|---|---|---:|---:|
| ci / check | clippy | 1m39s | 222 |
| ci / check | doc | 10s | 222 |
| ci / check | deny | 3s | 221 |
| ci / check | fmt | 2s | 228 |
| ci / test | unit | 12m10s | 206 |
| ci / test | e2e | 11s | 202 |
| ci / test | agents-smoke | 1s | 142 |
| observatory / timings | build-dev | 11m06s | 22 |
| observatory / timings | build-release | 5m24s | 23 |
| stress / stress | stress | 17m27s | 19 |

## Cache effectiveness (last 30 days)

| workflow / job | rust-cache hit rate | samples |
|---|---:|---:|
| ci / check | 44% | 236 |
| ci / test | 45% | 234 |
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

## Slowest tests (latest instrumented run, `4efd7354c`)

| test | wall |
|---|---:|
| `phux-server::terminal_actor::tests::resize_desync_then_both_shrink_does_not_overflow` | 4.015s |
| `phux-record::golden_cast::golden_cast_renders_gif_and_apng_and_frame_counts_agree` | 3.980s |
| `phux-server::terminal_actor::tests::xtwinops_size_queries_answered_from_resized_geometry` | 1.455s |
| `phux-server::phux_3uv_acked_incremental::acked_incremental_converges_and_seq_is_monotonic` | 1.312s |
| `phux-server::agent_detect::a_plain_shell_pane_never_gets_an_agent_record` | 1.215s |
| `phux-server::agent_detect::deleting_the_record_hands_it_back_to_the_detector` | 1.115s |
| `phux-server::agent_detect::an_identity_only_set_gets_its_state_filled_in_by_the_detector` | 1.114s |
| `phux-server::server_idle_exit::connecting_disarms_the_idle_clock` | 1.062s |
| `phux-server::server_self_exit::server_without_clients_does_not_self_exit_on_seed_pane_death` | 1.013s |
| `phux::config_plugin_actions::config_run_timeout_returns_125_and_json_timeout` | 1.008s |

## Recent runs

| when | workflow | event | branch | result | wall | runner time |
|---|---|---|---|---|---:|---:|
| 2026-08-03 03:13 | ci | pull_request | release-please--branches--main-- | skipped | 9s | 0s |
| 2026-08-03 03:13 | conventional-commits | pull_request | release-please--branches--main-- | success | 17s | 12s |
| 2026-08-03 03:13 | conventional-commits | pull_request | release-please--branches--main-- | success | 26s | 17s |
| 2026-08-03 03:13 | conventional-commits | pull_request | release-please--branches--main-- | cancelled | 2s | 1s |
| 2026-08-03 03:13 | ci | pull_request | release-please--branches--main-- | skipped | 8s | 0s |
| 2026-08-03 03:12 | release-please | push | main | success | 53s | 40s |
| 2026-08-03 03:08 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-08-03 03:08 | conventional-commits | pull_request | release-please--branches--main-- | success | 25s | 17s |
| 2026-08-03 03:07 | stress | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-08-03 03:07 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-08-03 03:07 | conventional-commits | pull_request | release-please--branches--main-- | success | 17s | 13s |
| 2026-08-03 03:07 | release-please | push | main | success | 57s | 44s |
| 2026-08-03 03:04 | release | workflow_dispatch | fix/release-zig-0.16-checksums | success | 8m11s | 20m15s |
| 2026-08-03 03:04 | conventional-commits | pull_request | fix/release-zig-0.16-checksums | success | 19s | 17s |
| 2026-08-03 03:04 | ci | pull_request | fix/release-zig-0.16-checksums | success | 11m36s | 13m49s |
| 2026-08-03 03:00 | conventional-commits | pull_request | feat/ux-wave-12 | success | 18s | 15s |
| 2026-08-03 03:00 | ci | pull_request | feat/ux-wave-12 | success | 12m17s | 14m54s |
| 2026-08-03 02:53 | stress | pull_request | release-please--branches--main-- | skipped | 2s | 0s |
| 2026-08-03 02:53 | release-please | push | main | failure | 1m37s | 2m07s |
| 2026-08-03 02:53 | observatory | push | main | success | 12m40s | 26m09s |
| 2026-08-03 02:53 | ci | push | main | success | 13m17s | 17m59s |
| 2026-08-03 02:52 | ci | pull_request | release-please--branches--main-- | success | 13m02s | 17m35s |
| 2026-08-03 02:22 | ci | pull_request | release-please--branches--main-- | skipped | 11s | 0s |
| 2026-08-03 02:22 | conventional-commits | pull_request | release-please--branches--main-- | success | 34s | 17s |
| 2026-08-03 02:21 | ci | pull_request | release-please--branches--main-- | skipped | 9s | 0s |
| 2026-08-03 02:21 | conventional-commits | pull_request | release-please--branches--main-- | cancelled | 28s | 17s |
| 2026-08-03 02:21 | conventional-commits | pull_request | release-please--branches--main-- | cancelled | 2s | 0s |
| 2026-08-03 02:21 | release-please | push | main | success | 48s | 41s |
| 2026-08-03 02:21 | ci | push | main | success | 10m14s | 12m40s |
| 2026-08-03 02:10 | conventional-commits | pull_request | feat/ux-wave-11 | success | 28s | 18s |

---

Query the raw store directly, e.g. every recorded ci run's wall time:

```sh
git fetch origin ci-metrics && git show origin/ci-metrics:runs/2026-08.ndjson \
  | jq -r 'select(.kind == "run" and .workflow == "ci") | [.created_at, .conclusion, .duration_s] | @tsv'
```
