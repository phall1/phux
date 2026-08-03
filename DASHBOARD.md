# phux CI dashboard

Generated 2026-08-03T00:52:54Z by the ci-metrics workflow. Do not edit —
every table is re-rendered from `runs/*.ndjson` on each update.
Machine rollup: [`site/summary.json`](site/summary.json), rendered live at
<https://phux.phall.io/ci>.

## Workflows, last 30 days

| workflow | runs | success | median | p95 | runner minutes |
|---|---:|---:|---:|---:|---:|
| ci | 388 | 56% | 11m58s | 18m24s | 4374 |
| observatory | 23 | 91% | 12m23s | 12m56s | 533 |
| stress | 40 | 38% | 2m39s | 22m37s | 334 |
| release-please | 83 | 98% | 46s | 7m37s | 199 |
| conventional-commits | 351 | 79% | 16s | 24s | 71 |

## ci jobs, last 30 days

| job | runs | median queue | median wall | p95 wall |
|---|---:|---:|---:|---:|
| test | 382 | 2s | 11m39s | 17m30s |
| check | 380 | 2s | 2m37s | 5m05s |
| detect docs-only | 383 | 2s | 5s | 8s |

## Slowest ci steps (median, last 30 days)

| job | step | median | samples |
|---|---|---:|---:|
| test | tests (unit + e2e) | 11m25s | 13 |
| check | rust checks (fmt + clippy + doc + deny) | 3m01s | 15 |
| check | Run Swatinem/rust-cache@v2 | 15s | 16 |
| test | Run Swatinem/rust-cache@v2 | 14s | 16 |
| check | docs-check | 11s | 15 |
| test | agents smoke | 11s | 13 |
| check | runner disk headroom | 8s | 16 |
| test | runner disk headroom | 7s | 16 |
| check | Run DeterminateSystems/nix-installer-action@v22 | 6s | 16 |
| test | Run DeterminateSystems/nix-installer-action@v22 | 6s | 16 |

## Cargo phases inside the lanes (median, last 30 days)

| workflow / job | phase | median | samples |
|---|---|---:|---:|
| ci / check | clippy | 1m47s | 212 |
| ci / check | doc | 10s | 212 |
| ci / check | deny | 3s | 211 |
| ci / check | fmt | 2s | 218 |
| ci / test | unit | 12m20s | 197 |
| ci / test | e2e | 11s | 193 |
| ci / test | agents-smoke | 1s | 133 |
| observatory / timings | build-dev | 11m06s | 21 |
| observatory / timings | build-release | 5m23s | 22 |
| stress / stress | stress | 17m27s | 19 |

## Cache effectiveness (last 30 days)

| workflow / job | rust-cache hit rate | samples |
|---|---:|---:|
| ci / check | 43% | 226 |
| ci / test | 44% | 224 |
| stress / stress | 16% | 19 |

## Cold build (observatory)

### dev: 11m28s (previous: 10m55s) — 537 units at `06682f512`

| slowest units | wall |
|---|---:|
| `libghostty-vt-sys build script (run)` | 176.25s |
| `phux-server lib (test)` | 108.56s |
| `phux bin "phux"` | 102.59s |
| `phux-client lib (test)` | 85.72s |
| `phux-server` | 83.16s |
| `phux bin "phux" (test)` | 58.34s |
| `phux-config` | 36.25s |
| `phux-server test "spawn_terminal" (test)` | 32.95s |

### release: 6m10s (previous: 6m12s) — 366 units at `06682f512`

| slowest units | wall |
|---|---:|
| `libghostty-vt-sys build script (run)` | 180.0s |
| `phux bin "phux"` | 136.49s |
| `phux-server` | 28.77s |
| `phux-config` | 24.35s |
| `phux-mcp bin "phux-mcp"` | 23.73s |
| `regex-automata` | 12.38s |
| `rustls` | 12.1s |
| `ring build script (run)` | 11.88s |

## Release binary size

| binary | size | previous |
|---|---:|---:|
| `phux` | 15.4 MiB | 15.4 MiB |
| `phux-mcp` | 2.1 MiB | 2.1 MiB |

## Dependency graph

- locked packages: **438** (previous: 438) — 13 workspace members, 52 direct deps
- duplicate versions: **32** (previous: 32)
- proc-macro crates: 33; build-script crates: 67

## Slowest tests (latest instrumented run, `13609b532`)

| test | wall |
|---|---:|
| `phux-record::golden_cast::golden_cast_renders_gif_and_apng_and_frame_counts_agree` | 3.987s |
| `phux-server::terminal_actor::tests::resize_desync_then_both_shrink_does_not_overflow` | 3.965s |
| `phux-server::terminal_actor::tests::xtwinops_size_queries_answered_from_resized_geometry` | 1.458s |
| `phux-server::phux_3uv_acked_incremental::acked_incremental_converges_and_seq_is_monotonic` | 1.312s |
| `phux-server::agent_detect::a_plain_shell_pane_never_gets_an_agent_record` | 1.216s |
| `phux-server::agent_detect::an_identity_only_set_gets_its_state_filled_in_by_the_detector` | 1.115s |
| `phux-server::agent_detect::deleting_the_record_hands_it_back_to_the_detector` | 1.114s |
| `phux-server::server_idle_exit::connecting_disarms_the_idle_clock` | 1.062s |
| `phux::config_plugin_actions::config_run_timeout_returns_125_and_json_timeout` | 1.014s |
| `phux-server::server_self_exit::server_without_clients_does_not_self_exit_on_seed_pane_death` | 1.013s |

## Recent runs

| when | workflow | event | branch | result | wall | runner time |
|---|---|---|---|---|---:|---:|
| 2026-08-03 00:52 | stress | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-08-03 00:52 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-08-03 00:38 | conventional-commits | pull_request | feat/ux-wave-9 | success | 15s | 12s |
| 2026-08-03 00:38 | ci | pull_request | feat/ux-wave-9 | success | 13m10s | 15m35s |
| 2026-08-03 00:30 | stress | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-08-03 00:29 | release-please | push | main | failure | 1m36s | 2m14s |
| 2026-08-03 00:29 | observatory | push | main | success | 12m34s | 25m58s |
| 2026-08-03 00:29 | ci | push | main | success | 12m54s | 17m39s |
| 2026-08-03 00:29 | ci | pull_request | release-please--branches--main-- | success | 12m45s | 17m14s |
| 2026-08-03 00:13 | ci | pull_request | release-please--branches--main-- | skipped | 2s | 0s |
| 2026-08-03 00:13 | conventional-commits | pull_request | release-please--branches--main-- | success | 14s | 12s |
| 2026-08-03 00:12 | ci | pull_request | release-please--branches--main-- | skipped | 2s | 0s |
| 2026-08-03 00:12 | conventional-commits | pull_request | release-please--branches--main-- | cancelled | 2s | 0s |
| 2026-08-03 00:12 | conventional-commits | pull_request | release-please--branches--main-- | success | 19s | 14s |
| 2026-08-03 00:12 | release-please | push | main | success | 58s | 46s |
| 2026-08-03 00:12 | observatory | push | main | success | 12m23s | 25m33s |
| 2026-08-03 00:12 | ci | push | main | success | 13m12s | 18m03s |
| 2026-08-02 23:36 | conventional-commits | pull_request | feat/ux-wave-8 | success | 26s | 16s |
| 2026-08-02 23:36 | ci | pull_request | feat/ux-wave-8 | success | 13m01s | 17m07s |
| 2026-08-02 23:36 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-08-02 23:36 | conventional-commits | pull_request | release-please--branches--main-- | success | 16s | 13s |
| 2026-08-02 23:35 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-08-02 23:35 | conventional-commits | pull_request | release-please--branches--main-- | success | 14s | 10s |
| 2026-08-02 23:35 | conventional-commits | pull_request | release-please--branches--main-- | cancelled | 2s | 0s |
| 2026-08-02 23:35 | release-please | push | main | success | 42s | 38s |
| 2026-08-02 23:35 | observatory | push | main | success | 12m41s | 25m50s |
| 2026-08-02 23:35 | ci | push | main | success | 12m56s | 18m09s |
| 2026-08-02 23:30 | conventional-commits | pull_request | feat/ux-wave-8 | success | 20s | 10s |
| 2026-08-02 23:30 | ci | pull_request | feat/ux-wave-8 | cancelled | 5m46s | 9m47s |
| 2026-08-02 23:30 | conventional-commits | pull_request | chore/zig-0.16-libghostty-bump | success | 17s | 11s |

---

Query the raw store directly, e.g. every recorded ci run's wall time:

```sh
git fetch origin ci-metrics && git show origin/ci-metrics:runs/2026-08.ndjson \
  | jq -r 'select(.kind == "run" and .workflow == "ci") | [.created_at, .conclusion, .duration_s] | @tsv'
```
