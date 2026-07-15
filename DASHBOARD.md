# phux CI dashboard

Generated 2026-07-15T18:21:40Z by the ci-metrics workflow. Do not edit —
every table is re-rendered from `runs/*.ndjson` on each update.
Machine rollup: [`site/summary.json`](site/summary.json), rendered live at
<https://phux.phall.io/ci>.

## Workflows, last 30 days

| workflow | runs | success | median | p95 | runner minutes |
|---|---:|---:|---:|---:|---:|
| ci | 106 | 58% | 13m15s | 16m54s | 1267 |
| observatory | 4 | 75% | 11m54s | 11m56s | 94 |
| stress | 6 | 50% | 6s | 21m33s | 65 |
| release-please | 17 | 100% | 40s | 52s | 25 |
| conventional-commits | 100 | 86% | 16s | 20s | 19 |

## ci jobs, last 30 days

| job | runs | median queue | median wall | p95 wall |
|---|---:|---:|---:|---:|
| test | 105 | 2s | 13m04s | 16m41s |
| check | 104 | 2s | 2m36s | 4m26s |
| detect docs-only | 106 | 2s | 5s | 7s |

## Slowest ci steps (median, last 30 days)

| job | step | median | samples |
|---|---|---:|---:|
| test | tests (unit + e2e) | 14m04s | 12 |
| check | rust checks (fmt + clippy + doc + deny) | 3m26s | 12 |
| check | Run Swatinem/rust-cache@v2 | 18s | 12 |
| test | Run Swatinem/rust-cache@v2 | 18s | 13 |
| check | Run DeterminateSystems/nix-installer-action@v22 | 9s | 13 |
| check | docs-check | 9s | 11 |
| test | Run DeterminateSystems/nix-installer-action@v22 | 8s | 14 |

## Cargo phases inside the lanes (median, last 30 days)

| workflow / job | phase | median | samples |
|---|---|---:|---:|
| ci / check | clippy | 2m08s | 49 |
| ci / check | doc | 14s | 49 |
| ci / check | deny | 3s | 49 |
| ci / check | fmt | 1s | 51 |
| ci / test | unit | 12m04s | 41 |
| ci / test | e2e | 9s | 40 |
| observatory / timings | build-dev | 10m47s | 3 |
| observatory / timings | build-release | 4m55s | 4 |
| stress / stress | stress | 20m31s | 1 |

## Cache effectiveness (last 30 days)

| workflow / job | rust-cache hit rate | samples |
|---|---:|---:|
| ci / check | 31% | 54 |
| ci / test | 46% | 52 |
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

## Slowest tests (latest instrumented run, `c537b6dc5`)

| test | wall |
|---|---:|
| `phux-server::perf_bursty_output::synthesize_against_reference_alloc_bounded_under_full_churn` | 86.235s |
| `phux-server::terminal_actor::tests::resize_desync_then_both_shrink_does_not_overflow` | 23.667s |
| `phux-server::agent_detect::a_plain_shell_pane_never_gets_an_agent_record` | 6.019s |
| `phux-server::agent_detect::an_identity_only_set_gets_its_state_filled_in_by_the_detector` | 3.819s |
| `phux-server::agent_detect::deleting_the_record_hands_it_back_to_the_detector` | 3.815s |
| `phux-server::agent_detect::detector_publishes_blocked_from_a_live_prompt_box` | 3.518s |
| `phux-server::agent_events::unattached_subscriber_receives_events` | 3.015s |
| `phux-server::phux_3uv_acked_incremental::acked_incremental_converges_and_seq_is_monotonic` | 2.515s |
| `phux::bin/phux::commands::overlay::tests::wedged_tailscale_binary_is_killed_at_the_deadline` | 2.020s |
| `phux-server::l2_adversarial::test_subscribe_events_no_loss` | 2.016s |

## Recent runs

| when | workflow | event | branch | result | wall | runner time |
|---|---|---|---|---|---:|---:|
| 2026-07-15 18:21 | conventional-commits | pull_request | fix/seed-pane-cwd | success | 18s | 14s |
| 2026-07-15 18:20 | conventional-commits | pull_request | fix/phux-socket-inject | success | 17s | 13s |
| 2026-07-15 18:15 | conventional-commits | pull_request | feat/phux-pair-qr | success | 20s | 16s |
| 2026-07-15 18:15 | conventional-commits | pull_request | feat/phux-detach | success | 14s | 10s |
| 2026-07-15 18:15 | ci | pull_request | feat/phux-detach | skipped | 1s | 0s |
| 2026-07-15 18:15 | conventional-commits | pull_request | feat/phux-detach | cancelled | 3s | 2s |
| 2026-07-15 18:07 | conventional-commits | pull_request | fix/pi-spatial-parity | success | 15s | 13s |
| 2026-07-15 18:07 | conventional-commits | pull_request | fix/pi-spatial-parity | success | 13s | 11s |
| 2026-07-15 18:07 | ci | pull_request | fix/pi-spatial-parity | cancelled | 19s | 23s |
| 2026-07-15 18:05 | conventional-commits | pull_request | release-please--branches--main-- | success | 17s | 10s |
| 2026-07-15 18:05 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-07-15 18:05 | ci | pull_request | release-please--branches--main-- | skipped | 2s | 0s |
| 2026-07-15 18:05 | conventional-commits | pull_request | release-please--branches--main-- | cancelled | 25s | 11s |
| 2026-07-15 18:04 | conventional-commits | pull_request | release-please--branches--main-- | cancelled | 18s | 14s |
| 2026-07-15 18:04 | release-please | push | main | success | 43s | 37s |
| 2026-07-15 18:04 | ci | push | main | success | 13m31s | 18m08s |
| 2026-07-15 17:59 | ci | pull_request | feat/phux-pair-qr | success | 15m00s | 18m58s |
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

---

Query the raw store directly, e.g. every recorded ci run's wall time:

```sh
git fetch origin ci-metrics && git show origin/ci-metrics:runs/2026-07.ndjson \
  | jq -r 'select(.kind == "run" and .workflow == "ci") | [.created_at, .conclusion, .duration_s] | @tsv'
```
