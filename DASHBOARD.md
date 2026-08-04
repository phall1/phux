# phux CI dashboard

Generated 2026-08-04T19:33:08Z by the ci-metrics workflow. Do not edit —
every table is re-rendered from `runs/*.ndjson` on each update.
Machine rollup: [`site/summary.json`](site/summary.json), rendered live at
<https://phux.phall.io/ci>.

## Workflows, last 30 days

| workflow | runs | success | median | p95 | runner minutes |
|---|---:|---:|---:|---:|---:|
| ci | 464 | 53% | 10m29s | 18m14s | 5045 |
| observatory | 29 | 93% | 12m23s | 13m00s | 673 |
| stress | 46 | 37% | 11s | 22m15s | 347 |
| release-please | 98 | 97% | 48s | 7m42s | 231 |
| conventional-commits | 419 | 80% | 17s | 25s | 85 |
| release | 3 | 100% | 8m02s | 8m02s | 61 |

## ci jobs, last 30 days

| job | runs | median queue | median wall | p95 wall |
|---|---:|---:|---:|---:|
| test | 455 | 2s | 10m26s | 17m30s |
| check | 456 | 2s | 2m33s | 5m18s |
| detect docs-only | 459 | 2s | 5s | 9s |

## Slowest ci steps (median, last 30 days)

| job | step | median | samples |
|---|---|---:|---:|
| test | tests (unit + e2e) | 8m11s | 10 |
| check | rust checks (fmt + clippy + doc + deny) | 4m31s | 17 |
| check | Run Swatinem/rust-cache@v2 | 14s | 18 |
| test | Run Swatinem/rust-cache@v2 | 14s | 17 |
| check | docs-check | 11s | 17 |
| test | agents smoke | 11s | 10 |
| check | Run DeterminateSystems/nix-installer-action@v22 | 7s | 18 |
| check | runner disk headroom | 7s | 18 |
| test | Run DeterminateSystems/nix-installer-action@v22 | 7s | 17 |
| test | runner disk headroom | 7s | 17 |

## Cargo phases inside the lanes (median, last 30 days)

| workflow / job | phase | median | samples |
|---|---|---:|---:|
| ci / check | clippy | 1m47s | 256 |
| ci / check | doc | 10s | 256 |
| ci / check | deny | 3s | 254 |
| ci / check | fmt | 2s | 263 |
| ci / test | unit | 11m47s | 235 |
| ci / test | e2e | 18s | 228 |
| ci / test | agents-smoke | 1s | 164 |
| observatory / timings | build-dev | 11m00s | 27 |
| observatory / timings | build-release | 5m26s | 28 |
| stress / stress | stress | 16m19s | 21 |

## Cache effectiveness (last 30 days)

| workflow / job | rust-cache hit rate | samples |
|---|---:|---:|
| ci / check | 44% | 271 |
| ci / test | 44% | 270 |
| stress / stress | 14% | 21 |

## Cold build (observatory)

### dev: 10m14s (previous: 7m53s) — 552 units at `b442296c3`

| slowest units | wall |
|---|---:|
| `libghostty-vt-sys build script (run)` | 263.87s |
| `phux bin "phux"` | 115.42s |
| `phux-server lib (test)` | 112.62s |
| `phux-server` | 103.37s |
| `phux-client lib (test)` | 95.92s |
| `phux bin "phux" (test)` | 65.53s |
| `phux-server-testkit` | 39.89s |
| `phux-config` | 37.74s |

### release: 6m36s (previous: 5m23s) — 368 units at `b442296c3`

| slowest units | wall |
|---|---:|
| `libghostty-vt-sys build script (run)` | 201.13s |
| `phux bin "phux"` | 137.34s |
| `phux-server` | 32.14s |
| `phux-config` | 29.23s |
| `phux-mcp bin "phux-mcp"` | 22.45s |
| `phux-server-testkit` | 16.48s |
| `rustls` | 13.61s |
| `phux-client` | 13.11s |

## Release binary size

| binary | size | previous |
|---|---:|---:|
| `phux` | 16.4 MiB | 15.5 MiB |
| `phux-mcp` | 2.1 MiB | 2.1 MiB |

## Dependency graph

- locked packages: **441** (previous: 439) — 15 workspace members, 53 direct deps
- duplicate versions: **33** (previous: 32)
- proc-macro crates: 33; build-script crates: 67

## Slowest tests (latest instrumented run, `d92c502c7`)

| test | wall |
|---|---:|
| `phux-server::runtime::attach::tests::prepare_attach_rejects_pane_source_count_before_registration` | 1.604s |
| `phux-server::terminal_actor::tests::xtwinops_size_queries_answered_from_resized_geometry` | 1.446s |
| `phux-record::golden_cast::golden_cast_renders_gif_and_apng_and_frame_counts_agree` | 1.431s |
| `phux-server::phux_3uv_acked_incremental::acked_incremental_converges_and_seq_is_monotonic` | 1.310s |
| `phux-server::agent_detect::a_plain_shell_pane_never_gets_an_agent_record` | 1.214s |
| `phux-server::agent_detect::deleting_the_record_hands_it_back_to_the_detector` | 1.112s |
| `phux-server::agent_detect::an_identity_only_set_gets_its_state_filled_in_by_the_detector` | 1.112s |
| `phux-server::server_idle_exit::connecting_disarms_the_idle_clock` | 1.061s |
| `phux-server::server_self_exit::server_without_clients_does_not_self_exit_on_seed_pane_death` | 1.013s |
| `phux::config_plugin_actions::config_run_timeout_returns_125_and_json_timeout` | 1.010s |

## Recent runs

| when | workflow | event | branch | result | wall | runner time |
|---|---|---|---|---|---:|---:|
| 2026-08-04 19:22 | ci | pull_request | release-please--branches--main-- | success | 10m29s | 16m10s |
| 2026-08-04 16:33 | conventional-commits | pull_request | release-please--branches--main-- | success | 23s | 15s |
| 2026-08-04 16:33 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-08-04 16:33 | conventional-commits | pull_request | release-please--branches--main-- | success | 23s | 12s |
| 2026-08-04 16:33 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-08-04 16:33 | conventional-commits | pull_request | release-please--branches--main-- | cancelled | 4s | 3s |
| 2026-08-04 16:32 | release-please | push | main | success | 59s | 40s |
| 2026-08-04 16:32 | ci | push | main | success | 7m27s | 9m46s |
| 2026-08-04 16:25 | conventional-commits | pull_request | fix/search-result-release | success | 17s | 14s |
| 2026-08-04 16:25 | ci | pull_request | fix/search-result-release | success | 7m17s | 9m33s |
| 2026-08-04 09:41 | stress | schedule | main | success | 6m56s | 6m52s |
| 2026-08-04 08:52 | ci | pull_request | release-please--branches--main-- | skipped | 2s | 0s |
| 2026-08-04 08:52 | conventional-commits | pull_request | release-please--branches--main-- | success | 20s | 13s |
| 2026-08-04 08:51 | conventional-commits | pull_request | release-please--branches--main-- | cancelled | 30s | 20s |
| 2026-08-04 08:51 | conventional-commits | pull_request | release-please--branches--main-- | cancelled | 2s | 1s |
| 2026-08-04 08:51 | ci | pull_request | release-please--branches--main-- | skipped | 10s | 0s |
| 2026-08-04 08:51 | release-please | push | main | success | 50s | 39s |
| 2026-08-04 08:51 | ci | push | main | success | 11m25s | 17m07s |
| 2026-08-04 08:51 | observatory | push | main | success | 13m14s | 25m45s |
| 2026-08-04 08:40 | conventional-commits | pull_request | feat/negotiated-libghostty-codec | success | 20s | 16s |
| 2026-08-04 08:40 | ci | pull_request | feat/negotiated-libghostty-codec | success | 10m10s | 15m35s |
| 2026-08-04 07:44 | conventional-commits | pull_request | feat/negotiated-libghostty-codec | success | 18s | 15s |
| 2026-08-04 07:44 | ci | pull_request | feat/negotiated-libghostty-codec | failure | 11m20s | 16m54s |
| 2026-08-03 22:52 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-08-03 22:52 | conventional-commits | pull_request | release-please--branches--main-- | success | 22s | 17s |
| 2026-08-03 22:52 | conventional-commits | pull_request | release-please--branches--main-- | cancelled | 1s | 0s |
| 2026-08-03 22:52 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-08-03 22:52 | conventional-commits | pull_request | release-please--branches--main-- | success | 22s | 12s |
| 2026-08-03 22:51 | release-please | push | main | success | 51s | 44s |
| 2026-08-03 22:51 | ci | push | main | success | 7m12s | 9m38s |

---

Query the raw store directly, e.g. every recorded ci run's wall time:

```sh
git fetch origin ci-metrics && git show origin/ci-metrics:runs/2026-08.ndjson \
  | jq -r 'select(.kind == "run" and .workflow == "ci") | [.created_at, .conclusion, .duration_s] | @tsv'
```
