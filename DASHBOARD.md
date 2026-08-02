# phux CI dashboard

Generated 2026-08-02T17:33:14Z by the ci-metrics workflow. Do not edit —
every table is re-rendered from `runs/*.ndjson` on each update.
Machine rollup: [`site/summary.json`](site/summary.json), rendered live at
<https://phux.phall.io/ci>.

## Workflows, last 30 days

| workflow | runs | success | median | p95 | runner minutes |
|---|---:|---:|---:|---:|---:|
| ci | 361 | 56% | 12m28s | 18m24s | 4131 |
| observatory | 19 | 89% | 12m23s | 13m00s | 433 |
| stress | 36 | 42% | 5m20s | 22m37s | 334 |
| release-please | 77 | 99% | 45s | 7m37s | 175 |
| conventional-commits | 330 | 79% | 16s | 24s | 67 |

## ci jobs, last 30 days

| job | runs | median queue | median wall | p95 wall |
|---|---:|---:|---:|---:|
| test | 355 | 2s | 12m08s | 17m38s |
| check | 353 | 2s | 2m38s | 5m05s |
| detect docs-only | 356 | 2s | 5s | 8s |

## Slowest ci steps (median, last 30 days)

| job | step | median | samples |
|---|---|---:|---:|
| test | tests (unit + e2e) | 10m12s | 14 |
| check | rust checks (fmt + clippy + doc + deny) | 1m24s | 14 |
| test | Run Swatinem/rust-cache@v2 | 18s | 15 |
| check | Run Swatinem/rust-cache@v2 | 17s | 15 |
| test | agents smoke | 11s | 14 |
| check | docs-check | 10s | 12 |
| check | Run DeterminateSystems/nix-installer-action@v22 | 6s | 15 |
| check | runner disk headroom | 6s | 15 |
| test | Run DeterminateSystems/nix-installer-action@v22 | 6s | 15 |
| test | runner disk headroom | 6s | 15 |

## Cargo phases inside the lanes (median, last 30 days)

| workflow / job | phase | median | samples |
|---|---|---:|---:|
| ci / check | clippy | 1m40s | 196 |
| ci / check | doc | 10s | 196 |
| ci / check | deny | 3s | 195 |
| ci / check | fmt | 2s | 201 |
| ci / test | unit | 12m31s | 182 |
| ci / test | e2e | 11s | 179 |
| ci / test | agents-smoke | 1s | 119 |
| observatory / timings | build-dev | 11m06s | 17 |
| observatory / timings | build-release | 5m13s | 18 |
| stress / stress | stress | 17m27s | 19 |

## Cache effectiveness (last 30 days)

| workflow / job | rust-cache hit rate | samples |
|---|---:|---:|
| ci / check | 43% | 209 |
| ci / test | 45% | 207 |
| stress / stress | 16% | 19 |

## Cold build (observatory)

### dev: 8m27s (previous: 11m02s) — 537 units at `42781794f`

| slowest units | wall |
|---|---:|
| `libghostty-vt-sys build script (run)` | 79.66s |
| `phux bin "phux"` | 74.86s |
| `phux-client lib (test)` | 65.1s |
| `phux-server` | 64.09s |
| `phux-server lib (test)` | 45.88s |
| `phux bin "phux" (test)` | 37.32s |
| `rustls` | 36.87s |
| `phux-config` | 28.44s |

### release: 5m38s (previous: 5m34s) — 366 units at `42781794f`

| slowest units | wall |
|---|---:|
| `libghostty-vt-sys build script (run)` | 150.66s |
| `phux bin "phux"` | 137.72s |
| `phux-server` | 26.23s |
| `phux-config` | 24.9s |
| `regex-automata` | 24.57s |
| `phux-mcp bin "phux-mcp"` | 23.85s |
| `rustls` | 16.81s |
| `clap_builder` | 13.58s |

## Release binary size

| binary | size | previous |
|---|---:|---:|
| `phux` | 14.8 MiB | 14.8 MiB |
| `phux-mcp` | 2.1 MiB | 2.1 MiB |

## Dependency graph

- locked packages: **438** (previous: 438) — 13 workspace members, 52 direct deps
- duplicate versions: **32** (previous: 32)
- proc-macro crates: 33; build-script crates: 67

## Slowest tests (latest instrumented run, `986bef02a`)

| test | wall |
|---|---:|
| `phux-server::terminal_actor::tests::resize_desync_then_both_shrink_does_not_overflow` | 2.643s |
| `phux-record::golden_cast::golden_cast_renders_gif_and_apng_and_frame_counts_agree` | 2.210s |
| `phux-server::terminal_actor::tests::xtwinops_size_queries_answered_from_resized_geometry` | 1.454s |
| `phux-server::phux_3uv_acked_incremental::acked_incremental_converges_and_seq_is_monotonic` | 1.312s |
| `phux-server::agent_detect::a_plain_shell_pane_never_gets_an_agent_record` | 1.214s |
| `phux-server::agent_detect::an_identity_only_set_gets_its_state_filled_in_by_the_detector` | 1.114s |
| `phux-server::agent_detect::deleting_the_record_hands_it_back_to_the_detector` | 1.112s |
| `phux-server::server_idle_exit::connecting_disarms_the_idle_clock` | 1.061s |
| `phux-server::server_self_exit::server_without_clients_does_not_self_exit_on_seed_pane_death` | 1.012s |
| `phux::config_plugin_actions::config_run_timeout_returns_125_and_json_timeout` | 1.011s |

## Recent runs

| when | workflow | event | branch | result | wall | runner time |
|---|---|---|---|---|---:|---:|
| 2026-08-02 17:32 | ci | pull_request | release-please--branches--main-- | skipped | 10s | 0s |
| 2026-08-02 17:32 | conventional-commits | pull_request | release-please--branches--main-- | success | 15s | 13s |
| 2026-08-02 17:32 | conventional-commits | pull_request | release-please--branches--main-- | cancelled | 2s | 1s |
| 2026-08-02 17:32 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-08-02 17:32 | conventional-commits | pull_request | release-please--branches--main-- | success | 16s | 12s |
| 2026-08-02 17:32 | release-please | push | main | success | 48s | 37s |
| 2026-08-02 17:20 | conventional-commits | pull_request | feat/phux-p39-move-terminal | success | 17s | 13s |
| 2026-08-02 17:20 | ci | pull_request | feat/phux-p39-move-terminal | success | 10m49s | 13m12s |
| 2026-08-02 16:20 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-08-02 16:20 | conventional-commits | pull_request | release-please--branches--main-- | success | 16s | 12s |
| 2026-08-02 16:20 | conventional-commits | pull_request | release-please--branches--main-- | success | 16s | 12s |
| 2026-08-02 16:20 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-08-02 16:20 | conventional-commits | pull_request | release-please--branches--main-- | cancelled | 2s | 0s |
| 2026-08-02 16:19 | release-please | push | main | success | 46s | 40s |
| 2026-08-02 16:19 | observatory | push | main | success | 11m41s | 22m34s |
| 2026-08-02 16:19 | ci | push | main | success | 12m28s | 16m27s |
| 2026-08-02 16:19 | conventional-commits | pull_request | phux-cull | success | 17s | 14s |
| 2026-08-02 13:14 | conventional-commits | pull_request | phux-cull | cancelled | 9s | 6s |
| 2026-08-02 13:14 | conventional-commits | pull_request | phux-cull | success | 21s | 16s |
| 2026-08-02 13:13 | conventional-commits | pull_request | phux-cull | failure | 14s | 11s |
| 2026-08-02 13:13 | conventional-commits | pull_request | phux-cull | failure | 23s | 14s |
| 2026-08-02 13:13 | ci | pull_request | phux-cull | success | 12m06s | 15m52s |
| 2026-08-02 12:29 | conventional-commits | pull_request | phux-cull | failure | 19s | 9s |
| 2026-08-02 12:29 | ci | pull_request | phux-cull | success | 11m43s | 15m22s |
| 2026-08-02 09:13 | stress | schedule | main | failure | 8m38s | 8m33s |
| 2026-08-02 08:50 | conventional-commits | pull_request | feat/phux-do1-relay-end-to-end | success | 14s | 11s |
| 2026-08-02 08:50 | ci | pull_request | feat/phux-do1-relay-end-to-end | success | 11m30s | 13m50s |
| 2026-08-02 08:26 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-08-02 08:26 | conventional-commits | pull_request | release-please--branches--main-- | success | 18s | 15s |
| 2026-08-02 08:25 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |

---

Query the raw store directly, e.g. every recorded ci run's wall time:

```sh
git fetch origin ci-metrics && git show origin/ci-metrics:runs/2026-08.ndjson \
  | jq -r 'select(.kind == "run" and .workflow == "ci") | [.created_at, .conclusion, .duration_s] | @tsv'
```
