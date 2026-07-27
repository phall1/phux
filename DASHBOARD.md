# phux CI dashboard

Generated 2026-07-27T10:29:01Z by the ci-metrics workflow. Do not edit —
every table is re-rendered from `runs/*.ndjson` on each update.
Machine rollup: [`site/summary.json`](site/summary.json), rendered live at
<https://phux.phall.io/ci>.

## Workflows, last 30 days

| workflow | runs | success | median | p95 | runner minutes |
|---|---:|---:|---:|---:|---:|
| ci | 239 | 59% | 13m36s | 18m11s | 2859 |
| observatory | 14 | 86% | 12m07s | 12m56s | 315 |
| stress | 23 | 57% | 16m36s | 22m37s | 264 |
| release-please | 46 | 98% | 44s | 7m03s | 99 |
| conventional-commits | 217 | 82% | 16s | 21s | 45 |

## ci jobs, last 30 days

| job | runs | median queue | median wall | p95 wall |
|---|---:|---:|---:|---:|
| test | 236 | 2s | 13m25s | 17m49s |
| check | 234 | 2s | 2m57s | 5m11s |
| detect docs-only | 239 | 2s | 5s | 8s |

## Slowest ci steps (median, last 30 days)

| job | step | median | samples |
|---|---|---:|---:|
| test | tests (unit + e2e) | 13m40s | 14 |
| check | rust checks (fmt + clippy + doc + deny) | 3m06s | 14 |
| check | runner disk headroom | 1m00s | 15 |
| test | runner disk headroom | 58s | 15 |
| check | Run Swatinem/rust-cache@v2 | 18s | 15 |
| test | Run Swatinem/rust-cache@v2 | 18s | 15 |
| test | agents smoke | 12s | 14 |
| check | docs-check | 10s | 14 |
| check | Run DeterminateSystems/nix-installer-action@v22 | 8s | 15 |
| test | Run DeterminateSystems/nix-installer-action@v22 | 8s | 15 |
| check | formula-check | 6s | 2 |
| check | generated-font check | 5s | 2 |

## Cargo phases inside the lanes (median, last 30 days)

| workflow / job | phase | median | samples |
|---|---|---:|---:|
| ci / check | clippy | 1m59s | 126 |
| ci / check | doc | 11s | 126 |
| ci / check | deny | 4s | 125 |
| ci / check | fmt | 2s | 129 |
| ci / test | unit | 13m42s | 115 |
| ci / test | e2e | 10s | 114 |
| ci / test | agents-smoke | 1s | 55 |
| observatory / timings | build-dev | 11m06s | 12 |
| observatory / timings | build-release | 5m11s | 13 |
| stress / stress | stress | 19m05s | 12 |

## Cache effectiveness (last 30 days)

| workflow / job | rust-cache hit rate | samples |
|---|---:|---:|
| ci / check | 32% | 136 |
| ci / test | 37% | 134 |
| stress / stress | 17% | 12 |

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

## Slowest tests (latest instrumented run, `f8112127d`)

| test | wall |
|---|---:|
| `phux-server::perf_bursty_output::synthesize_against_reference_alloc_bounded_under_full_churn` | 96.739s |
| `phux-server::terminal_actor::tests::resize_desync_then_both_shrink_does_not_overflow` | 24.166s |
| `phux-server::agent_detect::a_plain_shell_pane_never_gets_an_agent_record` | 6.016s |
| `phux-relay::relay_auth::stalled_preamble_does_not_wedge_relay` | 5.150s |
| `phux-server::agent_detect::an_identity_only_set_gets_its_state_filled_in_by_the_detector` | 3.817s |
| `phux-server::agent_detect::deleting_the_record_hands_it_back_to_the_detector` | 3.814s |
| `phux-server::agent_detect::detector_publishes_blocked_from_a_live_prompt_box` | 3.515s |
| `phux-server::agent_events::unattached_subscriber_receives_events` | 3.013s |
| `phux-record::golden_cast::both_containers_agree_on_the_frame_count_for_one_recording` | 2.870s |
| `phux-server::phux_3uv_acked_incremental::acked_incremental_converges_and_seq_is_monotonic` | 2.513s |

## Recent runs

| when | workflow | event | branch | result | wall | runner time |
|---|---|---|---|---|---:|---:|
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

---

Query the raw store directly, e.g. every recorded ci run's wall time:

```sh
git fetch origin ci-metrics && git show origin/ci-metrics:runs/2026-07.ndjson \
  | jq -r 'select(.kind == "run" and .workflow == "ci") | [.created_at, .conclusion, .duration_s] | @tsv'
```
