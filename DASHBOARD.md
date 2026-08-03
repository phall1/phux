# phux CI dashboard

Generated 2026-08-03T05:40:57Z by the ci-metrics workflow. Do not edit —
every table is re-rendered from `runs/*.ndjson` on each update.
Machine rollup: [`site/summary.json`](site/summary.json), rendered live at
<https://phux.phall.io/ci>.

## Workflows, last 30 days

| workflow | runs | success | median | p95 | runner minutes |
|---|---:|---:|---:|---:|---:|
| ci | 436 | 54% | 11m08s | 18m24s | 4758 |
| observatory | 25 | 92% | 12m23s | 12m56s | 582 |
| stress | 43 | 35% | 11s | 22m15s | 334 |
| release-please | 93 | 97% | 47s | 7m37s | 207 |
| conventional-commits | 395 | 80% | 16s | 25s | 80 |
| release | 3 | 100% | 8m02s | 8m02s | 61 |

## ci jobs, last 30 days

| job | runs | median queue | median wall | p95 wall |
|---|---:|---:|---:|---:|
| test | 427 | 2s | 10m52s | 17m30s |
| check | 428 | 2s | 2m33s | 5m05s |
| detect docs-only | 431 | 2s | 5s | 8s |

## Slowest ci steps (median, last 30 days)

| job | step | median | samples |
|---|---|---:|---:|
| test | tests (unit + e2e) | 8m45s | 9 |
| check | rust checks (fmt + clippy + doc + deny) | 1m18s | 13 |
| check | Run Swatinem/rust-cache@v2 | 16s | 14 |
| test | Run Swatinem/rust-cache@v2 | 15s | 14 |
| check | docs-check | 11s | 13 |
| test | agents smoke | 11s | 9 |
| check | Run DeterminateSystems/nix-installer-action@v22 | 7s | 14 |
| test | Run DeterminateSystems/nix-installer-action@v22 | 7s | 14 |
| test | runner disk headroom | 7s | 14 |
| check | runner disk headroom | 6s | 14 |

## Cargo phases inside the lanes (median, last 30 days)

| workflow / job | phase | median | samples |
|---|---|---:|---:|
| ci / check | clippy | 1m38s | 238 |
| ci / check | doc | 10s | 238 |
| ci / check | deny | 3s | 236 |
| ci / check | fmt | 2s | 244 |
| ci / test | unit | 12m02s | 217 |
| ci / test | e2e | 12s | 213 |
| ci / test | agents-smoke | 1s | 153 |
| observatory / timings | build-dev | 11m06s | 23 |
| observatory / timings | build-release | 5m24s | 24 |
| stress / stress | stress | 17m27s | 19 |

## Cache effectiveness (last 30 days)

| workflow / job | rust-cache hit rate | samples |
|---|---:|---:|
| ci / check | 45% | 252 |
| ci / test | 45% | 251 |
| stress / stress | 16% | 19 |

## Cold build (observatory)

### dev: 8m09s (previous: 11m30s) — 541 units at `15cea5a7f`

| slowest units | wall |
|---|---:|
| `libghostty-vt-sys build script (run)` | 180.23s |
| `phux bin "phux"` | 103.52s |
| `phux-client lib (test)` | 86.19s |
| `phux-server` | 82.17s |
| `phux-server lib (test)` | 76.32s |
| `phux bin "phux" (test)` | 58.62s |
| `phux-config` | 35.7s |
| `regex-automata` | 31.85s |

### release: 6m05s (previous: 6m12s) — 367 units at `15cea5a7f`

| slowest units | wall |
|---|---:|
| `libghostty-vt-sys build script (run)` | 184.75s |
| `phux bin "phux"` | 127.86s |
| `phux-server` | 30.1s |
| `phux-config` | 25.13s |
| `phux-mcp bin "phux-mcp"` | 21.78s |
| `regex-automata` | 13.48s |
| `rustls` | 13.44s |
| `ring build script (run)` | 12.84s |

## Release binary size

| binary | size | previous |
|---|---:|---:|
| `phux` | 15.5 MiB | 15.5 MiB |
| `phux-mcp` | 2.1 MiB | 2.1 MiB |

## Dependency graph

- locked packages: **439** (previous: 438) — 14 workspace members, 52 direct deps
- duplicate versions: **32** (previous: 32)
- proc-macro crates: 33; build-script crates: 67

## Slowest tests (latest instrumented run, `fa0c818bb`)

| test | wall |
|---|---:|
| `phux-record::golden_cast::golden_cast_renders_gif_and_apng_and_frame_counts_agree` | 3.986s |
| `phux-server::terminal_actor::tests::resize_desync_then_both_shrink_does_not_overflow` | 3.982s |
| `phux-server::terminal_actor::tests::xtwinops_size_queries_answered_from_resized_geometry` | 1.454s |
| `phux-server::phux_3uv_acked_incremental::acked_incremental_converges_and_seq_is_monotonic` | 1.314s |
| `phux-server::agent_detect::a_plain_shell_pane_never_gets_an_agent_record` | 1.215s |
| `phux-server::agent_detect::deleting_the_record_hands_it_back_to_the_detector` | 1.115s |
| `phux-server::agent_detect::an_identity_only_set_gets_its_state_filled_in_by_the_detector` | 1.114s |
| `phux-server::server_self_exit::server_without_clients_does_not_self_exit_on_seed_pane_death` | 1.014s |
| `phux-server::server_idle_exit::connecting_disarms_the_idle_clock` | 1.014s |
| `phux::config_plugin_actions::config_run_timeout_returns_125_and_json_timeout` | 1.010s |

## Recent runs

| when | workflow | event | branch | result | wall | runner time |
|---|---|---|---|---|---:|---:|
| 2026-08-03 05:40 | conventional-commits | pull_request | feat/negotiated-libghostty-codec | success | 17s | 14s |
| 2026-08-03 05:35 | stress | pull_request | release-please--branches--main-- | skipped | 7s | 0s |
| 2026-08-03 05:24 | ci | pull_request | release-please--branches--main-- | success | 10m21s | 15m07s |
| 2026-08-03 05:08 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-08-03 05:08 | conventional-commits | pull_request | release-please--branches--main-- | success | 24s | 14s |
| 2026-08-03 05:07 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-08-03 05:07 | conventional-commits | pull_request | release-please--branches--main-- | success | 22s | 19s |
| 2026-08-03 05:07 | release-please | push | main | success | 1m05s | 47s |
| 2026-08-03 05:07 | ci | push | main | success | 7m38s | 9m18s |
| 2026-08-03 04:59 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-08-03 04:59 | conventional-commits | pull_request | release-please--branches--main-- | success | 20s | 11s |
| 2026-08-03 04:58 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-08-03 04:58 | conventional-commits | pull_request | release-please--branches--main-- | cancelled | 2s | 1s |
| 2026-08-03 04:58 | conventional-commits | pull_request | release-please--branches--main-- | success | 21s | 17s |
| 2026-08-03 04:58 | release-please | push | main | success | 54s | 47s |
| 2026-08-03 04:58 | ci | push | main | success | 9m36s | 14m13s |
| 2026-08-03 04:58 | observatory | push | main | success | 12m15s | 22m22s |
| 2026-08-03 04:56 | conventional-commits | pull_request | feat/ux-followups | success | 18s | 14s |
| 2026-08-03 04:56 | ci | pull_request | feat/ux-followups | success | 10m24s | 12m39s |
| 2026-08-03 04:53 | ci | pull_request | feat/negotiated-libghostty-codec | cancelled | 26m01s | 31m49s |
| 2026-08-03 04:53 | ci | pull_request | feat/negotiated-libghostty-codec | cancelled | 11m33s | 17m32s |
| 2026-08-03 04:53 | conventional-commits | pull_request | feat/negotiated-libghostty-codec | success | 26s | 17s |
| 2026-08-03 04:51 | ci | pull_request | feat/negotiated-libghostty-codec | skipped | 0s | 0s |
| 2026-08-03 04:51 | conventional-commits | pull_request | feat/negotiated-libghostty-codec | success | 20s | 15s |
| 2026-08-03 04:47 | conventional-commits | pull_request | perf/ci-extract-server-testkit | success | 23s | 13s |
| 2026-08-03 04:47 | ci | pull_request | perf/ci-extract-server-testkit | success | 9m23s | 14m09s |
| 2026-08-03 04:42 | ci | pull_request | feat/negotiated-libghostty-codec | skipped | 1s | 0s |
| 2026-08-03 04:42 | conventional-commits | pull_request | feat/negotiated-libghostty-codec | success | 17s | 13s |
| 2026-08-03 04:08 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-08-03 04:08 | conventional-commits | pull_request | release-please--branches--main-- | success | 20s | 12s |

---

Query the raw store directly, e.g. every recorded ci run's wall time:

```sh
git fetch origin ci-metrics && git show origin/ci-metrics:runs/2026-08.ndjson \
  | jq -r 'select(.kind == "run" and .workflow == "ci") | [.created_at, .conclusion, .duration_s] | @tsv'
```
