# phux CI dashboard

Generated 2026-08-02T23:44:37Z by the ci-metrics workflow. Do not edit —
every table is re-rendered from `runs/*.ndjson` on each update.
Machine rollup: [`site/summary.json`](site/summary.json), rendered live at
<https://phux.phall.io/ci>.

## Workflows, last 30 days

| workflow | runs | success | median | p95 | runner minutes |
|---|---:|---:|---:|---:|---:|
| ci | 379 | 55% | 11m50s | 18m24s | 4270 |
| observatory | 20 | 90% | 12m07s | 13m00s | 456 |
| stress | 38 | 39% | 4m27s | 22m37s | 334 |
| release-please | 81 | 99% | 45s | 7m42s | 196 |
| conventional-commits | 347 | 79% | 16s | 24s | 70 |

## ci jobs, last 30 days

| job | runs | median queue | median wall | p95 wall |
|---|---:|---:|---:|---:|
| test | 373 | 2s | 11m30s | 17m34s |
| check | 371 | 2s | 2m37s | 5m05s |
| detect docs-only | 374 | 2s | 5s | 8s |

## Slowest ci steps (median, last 30 days)

| job | step | median | samples |
|---|---|---:|---:|
| test | tests (unit + e2e) | 10m12s | 13 |
| check | rust checks (fmt + clippy + doc + deny) | 2m52s | 14 |
| check | Run Swatinem/rust-cache@v2 | 16s | 15 |
| test | Run Swatinem/rust-cache@v2 | 15s | 16 |
| test | agents smoke | 11s | 13 |
| check | docs-check | 10s | 14 |
| check | runner disk headroom | 8s | 15 |
| test | runner disk headroom | 8s | 16 |
| test | Run DeterminateSystems/nix-installer-action@v22 | 7s | 16 |
| check | Run DeterminateSystems/nix-installer-action@v22 | 6s | 15 |

## Cargo phases inside the lanes (median, last 30 days)

| workflow / job | phase | median | samples |
|---|---|---:|---:|
| ci / check | clippy | 1m40s | 206 |
| ci / check | doc | 10s | 206 |
| ci / check | deny | 3s | 205 |
| ci / check | fmt | 2s | 212 |
| ci / test | unit | 12m23s | 191 |
| ci / test | e2e | 11s | 187 |
| ci / test | agents-smoke | 1s | 127 |
| observatory / timings | build-dev | 11m02s | 18 |
| observatory / timings | build-release | 5m13s | 19 |
| stress / stress | stress | 17m27s | 19 |

## Cache effectiveness (last 30 days)

| workflow / job | rust-cache hit rate | samples |
|---|---:|---:|
| ci / check | 44% | 220 |
| ci / test | 45% | 218 |
| stress / stress | 16% | 19 |

## Cold build (observatory)

### dev: 10m39s (previous: 8m27s) — 537 units at `e9669163f`

| slowest units | wall |
|---|---:|
| `libghostty-vt-sys build script (run)` | 119.43s |
| `phux bin "phux"` | 92.05s |
| `phux-client lib (test)` | 80.54s |
| `phux-server` | 78.87s |
| `phux-server lib (test)` | 56.93s |
| `phux bin "phux" (test)` | 46.33s |
| `rustls` | 44.92s |
| `phux-config` | 34.94s |

### release: 4m43s (previous: 5m38s) — 366 units at `e9669163f`

| slowest units | wall |
|---|---:|
| `phux bin "phux"` | 120.27s |
| `libghostty-vt-sys build script (run)` | 118.32s |
| `phux-server` | 22.53s |
| `phux-config` | 21.72s |
| `phux-mcp bin "phux-mcp"` | 19.72s |
| `clap_builder` | 13.7s |
| `regex-automata` | 13.46s |
| `rustls` | 12.44s |

## Release binary size

| binary | size | previous |
|---|---:|---:|
| `phux` | 14.9 MiB | 14.8 MiB |
| `phux-mcp` | 2.1 MiB | 2.1 MiB |

## Dependency graph

- locked packages: **438** (previous: 438) — 13 workspace members, 52 direct deps
- duplicate versions: **32** (previous: 32)
- proc-macro crates: 33; build-script crates: 67

## Slowest tests (latest instrumented run, `29965354c`)

| test | wall |
|---|---:|
| `phux-record::golden_cast::golden_cast_renders_gif_and_apng_and_frame_counts_agree` | 4.003s |
| `phux-server::terminal_actor::tests::resize_desync_then_both_shrink_does_not_overflow` | 3.967s |
| `phux-server::agent_detect::a_plain_shell_pane_never_gets_an_agent_record` | 1.758s |
| `phux-server::terminal_actor::tests::xtwinops_size_queries_answered_from_resized_geometry` | 1.353s |
| `phux-server::phux_3uv_acked_incremental::acked_incremental_converges_and_seq_is_monotonic` | 1.313s |
| `phux-server::agent_detect::an_identity_only_set_gets_its_state_filled_in_by_the_detector` | 1.117s |
| `phux-server::agent_detect::deleting_the_record_hands_it_back_to_the_detector` | 1.115s |
| `phux-server::server_idle_exit::connecting_disarms_the_idle_clock` | 1.064s |
| `phux-server::server_self_exit::server_without_clients_does_not_self_exit_on_seed_pane_death` | 1.014s |
| `phux::config_plugin_actions::config_run_timeout_returns_125_and_json_timeout` | 1.011s |

## Recent runs

| when | workflow | event | branch | result | wall | runner time |
|---|---|---|---|---|---:|---:|
| 2026-08-02 23:36 | conventional-commits | pull_request | feat/ux-wave-8 | success | 26s | 16s |
| 2026-08-02 23:36 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-08-02 23:36 | conventional-commits | pull_request | release-please--branches--main-- | success | 16s | 13s |
| 2026-08-02 23:35 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-08-02 23:35 | conventional-commits | pull_request | release-please--branches--main-- | success | 14s | 10s |
| 2026-08-02 23:35 | conventional-commits | pull_request | release-please--branches--main-- | cancelled | 2s | 0s |
| 2026-08-02 23:35 | release-please | push | main | success | 42s | 38s |
| 2026-08-02 23:30 | conventional-commits | pull_request | feat/ux-wave-8 | success | 20s | 10s |
| 2026-08-02 23:30 | ci | pull_request | feat/ux-wave-8 | cancelled | 5m46s | 9m47s |
| 2026-08-02 23:30 | conventional-commits | pull_request | chore/zig-0.16-libghostty-bump | success | 17s | 11s |
| 2026-08-02 23:30 | conventional-commits | pull_request | chore/zig-0.16-libghostty-bump | cancelled | 15s | 4s |
| 2026-08-02 23:30 | ci | pull_request | chore/zig-0.16-libghostty-bump | success | 14m05s | 18m53s |
| 2026-08-02 23:24 | conventional-commits | pull_request | chore/zig-0.16-libghostty-bump | success | 19s | 16s |
| 2026-08-02 23:24 | ci | pull_request | chore/zig-0.16-libghostty-bump | cancelled | 6m40s | 11m18s |
| 2026-08-02 22:30 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-08-02 22:30 | conventional-commits | pull_request | release-please--branches--main-- | success | 15s | 12s |
| 2026-08-02 22:30 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-08-02 22:30 | conventional-commits | pull_request | release-please--branches--main-- | success | 17s | 13s |
| 2026-08-02 22:30 | conventional-commits | pull_request | release-please--branches--main-- | cancelled | 2s | 0s |
| 2026-08-02 22:29 | release-please | push | main | success | 47s | 42s |
| 2026-08-02 22:29 | ci | push | main | success | 10m07s | 12m27s |
| 2026-08-02 22:19 | conventional-commits | pull_request | feat/ux-wave-7 | success | 23s | 14s |
| 2026-08-02 22:19 | ci | pull_request | feat/ux-wave-7 | success | 10m16s | 12m48s |
| 2026-08-02 21:55 | conventional-commits | pull_request | feat/bootstrap-chunk-params | success | 18s | 13s |
| 2026-08-02 21:55 | ci | pull_request | feat/bootstrap-chunk-params | failure | 2m13s | 3m37s |
| 2026-08-02 21:55 | conventional-commits | pull_request | feat/bootstrap-chunk-params | cancelled | 12s | 3s |
| 2026-08-02 21:55 | ci | pull_request | feat/bootstrap-chunk-params | cancelled | 12s | 8s |
| 2026-08-02 21:45 | ci | pull_request | release-please--branches--main-- | skipped | 2s | 0s |
| 2026-08-02 21:45 | conventional-commits | pull_request | release-please--branches--main-- | success | 22s | 18s |
| 2026-08-02 21:45 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |

---

Query the raw store directly, e.g. every recorded ci run's wall time:

```sh
git fetch origin ci-metrics && git show origin/ci-metrics:runs/2026-08.ndjson \
  | jq -r 'select(.kind == "run" and .workflow == "ci") | [.created_at, .conclusion, .duration_s] | @tsv'
```
