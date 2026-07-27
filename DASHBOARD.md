# phux CI dashboard

Generated 2026-07-27T20:19:11Z by the ci-metrics workflow. Do not edit —
every table is re-rendered from `runs/*.ndjson` on each update.
Machine rollup: [`site/summary.json`](site/summary.json), rendered live at
<https://phux.phall.io/ci>.

## Workflows, last 30 days

| workflow | runs | success | median | p95 | runner minutes |
|---|---:|---:|---:|---:|---:|
| ci | 252 | 59% | 13m37s | 18m14s | 3051 |
| observatory | 15 | 87% | 12m07s | 12m56s | 336 |
| stress | 25 | 52% | 6m52s | 22m37s | 270 |
| release-please | 50 | 98% | 45s | 7m36s | 122 |
| conventional-commits | 227 | 81% | 16s | 22s | 48 |

## ci jobs, last 30 days

| job | runs | median queue | median wall | p95 wall |
|---|---:|---:|---:|---:|
| test | 249 | 2s | 13m27s | 17m46s |
| check | 247 | 2s | 3m16s | 5m11s |
| detect docs-only | 252 | 2s | 5s | 8s |

## Slowest ci steps (median, last 30 days)

| job | step | median | samples |
|---|---|---:|---:|
| test | tests (unit + e2e) | 14m32s | 15 |
| check | rust checks (fmt + clippy + doc + deny) | 3m06s | 17 |
| test | runner disk headroom | 54s | 17 |
| check | runner disk headroom | 29s | 18 |
| check | Run Swatinem/rust-cache@v2 | 17s | 18 |
| test | Run Swatinem/rust-cache@v2 | 17s | 17 |
| check | docs-check | 13s | 17 |
| test | agents smoke | 12s | 15 |
| test | Run DeterminateSystems/nix-installer-action@v22 | 9s | 17 |
| check | Run DeterminateSystems/nix-installer-action@v22 | 8s | 18 |
| check | formula-check | 6s | 12 |
| check | e2e lane coverage | 5s | 12 |

## Cargo phases inside the lanes (median, last 30 days)

| workflow / job | phase | median | samples |
|---|---|---:|---:|
| ci / check | clippy | 1m53s | 136 |
| ci / check | doc | 11s | 136 |
| ci / check | deny | 4s | 135 |
| ci / check | fmt | 2s | 139 |
| ci / test | unit | 13m24s | 124 |
| ci / test | e2e | 10s | 123 |
| ci / test | agents-smoke | 1s | 64 |
| observatory / timings | build-dev | 11m06s | 13 |
| observatory / timings | build-release | 5m11s | 14 |
| stress / stress | stress | 19m05s | 13 |

## Cache effectiveness (last 30 days)

| workflow / job | rust-cache hit rate | samples |
|---|---:|---:|
| ci / check | 32% | 146 |
| ci / test | 36% | 144 |
| stress / stress | 15% | 13 |

## Cold build (observatory)

### dev: 7m50s (previous: 11m25s) — 538 units at `65a296b14`

| slowest units | wall |
|---|---:|
| `libghostty-vt-sys build script (run)` | 71.65s |
| `phux-client lib (test)` | 67.3s |
| `phux bin "phux"` | 65.29s |
| `phux-server` | 56.66s |
| `phux-server lib (test)` | 39.45s |
| `rustls` | 35.03s |
| `phux bin "phux" (test)` | 29.77s |
| `quinn-proto` | 24.47s |

### release: 5m25s (previous: 5m26s) — 365 units at `65a296b14`

| slowest units | wall |
|---|---:|
| `libghostty-vt-sys build script (run)` | 147.32s |
| `phux bin "phux"` | 127.22s |
| `phux-server` | 27.89s |
| `phux-mcp bin "phux-mcp"` | 22.87s |
| `phux-config` | 21.92s |
| `regex-automata` | 21.17s |
| `clap_builder` | 16.04s |
| `rustls` | 15.53s |

## Release binary size

| binary | size | previous |
|---|---:|---:|
| `phux` | 14.5 MiB | 14.5 MiB |
| `phux-mcp` | 2.1 MiB | 2.1 MiB |

## Dependency graph

- locked packages: **438** (previous: 438) — 13 workspace members, 51 direct deps
- duplicate versions: **32** (previous: 32)
- proc-macro crates: 33; build-script crates: 67

## Slowest tests (latest instrumented run, `98775e0f3`)

| test | wall |
|---|---:|
| `phux-server::perf_bursty_output::synthesize_against_reference_alloc_bounded_under_full_churn` | 86.831s |
| `phux-server::terminal_actor::tests::resize_desync_then_both_shrink_does_not_overflow` | 23.455s |
| `phux-server::agent_detect::a_plain_shell_pane_never_gets_an_agent_record` | 6.018s |
| `phux-relay::relay_auth::stalled_preamble_does_not_wedge_relay` | 5.114s |
| `phux-server::agent_detect::deleting_the_record_hands_it_back_to_the_detector` | 3.819s |
| `phux-server::agent_detect::an_identity_only_set_gets_its_state_filled_in_by_the_detector` | 3.819s |
| `phux-record::golden_cast::both_containers_agree_on_the_frame_count_for_one_recording` | 3.556s |
| `phux-server::agent_detect::detector_publishes_blocked_from_a_live_prompt_box` | 3.520s |
| `phux-server::agent_events::unattached_subscriber_receives_events` | 3.018s |
| `phux-server::server_idle_exit::without_the_flag_an_unattended_server_stays_up` | 3.016s |

## Recent runs

| when | workflow | event | branch | result | wall | runner time |
|---|---|---|---|---|---:|---:|
| 2026-07-27 20:03 | conventional-commits | pull_request | ephemeral-lifetime-and-playback | success | 15s | 13s |
| 2026-07-27 20:03 | ci | pull_request | ephemeral-lifetime-and-playback | success | 15m53s | 19m34s |
| 2026-07-27 13:53 | stress | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-07-27 13:53 | release-please | push | main | success | 8m35s | 20m28s |
| 2026-07-27 13:53 | observatory | push | main | success | 11m08s | 21m07s |
| 2026-07-27 13:53 | ci | push | main | success | 24m26s | 22m26s |
| 2026-07-27 13:36 | ci | pull_request | release-please--branches--main-- | success | 16m23s | 18m03s |
| 2026-07-27 13:36 | conventional-commits | pull_request | release-please--branches--main-- | success | 48s | 11s |
| 2026-07-27 13:35 | conventional-commits | pull_request | release-please--branches--main-- | cancelled | 1s | 1s |
| 2026-07-27 13:35 | ci | pull_request | release-please--branches--main-- | cancelled | 1m29s | 1m02s |
| 2026-07-27 13:35 | conventional-commits | pull_request | release-please--branches--main-- | cancelled | 1m27s | 1m25s |
| 2026-07-27 13:34 | ci | push | main | success | 25m37s | 18m36s |
| 2026-07-27 13:34 | release-please | push | main | success | 1m58s | 46s |
| 2026-07-27 13:33 | conventional-commits | pull_request | feat/gascity-runtime | success | 21s | 16s |
| 2026-07-27 13:33 | ci | pull_request | feat/gascity-runtime | success | 16m37s | 20m14s |
| 2026-07-27 13:28 | ci | pull_request | release-please--branches--main-- | cancelled | 6m59s | 11m48s |
| 2026-07-27 13:27 | release-please | push | main | success | 28s | 23s |
| 2026-07-27 13:27 | ci | push | main | success | 15m40s | 20m12s |
| 2026-07-27 13:12 | conventional-commits | pull_request | deflake-attach-latency | success | 14s | 11s |
| 2026-07-27 13:12 | ci | pull_request | deflake-attach-latency | success | 15m11s | 19m40s |
| 2026-07-27 12:05 | conventional-commits | pull_request | release-please--branches--main-- | success | 19s | 12s |
| 2026-07-27 12:05 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-07-27 12:04 | conventional-commits | pull_request | release-please--branches--main-- | cancelled | 2s | 1s |
| 2026-07-27 12:04 | ci | pull_request | release-please--branches--main-- | skipped | 2s | 0s |
| 2026-07-27 12:04 | conventional-commits | pull_request | release-please--branches--main-- | cancelled | 28s | 23s |
| 2026-07-27 12:04 | release-please | push | main | success | 55s | 48s |
| 2026-07-27 12:04 | ci | push | main | success | 16m57s | 20m28s |
| 2026-07-27 11:48 | conventional-commits | pull_request | seams-residual | success | 19s | 15s |
| 2026-07-27 11:48 | ci | pull_request | seams-residual | success | 15m36s | 20m07s |
| 2026-07-27 10:48 | stress | schedule | main | failure | 6m13s | 6m09s |

---

Query the raw store directly, e.g. every recorded ci run's wall time:

```sh
git fetch origin ci-metrics && git show origin/ci-metrics:runs/2026-07.ndjson \
  | jq -r 'select(.kind == "run" and .workflow == "ci") | [.created_at, .conclusion, .duration_s] | @tsv'
```
