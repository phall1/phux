# phux CI dashboard

Generated 2026-07-15T04:55:03Z by the ci-metrics workflow. Do not edit —
every table is re-rendered from `runs/*.ndjson` on each update.
Machine rollup: [`site/summary.json`](site/summary.json), rendered live at
<https://phux.phall.io/ci>.

## Workflows, last 30 days

| workflow | runs | success | median | p95 | runner minutes |
|---|---:|---:|---:|---:|---:|
| ci | 36 | 67% | 13m29s | 16m29s | 483 |
| stress | 3 | 67% | 21m11s | 21m11s | 43 |
| observatory | 1 | 100% | 11m44s | 11m44s | 23 |
| conventional-commits | 19 | 89% | 16s | 19s | 4 |
| release-please | 6 | 100% | 20s | 37s | 2 |

## ci jobs, last 30 days

| job | runs | median queue | median wall | p95 wall |
|---|---:|---:|---:|---:|
| test | 36 | 2s | 13m19s | 15m56s |
| check | 36 | 2s | 2m36s | 4m09s |
| detect docs-only | 36 | 2s | 5s | 7s |

## Slowest ci steps (median, last 30 days)

| job | step | median | samples |
|---|---|---:|---:|
| test | tests (unit + e2e) | 12m41s | 16 |
| check | rust checks (fmt + clippy + doc + deny) | 1m45s | 19 |
| test | Run Swatinem/rust-cache@v2 | 23s | 19 |
| check | Run Swatinem/rust-cache@v2 | 22s | 21 |
| check | docs-check | 9s | 19 |
| check | Run DeterminateSystems/nix-installer-action@v22 | 8s | 22 |
| test | Run DeterminateSystems/nix-installer-action@v22 | 8s | 21 |

## Cargo phases inside the lanes (median, last 30 days)

| workflow / job | phase | median | samples |
|---|---|---:|---:|
| ci / check | clippy | 40s | 6 |
| ci / check | doc | 13s | 6 |
| ci / check | deny | 3s | 6 |
| ci / check | fmt | 1s | 6 |
| ci / test | unit | 11m47s | 4 |
| ci / test | e2e | 8s | 4 |
| observatory / timings | build-dev | 10m39s | 1 |
| observatory / timings | build-release | 4m55s | 1 |

## Cache effectiveness (last 30 days)

| workflow / job | rust-cache hit rate | samples |
|---|---:|---:|
| ci / check | 71% | 7 |
| ci / test | 100% | 5 |

## Cold build (observatory)

### dev: 10m40s — 519 units at `c3529bbc2`

| slowest units | wall |
|---|---:|
| `libghostty-vt-sys build script (run)` | 118.96s |
| `phux-server lib (test)` | 87.51s |
| `phux bin "phux"` | 66.67s |
| `phux-client lib (test)` | 59.05s |
| `phux-server` | 53.12s |
| `rustls` | 45.79s |
| `phux-server test "hub_relay_federation" (test)` | 33.32s |
| `phux-server test "spawn_terminal" (test)` | 32.92s |

### release: 4m55s — 358 units at `c3529bbc2`

| slowest units | wall |
|---|---:|
| `libghostty-vt-sys build script (run)` | 154.26s |
| `phux bin "phux"` | 97.2s |
| `regex-automata` | 20.13s |
| `phux-server` | 19.89s |
| `phux-mcp bin "phux-mcp"` | 19.31s |
| `phux-config` | 17.72s |
| `rustls` | 17.27s |
| `clap_builder` | 15.17s |

## Release binary size

| binary | size | previous |
|---|---:|---:|
| `phux` | 12.6 MiB | - |
| `phux-mcp` | 2.0 MiB | - |

## Dependency graph

- locked packages: **431** — 11 workspace members, 47 direct deps
- duplicate versions: **32**
- proc-macro crates: 33; build-script crates: 67

## Slowest tests (latest instrumented run, `a1d38de0c`)

| test | wall |
|---|---:|
| `phux-server::perf_bursty_output::synthesize_against_reference_alloc_bounded_under_full_churn` | 113.009s |
| `phux-server::terminal_actor::tests::resize_desync_then_both_shrink_does_not_overflow` | 29.084s |
| `phux-server::agent_detect::a_plain_shell_pane_never_gets_an_agent_record` | 6.018s |
| `phux-server::agent_detect::an_identity_only_set_gets_its_state_filled_in_by_the_detector` | 3.818s |
| `phux-server::agent_detect::deleting_the_record_hands_it_back_to_the_detector` | 3.814s |
| `phux-server::agent_detect::detector_publishes_blocked_from_a_live_prompt_box` | 3.515s |
| `phux-server::agent_events::unattached_subscriber_receives_events` | 3.014s |
| `phux-server::phux_3uv_acked_incremental::acked_incremental_converges_and_seq_is_monotonic` | 2.515s |
| `phux-server::l2_adversarial::test_subscribe_events_no_loss` | 2.016s |
| `phux-server::runtime::input_lane::tests::lane_routed_input_interleaves_with_a_large_pty_output_burst` | 2.003s |

## Recent runs

| when | workflow | event | branch | result | wall | runner time |
|---|---|---|---|---|---:|---:|
| 2026-07-15 04:54 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-07-15 04:54 | conventional-commits | pull_request | release-please--branches--main-- | cancelled | 2s | 1s |
| 2026-07-15 04:40 | conventional-commits | pull_request | fix/dashboard-first-size-record | success | 16s | 12s |
| 2026-07-15 04:40 | ci | pull_request | fix/dashboard-first-size-record | success | 13m55s | 16m26s |
| 2026-07-15 04:26 | observatory | workflow_dispatch | main | success | 11m44s | 23m06s |
| 2026-07-15 04:25 | release-please | push | main | success | 20s | 17s |
| 2026-07-15 04:25 | ci | push | main | success | 13m36s | 16m17s |
| 2026-07-15 04:11 | conventional-commits | pull_request | feat/ci-observability | success | 13s | 10s |
| 2026-07-15 04:11 | ci | pull_request | feat/ci-observability | success | 13m41s | 16m16s |
| 2026-07-15 04:08 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-07-15 04:08 | conventional-commits | pull_request | release-please--branches--main-- | success | 16s | 12s |
| 2026-07-15 04:08 | conventional-commits | pull_request | release-please--branches--main-- | success | 13s | 9s |
| 2026-07-15 04:08 | conventional-commits | pull_request | release-please--branches--main-- | cancelled | 1s | 0s |
| 2026-07-15 04:08 | ci | pull_request | release-please--branches--main-- | skipped | 2s | 0s |
| 2026-07-15 04:08 | release-please | push | main | success | 37s | 32s |
| 2026-07-15 04:08 | ci | push | main | success | 13m29s | 17m25s |
| 2026-07-15 03:56 | ci | pull_request | feat/ci-observability | success | 14m03s | 17m54s |
| 2026-07-15 03:56 | conventional-commits | pull_request | feat/ci-observability | success | 18s | 14s |
| 2026-07-15 03:52 | ci | pull_request | fix/mouse-encoder-size-and-scrol | success | 16m03s | 18m26s |
| 2026-07-15 03:52 | conventional-commits | pull_request | fix/mouse-encoder-size-and-scrol | success | 15s | 12s |
| 2026-07-15 03:42 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-07-15 03:42 | conventional-commits | pull_request | release-please--branches--main-- | success | 14s | 10s |
| 2026-07-15 03:42 | ci | push | main | success | 13m41s | 16m11s |
| 2026-07-15 03:42 | release-please | push | main | success | 16s | 13s |
| 2026-07-15 03:40 | ci | pull_request | feat/ci-observability | failure | 13m10s | 16m13s |
| 2026-07-15 03:40 | conventional-commits | pull_request | feat/ci-observability | success | 12s | 10s |
| 2026-07-15 03:39 | conventional-commits | pull_request | feat/ci-observability | success | 16s | 12s |
| 2026-07-15 03:39 | ci | pull_request | feat/ci-observability | cancelled | 1m11s | 1m24s |
| 2026-07-15 03:38 | conventional-commits | pull_request | feat/ci-observability | success | 17s | 14s |
| 2026-07-15 03:38 | ci | pull_request | feat/ci-observability | cancelled | 1m28s | 1m45s |

---

Query the raw store directly, e.g. every recorded ci run's wall time:

```sh
git fetch origin ci-metrics && git show origin/ci-metrics:runs/2026-07.ndjson \
  | jq -r 'select(.kind == "run" and .workflow == "ci") | [.created_at, .conclusion, .duration_s] | @tsv'
```
