# phux CI dashboard

Generated 2026-08-03T02:10:46Z by the ci-metrics workflow. Do not edit —
every table is re-rendered from `runs/*.ndjson` on each update.
Machine rollup: [`site/summary.json`](site/summary.json), rendered live at
<https://phux.phall.io/ci>.

## Workflows, last 30 days

| workflow | runs | success | median | p95 | runner minutes |
|---|---:|---:|---:|---:|---:|
| ci | 395 | 55% | 11m38s | 18m24s | 4420 |
| observatory | 23 | 91% | 12m23s | 12m56s | 533 |
| stress | 40 | 38% | 2m39s | 22m37s | 334 |
| release-please | 85 | 98% | 46s | 7m37s | 200 |
| conventional-commits | 359 | 79% | 16s | 24s | 72 |

## ci jobs, last 30 days

| job | runs | median queue | median wall | p95 wall |
|---|---:|---:|---:|---:|
| test | 388 | 2s | 11m22s | 17m30s |
| check | 387 | 2s | 2m36s | 5m05s |
| detect docs-only | 390 | 2s | 5s | 8s |

## Slowest ci steps (median, last 30 days)

| job | step | median | samples |
|---|---|---:|---:|
| test | tests (unit + e2e) | 11m25s | 12 |
| check | rust checks (fmt + clippy + doc + deny) | 3m00s | 15 |
| check | Run Swatinem/rust-cache@v2 | 16s | 16 |
| test | Run Swatinem/rust-cache@v2 | 14s | 16 |
| check | docs-check | 11s | 15 |
| test | agents smoke | 10s | 12 |
| check | runner disk headroom | 8s | 16 |
| test | runner disk headroom | 7s | 16 |
| check | Run DeterminateSystems/nix-installer-action@v22 | 6s | 16 |
| test | Run DeterminateSystems/nix-installer-action@v22 | 6s | 16 |

## Cargo phases inside the lanes (median, last 30 days)

| workflow / job | phase | median | samples |
|---|---|---:|---:|
| ci / check | clippy | 1m40s | 216 |
| ci / check | doc | 10s | 216 |
| ci / check | deny | 3s | 215 |
| ci / check | fmt | 2s | 222 |
| ci / test | unit | 12m12s | 200 |
| ci / test | e2e | 11s | 196 |
| ci / test | agents-smoke | 1s | 136 |
| observatory / timings | build-dev | 11m06s | 21 |
| observatory / timings | build-release | 5m23s | 22 |
| stress / stress | stress | 17m27s | 19 |

## Cache effectiveness (last 30 days)

| workflow / job | rust-cache hit rate | samples |
|---|---:|---:|
| ci / check | 44% | 230 |
| ci / test | 45% | 228 |
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

## Slowest tests (latest instrumented run, `2b1ae2986`)

| test | wall |
|---|---:|
| `phux-record::golden_cast::golden_cast_renders_gif_and_apng_and_frame_counts_agree` | 3.992s |
| `phux-server::terminal_actor::tests::resize_desync_then_both_shrink_does_not_overflow` | 3.959s |
| `phux-server::terminal_actor::tests::degenerate_resize_storm_does_not_panic_actor` | 1.458s |
| `phux-server::terminal_actor::tests::xtwinops_size_queries_answered_from_resized_geometry` | 1.457s |
| `phux-server::phux_3uv_acked_incremental::acked_incremental_converges_and_seq_is_monotonic` | 1.312s |
| `phux-server::agent_detect::a_plain_shell_pane_never_gets_an_agent_record` | 1.216s |
| `phux-server::agent_detect::an_identity_only_set_gets_its_state_filled_in_by_the_detector` | 1.116s |
| `phux-server::agent_detect::deleting_the_record_hands_it_back_to_the_detector` | 1.114s |
| `phux-server::server_idle_exit::connecting_disarms_the_idle_clock` | 1.062s |
| `phux-server::server_self_exit::server_without_clients_does_not_self_exit_on_seed_pane_death` | 1.013s |

## Recent runs

| when | workflow | event | branch | result | wall | runner time |
|---|---|---|---|---|---:|---:|
| 2026-08-03 02:10 | conventional-commits | pull_request | feat/ux-wave-11 | success | 28s | 18s |
| 2026-08-03 02:02 | conventional-commits | pull_request | feat/ux-wave-11 | success | 22s | 13s |
| 2026-08-03 02:02 | ci | pull_request | feat/ux-wave-11 | cancelled | 7m29s | 9m49s |
| 2026-08-03 01:30 | conventional-commits | pull_request | release-please--branches--main-- | success | 16s | 12s |
| 2026-08-03 01:30 | ci | pull_request | release-please--branches--main-- | skipped | 10s | 0s |
| 2026-08-03 01:30 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-08-03 01:30 | conventional-commits | pull_request | release-please--branches--main-- | cancelled | 2s | 1s |
| 2026-08-03 01:30 | conventional-commits | pull_request | release-please--branches--main-- | success | 18s | 14s |
| 2026-08-03 01:29 | release-please | push | main | success | 50s | 37s |
| 2026-08-03 01:29 | ci | push | main | success | 10m22s | 12m44s |
| 2026-08-03 01:19 | conventional-commits | pull_request | feat/ux-wave-10 | success | 28s | 18s |
| 2026-08-03 01:19 | ci | pull_request | feat/ux-wave-10 | success | 9m50s | 12m02s |
| 2026-08-03 00:53 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-08-03 00:53 | conventional-commits | pull_request | release-please--branches--main-- | success | 18s | 15s |
| 2026-08-03 00:52 | stress | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-08-03 00:52 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-08-03 00:52 | conventional-commits | pull_request | release-please--branches--main-- | success | 20s | 11s |
| 2026-08-03 00:52 | release-please | push | main | success | 55s | 42s |
| 2026-08-03 00:52 | ci | push | main | success | 9m36s | 11m53s |
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

---

Query the raw store directly, e.g. every recorded ci run's wall time:

```sh
git fetch origin ci-metrics && git show origin/ci-metrics:runs/2026-08.ndjson \
  | jq -r 'select(.kind == "run" and .workflow == "ci") | [.created_at, .conclusion, .duration_s] | @tsv'
```
