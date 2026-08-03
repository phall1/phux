# phux CI dashboard

Generated 2026-08-03T22:51:51Z by the ci-metrics workflow. Do not edit —
every table is re-rendered from `runs/*.ndjson` on each update.
Machine rollup: [`site/summary.json`](site/summary.json), rendered live at
<https://phux.phall.io/ci>.

## Workflows, last 30 days

| workflow | runs | success | median | p95 | runner minutes |
|---|---:|---:|---:|---:|---:|
| ci | 451 | 53% | 10m55s | 18m24s | 4950 |
| observatory | 28 | 93% | 12m23s | 12m56s | 647 |
| stress | 45 | 36% | 11s | 22m15s | 340 |
| release-please | 95 | 97% | 48s | 7m42s | 229 |
| conventional-commits | 407 | 80% | 17s | 25s | 83 |
| release | 3 | 100% | 8m02s | 8m02s | 61 |

## ci jobs, last 30 days

| job | runs | median queue | median wall | p95 wall |
|---|---:|---:|---:|---:|
| test | 442 | 2s | 10m38s | 17m30s |
| check | 443 | 2s | 2m35s | 5m17s |
| detect docs-only | 446 | 2s | 5s | 8s |

## Slowest ci steps (median, last 30 days)

| job | step | median | samples |
|---|---|---:|---:|
| test | tests (unit + e2e) | 8m18s | 10 |
| check | rust checks (fmt + clippy + doc + deny) | 4m08s | 18 |
| check | Run Swatinem/rust-cache@v2 | 14s | 19 |
| test | Run Swatinem/rust-cache@v2 | 13s | 19 |
| check | docs-check | 11s | 18 |
| test | agents smoke | 11s | 10 |
| check | Run DeterminateSystems/nix-installer-action@v22 | 7s | 19 |
| check | runner disk headroom | 7s | 19 |
| test | runner disk headroom | 7s | 19 |
| test | Run DeterminateSystems/nix-installer-action@v22 | 6s | 19 |

## Cargo phases inside the lanes (median, last 30 days)

| workflow / job | phase | median | samples |
|---|---|---:|---:|
| ci / check | clippy | 1m47s | 249 |
| ci / check | doc | 10s | 249 |
| ci / check | deny | 3s | 247 |
| ci / check | fmt | 2s | 256 |
| ci / test | unit | 11m50s | 228 |
| ci / test | e2e | 18s | 221 |
| ci / test | agents-smoke | 1s | 158 |
| observatory / timings | build-dev | 11m00s | 26 |
| observatory / timings | build-release | 5m26s | 27 |
| stress / stress | stress | 16m19s | 20 |

## Cache effectiveness (last 30 days)

| workflow / job | rust-cache hit rate | samples |
|---|---:|---:|
| ci / check | 44% | 264 |
| ci / test | 44% | 263 |
| stress / stress | 15% | 20 |

## Cold build (observatory)

### dev: 7m53s (previous: 8m00s) — 542 units at `e33c51bd7`

| slowest units | wall |
|---|---:|
| `libghostty-vt-sys build script (run)` | 163.15s |
| `phux-server lib (test)` | 112.86s |
| `phux bin "phux"` | 103.16s |
| `phux-server` | 80.82s |
| `phux-client lib (test)` | 76.01s |
| `phux bin "phux" (test)` | 61.08s |
| `phux-config` | 35.05s |
| `phux-server-testkit` | 30.16s |

### release: 5m23s (previous: 6m21s) — 367 units at `e33c51bd7`

| slowest units | wall |
|---|---:|
| `libghostty-vt-sys build script (run)` | 145.96s |
| `phux bin "phux"` | 125.56s |
| `phux-server` | 28.73s |
| `phux-config` | 21.63s |
| `phux-mcp bin "phux-mcp"` | 20.6s |
| `phux-server-testkit` | 13.67s |
| `rustls` | 12.47s |
| `phux-client` | 10.05s |

## Release binary size

| binary | size | previous |
|---|---:|---:|
| `phux` | 15.5 MiB | 15.5 MiB |
| `phux-mcp` | 2.1 MiB | 2.1 MiB |

## Dependency graph

- locked packages: **439** (previous: 439) — 14 workspace members, 52 direct deps
- duplicate versions: **32** (previous: 32)
- proc-macro crates: 33; build-script crates: 67

## Slowest tests (latest instrumented run, `35b1b8075`)

| test | wall |
|---|---:|
| `phux-record::golden_cast::golden_cast_renders_gif_and_apng_and_frame_counts_agree` | 3.989s |
| `phux-server::terminal_actor::tests::resize_desync_then_both_shrink_does_not_overflow` | 3.986s |
| `phux-server::terminal_actor::tests::xtwinops_size_queries_answered_from_resized_geometry` | 1.454s |
| `phux-server::phux_3uv_acked_incremental::acked_incremental_converges_and_seq_is_monotonic` | 1.312s |
| `phux-server::agent_detect::a_plain_shell_pane_never_gets_an_agent_record` | 1.216s |
| `phux-server::agent_detect::deleting_the_record_hands_it_back_to_the_detector` | 1.115s |
| `phux-server::agent_detect::an_identity_only_set_gets_its_state_filled_in_by_the_detector` | 1.115s |
| `phux-server::server_self_exit::server_without_clients_does_not_self_exit_on_seed_pane_death` | 1.013s |
| `phux-server::server_idle_exit::connecting_disarms_the_idle_clock` | 1.013s |
| `phux::config_plugin_actions::config_run_timeout_returns_125_and_json_timeout` | 1.010s |

## Recent runs

| when | workflow | event | branch | result | wall | runner time |
|---|---|---|---|---|---:|---:|
| 2026-08-03 22:44 | ci | pull_request | refactor/serverstate-tables | success | 6m48s | 8m59s |
| 2026-08-03 22:44 | conventional-commits | pull_request | refactor/serverstate-tables | success | 21s | 16s |
| 2026-08-03 15:59 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-08-03 15:59 | conventional-commits | pull_request | release-please--branches--main-- | success | 16s | 13s |
| 2026-08-03 15:58 | stress | pull_request | release-please--branches--main-- | skipped | 2s | 0s |
| 2026-08-03 15:58 | ci | pull_request | release-please--branches--main-- | skipped | 8s | 0s |
| 2026-08-03 15:58 | conventional-commits | pull_request | release-please--branches--main-- | success | 19s | 17s |
| 2026-08-03 15:58 | release-please | push | main | success | 1m01s | 46s |
| 2026-08-03 15:58 | ci | push | main | success | 9m59s | 15m26s |
| 2026-08-03 15:58 | observatory | push | main | success | 11m22s | 21m13s |
| 2026-08-03 14:33 | conventional-commits | pull_request | refactor/serverstate-tables | success | 15s | 12s |
| 2026-08-03 14:33 | ci | pull_request | refactor/serverstate-tables | success | 9m30s | 14m09s |
| 2026-08-03 14:32 | conventional-commits | pull_request | refactor/serverstate-tables | success | 18s | 16s |
| 2026-08-03 14:32 | ci | pull_request | refactor/serverstate-tables | cancelled | 27s | 29s |
| 2026-08-03 10:49 | stress | schedule | main | success | 6m42s | 6m32s |
| 2026-08-03 09:43 | observatory | schedule | main | success | 12m41s | 22m54s |
| 2026-08-03 08:49 | conventional-commits | pull_request | refactor/serverstate-decompositi | success | 19s | 15s |
| 2026-08-03 08:49 | ci | pull_request | refactor/serverstate-decompositi | success | 13m52s | 21m49s |
| 2026-08-03 07:24 | conventional-commits | pull_request | feat/negotiated-libghostty-codec | success | 44s | 41s |
| 2026-08-03 07:24 | ci | pull_request | feat/negotiated-libghostty-codec | failure | 12m01s | 17m31s |
| 2026-08-03 07:10 | conventional-commits | pull_request | feat/negotiated-libghostty-codec | success | 18s | 14s |
| 2026-08-03 07:10 | ci | pull_request | feat/negotiated-libghostty-codec | failure | 10m58s | 17m29s |
| 2026-08-03 06:42 | conventional-commits | pull_request | feat/negotiated-libghostty-codec | success | 21s | 11s |
| 2026-08-03 06:42 | ci | pull_request | feat/negotiated-libghostty-codec | failure | 10m01s | 15m42s |
| 2026-08-03 06:14 | conventional-commits | pull_request | feat/negotiated-libghostty-codec | success | 17s | 14s |
| 2026-08-03 06:14 | ci | pull_request | feat/negotiated-libghostty-codec | cancelled | 20m16s | 26m00s |
| 2026-08-03 06:01 | conventional-commits | pull_request | feat/negotiated-libghostty-codec | success | 18s | 14s |
| 2026-08-03 06:01 | ci | pull_request | feat/negotiated-libghostty-codec | failure | 9m13s | 14m56s |
| 2026-08-03 05:47 | conventional-commits | pull_request | feat/negotiated-libghostty-codec | success | 22s | 12s |
| 2026-08-03 05:47 | ci | pull_request | feat/negotiated-libghostty-codec | failure | 10m00s | 15m26s |

---

Query the raw store directly, e.g. every recorded ci run's wall time:

```sh
git fetch origin ci-metrics && git show origin/ci-metrics:runs/2026-08.ndjson \
  | jq -r 'select(.kind == "run" and .workflow == "ci") | [.created_at, .conclusion, .duration_s] | @tsv'
```
