# phux CI dashboard

Generated 2026-08-03T03:06:32Z by the ci-metrics workflow. Do not edit —
every table is re-rendered from `runs/*.ndjson` on each update.
Machine rollup: [`site/summary.json`](site/summary.json), rendered live at
<https://phux.phall.io/ci>.

## Workflows, last 30 days

| workflow | runs | success | median | p95 | runner minutes |
|---|---:|---:|---:|---:|---:|
| ci | 401 | 56% | 11m36s | 18m24s | 4482 |
| observatory | 24 | 92% | 12m23s | 12m56s | 559 |
| stress | 41 | 37% | 2m39s | 22m37s | 334 |
| release-please | 87 | 97% | 46s | 7m37s | 203 |
| conventional-commits | 364 | 79% | 16s | 24s | 73 |

## ci jobs, last 30 days

| job | runs | median queue | median wall | p95 wall |
|---|---:|---:|---:|---:|
| test | 394 | 2s | 11m20s | 17m30s |
| check | 393 | 2s | 2m36s | 5m05s |
| detect docs-only | 396 | 2s | 5s | 8s |

## Slowest ci steps (median, last 30 days)

| job | step | median | samples |
|---|---|---:|---:|
| test | tests (unit + e2e) | 11m31s | 14 |
| check | rust checks (fmt + clippy + doc + deny) | 3m40s | 17 |
| check | Run Swatinem/rust-cache@v2 | 15s | 17 |
| test | Run Swatinem/rust-cache@v2 | 14s | 17 |
| check | docs-check | 11s | 17 |
| test | agents smoke | 10s | 14 |
| check | runner disk headroom | 7s | 17 |
| test | runner disk headroom | 7s | 17 |
| check | Run DeterminateSystems/nix-installer-action@v22 | 6s | 17 |
| test | Run DeterminateSystems/nix-installer-action@v22 | 6s | 17 |

## Cargo phases inside the lanes (median, last 30 days)

| workflow / job | phase | median | samples |
|---|---|---:|---:|
| ci / check | clippy | 1m40s | 220 |
| ci / check | doc | 10s | 220 |
| ci / check | deny | 3s | 219 |
| ci / check | fmt | 2s | 226 |
| ci / test | unit | 12m11s | 204 |
| ci / test | e2e | 11s | 200 |
| ci / test | agents-smoke | 1s | 140 |
| observatory / timings | build-dev | 11m06s | 22 |
| observatory / timings | build-release | 5m24s | 23 |
| stress / stress | stress | 17m27s | 19 |

## Cache effectiveness (last 30 days)

| workflow / job | rust-cache hit rate | samples |
|---|---:|---:|
| ci / check | 44% | 234 |
| ci / test | 45% | 232 |
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

## Slowest tests (latest instrumented run, `55956b722`)

| test | wall |
|---|---:|
| `phux-record::golden_cast::golden_cast_renders_gif_and_apng_and_frame_counts_agree` | 4.633s |
| `phux-server::terminal_actor::tests::resize_desync_then_both_shrink_does_not_overflow` | 4.019s |
| `phux-server::terminal_actor::tests::xtwinops_size_queries_answered_from_resized_geometry` | 1.456s |
| `phux-server::phux_3uv_acked_incremental::acked_incremental_converges_and_seq_is_monotonic` | 1.312s |
| `phux-server::agent_detect::a_plain_shell_pane_never_gets_an_agent_record` | 1.215s |
| `phux-server::agent_detect::an_identity_only_set_gets_its_state_filled_in_by_the_detector` | 1.115s |
| `phux-server::agent_detect::deleting_the_record_hands_it_back_to_the_detector` | 1.114s |
| `phux-server::server_idle_exit::connecting_disarms_the_idle_clock` | 1.064s |
| `phux-server::server_self_exit::server_without_clients_does_not_self_exit_on_seed_pane_death` | 1.013s |
| `phux::config_plugin_actions::config_run_timeout_returns_125_and_json_timeout` | 1.011s |

## Recent runs

| when | workflow | event | branch | result | wall | runner time |
|---|---|---|---|---|---:|---:|
| 2026-08-03 03:04 | conventional-commits | pull_request | fix/release-zig-0.16-checksums | success | 19s | 17s |
| 2026-08-03 03:00 | conventional-commits | pull_request | feat/ux-wave-12 | success | 18s | 15s |
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
| 2026-08-03 02:10 | ci | pull_request | feat/ux-wave-11 | success | 11m08s | 13m01s |
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

---

Query the raw store directly, e.g. every recorded ci run's wall time:

```sh
git fetch origin ci-metrics && git show origin/ci-metrics:runs/2026-08.ndjson \
  | jq -r 'select(.kind == "run" and .workflow == "ci") | [.created_at, .conclusion, .duration_s] | @tsv'
```
