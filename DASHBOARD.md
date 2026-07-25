# phux CI dashboard

Generated 2026-07-25T21:29:01Z by the ci-metrics workflow. Do not edit —
every table is re-rendered from `runs/*.ndjson` on each update.
Machine rollup: [`site/summary.json`](site/summary.json), rendered live at
<https://phux.phall.io/ci>.

## Workflows, last 30 days

| workflow | runs | success | median | p95 | runner minutes |
|---|---:|---:|---:|---:|---:|
| ci | 177 | 59% | 13m31s | 17m57s | 2165 |
| stress | 20 | 65% | 18m05s | 23m45s | 257 |
| observatory | 9 | 78% | 12m07s | 12m42s | 190 |
| release-please | 31 | 97% | 43s | 7m03s | 72 |
| conventional-commits | 163 | 84% | 16s | 20s | 35 |

## ci jobs, last 30 days

| job | runs | median queue | median wall | p95 wall |
|---|---:|---:|---:|---:|
| test | 176 | 2s | 13m19s | 17m46s |
| check | 174 | 2s | 2m40s | 5m01s |
| detect docs-only | 177 | 2s | 5s | 7s |

## Slowest ci steps (median, last 30 days)

| job | step | median | samples |
|---|---|---:|---:|
| test | tests (unit + e2e) | 15m44s | 11 |
| check | rust checks (fmt + clippy + doc + deny) | 3m03s | 11 |
| check | runner disk headroom | 1m01s | 13 |
| test | runner disk headroom | 54s | 13 |
| check | Run Swatinem/rust-cache@v2 | 19s | 13 |
| test | Run Swatinem/rust-cache@v2 | 17s | 13 |
| test | agents smoke | 12s | 11 |
| check | docs-check | 10s | 13 |
| check | Run DeterminateSystems/nix-installer-action@v22 | 8s | 13 |
| test | Run DeterminateSystems/nix-installer-action@v22 | 8s | 13 |

## Cargo phases inside the lanes (median, last 30 days)

| workflow / job | phase | median | samples |
|---|---|---:|---:|
| ci / check | clippy | 1m59s | 92 |
| ci / check | doc | 12s | 92 |
| ci / check | deny | 4s | 92 |
| ci / check | fmt | 1s | 95 |
| ci / test | unit | 14m13s | 82 |
| ci / test | e2e | 10s | 81 |
| ci / test | agents-smoke | 1s | 22 |
| observatory / timings | build-dev | 11m06s | 7 |
| observatory / timings | build-release | 5m00s | 8 |
| stress / stress | stress | 19m15s | 11 |

## Cache effectiveness (last 30 days)

| workflow / job | rust-cache hit rate | samples |
|---|---:|---:|
| ci / check | 30% | 100 |
| ci / test | 31% | 98 |
| stress / stress | 18% | 11 |

## Cold build (observatory)

### dev: 11m01s (previous: 11m27s) — 520 units at `a27ecc10d`

| slowest units | wall |
|---|---:|
| `libghostty-vt-sys build script (run)` | 111.14s |
| `phux-server lib (test)` | 89.32s |
| `phux bin "phux"` | 71.94s |
| `phux-client lib (test)` | 63.33s |
| `phux-server` | 54.26s |
| `rustls` | 46.5s |
| `phux-server test "spawn_terminal" (test)` | 34.2s |
| `phux-server test "hub_relay_federation" (test)` | 33.44s |

### release: 4m10s (previous: 5m07s) — 359 units at `a27ecc10d`

| slowest units | wall |
|---|---:|
| `libghostty-vt-sys build script (run)` | 113.58s |
| `phux bin "phux"` | 95.73s |
| `phux-server` | 19.85s |
| `phux-mcp bin "phux-mcp"` | 19.15s |
| `regex-automata` | 16.16s |
| `phux-config` | 15.04s |
| `rustls` | 13.25s |
| `tracing-subscriber` | 9.55s |

## Release binary size

| binary | size | previous |
|---|---:|---:|
| `phux` | 12.8 MiB | 12.9 MiB |
| `phux-mcp` | 2.1 MiB | 2.1 MiB |

## Dependency graph

- locked packages: **432** (previous: 432) — 11 workspace members, 48 direct deps
- duplicate versions: **32** (previous: 32)
- proc-macro crates: 33; build-script crates: 67

## Slowest tests (latest instrumented run, `a7fc71589`)

| test | wall |
|---|---:|
| `phux-server::perf_bursty_output::synthesize_against_reference_alloc_bounded_under_full_churn` | 93.252s |
| `phux-server::terminal_actor::tests::resize_desync_then_both_shrink_does_not_overflow` | 23.505s |
| `phux-server::agent_detect::a_plain_shell_pane_never_gets_an_agent_record` | 6.015s |
| `phux-server::agent_detect::an_identity_only_set_gets_its_state_filled_in_by_the_detector` | 3.813s |
| `phux-server::agent_detect::deleting_the_record_hands_it_back_to_the_detector` | 3.812s |
| `phux-server::agent_detect::detector_publishes_blocked_from_a_live_prompt_box` | 3.513s |
| `phux-server::agent_events::unattached_subscriber_receives_events` | 3.013s |
| `phux-server::phux_3uv_acked_incremental::acked_incremental_converges_and_seq_is_monotonic` | 2.512s |
| `phux-server::l2_adversarial::test_subscribe_events_no_loss` | 2.014s |
| `phux::bin/phux::commands::overlay::tests::wedged_tailscale_binary_is_killed_at_the_deadline` | 2.013s |

## Recent runs

| when | workflow | event | branch | result | wall | runner time |
|---|---|---|---|---|---:|---:|
| 2026-07-25 21:27 | ci | pull_request | release-please--branches--main-- | skipped | 2s | 0s |
| 2026-07-25 21:27 | conventional-commits | pull_request | release-please--branches--main-- | success | 15s | 11s |
| 2026-07-25 21:27 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-07-25 21:27 | conventional-commits | pull_request | release-please--branches--main-- | cancelled | 2s | 1s |
| 2026-07-25 21:27 | conventional-commits | pull_request | release-please--branches--main-- | success | 18s | 13s |
| 2026-07-25 21:26 | release-please | push | main | success | 49s | 44s |
| 2026-07-25 21:26 | conventional-commits | pull_request | feat/always-on-server-and-ssh-en | success | 14s | 11s |
| 2026-07-25 21:26 | conventional-commits | pull_request | feat/cross-session-pane-move | success | 13s | 9s |
| 2026-07-25 20:50 | ci | push | main | failure | 4s | 2s |
| 2026-07-25 20:50 | observatory | push | main | failure | 4s | 6s |
| 2026-07-25 20:50 | release-please | push | main | failure | 4s | 2s |
| 2026-07-25 19:08 | ci | pull_request | feat/phux-doctor | failure | 5s | 3s |
| 2026-07-25 19:08 | conventional-commits | pull_request | feat/phux-doctor | failure | 5s | 3s |
| 2026-07-25 19:08 | conventional-commits | pull_request | feat/phux-doctor | success | 19s | 16s |
| 2026-07-25 18:45 | conventional-commits | pull_request | feat/herdr-parity-wave2 | failure | 4s | 4s |
| 2026-07-25 18:45 | ci | pull_request | feat/herdr-parity-wave2 | failure | 4s | 3s |
| 2026-07-25 18:45 | conventional-commits | pull_request | feat/herdr-parity-wave2 | success | 19s | 15s |
| 2026-07-25 18:45 | ci | pull_request | feat/herdr-parity-wave2 | success | 15m26s | 20m50s |
| 2026-07-25 17:45 | conventional-commits | pull_request | feat/always-on-server-and-ssh-en | failure | 5s | 4s |
| 2026-07-25 17:45 | ci | pull_request | feat/always-on-server-and-ssh-en | failure | 6s | 5s |
| 2026-07-25 17:16 | ci | pull_request | feat/herdr-parity-wave1 | failure | 4s | 3s |
| 2026-07-25 17:16 | conventional-commits | pull_request | feat/herdr-parity-wave1 | success | 18s | 15s |
| 2026-07-25 09:04 | stress | schedule | main | success | 18m05s | 18m01s |
| 2026-07-25 01:40 | ci | pull_request | release-please--branches--main-- | skipped | 2s | 0s |
| 2026-07-25 01:40 | conventional-commits | pull_request | release-please--branches--main-- | success | 19s | 14s |
| 2026-07-25 01:40 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-07-25 01:40 | conventional-commits | pull_request | release-please--branches--main-- | cancelled | 1s | 1s |
| 2026-07-25 01:40 | conventional-commits | pull_request | release-please--branches--main-- | success | 21s | 16s |
| 2026-07-25 01:39 | release-please | push | main | success | 39s | 35s |
| 2026-07-25 01:39 | ci | push | main | success | 18m24s | 24m18s |

---

Query the raw store directly, e.g. every recorded ci run's wall time:

```sh
git fetch origin ci-metrics && git show origin/ci-metrics:runs/2026-07.ndjson \
  | jq -r 'select(.kind == "run" and .workflow == "ci") | [.created_at, .conclusion, .duration_s] | @tsv'
```
