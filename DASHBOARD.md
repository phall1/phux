# phux CI dashboard

Generated 2026-08-02T13:13:49Z by the ci-metrics workflow. Do not edit —
every table is re-rendered from `runs/*.ndjson` on each update.
Machine rollup: [`site/summary.json`](site/summary.json), rendered live at
<https://phux.phall.io/ci>.

## Workflows, last 30 days

| workflow | runs | success | median | p95 | runner minutes |
|---|---:|---:|---:|---:|---:|
| ci | 354 | 56% | 12m49s | 18m24s | 4085 |
| observatory | 18 | 89% | 12m23s | 13m00s | 410 |
| stress | 36 | 42% | 5m20s | 22m37s | 334 |
| release-please | 75 | 99% | 45s | 7m37s | 174 |
| conventional-commits | 319 | 80% | 16s | 24s | 65 |

## ci jobs, last 30 days

| job | runs | median queue | median wall | p95 wall |
|---|---:|---:|---:|---:|
| test | 348 | 2s | 12m16s | 17m38s |
| check | 346 | 2s | 2m39s | 5m05s |
| detect docs-only | 349 | 2s | 5s | 8s |

## Slowest ci steps (median, last 30 days)

| job | step | median | samples |
|---|---|---:|---:|
| test | tests (unit + e2e) | 10m12s | 15 |
| check | rust checks (fmt + clippy + doc + deny) | 1m22s | 15 |
| test | Run Swatinem/rust-cache@v2 | 18s | 17 |
| check | Run Swatinem/rust-cache@v2 | 17s | 17 |
| test | agents smoke | 11s | 15 |
| check | docs-check | 10s | 13 |
| check | Run DeterminateSystems/nix-installer-action@v22 | 7s | 17 |
| check | runner disk headroom | 6s | 17 |
| test | Run DeterminateSystems/nix-installer-action@v22 | 6s | 17 |

## Cargo phases inside the lanes (median, last 30 days)

| workflow / job | phase | median | samples |
|---|---|---:|---:|
| ci / check | clippy | 1m40s | 193 |
| ci / check | doc | 10s | 193 |
| ci / check | deny | 3s | 192 |
| ci / check | fmt | 2s | 198 |
| ci / test | unit | 12m32s | 179 |
| ci / test | e2e | 11s | 176 |
| ci / test | agents-smoke | 1s | 116 |
| observatory / timings | build-dev | 11m06s | 16 |
| observatory / timings | build-release | 5m13s | 17 |
| stress / stress | stress | 17m27s | 19 |

## Cache effectiveness (last 30 days)

| workflow / job | rust-cache hit rate | samples |
|---|---:|---:|
| ci / check | 43% | 206 |
| ci / test | 45% | 204 |
| stress / stress | 16% | 19 |

## Cold build (observatory)

### dev: 11m02s (previous: 11m45s) — 543 units at `6c7d65e8e`

| slowest units | wall |
|---|---:|
| `libghostty-vt-sys build script (run)` | 121.35s |
| `phux bin "phux"` | 90.05s |
| `phux-client lib (test)` | 88.53s |
| `phux-server` | 77.84s |
| `phux-server lib (test)` | 59.62s |
| `rustls` | 45.79s |
| `phux bin "phux" (test)` | 44.64s |
| `phux-config` | 34.72s |

### release: 5m34s (previous: 5m35s) — 366 units at `6c7d65e8e`

| slowest units | wall |
|---|---:|
| `libghostty-vt-sys build script (run)` | 147.37s |
| `phux bin "phux"` | 135.53s |
| `phux-server` | 28.16s |
| `phux-config` | 24.01s |
| `phux-mcp bin "phux-mcp"` | 22.81s |
| `regex-automata` | 21.31s |
| `rustls` | 14.95s |
| `clap_builder` | 13.03s |

## Release binary size

| binary | size | previous |
|---|---:|---:|
| `phux` | 14.8 MiB | 14.7 MiB |
| `phux-mcp` | 2.1 MiB | 2.1 MiB |

## Dependency graph

- locked packages: **438** (previous: 438) — 13 workspace members, 52 direct deps
- duplicate versions: **32** (previous: 32)
- proc-macro crates: 33; build-script crates: 67

## Slowest tests (latest instrumented run, `987763581`)

| test | wall |
|---|---:|
| `phux-server::terminal_actor::tests::resize_desync_then_both_shrink_does_not_overflow` | 2.633s |
| `phux-record::golden_cast::golden_cast_renders_gif_and_apng_and_frame_counts_agree` | 2.203s |
| `phux-server::terminal_actor::tests::xtwinops_size_queries_answered_from_resized_geometry` | 1.452s |
| `phux-server::phux_3uv_acked_incremental::acked_incremental_converges_and_seq_is_monotonic` | 1.318s |
| `phux-server::agent_detect::a_plain_shell_pane_never_gets_an_agent_record` | 1.213s |
| `phux-server::agent_detect::deleting_the_record_hands_it_back_to_the_detector` | 1.113s |
| `phux-server::agent_detect::an_identity_only_set_gets_its_state_filled_in_by_the_detector` | 1.113s |
| `phux-server::server_idle_exit::connecting_disarms_the_idle_clock` | 1.061s |
| `phux-server::server_self_exit::server_without_clients_does_not_self_exit_on_seed_pane_death` | 1.015s |
| `phux::config_plugin_actions::config_run_timeout_returns_125_and_json_timeout` | 1.012s |

## Recent runs

| when | workflow | event | branch | result | wall | runner time |
|---|---|---|---|---|---:|---:|
| 2026-08-02 13:13 | conventional-commits | pull_request | phux-cull | failure | 14s | 11s |
| 2026-08-02 12:29 | conventional-commits | pull_request | phux-cull | failure | 19s | 9s |
| 2026-08-02 12:29 | ci | pull_request | phux-cull | success | 11m43s | 15m22s |
| 2026-08-02 09:13 | stress | schedule | main | failure | 8m38s | 8m33s |
| 2026-08-02 08:50 | conventional-commits | pull_request | feat/phux-do1-relay-end-to-end | success | 14s | 11s |
| 2026-08-02 08:50 | ci | pull_request | feat/phux-do1-relay-end-to-end | success | 11m30s | 13m50s |
| 2026-08-02 08:26 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-08-02 08:26 | conventional-commits | pull_request | release-please--branches--main-- | success | 18s | 15s |
| 2026-08-02 08:25 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-08-02 08:25 | conventional-commits | pull_request | release-please--branches--main-- | cancelled | 2s | 0s |
| 2026-08-02 08:25 | conventional-commits | pull_request | release-please--branches--main-- | success | 16s | 11s |
| 2026-08-02 08:25 | release-please | push | main | success | 44s | 39s |
| 2026-08-02 08:25 | ci | push | main | success | 12m20s | 14m36s |
| 2026-08-02 08:18 | conventional-commits | pull_request | docs/phux-bd3-relay-spec-addendu | success | 19s | 14s |
| 2026-08-02 08:18 | ci | pull_request | docs/phux-bd3-relay-spec-addendu | success | 2m06s | 2m43s |
| 2026-08-02 08:14 | conventional-commits | pull_request | feat/ux-wave-5 | success | 19s | 14s |
| 2026-08-02 08:14 | ci | pull_request | feat/ux-wave-5 | success | 10m55s | 13m14s |
| 2026-08-02 07:30 | conventional-commits | pull_request | feat/ux-wave-5 | success | 23s | 19s |
| 2026-08-02 07:30 | ci | pull_request | feat/ux-wave-5 | failure | 11m22s | 13m56s |
| 2026-08-02 06:52 | conventional-commits | pull_request | feat/ux-wave-5 | failure | 14s | 12s |
| 2026-08-02 06:49 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-08-02 06:49 | conventional-commits | pull_request | release-please--branches--main-- | success | 18s | 15s |
| 2026-08-02 06:48 | conventional-commits | pull_request | release-please--branches--main-- | cancelled | 2s | 1s |
| 2026-08-02 06:48 | ci | pull_request | release-please--branches--main-- | skipped | 10s | 0s |
| 2026-08-02 06:48 | conventional-commits | pull_request | release-please--branches--main-- | success | 15s | 12s |
| 2026-08-02 06:48 | release-please | push | main | success | 45s | 38s |
| 2026-08-02 06:48 | ci | push | main | failure | 16m18s | 13m35s |
| 2026-08-02 06:48 | conventional-commits | pull_request | feat/ux-wave-5 | failure | 20s | 17s |
| 2026-08-02 06:48 | ci | pull_request | feat/ux-wave-5 | success | 10m59s | 13m34s |
| 2026-08-02 06:47 | conventional-commits | pull_request | fix/adr-0066-collision | success | 17s | 13s |

---

Query the raw store directly, e.g. every recorded ci run's wall time:

```sh
git fetch origin ci-metrics && git show origin/ci-metrics:runs/2026-08.ndjson \
  | jq -r 'select(.kind == "run" and .workflow == "ci") | [.created_at, .conclusion, .duration_s] | @tsv'
```
