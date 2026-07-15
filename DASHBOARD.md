# phux CI dashboard

Generated 2026-07-15T09:33:01Z by the ci-metrics workflow. Do not edit —
every table is re-rendered from `runs/*.ndjson` on each update.
Machine rollup: [`site/summary.json`](site/summary.json), rendered live at
<https://phux.phall.io/ci>.

## Workflows, last 30 days

| workflow | runs | success | median | p95 | runner minutes |
|---|---:|---:|---:|---:|---:|
| ci | 46 | 65% | 13m29s | 16m52s | 616 |
| observatory | 2 | 100% | 11m44s | 11m44s | 47 |
| stress | 5 | 40% | 6s | 21m11s | 43 |
| release-please | 9 | 100% | 35s | 44s | 20 |
| conventional-commits | 28 | 93% | 16s | 19s | 6 |

## ci jobs, last 30 days

| job | runs | median queue | median wall | p95 wall |
|---|---:|---:|---:|---:|
| test | 45 | 2s | 13m22s | 16m41s |
| check | 44 | 2s | 2m37s | 4m22s |
| detect docs-only | 46 | 2s | 5s | 7s |

## Slowest ci steps (median, last 30 days)

| job | step | median | samples |
|---|---|---:|---:|
| test | tests (unit + e2e) | 12m42s | 16 |
| check | rust checks (fmt + clippy + doc + deny) | 1m48s | 19 |
| check | Run Swatinem/rust-cache@v2 | 22s | 20 |
| test | Run Swatinem/rust-cache@v2 | 22s | 18 |
| check | docs-check | 9s | 19 |
| check | Run DeterminateSystems/nix-installer-action@v22 | 8s | 21 |
| test | Run DeterminateSystems/nix-installer-action@v22 | 8s | 20 |

## Cargo phases inside the lanes (median, last 30 days)

| workflow / job | phase | median | samples |
|---|---|---:|---:|
| ci / check | clippy | 1m59s | 13 |
| ci / check | doc | 14s | 13 |
| ci / check | deny | 3s | 13 |
| ci / check | fmt | 1s | 13 |
| ci / test | unit | 11m56s | 11 |
| ci / test | e2e | 8s | 10 |
| observatory / timings | build-dev | 10m39s | 2 |
| observatory / timings | build-release | 4m55s | 2 |

## Cache effectiveness (last 30 days)

| workflow / job | rust-cache hit rate | samples |
|---|---:|---:|
| ci / check | 43% | 14 |
| ci / test | 50% | 12 |

## Cold build (observatory)

### dev: 10m48s (previous: 10m40s) — 519 units at `0abd5a5ed`

| slowest units | wall |
|---|---:|
| `libghostty-vt-sys build script (run)` | 118.77s |
| `phux-server lib (test)` | 89.01s |
| `phux bin "phux"` | 67.92s |
| `phux-client lib (test)` | 59.91s |
| `phux-server` | 53.93s |
| `rustls` | 44.92s |
| `phux-server test "hub_relay_federation" (test)` | 33.62s |
| `phux-server test "spawn_terminal" (test)` | 33.28s |

### release: 5m01s (previous: 4m55s) — 358 units at `0abd5a5ed`

| slowest units | wall |
|---|---:|
| `libghostty-vt-sys build script (run)` | 156.92s |
| `phux bin "phux"` | 98.53s |
| `phux-server` | 22.45s |
| `regex-automata` | 20.4s |
| `phux-mcp bin "phux-mcp"` | 20.0s |
| `phux-config` | 17.06s |
| `rustls` | 17.04s |
| `clap_builder` | 16.5s |

## Release binary size

| binary | size | previous |
|---|---:|---:|
| `phux` | 12.6 MiB | 12.6 MiB |
| `phux-mcp` | 2.0 MiB | 2.0 MiB |

## Dependency graph

- locked packages: **431** (previous: 431) — 11 workspace members, 47 direct deps
- duplicate versions: **32** (previous: 32)
- proc-macro crates: 33; build-script crates: 67

## Slowest tests (latest instrumented run, `f3e5455d2`)

| test | wall |
|---|---:|
| `phux-server::perf_bursty_output::synthesize_against_reference_alloc_bounded_under_full_churn` | 108.337s |
| `phux-server::terminal_actor::tests::resize_desync_then_both_shrink_does_not_overflow` | 27.306s |
| `phux-server::agent_detect::a_plain_shell_pane_never_gets_an_agent_record` | 6.014s |
| `phux-server::agent_detect::an_identity_only_set_gets_its_state_filled_in_by_the_detector` | 3.814s |
| `phux-server::agent_detect::deleting_the_record_hands_it_back_to_the_detector` | 3.813s |
| `phux-server::agent_detect::detector_publishes_blocked_from_a_live_prompt_box` | 3.516s |
| `phux-server::agent_events::unattached_subscriber_receives_events` | 3.014s |
| `phux-server::phux_3uv_acked_incremental::acked_incremental_converges_and_seq_is_monotonic` | 2.513s |
| `phux-server::l2_adversarial::test_subscribe_events_no_loss` | 2.016s |
| `phux-server::phux_0q8_no_double_emit::live_output_is_delivered_exactly_once` | 1.514s |

## Recent runs

| when | workflow | event | branch | result | wall | runner time |
|---|---|---|---|---|---:|---:|
| 2026-07-15 09:22 | conventional-commits | pull_request | feat/opencode-integration | success | 13s | 9s |
| 2026-07-15 09:22 | ci | pull_request | feat/opencode-integration | failure | 10m13s | 14m41s |
| 2026-07-15 09:21 | conventional-commits | pull_request | fix/relay-retained-snapshot | success | 16s | 13s |
| 2026-07-15 09:17 | conventional-commits | pull_request | release-please--branches--main-- | success | 15s | 12s |
| 2026-07-15 09:17 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-07-15 09:17 | stress | pull_request | release-please--branches--main-- | skipped | 6s | 0s |
| 2026-07-15 09:17 | ci | pull_request | release-please--branches--main-- | skipped | 7s | 0s |
| 2026-07-15 09:17 | conventional-commits | pull_request | release-please--branches--main-- | success | 20s | 17s |
| 2026-07-15 09:16 | release-please | push | main | success | 43s | 38s |
| 2026-07-15 09:16 | conventional-commits | pull_request | fix/watch-dirty-idle-starvation | success | 19s | 16s |
| 2026-07-15 09:16 | ci | pull_request | fix/watch-dirty-idle-starvation | success | 16m49s | 20m52s |
| 2026-07-15 09:02 | conventional-commits | pull_request | fix/watch-dirty-idle-starvation | success | 15s | 11s |
| 2026-07-15 09:02 | ci | pull_request | fix/watch-dirty-idle-starvation | success | 13m15s | 17m16s |
| 2026-07-15 08:59 | conventional-commits | pull_request | feat/pi-extension | success | 21s | 16s |
| 2026-07-15 08:59 | ci | pull_request | feat/pi-extension | success | 16m52s | 21m07s |
| 2026-07-15 08:44 | stress | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-07-15 08:44 | release-please | push | main | success | 7m03s | 17m10s |
| 2026-07-15 08:44 | observatory | push | main | success | 11m54s | 23m34s |
| 2026-07-15 08:44 | ci | push | main | success | 17m33s | 21m51s |
| 2026-07-15 08:27 | ci | pull_request | release-please--branches--main-- | success | 16m25s | 20m25s |
| 2026-07-15 04:55 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-07-15 04:55 | conventional-commits | pull_request | release-please--branches--main-- | success | 14s | 11s |
| 2026-07-15 04:54 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-07-15 04:54 | conventional-commits | pull_request | release-please--branches--main-- | cancelled | 2s | 1s |
| 2026-07-15 04:54 | conventional-commits | pull_request | release-please--branches--main-- | success | 17s | 14s |
| 2026-07-15 04:54 | release-please | push | main | success | 35s | 30s |
| 2026-07-15 04:54 | ci | push | main | success | 13m37s | 16m04s |
| 2026-07-15 04:40 | conventional-commits | pull_request | fix/dashboard-first-size-record | success | 16s | 12s |
| 2026-07-15 04:40 | ci | pull_request | fix/dashboard-first-size-record | success | 13m55s | 16m26s |
| 2026-07-15 04:26 | observatory | workflow_dispatch | main | success | 11m44s | 23m06s |

---

Query the raw store directly, e.g. every recorded ci run's wall time:

```sh
git fetch origin ci-metrics && git show origin/ci-metrics:runs/2026-07.ndjson \
  | jq -r 'select(.kind == "run" and .workflow == "ci") | [.created_at, .conclusion, .duration_s] | @tsv'
```
