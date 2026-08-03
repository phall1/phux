# phux CI dashboard

Generated 2026-08-03T05:08:34Z by the ci-metrics workflow. Do not edit —
every table is re-rendered from `runs/*.ndjson` on each update.
Machine rollup: [`site/summary.json`](site/summary.json), rendered live at
<https://phux.phall.io/ci>.

## Workflows, last 30 days

| workflow | runs | success | median | p95 | runner minutes |
|---|---:|---:|---:|---:|---:|
| ci | 432 | 54% | 11m08s | 18m14s | 4684 |
| observatory | 24 | 92% | 12m23s | 12m56s | 559 |
| stress | 42 | 36% | 11s | 22m15s | 334 |
| release-please | 93 | 97% | 47s | 7m37s | 207 |
| conventional-commits | 393 | 80% | 16s | 25s | 79 |
| release | 3 | 100% | 8m02s | 8m02s | 61 |

## ci jobs, last 30 days

| job | runs | median queue | median wall | p95 wall |
|---|---:|---:|---:|---:|
| test | 423 | 2s | 10m52s | 17m24s |
| check | 424 | 2s | 2m32s | 5m03s |
| detect docs-only | 427 | 2s | 5s | 8s |

## Slowest ci steps (median, last 30 days)

| job | step | median | samples |
|---|---|---:|---:|
| test | tests (unit + e2e) | 8m31s | 8 |
| check | rust checks (fmt + clippy + doc + deny) | 1m17s | 10 |
| test | Run Swatinem/rust-cache@v2 | 20s | 11 |
| check | Run Swatinem/rust-cache@v2 | 19s | 11 |
| check | docs-check | 11s | 10 |
| test | agents smoke | 11s | 8 |
| check | Run DeterminateSystems/nix-installer-action@v22 | 7s | 11 |
| check | runner disk headroom | 7s | 11 |
| test | Run DeterminateSystems/nix-installer-action@v22 | 7s | 11 |
| test | runner disk headroom | 7s | 11 |

## Cargo phases inside the lanes (median, last 30 days)

| workflow / job | phase | median | samples |
|---|---|---:|---:|
| ci / check | clippy | 1m38s | 234 |
| ci / check | doc | 10s | 234 |
| ci / check | deny | 3s | 232 |
| ci / check | fmt | 2s | 240 |
| ci / test | unit | 12m04s | 215 |
| ci / test | e2e | 12s | 211 |
| ci / test | agents-smoke | 1s | 151 |
| observatory / timings | build-dev | 11m06s | 22 |
| observatory / timings | build-release | 5m24s | 23 |
| stress / stress | stress | 17m27s | 19 |

## Cache effectiveness (last 30 days)

| workflow / job | rust-cache hit rate | samples |
|---|---:|---:|
| ci / check | 46% | 248 |
| ci / test | 46% | 246 |
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

## Slowest tests (latest instrumented run, `15cea5a7f`)

| test | wall |
|---|---:|
| `phux-record::golden_cast::golden_cast_renders_gif_and_apng_and_frame_counts_agree` | 3.997s |
| `phux-server::terminal_actor::tests::resize_desync_then_both_shrink_does_not_overflow` | 3.983s |
| `phux-server::terminal_actor::tests::xtwinops_size_queries_answered_from_resized_geometry` | 1.456s |
| `phux-server::phux_3uv_acked_incremental::acked_incremental_converges_and_seq_is_monotonic` | 1.313s |
| `phux-server::agent_detect::a_plain_shell_pane_never_gets_an_agent_record` | 1.216s |
| `phux-server::phux_0q8_no_double_emit::live_output_is_delivered_exactly_once` | 1.116s |
| `phux-server::agent_detect::deleting_the_record_hands_it_back_to_the_detector` | 1.115s |
| `phux-server::agent_detect::an_identity_only_set_gets_its_state_filled_in_by_the_detector` | 1.114s |
| `phux-server::server_idle_exit::connecting_disarms_the_idle_clock` | 1.014s |
| `phux-server::server_self_exit::server_without_clients_does_not_self_exit_on_seed_pane_death` | 1.013s |

## Recent runs

| when | workflow | event | branch | result | wall | runner time |
|---|---|---|---|---|---:|---:|
| 2026-08-03 05:08 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-08-03 05:07 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-08-03 05:07 | conventional-commits | pull_request | release-please--branches--main-- | success | 22s | 19s |
| 2026-08-03 05:07 | release-please | push | main | success | 1m05s | 47s |
| 2026-08-03 04:59 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-08-03 04:59 | conventional-commits | pull_request | release-please--branches--main-- | success | 20s | 11s |
| 2026-08-03 04:58 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-08-03 04:58 | conventional-commits | pull_request | release-please--branches--main-- | cancelled | 2s | 1s |
| 2026-08-03 04:58 | conventional-commits | pull_request | release-please--branches--main-- | success | 21s | 17s |
| 2026-08-03 04:58 | release-please | push | main | success | 54s | 47s |
| 2026-08-03 04:58 | ci | push | main | success | 9m36s | 14m13s |
| 2026-08-03 04:56 | conventional-commits | pull_request | feat/ux-followups | success | 18s | 14s |
| 2026-08-03 04:56 | ci | pull_request | feat/ux-followups | success | 10m24s | 12m39s |
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

---

Query the raw store directly, e.g. every recorded ci run's wall time:

```sh
git fetch origin ci-metrics && git show origin/ci-metrics:runs/2026-08.ndjson \
  | jq -r 'select(.kind == "run" and .workflow == "ci") | [.created_at, .conclusion, .duration_s] | @tsv'
```
