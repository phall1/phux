# phux CI dashboard

Generated 2026-07-15T11:12:48Z by the ci-metrics workflow. Do not edit —
every table is re-rendered from `runs/*.ndjson` on each update.
Machine rollup: [`site/summary.json`](site/summary.json), rendered live at
<https://phux.phall.io/ci>.

## Workflows, last 30 days

| workflow | runs | success | median | p95 | runner minutes |
|---|---:|---:|---:|---:|---:|
| ci | 87 | 61% | 13m29s | 16m52s | 1081 |
| observatory | 3 | 67% | 11m54s | 11m54s | 70 |
| stress | 6 | 50% | 6s | 21m33s | 65 |
| release-please | 14 | 100% | 37s | 48s | 23 |
| conventional-commits | 79 | 90% | 16s | 19s | 15 |

## ci jobs, last 30 days

| job | runs | median queue | median wall | p95 wall |
|---|---:|---:|---:|---:|
| test | 86 | 2s | 13m19s | 16m41s |
| check | 85 | 2s | 2m36s | 4m23s |
| detect docs-only | 87 | 2s | 5s | 7s |

## Slowest ci steps (median, last 30 days)

| job | step | median | samples |
|---|---|---:|---:|
| test | tests (unit + e2e) | 13m14s | 13 |
| check | rust checks (fmt + clippy + doc + deny) | 2m29s | 18 |
| test | Run Swatinem/rust-cache@v2 | 24s | 17 |
| check | Run Swatinem/rust-cache@v2 | 21s | 18 |
| check | docs-check | 9s | 18 |
| test | Run DeterminateSystems/nix-installer-action@v22 | 9s | 18 |
| check | Run DeterminateSystems/nix-installer-action@v22 | 8s | 19 |

## Cargo phases inside the lanes (median, last 30 days)

| workflow / job | phase | median | samples |
|---|---|---:|---:|
| ci / check | clippy | 1m59s | 39 |
| ci / check | doc | 14s | 39 |
| ci / check | deny | 3s | 39 |
| ci / check | fmt | 1s | 41 |
| ci / test | unit | 11m57s | 32 |
| ci / test | e2e | 9s | 31 |
| observatory / timings | build-dev | 10m39s | 2 |
| observatory / timings | build-release | 4m55s | 3 |
| stress / stress | stress | 20m31s | 1 |

## Cache effectiveness (last 30 days)

| workflow / job | rust-cache hit rate | samples |
|---|---:|---:|
| ci / check | 39% | 44 |
| ci / test | 52% | 42 |
| stress / stress | 0% | 1 |

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

### release: 4m56s (previous: 5m01s) — 358 units at `44ade3883`

| slowest units | wall |
|---|---:|
| `libghostty-vt-sys build script (run)` | 154.65s |
| `phux bin "phux"` | 98.23s |
| `regex-automata` | 21.84s |
| `phux-server` | 20.36s |
| `phux-mcp bin "phux-mcp"` | 20.06s |
| `phux-config` | 17.83s |
| `rustls` | 16.0s |
| `clap_builder` | 14.47s |

## Release binary size

| binary | size | previous |
|---|---:|---:|
| `phux` | 12.6 MiB | 12.6 MiB |
| `phux-mcp` | 2.0 MiB | 2.0 MiB |

## Dependency graph

- locked packages: **431** (previous: 431) — 11 workspace members, 47 direct deps
- duplicate versions: **32** (previous: 32)
- proc-macro crates: 33; build-script crates: 67

## Slowest tests (latest instrumented run, `ea44ca48a`)

| test | wall |
|---|---:|
| `phux-server::perf_bursty_output::synthesize_against_reference_alloc_bounded_under_full_churn` | 111.564s |
| `phux-server::terminal_actor::tests::resize_desync_then_both_shrink_does_not_overflow` | 26.981s |
| `phux-server::agent_detect::a_plain_shell_pane_never_gets_an_agent_record` | 6.016s |
| `phux-server::agent_detect::an_identity_only_set_gets_its_state_filled_in_by_the_detector` | 3.816s |
| `phux-server::agent_detect::deleting_the_record_hands_it_back_to_the_detector` | 3.814s |
| `phux-server::agent_detect::detector_publishes_blocked_from_a_live_prompt_box` | 3.516s |
| `phux-server::agent_events::unattached_subscriber_receives_events` | 3.014s |
| `phux-server::phux_3uv_acked_incremental::acked_incremental_converges_and_seq_is_monotonic` | 2.514s |
| `phux-server::l2_adversarial::test_subscribe_events_no_loss` | 2.015s |
| `phux-server::phux_0q8_no_double_emit::live_output_is_delivered_exactly_once` | 1.515s |

## Recent runs

| when | workflow | event | branch | result | wall | runner time |
|---|---|---|---|---|---:|---:|
| 2026-07-15 11:12 | conventional-commits | pull_request | swarm/spatial-final | success | 18s | 10s |
| 2026-07-15 11:11 | conventional-commits | pull_request | swarm/spatial-final | cancelled | 14s | 10s |
| 2026-07-15 11:11 | conventional-commits | pull_request | swarm/spatial-final | success | 15s | 12s |
| 2026-07-15 11:11 | ci | pull_request | swarm/spatial-final | cancelled | 48s | 1m15s |
| 2026-07-15 11:09 | conventional-commits | pull_request | swarm/input-encode-final | success | 14s | 10s |
| 2026-07-15 11:00 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-07-15 11:00 | conventional-commits | pull_request | release-please--branches--main-- | success | 18s | 15s |
| 2026-07-15 10:59 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-07-15 10:59 | conventional-commits | pull_request | release-please--branches--main-- | cancelled | 2s | 0s |
| 2026-07-15 10:59 | conventional-commits | pull_request | release-please--branches--main-- | success | 17s | 12s |
| 2026-07-15 10:59 | release-please | push | main | success | 42s | 35s |
| 2026-07-15 10:59 | observatory | push | main | failure | 11m56s | 23m14s |
| 2026-07-15 10:42 | conventional-commits | pull_request | swarm/foundation-train | success | 14s | 10s |
| 2026-07-15 10:42 | ci | pull_request | swarm/foundation-train | success | 16m44s | 21m10s |
| 2026-07-15 10:41 | conventional-commits | pull_request | swarm/input-encode-train | success | 17s | 13s |
| 2026-07-15 10:41 | ci | pull_request | swarm/input-encode-train | success | 16m44s | 20m54s |
| 2026-07-15 10:29 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-07-15 10:29 | conventional-commits | pull_request | release-please--branches--main-- | success | 15s | 12s |
| 2026-07-15 10:28 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-07-15 10:28 | conventional-commits | pull_request | release-please--branches--main-- | cancelled | 1s | 1s |
| 2026-07-15 10:28 | conventional-commits | pull_request | release-please--branches--main-- | success | 14s | 11s |
| 2026-07-15 10:28 | release-please | push | main | success | 38s | 33s |
| 2026-07-15 10:28 | ci | push | main | success | 13m38s | 15m46s |
| 2026-07-15 10:27 | conventional-commits | pull_request | swarm/spawn-placement | success | 16s | 12s |
| 2026-07-15 10:27 | ci | pull_request | swarm/spawn-placement | success | 16m54s | 20m35s |
| 2026-07-15 10:25 | conventional-commits | pull_request | swarm/foundation-train | success | 15s | 12s |
| 2026-07-15 10:25 | ci | pull_request | swarm/foundation-train | success | 17m05s | 21m13s |
| 2026-07-15 10:25 | conventional-commits | pull_request | swarm/layout-cli | success | 16s | 13s |
| 2026-07-15 10:25 | ci | pull_request | swarm/layout-cli | success | 14m49s | 16m46s |
| 2026-07-15 10:19 | conventional-commits | pull_request | fix/client-local-focus | success | 14s | 10s |

---

Query the raw store directly, e.g. every recorded ci run's wall time:

```sh
git fetch origin ci-metrics && git show origin/ci-metrics:runs/2026-07.ndjson \
  | jq -r 'select(.kind == "run" and .workflow == "ci") | [.created_at, .conclusion, .duration_s] | @tsv'
```
