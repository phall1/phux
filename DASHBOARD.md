# phux CI dashboard

Generated 2026-07-27T09:25:01Z by the ci-metrics workflow. Do not edit —
every table is re-rendered from `runs/*.ndjson` on each update.
Machine rollup: [`site/summary.json`](site/summary.json), rendered live at
<https://phux.phall.io/ci>.

## Workflows, last 30 days

| workflow | runs | success | median | p95 | runner minutes |
|---|---:|---:|---:|---:|---:|
| ci | 238 | 58% | 13m34s | 18m11s | 2838 |
| observatory | 12 | 83% | 12m07s | 12m56s | 265 |
| stress | 23 | 57% | 16m36s | 22m37s | 264 |
| release-please | 46 | 98% | 44s | 7m03s | 99 |
| conventional-commits | 217 | 82% | 16s | 21s | 45 |

## ci jobs, last 30 days

| job | runs | median queue | median wall | p95 wall |
|---|---:|---:|---:|---:|
| test | 235 | 2s | 13m25s | 17m49s |
| check | 233 | 2s | 2m57s | 5m05s |
| detect docs-only | 238 | 2s | 5s | 8s |

## Slowest ci steps (median, last 30 days)

| job | step | median | samples |
|---|---|---:|---:|
| test | tests (unit + e2e) | 13m35s | 14 |
| check | rust checks (fmt + clippy + doc + deny) | 1m37s | 14 |
| test | runner disk headroom | 1m01s | 15 |
| check | runner disk headroom | 1m00s | 15 |
| test | Run Swatinem/rust-cache@v2 | 22s | 15 |
| check | Run Swatinem/rust-cache@v2 | 18s | 15 |
| test | agents smoke | 12s | 14 |
| check | docs-check | 10s | 14 |
| check | Run DeterminateSystems/nix-installer-action@v22 | 8s | 15 |
| test | Run DeterminateSystems/nix-installer-action@v22 | 8s | 15 |
| check | formula-check | 6s | 1 |
| check | e2e lane coverage | 5s | 1 |

## Cargo phases inside the lanes (median, last 30 days)

| workflow / job | phase | median | samples |
|---|---|---:|---:|
| ci / check | clippy | 1m59s | 125 |
| ci / check | doc | 11s | 125 |
| ci / check | deny | 3s | 124 |
| ci / check | fmt | 2s | 128 |
| ci / test | unit | 13m42s | 114 |
| ci / test | e2e | 10s | 113 |
| ci / test | agents-smoke | 1s | 54 |
| observatory / timings | build-dev | 11m06s | 10 |
| observatory / timings | build-release | 5m07s | 11 |
| stress / stress | stress | 19m05s | 12 |

## Cache effectiveness (last 30 days)

| workflow / job | rust-cache hit rate | samples |
|---|---:|---:|
| ci / check | 33% | 135 |
| ci / test | 37% | 133 |
| stress / stress | 17% | 12 |

## Cold build (observatory)

### dev: 11m14s (previous: 10m43s) — 535 units at `5e38ec985`

| slowest units | wall |
|---|---:|
| `libghostty-vt-sys build script (run)` | 125.2s |
| `phux bin "phux"` | 91.38s |
| `phux-client lib (test)` | 88.73s |
| `phux-server` | 80.86s |
| `phux-server lib (test)` | 57.82s |
| `rustls` | 51.39s |
| `phux bin "phux" (test)` | 40.64s |
| `quinn-proto` | 37.59s |

### release: 5m34s (previous: 5m35s) — 365 units at `5e38ec985`

| slowest units | wall |
|---|---:|
| `libghostty-vt-sys build script (run)` | 160.07s |
| `phux bin "phux"` | 122.86s |
| `phux-server` | 26.68s |
| `phux-config` | 23.54s |
| `phux-mcp bin "phux-mcp"` | 21.77s |
| `regex-automata` | 18.15s |
| `clap_builder` | 16.48s |
| `rustls` | 15.2s |

## Release binary size

| binary | size | previous |
|---|---:|---:|
| `phux` | 14.5 MiB | 14.5 MiB |
| `phux-mcp` | 2.1 MiB | 2.1 MiB |

## Dependency graph

- locked packages: **438** (previous: 438) — 13 workspace members, 51 direct deps
- duplicate versions: **32** (previous: 32)
- proc-macro crates: 33; build-script crates: 67

## Slowest tests (latest instrumented run, `788a6a085`)

| test | wall |
|---|---:|
| `phux-server::perf_bursty_output::synthesize_against_reference_alloc_bounded_under_full_churn` | 88.296s |
| `phux-server::terminal_actor::tests::resize_desync_then_both_shrink_does_not_overflow` | 22.493s |
| `phux-server::agent_detect::a_plain_shell_pane_never_gets_an_agent_record` | 6.020s |
| `phux-relay::relay_auth::stalled_preamble_does_not_wedge_relay` | 5.114s |
| `phux-server::agent_detect::an_identity_only_set_gets_its_state_filled_in_by_the_detector` | 3.822s |
| `phux-server::agent_detect::deleting_the_record_hands_it_back_to_the_detector` | 3.818s |
| `phux-server::agent_detect::detector_publishes_blocked_from_a_live_prompt_box` | 3.519s |
| `phux-record::golden_cast::both_containers_agree_on_the_frame_count_for_one_recording` | 3.132s |
| `phux-server::agent_events::unattached_subscriber_receives_events` | 3.019s |
| `phux-server::phux_3uv_acked_incremental::acked_incremental_converges_and_seq_is_monotonic` | 2.516s |

## Recent runs

| when | workflow | event | branch | result | wall | runner time |
|---|---|---|---|---|---:|---:|
| 2026-07-27 09:24 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-07-27 09:24 | conventional-commits | pull_request | release-please--branches--main-- | success | 15s | 12s |
| 2026-07-27 09:24 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-07-27 09:24 | conventional-commits | pull_request | release-please--branches--main-- | success | 18s | 14s |
| 2026-07-27 09:24 | stress | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-07-27 09:23 | release-please | push | main | success | 43s | 37s |
| 2026-07-27 09:06 | conventional-commits | pull_request | seams-and-guardrails | success | 13s | 10s |
| 2026-07-27 09:06 | ci | pull_request | seams-and-guardrails | success | 17m07s | 21m49s |
| 2026-07-27 06:01 | stress | pull_request | release-please--branches--main-- | skipped | 9s | 0s |
| 2026-07-27 06:01 | release-please | push | main | success | 7m42s | 18m39s |
| 2026-07-27 06:01 | observatory | push | main | success | 12m23s | 25m09s |
| 2026-07-27 06:01 | ci | push | main | success | 17m46s | 22m55s |
| 2026-07-27 05:44 | ci | pull_request | release-please--branches--main-- | success | 16m46s | 21m31s |
| 2026-07-27 05:44 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-07-27 05:44 | conventional-commits | pull_request | release-please--branches--main-- | success | 18s | 14s |
| 2026-07-27 05:43 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-07-27 05:43 | conventional-commits | pull_request | release-please--branches--main-- | cancelled | 2s | 1s |
| 2026-07-27 05:43 | conventional-commits | pull_request | release-please--branches--main-- | success | 17s | 12s |
| 2026-07-27 05:43 | release-please | push | main | success | 51s | 44s |
| 2026-07-27 05:43 | observatory | push | main | success | 11m54s | 24m43s |
| 2026-07-27 05:43 | ci | push | main | success | 17m25s | 22m17s |
| 2026-07-27 05:25 | conventional-commits | pull_request | worktree-federated-enchanting-ye | success | 15s | 10s |
| 2026-07-27 05:25 | ci | pull_request | worktree-federated-enchanting-ye | success | 17m32s | 21m53s |
| 2026-07-27 05:18 | conventional-commits | pull_request | worktree-federated-enchanting-ye | success | 18s | 14s |
| 2026-07-27 05:18 | ci | pull_request | worktree-federated-enchanting-ye | cancelled | 7m33s | 12m14s |
| 2026-07-26 09:19 | stress | schedule | main | failure | 6m52s | 6m49s |
| 2026-07-26 03:53 | ci | pull_request | release-please--branches--main-- | skipped | 2s | 0s |
| 2026-07-26 03:53 | conventional-commits | pull_request | release-please--branches--main-- | success | 15s | 12s |
| 2026-07-26 03:52 | conventional-commits | pull_request | release-please--branches--main-- | success | 19s | 15s |
| 2026-07-26 03:52 | conventional-commits | pull_request | release-please--branches--main-- | cancelled | 2s | 0s |

---

Query the raw store directly, e.g. every recorded ci run's wall time:

```sh
git fetch origin ci-metrics && git show origin/ci-metrics:runs/2026-07.ndjson \
  | jq -r 'select(.kind == "run" and .workflow == "ci") | [.created_at, .conclusion, .duration_s] | @tsv'
```
