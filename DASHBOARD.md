# phux CI dashboard

Generated 2026-07-26T03:53:58Z by the ci-metrics workflow. Do not edit —
every table is re-rendered from `runs/*.ndjson` on each update.
Machine rollup: [`site/summary.json`](site/summary.json), rendered live at
<https://phux.phall.io/ci>.

## Workflows, last 30 days

| workflow | runs | success | median | p95 | runner minutes |
|---|---:|---:|---:|---:|---:|
| ci | 226 | 58% | 13m31s | 18m11s | 2675 |
| stress | 20 | 65% | 18m05s | 23m45s | 257 |
| observatory | 10 | 80% | 12m07s | 12m56s | 215 |
| release-please | 43 | 98% | 44s | 54s | 79 |
| conventional-commits | 209 | 82% | 16s | 21s | 43 |

## ci jobs, last 30 days

| job | runs | median queue | median wall | p95 wall |
|---|---:|---:|---:|---:|
| test | 223 | 2s | 13m22s | 17m49s |
| check | 221 | 2s | 2m49s | 5m05s |
| detect docs-only | 226 | 2s | 5s | 7s |

## Slowest ci steps (median, last 30 days)

| job | step | median | samples |
|---|---|---:|---:|
| test | tests (unit + e2e) | 12m42s | 14 |
| check | rust checks (fmt + clippy + doc + deny) | 1m31s | 13 |
| check | runner disk headroom | 1m08s | 13 |
| test | runner disk headroom | 1m01s | 14 |
| test | Run Swatinem/rust-cache@v2 | 23s | 14 |
| check | Run Swatinem/rust-cache@v2 | 22s | 13 |
| test | agents smoke | 12s | 14 |
| check | docs-check | 10s | 13 |
| check | Run DeterminateSystems/nix-installer-action@v22 | 8s | 13 |
| test | Run DeterminateSystems/nix-installer-action@v22 | 8s | 14 |

## Cargo phases inside the lanes (median, last 30 days)

| workflow / job | phase | median | samples |
|---|---|---:|---:|
| ci / check | clippy | 1m53s | 117 |
| ci / check | doc | 11s | 117 |
| ci / check | deny | 4s | 117 |
| ci / check | fmt | 2s | 120 |
| ci / test | unit | 13m52s | 107 |
| ci / test | e2e | 10s | 106 |
| ci / test | agents-smoke | 1s | 47 |
| observatory / timings | build-dev | 11m06s | 8 |
| observatory / timings | build-release | 5m01s | 9 |
| stress / stress | stress | 19m15s | 11 |

## Cache effectiveness (last 30 days)

| workflow / job | rust-cache hit rate | samples |
|---|---:|---:|
| ci / check | 35% | 127 |
| ci / test | 38% | 125 |
| stress / stress | 18% | 11 |

## Cold build (observatory)

### dev: 11m44s (previous: 11m01s) — 528 units at `0ae92367a`

| slowest units | wall |
|---|---:|
| `libghostty-vt-sys build script (run)` | 122.95s |
| `phux-server lib (test)` | 99.24s |
| `phux bin "phux"` | 81.19s |
| `phux-client lib (test)` | 65.43s |
| `phux-server` | 57.01s |
| `rustls` | 47.45s |
| `phux-server test "spawn_terminal" (test)` | 35.61s |
| `phux-server test "hub_relay_federation" (test)` | 34.74s |

### release: 5m13s (previous: 4m10s) — 362 units at `0ae92367a`

| slowest units | wall |
|---|---:|
| `libghostty-vt-sys build script (run)` | 148.24s |
| `phux bin "phux"` | 117.54s |
| `phux-server` | 24.08s |
| `phux-mcp bin "phux-mcp"` | 22.58s |
| `phux-config` | 22.25s |
| `regex-automata` | 19.92s |
| `clap_builder` | 17.54s |
| `rustls` | 14.46s |

## Release binary size

| binary | size | previous |
|---|---:|---:|
| `phux` | 13.6 MiB | 12.8 MiB |
| `phux-mcp` | 2.1 MiB | 2.1 MiB |

## Dependency graph

- locked packages: **435** (previous: 432) — 12 workspace members, 50 direct deps
- duplicate versions: **32** (previous: 32)
- proc-macro crates: 33; build-script crates: 67

## Slowest tests (latest instrumented run, `96abcd649`)

| test | wall |
|---|---:|
| `phux-server::perf_bursty_output::synthesize_against_reference_alloc_bounded_under_full_churn` | 112.345s |
| `phux-server::terminal_actor::tests::resize_desync_then_both_shrink_does_not_overflow` | 28.244s |
| `phux-server::agent_detect::a_plain_shell_pane_never_gets_an_agent_record` | 6.020s |
| `phux-relay::relay_auth::stalled_preamble_does_not_wedge_relay` | 5.115s |
| `phux-server::agent_detect::deleting_the_record_hands_it_back_to_the_detector` | 3.821s |
| `phux-server::agent_detect::an_identity_only_set_gets_its_state_filled_in_by_the_detector` | 3.821s |
| `phux-server::agent_detect::detector_publishes_blocked_from_a_live_prompt_box` | 3.520s |
| `phux-server::agent_events::unattached_subscriber_receives_events` | 3.020s |
| `phux-server::phux_3uv_acked_incremental::acked_incremental_converges_and_seq_is_monotonic` | 2.517s |
| `phux::bin/phux::commands::overlay::tests::wedged_tailscale_binary_is_killed_at_the_deadline` | 2.022s |

## Recent runs

| when | workflow | event | branch | result | wall | runner time |
|---|---|---|---|---|---:|---:|
| 2026-07-26 03:53 | ci | pull_request | release-please--branches--main-- | skipped | 2s | 0s |
| 2026-07-26 03:53 | conventional-commits | pull_request | release-please--branches--main-- | success | 15s | 12s |
| 2026-07-26 03:52 | conventional-commits | pull_request | release-please--branches--main-- | success | 19s | 15s |
| 2026-07-26 03:52 | conventional-commits | pull_request | release-please--branches--main-- | cancelled | 2s | 0s |
| 2026-07-26 03:52 | ci | pull_request | release-please--branches--main-- | skipped | 10s | 0s |
| 2026-07-26 03:52 | release-please | push | main | success | 45s | 41s |
| 2026-07-26 03:52 | conventional-commits | pull_request | work/version-negotiation | success | 16s | 13s |
| 2026-07-26 02:55 | release-please | push | main | success | 28s | 23s |
| 2026-07-26 02:55 | ci | push | main | success | 16m27s | 19m37s |
| 2026-07-26 02:54 | conventional-commits | pull_request | test/put-file-e2e | success | 19s | 14s |
| 2026-07-26 02:54 | ci | pull_request | test/put-file-e2e | success | 14m30s | 18m09s |
| 2026-07-26 02:16 | ci | pull_request | release-please--branches--main-- | skipped | 2s | 0s |
| 2026-07-26 02:16 | conventional-commits | pull_request | release-please--branches--main-- | success | 20s | 16s |
| 2026-07-26 02:16 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-07-26 02:16 | conventional-commits | pull_request | release-please--branches--main-- | cancelled | 1s | 1s |
| 2026-07-26 02:16 | conventional-commits | pull_request | release-please--branches--main-- | success | 15s | 11s |
| 2026-07-26 02:15 | release-please | push | main | success | 41s | 35s |
| 2026-07-26 02:15 | ci | push | main | success | 15m23s | 18m51s |
| 2026-07-26 02:15 | conventional-commits | pull_request | feat/put-file | success | 14s | 11s |
| 2026-07-26 02:15 | ci | pull_request | feat/put-file | success | 16m21s | 19m52s |
| 2026-07-26 01:46 | ci | pull_request | release-please--branches--main-- | skipped | 2s | 0s |
| 2026-07-26 01:46 | conventional-commits | pull_request | release-please--branches--main-- | success | 15s | 11s |
| 2026-07-26 01:45 | conventional-commits | pull_request | release-please--branches--main-- | cancelled | 2s | 1s |
| 2026-07-26 01:45 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-07-26 01:45 | conventional-commits | pull_request | release-please--branches--main-- | success | 15s | 11s |
| 2026-07-26 01:45 | release-please | push | main | success | 47s | 40s |
| 2026-07-26 01:45 | ci | push | main | success | 13m54s | 16m48s |
| 2026-07-26 01:45 | conventional-commits | pull_request | feat/claude-in-phux | success | 19s | 15s |
| 2026-07-26 01:45 | ci | pull_request | feat/claude-in-phux | success | 14m51s | 18m57s |
| 2026-07-26 01:26 | conventional-commits | pull_request | release-please--branches--main-- | success | 20s | 16s |

---

Query the raw store directly, e.g. every recorded ci run's wall time:

```sh
git fetch origin ci-metrics && git show origin/ci-metrics:runs/2026-07.ndjson \
  | jq -r 'select(.kind == "run" and .workflow == "ci") | [.created_at, .conclusion, .duration_s] | @tsv'
```
