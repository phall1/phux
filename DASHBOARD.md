# phux CI dashboard

Generated 2026-07-15T18:05:45Z by the ci-metrics workflow. Do not edit —
every table is re-rendered from `runs/*.ndjson` on each update.
Machine rollup: [`site/summary.json`](site/summary.json), rendered live at
<https://phux.phall.io/ci>.

## Workflows, last 30 days

| workflow | runs | success | median | p95 | runner minutes |
|---|---:|---:|---:|---:|---:|
| ci | 102 | 59% | 13m15s | 16m54s | 1229 |
| observatory | 4 | 75% | 11m54s | 11m56s | 94 |
| stress | 6 | 50% | 6s | 21m33s | 65 |
| release-please | 17 | 100% | 40s | 52s | 25 |
| conventional-commits | 93 | 86% | 16s | 20s | 17 |

## ci jobs, last 30 days

| job | runs | median queue | median wall | p95 wall |
|---|---:|---:|---:|---:|
| test | 101 | 2s | 13m04s | 16m55s |
| check | 100 | 2s | 2m36s | 4m26s |
| detect docs-only | 102 | 2s | 5s | 7s |

## Slowest ci steps (median, last 30 days)

| job | step | median | samples |
|---|---|---:|---:|
| test | tests (unit + e2e) | 15m12s | 13 |
| check | rust checks (fmt + clippy + doc + deny) | 3m21s | 15 |
| check | Run Swatinem/rust-cache@v2 | 19s | 15 |
| test | Run Swatinem/rust-cache@v2 | 18s | 14 |
| check | Run DeterminateSystems/nix-installer-action@v22 | 9s | 16 |
| check | docs-check | 9s | 14 |
| test | Run DeterminateSystems/nix-installer-action@v22 | 8s | 15 |

## Cargo phases inside the lanes (median, last 30 days)

| workflow / job | phase | median | samples |
|---|---|---:|---:|
| ci / check | clippy | 2m08s | 47 |
| ci / check | doc | 14s | 47 |
| ci / check | deny | 3s | 47 |
| ci / check | fmt | 1s | 49 |
| ci / test | unit | 12m04s | 39 |
| ci / test | e2e | 9s | 38 |
| observatory / timings | build-dev | 10m47s | 3 |
| observatory / timings | build-release | 4m55s | 4 |
| stress / stress | stress | 20m31s | 1 |

## Cache effectiveness (last 30 days)

| workflow / job | rust-cache hit rate | samples |
|---|---:|---:|
| ci / check | 33% | 52 |
| ci / test | 46% | 50 |
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

## Slowest tests (latest instrumented run, `d784a38b2`)

| test | wall |
|---|---:|
| `phux-server::perf_bursty_output::synthesize_against_reference_alloc_bounded_under_full_churn` | 94.408s |
| `phux-server::terminal_actor::tests::resize_desync_then_both_shrink_does_not_overflow` | 23.818s |
| `phux-server::agent_detect::a_plain_shell_pane_never_gets_an_agent_record` | 6.014s |
| `phux-server::agent_detect::deleting_the_record_hands_it_back_to_the_detector` | 3.812s |
| `phux-server::agent_detect::an_identity_only_set_gets_its_state_filled_in_by_the_detector` | 3.812s |
| `phux-server::agent_detect::detector_publishes_blocked_from_a_live_prompt_box` | 3.513s |
| `phux-server::agent_events::unattached_subscriber_receives_events` | 3.013s |
| `phux-server::phux_3uv_acked_incremental::acked_incremental_converges_and_seq_is_monotonic` | 2.512s |
| `phux::bin/phux::commands::overlay::tests::wedged_tailscale_binary_is_killed_at_the_deadline` | 2.015s |
| `phux-server::l2_adversarial::test_subscribe_events_no_loss` | 2.014s |

## Recent runs

| when | workflow | event | branch | result | wall | runner time |
|---|---|---|---|---|---:|---:|
| 2026-07-15 18:05 | conventional-commits | pull_request | release-please--branches--main-- | success | 17s | 10s |
| 2026-07-15 18:05 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-07-15 18:05 | ci | pull_request | release-please--branches--main-- | skipped | 2s | 0s |
| 2026-07-15 18:05 | conventional-commits | pull_request | release-please--branches--main-- | cancelled | 25s | 11s |
| 2026-07-15 18:04 | conventional-commits | pull_request | release-please--branches--main-- | cancelled | 18s | 14s |
| 2026-07-15 18:04 | release-please | push | main | success | 43s | 37s |
| 2026-07-15 17:59 | conventional-commits | pull_request | feat/phux-pair-qr | success | 20s | 14s |
| 2026-07-15 17:59 | ci | pull_request | feat/phux-pair-qr | skipped | 1s | 0s |
| 2026-07-15 17:59 | conventional-commits | pull_request | feat/phux-pair-qr | cancelled | 18s | 14s |
| 2026-07-15 17:46 | conventional-commits | pull_request | feat/dialout-connector-adr-spike | success | 18s | 13s |
| 2026-07-15 17:46 | ci | pull_request | feat/dialout-connector-adr-spike | success | 15m16s | 18m49s |
| 2026-07-15 17:38 | conventional-commits | pull_request | feat/dialout-connector-adr-spike | success | 16s | 12s |
| 2026-07-15 17:38 | ci | pull_request | feat/dialout-connector-adr-spike | cancelled | 8m18s | 12m28s |
| 2026-07-15 14:10 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-07-15 14:10 | conventional-commits | pull_request | release-please--branches--main-- | success | 20s | 15s |
| 2026-07-15 14:09 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-07-15 14:09 | conventional-commits | pull_request | release-please--branches--main-- | cancelled | 2s | 1s |
| 2026-07-15 14:09 | conventional-commits | pull_request | release-please--branches--main-- | success | 17s | 12s |
| 2026-07-15 14:09 | release-please | push | main | success | 52s | 45s |
| 2026-07-15 14:09 | ci | push | main | success | 11m36s | 15m55s |
| 2026-07-15 13:55 | conventional-commits | pull_request | feat/overlay-reachability-first- | success | 13s | 9s |
| 2026-07-15 13:55 | ci | pull_request | feat/overlay-reachability-first- | success | 13m18s | 17m12s |
| 2026-07-15 11:28 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-07-15 11:28 | conventional-commits | pull_request | release-please--branches--main-- | success | 13s | 10s |
| 2026-07-15 11:28 | conventional-commits | pull_request | release-please--branches--main-- | cancelled | 2s | 1s |
| 2026-07-15 11:28 | ci | pull_request | release-please--branches--main-- | skipped | 2s | 0s |
| 2026-07-15 11:28 | conventional-commits | pull_request | release-please--branches--main-- | success | 15s | 9s |
| 2026-07-15 11:27 | release-please | push | main | success | 40s | 34s |
| 2026-07-15 11:27 | observatory | push | main | success | 12m25s | 24m05s |
| 2026-07-15 11:27 | ci | push | main | success | 16m13s | 20m33s |

---

Query the raw store directly, e.g. every recorded ci run's wall time:

```sh
git fetch origin ci-metrics && git show origin/ci-metrics:runs/2026-07.ndjson \
  | jq -r 'select(.kind == "run" and .workflow == "ci") | [.created_at, .conclusion, .duration_s] | @tsv'
```
