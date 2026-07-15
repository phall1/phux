# phux CI dashboard

Generated 2026-07-15T19:37:59Z by the ci-metrics workflow. Do not edit —
every table is re-rendered from `runs/*.ndjson` on each update.
Machine rollup: [`site/summary.json`](site/summary.json), rendered live at
<https://phux.phall.io/ci>.

## Workflows, last 30 days

| workflow | runs | success | median | p95 | runner minutes |
|---|---:|---:|---:|---:|---:|
| ci | 130 | 59% | 13m18s | 17m15s | 1595 |
| observatory | 4 | 75% | 11m54s | 11m56s | 94 |
| stress | 6 | 50% | 6s | 21m33s | 65 |
| release-please | 20 | 100% | 41s | 54s | 28 |
| conventional-commits | 124 | 86% | 15s | 20s | 23 |

## ci jobs, last 30 days

| job | runs | median queue | median wall | p95 wall |
|---|---:|---:|---:|---:|
| test | 129 | 2s | 13m18s | 17m06s |
| check | 127 | 2s | 2m37s | 4m29s |
| detect docs-only | 130 | 2s | 5s | 7s |

## Slowest ci steps (median, last 30 days)

| job | step | median | samples |
|---|---|---:|---:|
| test | tests (unit + e2e) | 15m50s | 15 |
| check | rust checks (fmt + clippy + doc + deny) | 3m02s | 17 |
| check | Run Swatinem/rust-cache@v2 | 19s | 18 |
| test | Run Swatinem/rust-cache@v2 | 18s | 18 |
| check | Run DeterminateSystems/nix-installer-action@v22 | 9s | 18 |
| check | docs-check | 9s | 16 |
| test | Run DeterminateSystems/nix-installer-action@v22 | 8s | 18 |

## Cargo phases inside the lanes (median, last 30 days)

| workflow / job | phase | median | samples |
|---|---|---:|---:|
| ci / check | clippy | 2m03s | 66 |
| ci / check | doc | 14s | 66 |
| ci / check | deny | 3s | 66 |
| ci / check | fmt | 1s | 69 |
| ci / test | unit | 13m56s | 56 |
| ci / test | e2e | 9s | 55 |
| observatory / timings | build-dev | 10m47s | 3 |
| observatory / timings | build-release | 4m55s | 4 |
| stress / stress | stress | 20m31s | 1 |

## Cache effectiveness (last 30 days)

| workflow / job | rust-cache hit rate | samples |
|---|---:|---:|
| ci / check | 31% | 72 |
| ci / test | 38% | 69 |
| stress / stress | 0% | 1 |

## Cold build (observatory)

### dev: 11m06s (previous: 10m48s) — 517 units at `e7e699630`

| slowest units | wall |
|---|---:|
| `libghostty-vt-sys build script (run)` | 123.46s |
| `phux-server lib (test)` | 93.42s |
| `phux bin "phux"` | 73.11s |
| `phux-client lib (test)` | 66.62s |
| `phux-server` | 57.02s |
| `rustls` | 48.15s |
| `quinn-proto` | 36.6s |
| `phux-server test "spawn_terminal" (test)` | 34.34s |

### release: 5m01s (previous: 4m56s) — 358 units at `e7e699630`

| slowest units | wall |
|---|---:|
| `libghostty-vt-sys build script (run)` | 147.68s |
| `phux bin "phux"` | 108.03s |
| `regex-automata` | 25.46s |
| `phux-mcp bin "phux-mcp"` | 22.31s |
| `phux-server` | 21.78s |
| `phux-config` | 18.03s |
| `rustls` | 15.29s |
| `quinn-proto` | 12.66s |

## Release binary size

| binary | size | previous |
|---|---:|---:|
| `phux` | 12.8 MiB | 12.6 MiB |
| `phux-mcp` | 2.1 MiB | 2.0 MiB |

## Dependency graph

- locked packages: **431** (previous: 431) — 11 workspace members, 47 direct deps
- duplicate versions: **32** (previous: 32)
- proc-macro crates: 33; build-script crates: 67

## Slowest tests (latest instrumented run, `4bad375e6`)

| test | wall |
|---|---:|
| `phux-server::perf_bursty_output::synthesize_against_reference_alloc_bounded_under_full_churn` | 111.745s |
| `phux-server::terminal_actor::tests::resize_desync_then_both_shrink_does_not_overflow` | 26.932s |
| `phux-server::agent_detect::a_plain_shell_pane_never_gets_an_agent_record` | 6.017s |
| `phux-server::agent_detect::deleting_the_record_hands_it_back_to_the_detector` | 3.814s |
| `phux-server::agent_detect::an_identity_only_set_gets_its_state_filled_in_by_the_detector` | 3.814s |
| `phux-server::agent_detect::detector_publishes_blocked_from_a_live_prompt_box` | 3.517s |
| `phux-server::agent_events::unattached_subscriber_receives_events` | 3.014s |
| `phux-server::phux_3uv_acked_incremental::acked_incremental_converges_and_seq_is_monotonic` | 2.515s |
| `phux::bin/phux::commands::overlay::tests::wedged_tailscale_binary_is_killed_at_the_deadline` | 2.020s |
| `phux-server::l2_adversarial::test_subscribe_events_no_loss` | 2.015s |

## Recent runs

| when | workflow | event | branch | result | wall | runner time |
|---|---|---|---|---|---:|---:|
| 2026-07-15 19:37 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-07-15 19:37 | conventional-commits | pull_request | release-please--branches--main-- | success | 17s | 12s |
| 2026-07-15 19:37 | conventional-commits | pull_request | release-please--branches--main-- | cancelled | 2s | 1s |
| 2026-07-15 19:37 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-07-15 19:37 | conventional-commits | pull_request | release-please--branches--main-- | success | 15s | 11s |
| 2026-07-15 19:36 | conventional-commits | pull_request | feat/phux-detach | success | 14s | 11s |
| 2026-07-15 19:36 | release-please | push | main | success | 54s | 49s |
| 2026-07-15 19:24 | conventional-commits | pull_request | fix/hook-env-socket | success | 14s | 11s |
| 2026-07-15 19:19 | conventional-commits | pull_request | feat/phux-pair-qr | success | 16s | 12s |
| 2026-07-15 19:19 | ci | pull_request | feat/phux-pair-qr | success | 16m33s | 20m13s |
| 2026-07-15 19:18 | conventional-commits | pull_request | ci/agents-smoke | success | 18s | 15s |
| 2026-07-15 19:18 | ci | pull_request | ci/agents-smoke | failure | 8m39s | 12m07s |
| 2026-07-15 19:11 | conventional-commits | pull_request | release-please--branches--main-- | success | 16s | 14s |
| 2026-07-15 19:11 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-07-15 19:11 | conventional-commits | pull_request | release-please--branches--main-- | success | 15s | 12s |
| 2026-07-15 19:11 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-07-15 19:11 | conventional-commits | pull_request | release-please--branches--main-- | cancelled | 2s | 1s |
| 2026-07-15 19:10 | release-please | push | main | success | 50s | 43s |
| 2026-07-15 19:10 | ci | push | main | success | 18m42s | 23m48s |
| 2026-07-15 19:01 | conventional-commits | pull_request | feat/phux-pair-qr | success | 14s | 11s |
| 2026-07-15 19:01 | ci | pull_request | feat/phux-pair-qr | success | 16m37s | 20m28s |
| 2026-07-15 18:53 | conventional-commits | pull_request | train/wave-2026-07-15 | success | 14s | 12s |
| 2026-07-15 18:53 | conventional-commits | pull_request | train/wave-2026-07-15 | success | 13s | 10s |
| 2026-07-15 18:53 | ci | pull_request | train/wave-2026-07-15 | success | 16m19s | 19m30s |
| 2026-07-15 18:50 | conventional-commits | pull_request | train/wave-2026-07-15 | success | 24s | 20s |
| 2026-07-15 18:49 | conventional-commits | pull_request | train/wave-2026-07-15 | success | 14s | 11s |
| 2026-07-15 18:49 | ci | pull_request | train/wave-2026-07-15 | cancelled | 4m05s | 6m57s |
| 2026-07-15 18:44 | conventional-commits | pull_request | train/wave-2026-07-15 | success | 16s | 13s |
| 2026-07-15 18:44 | ci | pull_request | train/wave-2026-07-15 | cancelled | 5m10s | 9m48s |
| 2026-07-15 18:32 | conventional-commits | pull_request | chore/copy-mode-cleanups | success | 23s | 18s |

---

Query the raw store directly, e.g. every recorded ci run's wall time:

```sh
git fetch origin ci-metrics && git show origin/ci-metrics:runs/2026-07.ndjson \
  | jq -r 'select(.kind == "run" and .workflow == "ci") | [.created_at, .conclusion, .duration_s] | @tsv'
```
