# phux CI dashboard

Generated 2026-07-27T13:49:51Z by the ci-metrics workflow. Do not edit —
every table is re-rendered from `runs/*.ndjson` on each update.
Machine rollup: [`site/summary.json`](site/summary.json), rendered live at
<https://phux.phall.io/ci>.

## Workflows, last 30 days

| workflow | runs | success | median | p95 | runner minutes |
|---|---:|---:|---:|---:|---:|
| ci | 248 | 58% | 13m36s | 18m08s | 2972 |
| observatory | 14 | 86% | 12m07s | 12m56s | 315 |
| stress | 24 | 54% | 6m52s | 22m37s | 270 |
| release-please | 49 | 98% | 45s | 7m03s | 101 |
| conventional-commits | 226 | 81% | 16s | 22s | 48 |

## ci jobs, last 30 days

| job | runs | median queue | median wall | p95 wall |
|---|---:|---:|---:|---:|
| test | 245 | 2s | 13m25s | 17m46s |
| check | 243 | 2s | 3m15s | 5m05s |
| detect docs-only | 248 | 2s | 5s | 8s |

## Slowest ci steps (median, last 30 days)

| job | step | median | samples |
|---|---|---:|---:|
| test | tests (unit + e2e) | 13m52s | 15 |
| check | rust checks (fmt + clippy + doc + deny) | 3m06s | 16 |
| test | runner disk headroom | 58s | 17 |
| check | runner disk headroom | 53s | 17 |
| test | Run Swatinem/rust-cache@v2 | 18s | 17 |
| check | Run Swatinem/rust-cache@v2 | 17s | 17 |
| test | agents smoke | 13s | 15 |
| check | docs-check | 10s | 16 |
| test | Run DeterminateSystems/nix-installer-action@v22 | 9s | 17 |
| check | Run DeterminateSystems/nix-installer-action@v22 | 8s | 17 |
| check | e2e lane coverage | 5s | 8 |
| check | formula-check | 5s | 8 |

## Cargo phases inside the lanes (median, last 30 days)

| workflow / job | phase | median | samples |
|---|---|---:|---:|
| ci / check | clippy | 1m55s | 132 |
| ci / check | doc | 11s | 132 |
| ci / check | deny | 4s | 131 |
| ci / check | fmt | 2s | 135 |
| ci / test | unit | 13m24s | 120 |
| ci / test | e2e | 10s | 119 |
| ci / test | agents-smoke | 1s | 60 |
| observatory / timings | build-dev | 11m06s | 12 |
| observatory / timings | build-release | 5m11s | 13 |
| stress / stress | stress | 19m05s | 13 |

## Cache effectiveness (last 30 days)

| workflow / job | rust-cache hit rate | samples |
|---|---:|---:|
| ci / check | 32% | 142 |
| ci / test | 36% | 140 |
| stress / stress | 15% | 13 |

## Cold build (observatory)

### dev: 11m25s (previous: 10m45s) — 537 units at `f8112127d`

| slowest units | wall |
|---|---:|
| `libghostty-vt-sys build script (run)` | 125.06s |
| `phux bin "phux"` | 95.02s |
| `phux-client lib (test)` | 92.19s |
| `phux-server` | 81.77s |
| `phux-server lib (test)` | 64.66s |
| `rustls` | 51.79s |
| `phux bin "phux" (test)` | 43.07s |
| `quinn-proto` | 41.79s |

### release: 5m26s (previous: 5m30s) — 365 units at `f8112127d`

| slowest units | wall |
|---|---:|
| `libghostty-vt-sys build script (run)` | 156.26s |
| `phux bin "phux"` | 120.55s |
| `phux-server` | 27.34s |
| `phux-mcp bin "phux-mcp"` | 21.68s |
| `phux-config` | 20.96s |
| `regex-automata` | 19.75s |
| `rustls` | 15.65s |
| `clap_builder` | 15.23s |

## Release binary size

| binary | size | previous |
|---|---:|---:|
| `phux` | 14.5 MiB | 14.5 MiB |
| `phux-mcp` | 2.1 MiB | 2.1 MiB |

## Dependency graph

- locked packages: **438** (previous: 438) — 13 workspace members, 51 direct deps
- duplicate versions: **32** (previous: 32)
- proc-macro crates: 33; build-script crates: 67

## Slowest tests (latest instrumented run, `c100e924f`)

| test | wall |
|---|---:|
| `phux-server::perf_bursty_output::synthesize_against_reference_alloc_bounded_under_full_churn` | 86.346s |
| `phux-server::terminal_actor::tests::resize_desync_then_both_shrink_does_not_overflow` | 23.547s |
| `phux-server::agent_detect::a_plain_shell_pane_never_gets_an_agent_record` | 6.019s |
| `phux-relay::relay_auth::stalled_preamble_does_not_wedge_relay` | 5.115s |
| `phux-server::agent_detect::deleting_the_record_hands_it_back_to_the_detector` | 3.822s |
| `phux-server::agent_detect::an_identity_only_set_gets_its_state_filled_in_by_the_detector` | 3.819s |
| `phux-record::golden_cast::both_containers_agree_on_the_frame_count_for_one_recording` | 3.556s |
| `phux-server::agent_detect::detector_publishes_blocked_from_a_live_prompt_box` | 3.519s |
| `phux-server::agent_events::unattached_subscriber_receives_events` | 3.018s |
| `phux-server::phux_3uv_acked_incremental::acked_incremental_converges_and_seq_is_monotonic` | 2.516s |

## Recent runs

| when | workflow | event | branch | result | wall | runner time |
|---|---|---|---|---|---:|---:|
| 2026-07-27 13:36 | conventional-commits | pull_request | release-please--branches--main-- | success | 48s | 11s |
| 2026-07-27 13:35 | conventional-commits | pull_request | release-please--branches--main-- | cancelled | 1s | 1s |
| 2026-07-27 13:35 | ci | pull_request | release-please--branches--main-- | cancelled | 1m29s | 1m02s |
| 2026-07-27 13:35 | conventional-commits | pull_request | release-please--branches--main-- | cancelled | 1m27s | 1m25s |
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
| 2026-07-27 09:51 | observatory | schedule | main | success | 12m32s | 24m57s |
| 2026-07-27 09:24 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-07-27 09:24 | conventional-commits | pull_request | release-please--branches--main-- | success | 15s | 12s |
| 2026-07-27 09:24 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-07-27 09:24 | conventional-commits | pull_request | release-please--branches--main-- | success | 18s | 14s |
| 2026-07-27 09:24 | stress | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-07-27 09:23 | release-please | push | main | success | 43s | 37s |
| 2026-07-27 09:23 | observatory | push | main | success | 11m53s | 24m37s |

---

Query the raw store directly, e.g. every recorded ci run's wall time:

```sh
git fetch origin ci-metrics && git show origin/ci-metrics:runs/2026-07.ndjson \
  | jq -r 'select(.kind == "run" and .workflow == "ci") | [.created_at, .conclusion, .duration_s] | @tsv'
```
