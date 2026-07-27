# phux CI dashboard

Generated 2026-07-27T13:12:47Z by the ci-metrics workflow. Do not edit —
every table is re-rendered from `runs/*.ndjson` on each update.
Machine rollup: [`site/summary.json`](site/summary.json), rendered live at
<https://phux.phall.io/ci>.

## Workflows, last 30 days

| workflow | runs | success | median | p95 | runner minutes |
|---|---:|---:|---:|---:|---:|
| ci | 243 | 58% | 13m36s | 18m08s | 2899 |
| observatory | 14 | 86% | 12m07s | 12m56s | 315 |
| stress | 24 | 54% | 6m52s | 22m37s | 270 |
| release-please | 47 | 98% | 45s | 7m03s | 100 |
| conventional-commits | 222 | 82% | 16s | 21s | 46 |

## ci jobs, last 30 days

| job | runs | median queue | median wall | p95 wall |
|---|---:|---:|---:|---:|
| test | 240 | 2s | 13m25s | 17m49s |
| check | 238 | 2s | 2m57s | 5m11s |
| detect docs-only | 243 | 2s | 5s | 8s |

## Slowest ci steps (median, last 30 days)

| job | step | median | samples |
|---|---|---:|---:|
| test | tests (unit + e2e) | 13m52s | 14 |
| check | rust checks (fmt + clippy + doc + deny) | 3m06s | 14 |
| check | runner disk headroom | 1m00s | 15 |
| test | runner disk headroom | 58s | 15 |
| check | Run Swatinem/rust-cache@v2 | 18s | 15 |
| test | Run Swatinem/rust-cache@v2 | 18s | 15 |
| test | agents smoke | 13s | 14 |
| check | docs-check | 10s | 14 |
| test | Run DeterminateSystems/nix-installer-action@v22 | 9s | 15 |
| check | Run DeterminateSystems/nix-installer-action@v22 | 8s | 15 |
| check | formula-check | 6s | 4 |
| check | e2e lane coverage | 5s | 4 |

## Cargo phases inside the lanes (median, last 30 days)

| workflow / job | phase | median | samples |
|---|---|---:|---:|
| ci / check | clippy | 1m57s | 128 |
| ci / check | doc | 11s | 128 |
| ci / check | deny | 4s | 127 |
| ci / check | fmt | 2s | 131 |
| ci / test | unit | 13m42s | 117 |
| ci / test | e2e | 10s | 116 |
| ci / test | agents-smoke | 1s | 57 |
| observatory / timings | build-dev | 11m06s | 12 |
| observatory / timings | build-release | 5m11s | 13 |
| stress / stress | stress | 19m05s | 13 |

## Cache effectiveness (last 30 days)

| workflow / job | rust-cache hit rate | samples |
|---|---:|---:|
| ci / check | 32% | 138 |
| ci / test | 36% | 136 |
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

## Slowest tests (latest instrumented run, `8f7284de9`)

| test | wall |
|---|---:|
| `phux-server::perf_bursty_output::synthesize_against_reference_alloc_bounded_under_full_churn` | 110.890s |
| `phux-server::terminal_actor::tests::resize_desync_then_both_shrink_does_not_overflow` | 26.997s |
| `phux-server::agent_detect::a_plain_shell_pane_never_gets_an_agent_record` | 6.019s |
| `phux-relay::relay_auth::stalled_preamble_does_not_wedge_relay` | 5.114s |
| `phux-server::agent_detect::deleting_the_record_hands_it_back_to_the_detector` | 3.822s |
| `phux-server::agent_detect::an_identity_only_set_gets_its_state_filled_in_by_the_detector` | 3.818s |
| `phux-server::agent_detect::detector_publishes_blocked_from_a_live_prompt_box` | 3.518s |
| `phux-record::golden_cast::both_containers_agree_on_the_frame_count_for_one_recording` | 3.403s |
| `phux-server::agent_events::unattached_subscriber_receives_events` | 3.018s |
| `phux-record::golden_cast::golden_cast_renders_to_a_looping_gif` | 2.674s |

## Recent runs

| when | workflow | event | branch | result | wall | runner time |
|---|---|---|---|---|---:|---:|
| 2026-07-27 13:12 | conventional-commits | pull_request | deflake-attach-latency | success | 14s | 11s |
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
| 2026-07-27 09:23 | ci | push | main | success | 15m47s | 20m58s |
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

---

Query the raw store directly, e.g. every recorded ci run's wall time:

```sh
git fetch origin ci-metrics && git show origin/ci-metrics:runs/2026-07.ndjson \
  | jq -r 'select(.kind == "run" and .workflow == "ci") | [.created_at, .conclusion, .duration_s] | @tsv'
```
