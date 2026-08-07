# phux CI dashboard

Generated 2026-08-07T07:04:38Z by the ci-metrics workflow. Do not edit —
every table is re-rendered from `runs/*.ndjson` on each update.
Machine rollup: [`site/summary.json`](site/summary.json), rendered live at
<https://phux.phall.io/ci>.

## Workflows, last 30 days

| workflow | runs | success | median | p95 | runner minutes |
|---|---:|---:|---:|---:|---:|
| ci | 472 | 54% | 10m29s | 18m14s | 5128 |
| observatory | 31 | 94% | 12m23s | 13m14s | 725 |
| stress | 51 | 37% | 11s | 22m15s | 361 |
| release-please | 101 | 97% | 48s | 8m03s | 279 |
| conventional-commits | 423 | 80% | 17s | 25s | 86 |
| release | 3 | 100% | 8m02s | 8m02s | 61 |

## ci jobs, last 30 days

| job | runs | median queue | median wall | p95 wall |
|---|---:|---:|---:|---:|
| test | 462 | 2s | 10m26s | 17m24s |
| check | 464 | 2s | 2m33s | 5m23s |
| detect docs-only | 467 | 2s | 5s | 9s |

## Slowest ci steps (median, last 30 days)

| job | step | median | samples |
|---|---|---:|---:|
| test | tests (unit + e2e) | 8m38s | 14 |
| check | rust checks (fmt + clippy + doc + deny) | 4m08s | 15 |
| test | Run Swatinem/rust-cache@v2 | 15s | 16 |
| check | Run Swatinem/rust-cache@v2 | 14s | 16 |
| check | docs-check | 11s | 16 |
| test | agents smoke | 10s | 14 |
| check | Run DeterminateSystems/nix-installer-action@v22 | 7s | 16 |
| check | runner disk headroom | 7s | 16 |
| test | Run DeterminateSystems/nix-installer-action@v22 | 7s | 16 |
| test | runner disk headroom | 7s | 16 |

## Cargo phases inside the lanes (median, last 30 days)

| workflow / job | phase | median | samples |
|---|---|---:|---:|
| ci / check | clippy | 1m49s | 261 |
| ci / check | doc | 10s | 261 |
| ci / check | deny | 3s | 259 |
| ci / check | fmt | 2s | 268 |
| ci / test | unit | 11m47s | 240 |
| ci / test | e2e | 19s | 233 |
| ci / test | agents-smoke | 1s | 169 |
| observatory / timings | build-dev | 10m55s | 29 |
| observatory / timings | build-release | 5m29s | 30 |
| stress / stress | stress | 14m33s | 23 |

## Cache effectiveness (last 30 days)

| workflow / job | rust-cache hit rate | samples |
|---|---:|---:|
| ci / check | 43% | 277 |
| ci / test | 43% | 276 |
| stress / stress | 13% | 23 |

## Cold build (observatory)

### dev: 9m48s (previous: 10m06s) — 552 units at `8821f4145`

| slowest units | wall |
|---|---:|
| `libghostty-vt-sys build script (run)` | 256.0s |
| `phux-server lib (test)` | 143.28s |
| `phux bin "phux"` | 107.15s |
| `phux-server` | 97.63s |
| `phux-client lib (test)` | 88.83s |
| `phux bin "phux" (test)` | 61.98s |
| `phux-server-testkit` | 37.65s |
| `phux-config` | 35.81s |

### release: 6m51s (previous: 6m37s) — 368 units at `8821f4145`

| slowest units | wall |
|---|---:|
| `libghostty-vt-sys build script (run)` | 207.03s |
| `phux bin "phux"` | 142.52s |
| `phux-server` | 37.04s |
| `phux-config` | 29.1s |
| `phux-mcp bin "phux-mcp"` | 23.77s |
| `rustls` | 16.11s |
| `phux-server-testkit` | 14.48s |
| `phux-client` | 14.24s |

## Release binary size

| binary | size | previous |
|---|---:|---:|
| `phux` | 16.4 MiB | 16.4 MiB |
| `phux-mcp` | 2.1 MiB | 2.1 MiB |

## Dependency graph

- locked packages: **441** (previous: 441) — 15 workspace members, 53 direct deps
- duplicate versions: **33** (previous: 33)
- proc-macro crates: 33; build-script crates: 67

## Slowest tests (latest instrumented run, `ddc9a87f1`)

| test | wall |
|---|---:|
| `phux-server::runtime::attach::tests::prepare_attach_rejects_pane_source_count_before_registration` | 1.609s |
| `phux-server::terminal_actor::tests::xtwinops_size_queries_answered_from_resized_geometry` | 1.452s |
| `phux-record::golden_cast::golden_cast_renders_gif_and_apng_and_frame_counts_agree` | 1.431s |
| `phux-server::phux_3uv_acked_incremental::acked_incremental_converges_and_seq_is_monotonic` | 1.310s |
| `phux-server::agent_detect::a_plain_shell_pane_never_gets_an_agent_record` | 1.226s |
| `phux-server::agent_detect::deleting_the_record_hands_it_back_to_the_detector` | 1.113s |
| `phux-server::agent_detect::an_identity_only_set_gets_its_state_filled_in_by_the_detector` | 1.113s |
| `phux-server::server_idle_exit::connecting_disarms_the_idle_clock` | 1.061s |
| `phux-server::server_self_exit::server_without_clients_does_not_self_exit_on_seed_pane_death` | 1.014s |
| `phux::config_plugin_actions::config_run_timeout_returns_125_and_json_timeout` | 1.008s |

## Recent runs

| when | workflow | event | branch | result | wall | runner time |
|---|---|---|---|---|---:|---:|
| 2026-08-07 06:51 | conventional-commits | pull_request | docs-ios-consumer-contract | success | 14s | 11s |
| 2026-08-07 06:51 | ci | pull_request | docs-ios-consumer-contract | success | 13m15s | 18m24s |
| 2026-08-06 09:43 | stress | schedule | main | success | 6m49s | 6m45s |
| 2026-08-05 15:12 | stress | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-08-05 15:12 | release-please | push | main | success | 8m48s | 22m51s |
| 2026-08-05 15:12 | ci | push | main | success | 11m33s | 17m43s |
| 2026-08-05 15:12 | observatory | push | main | success | 13m55s | 25m53s |
| 2026-08-05 15:12 | ci | pull_request | release-please--branches--main-- | success | 11m16s | 16m51s |
| 2026-08-05 09:40 | stress | schedule | main | success | 7m05s | 6m57s |
| 2026-08-04 19:52 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-08-04 19:52 | conventional-commits | pull_request | release-please--branches--main-- | success | 14s | 11s |
| 2026-08-04 19:51 | stress | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-08-04 19:51 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-08-04 19:51 | conventional-commits | pull_request | release-please--branches--main-- | success | 23s | 14s |
| 2026-08-04 19:51 | release-please | push | main | success | 1m08s | 53s |
| 2026-08-04 19:51 | ci | push | main | success | 7m19s | 9m47s |
| 2026-08-04 19:48 | ci | pull_request | fix/relay-spec-status-current | success | 2m14s | 2m56s |
| 2026-08-04 19:48 | conventional-commits | pull_request | fix/relay-spec-status-current | success | 26s | 21s |
| 2026-08-04 19:33 | stress | pull_request | release-please--branches--main-- | skipped | 9s | 0s |
| 2026-08-04 19:33 | release-please | push | main | success | 10m03s | 23m51s |
| 2026-08-04 19:33 | ci | push | main | success | 11m09s | 17m16s |
| 2026-08-04 19:33 | observatory | push | main | success | 13m31s | 25m54s |
| 2026-08-04 19:22 | ci | pull_request | release-please--branches--main-- | success | 10m29s | 16m10s |
| 2026-08-04 16:33 | conventional-commits | pull_request | release-please--branches--main-- | success | 23s | 15s |
| 2026-08-04 16:33 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-08-04 16:33 | conventional-commits | pull_request | release-please--branches--main-- | success | 23s | 12s |
| 2026-08-04 16:33 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-08-04 16:33 | conventional-commits | pull_request | release-please--branches--main-- | cancelled | 4s | 3s |
| 2026-08-04 16:32 | release-please | push | main | success | 59s | 40s |
| 2026-08-04 16:32 | ci | push | main | success | 7m27s | 9m46s |

---

Query the raw store directly, e.g. every recorded ci run's wall time:

```sh
git fetch origin ci-metrics && git show origin/ci-metrics:runs/2026-08.ndjson \
  | jq -r 'select(.kind == "run" and .workflow == "ci") | [.created_at, .conclusion, .duration_s] | @tsv'
```
