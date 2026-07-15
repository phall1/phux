# phux CI dashboard

Generated 2026-07-15T18:33:17Z by the ci-metrics workflow. Do not edit —
every table is re-rendered from `runs/*.ndjson` on each update.
Machine rollup: [`site/summary.json`](site/summary.json), rendered live at
<https://phux.phall.io/ci>.

## Workflows, last 30 days

| workflow | runs | success | median | p95 | runner minutes |
|---|---:|---:|---:|---:|---:|
| ci | 111 | 59% | 13m17s | 17m10s | 1326 |
| observatory | 4 | 75% | 11m54s | 11m56s | 94 |
| stress | 6 | 50% | 6s | 21m33s | 65 |
| release-please | 18 | 100% | 40s | 52s | 26 |
| conventional-commits | 108 | 86% | 16s | 20s | 20 |

## ci jobs, last 30 days

| job | runs | median queue | median wall | p95 wall |
|---|---:|---:|---:|---:|
| test | 110 | 2s | 13m04s | 16m55s |
| check | 109 | 2s | 2m36s | 4m26s |
| detect docs-only | 111 | 2s | 5s | 7s |

## Slowest ci steps (median, last 30 days)

| job | step | median | samples |
|---|---|---:|---:|
| test | tests (unit + e2e) | 15m00s | 12 |
| check | rust checks (fmt + clippy + doc + deny) | 3m21s | 12 |
| check | Run Swatinem/rust-cache@v2 | 18s | 12 |
| test | Run Swatinem/rust-cache@v2 | 18s | 13 |
| check | Run DeterminateSystems/nix-installer-action@v22 | 9s | 13 |
| check | docs-check | 9s | 11 |
| test | Run DeterminateSystems/nix-installer-action@v22 | 8s | 14 |

## Cargo phases inside the lanes (median, last 30 days)

| workflow / job | phase | median | samples |
|---|---|---:|---:|
| ci / check | clippy | 2m08s | 52 |
| ci / check | doc | 14s | 52 |
| ci / check | deny | 3s | 52 |
| ci / check | fmt | 1s | 54 |
| ci / test | unit | 12m20s | 44 |
| ci / test | e2e | 9s | 43 |
| observatory / timings | build-dev | 10m47s | 3 |
| observatory / timings | build-release | 4m55s | 4 |
| stress / stress | stress | 20m31s | 1 |

## Cache effectiveness (last 30 days)

| workflow / job | rust-cache hit rate | samples |
|---|---:|---:|
| ci / check | 33% | 57 |
| ci / test | 44% | 55 |
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

## Slowest tests (latest instrumented run, `cef4d5a4e`)

| test | wall |
|---|---:|
| `phux-server::perf_bursty_output::synthesize_against_reference_alloc_bounded_under_full_churn` | 106.949s |
| `phux-server::terminal_actor::tests::resize_desync_then_both_shrink_does_not_overflow` | 26.747s |
| `phux-server::agent_detect::a_plain_shell_pane_never_gets_an_agent_record` | 6.018s |
| `phux-server::agent_detect::an_identity_only_set_gets_its_state_filled_in_by_the_detector` | 3.817s |
| `phux-server::agent_detect::deleting_the_record_hands_it_back_to_the_detector` | 3.814s |
| `phux-server::agent_detect::detector_publishes_blocked_from_a_live_prompt_box` | 3.515s |
| `phux-server::agent_events::unattached_subscriber_receives_events` | 3.014s |
| `phux-server::phux_3uv_acked_incremental::acked_incremental_converges_and_seq_is_monotonic` | 2.514s |
| `phux::bin/phux::commands::overlay::tests::wedged_tailscale_binary_is_killed_at_the_deadline` | 2.015s |
| `phux-server::l2_adversarial::test_subscribe_events_no_loss` | 2.015s |

## Recent runs

| when | workflow | event | branch | result | wall | runner time |
|---|---|---|---|---|---:|---:|
| 2026-07-15 18:32 | conventional-commits | pull_request | chore/copy-mode-cleanups | success | 23s | 18s |
| 2026-07-15 18:30 | conventional-commits | pull_request | ci/sccache-workspace-cache | success | 14s | 11s |
| 2026-07-15 18:26 | conventional-commits | pull_request | fix/socket-path-too-long | success | 16s | 12s |
| 2026-07-15 18:25 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-07-15 18:25 | conventional-commits | pull_request | release-please--branches--main-- | success | 13s | 11s |
| 2026-07-15 18:25 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-07-15 18:25 | conventional-commits | pull_request | release-please--branches--main-- | cancelled | 2s | 1s |
| 2026-07-15 18:25 | conventional-commits | pull_request | release-please--branches--main-- | success | 19s | 15s |
| 2026-07-15 18:24 | release-please | push | main | success | 42s | 36s |
| 2026-07-15 18:22 | conventional-commits | pull_request | fix/help-overlay-scroll | success | 14s | 11s |
| 2026-07-15 18:22 | conventional-commits | pull_request | fix/attach-last-flake | success | 15s | 11s |
| 2026-07-15 18:21 | conventional-commits | pull_request | fix/seed-pane-cwd | success | 18s | 14s |
| 2026-07-15 18:20 | conventional-commits | pull_request | fix/phux-socket-inject | success | 17s | 13s |
| 2026-07-15 18:15 | ci | pull_request | feat/phux-detach | success | 17m15s | 19m26s |
| 2026-07-15 18:15 | conventional-commits | pull_request | feat/phux-pair-qr | success | 20s | 16s |
| 2026-07-15 18:15 | ci | pull_request | feat/phux-pair-qr | success | 16m13s | 20m14s |
| 2026-07-15 18:15 | conventional-commits | pull_request | feat/phux-detach | success | 14s | 10s |
| 2026-07-15 18:15 | ci | pull_request | feat/phux-detach | skipped | 1s | 0s |
| 2026-07-15 18:15 | conventional-commits | pull_request | feat/phux-detach | cancelled | 3s | 2s |
| 2026-07-15 18:07 | conventional-commits | pull_request | fix/pi-spatial-parity | success | 15s | 13s |
| 2026-07-15 18:07 | ci | pull_request | fix/pi-spatial-parity | success | 17m10s | 19m19s |
| 2026-07-15 18:07 | conventional-commits | pull_request | fix/pi-spatial-parity | success | 13s | 11s |
| 2026-07-15 18:07 | ci | pull_request | fix/pi-spatial-parity | cancelled | 19s | 23s |
| 2026-07-15 18:05 | conventional-commits | pull_request | release-please--branches--main-- | success | 17s | 10s |
| 2026-07-15 18:05 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-07-15 18:05 | ci | pull_request | release-please--branches--main-- | skipped | 2s | 0s |
| 2026-07-15 18:05 | conventional-commits | pull_request | release-please--branches--main-- | cancelled | 25s | 11s |
| 2026-07-15 18:04 | conventional-commits | pull_request | release-please--branches--main-- | cancelled | 18s | 14s |
| 2026-07-15 18:04 | release-please | push | main | success | 43s | 37s |
| 2026-07-15 18:04 | ci | push | main | success | 13m31s | 18m08s |

---

Query the raw store directly, e.g. every recorded ci run's wall time:

```sh
git fetch origin ci-metrics && git show origin/ci-metrics:runs/2026-07.ndjson \
  | jq -r 'select(.kind == "run" and .workflow == "ci") | [.created_at, .conclusion, .duration_s] | @tsv'
```
