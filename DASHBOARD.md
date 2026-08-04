# phux CI dashboard

Generated 2026-08-04T19:52:25Z by the ci-metrics workflow. Do not edit —
every table is re-rendered from `runs/*.ndjson` on each update.
Machine rollup: [`site/summary.json`](site/summary.json), rendered live at
<https://phux.phall.io/ci>.

## Workflows, last 30 days

| workflow | runs | success | median | p95 | runner minutes |
|---|---:|---:|---:|---:|---:|
| ci | 468 | 53% | 10m26s | 18m14s | 5065 |
| observatory | 30 | 93% | 12m23s | 13m04s | 699 |
| stress | 48 | 35% | 9s | 22m15s | 347 |
| release-please | 100 | 97% | 48s | 7m47s | 256 |
| conventional-commits | 421 | 80% | 17s | 25s | 86 |
| release | 3 | 100% | 8m02s | 8m02s | 61 |

## ci jobs, last 30 days

| job | runs | median queue | median wall | p95 wall |
|---|---:|---:|---:|---:|
| test | 458 | 2s | 10m24s | 17m30s |
| check | 460 | 2s | 2m32s | 5m21s |
| detect docs-only | 463 | 2s | 5s | 9s |

## Slowest ci steps (median, last 30 days)

| job | step | median | samples |
|---|---|---:|---:|
| test | tests (unit + e2e) | 8m38s | 11 |
| check | rust checks (fmt + clippy + doc + deny) | 4m26s | 15 |
| check | Run Swatinem/rust-cache@v2 | 14s | 16 |
| test | Run Swatinem/rust-cache@v2 | 14s | 16 |
| check | docs-check | 11s | 16 |
| test | agents smoke | 11s | 11 |
| check | Run DeterminateSystems/nix-installer-action@v22 | 7s | 16 |
| check | runner disk headroom | 7s | 16 |
| test | Run DeterminateSystems/nix-installer-action@v22 | 7s | 16 |
| test | runner disk headroom | 7s | 16 |

## Cargo phases inside the lanes (median, last 30 days)

| workflow / job | phase | median | samples |
|---|---|---:|---:|
| ci / check | clippy | 1m48s | 257 |
| ci / check | doc | 10s | 257 |
| ci / check | deny | 3s | 255 |
| ci / check | fmt | 2s | 264 |
| ci / test | unit | 11m47s | 236 |
| ci / test | e2e | 19s | 229 |
| ci / test | agents-smoke | 1s | 165 |
| observatory / timings | build-dev | 10m55s | 28 |
| observatory / timings | build-release | 5m29s | 29 |
| stress / stress | stress | 16m19s | 21 |

## Cache effectiveness (last 30 days)

| workflow / job | rust-cache hit rate | samples |
|---|---:|---:|
| ci / check | 44% | 273 |
| ci / test | 44% | 272 |
| stress / stress | 14% | 21 |

## Cold build (observatory)

### dev: 10m06s (previous: 10m14s) — 552 units at `5b5c67a90`

| slowest units | wall |
|---|---:|
| `libghostty-vt-sys build script (run)` | 262.88s |
| `phux-server lib (test)` | 126.52s |
| `phux-server` | 100.59s |
| `phux-client lib (test)` | 95.83s |
| `phux bin "phux"` | 65.45s |
| `phux bin "phux" (test)` | 46.4s |
| `phux-server-testkit` | 38.9s |
| `phux-config` | 36.89s |

### release: 6m37s (previous: 6m36s) — 368 units at `5b5c67a90`

| slowest units | wall |
|---|---:|
| `libghostty-vt-sys build script (run)` | 192.07s |
| `phux bin "phux"` | 146.5s |
| `phux-server` | 33.4s |
| `phux-config` | 29.49s |
| `phux-mcp bin "phux-mcp"` | 23.9s |
| `phux-server-testkit` | 16.59s |
| `rustls` | 14.55s |
| `regex-automata` | 13.77s |

## Release binary size

| binary | size | previous |
|---|---:|---:|
| `phux` | 16.4 MiB | 16.4 MiB |
| `phux-mcp` | 2.1 MiB | 2.1 MiB |

## Dependency graph

- locked packages: **441** (previous: 441) — 15 workspace members, 53 direct deps
- duplicate versions: **33** (previous: 33)
- proc-macro crates: 33; build-script crates: 67

## Slowest tests (latest instrumented run, `5b5c67a90`)

| test | wall |
|---|---:|
| `phux-server::runtime::attach::tests::prepare_attach_rejects_pane_source_count_before_registration` | 1.646s |
| `phux-server::terminal_actor::tests::xtwinops_size_queries_answered_from_resized_geometry` | 1.449s |
| `phux-record::golden_cast::golden_cast_renders_gif_and_apng_and_frame_counts_agree` | 1.439s |
| `phux-server::phux_3uv_acked_incremental::acked_incremental_converges_and_seq_is_monotonic` | 1.311s |
| `phux-server::agent_detect::a_plain_shell_pane_never_gets_an_agent_record` | 1.214s |
| `phux-server::agent_detect::deleting_the_record_hands_it_back_to_the_detector` | 1.112s |
| `phux-server::agent_detect::an_identity_only_set_gets_its_state_filled_in_by_the_detector` | 1.111s |
| `phux-server::server_idle_exit::connecting_disarms_the_idle_clock` | 1.062s |
| `phux-server::server_self_exit::server_without_clients_does_not_self_exit_on_seed_pane_death` | 1.014s |
| `phux::config_plugin_actions::config_run_timeout_returns_125_and_json_timeout` | 1.012s |

## Recent runs

| when | workflow | event | branch | result | wall | runner time |
|---|---|---|---|---|---:|---:|
| 2026-08-04 19:52 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-08-04 19:51 | stress | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-08-04 19:51 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-08-04 19:51 | conventional-commits | pull_request | release-please--branches--main-- | success | 23s | 14s |
| 2026-08-04 19:51 | release-please | push | main | success | 1m08s | 53s |
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

---

Query the raw store directly, e.g. every recorded ci run's wall time:

```sh
git fetch origin ci-metrics && git show origin/ci-metrics:runs/2026-08.ndjson \
  | jq -r 'select(.kind == "run" and .workflow == "ci") | [.created_at, .conclusion, .duration_s] | @tsv'
```
