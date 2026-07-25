# phux CI dashboard

Generated 2026-07-25T23:15:41Z by the ci-metrics workflow. Do not edit —
every table is re-rendered from `runs/*.ndjson` on each update.
Machine rollup: [`site/summary.json`](site/summary.json), rendered live at
<https://phux.phall.io/ci>.

## Workflows, last 30 days

| workflow | runs | success | median | p95 | runner minutes |
|---|---:|---:|---:|---:|---:|
| ci | 194 | 60% | 13m31s | 18m24s | 2373 |
| stress | 20 | 65% | 18m05s | 23m45s | 257 |
| observatory | 10 | 80% | 12m07s | 12m56s | 215 |
| release-please | 34 | 97% | 43s | 7m03s | 74 |
| conventional-commits | 177 | 84% | 16s | 21s | 38 |

## ci jobs, last 30 days

| job | runs | median queue | median wall | p95 wall |
|---|---:|---:|---:|---:|
| test | 193 | 2s | 13m21s | 17m58s |
| check | 190 | 2s | 2m47s | 5m11s |
| detect docs-only | 194 | 2s | 5s | 7s |

## Slowest ci steps (median, last 30 days)

| job | step | median | samples |
|---|---|---:|---:|
| test | tests (unit + e2e) | 14m34s | 10 |
| check | rust checks (fmt + clippy + doc + deny) | 3m03s | 10 |
| check | runner disk headroom | 1m08s | 12 |
| test | runner disk headroom | 1m01s | 12 |
| check | Run Swatinem/rust-cache@v2 | 18s | 12 |
| test | Run Swatinem/rust-cache@v2 | 17s | 12 |
| test | agents smoke | 12s | 10 |
| check | docs-check | 10s | 12 |
| check | Run DeterminateSystems/nix-installer-action@v22 | 8s | 12 |
| test | Run DeterminateSystems/nix-installer-action@v22 | 8s | 12 |

## Cargo phases inside the lanes (median, last 30 days)

| workflow / job | phase | median | samples |
|---|---|---:|---:|
| ci / check | clippy | 2m00s | 101 |
| ci / check | doc | 12s | 101 |
| ci / check | deny | 4s | 101 |
| ci / check | fmt | 1s | 104 |
| ci / test | unit | 14m13s | 91 |
| ci / test | e2e | 10s | 90 |
| ci / test | agents-smoke | 1s | 31 |
| observatory / timings | build-dev | 11m06s | 8 |
| observatory / timings | build-release | 5m01s | 9 |
| stress / stress | stress | 19m15s | 11 |

## Cache effectiveness (last 30 days)

| workflow / job | rust-cache hit rate | samples |
|---|---:|---:|
| ci / check | 30% | 111 |
| ci / test | 31% | 109 |
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

## Slowest tests (latest instrumented run, `5c2df5a8f`)

| test | wall |
|---|---:|
| `phux-server::perf_bursty_output::synthesize_against_reference_alloc_bounded_under_full_churn` | 110.607s |
| `phux-server::terminal_actor::tests::resize_desync_then_both_shrink_does_not_overflow` | 27.157s |
| `phux-server::agent_detect::a_plain_shell_pane_never_gets_an_agent_record` | 6.018s |
| `phux-relay::relay_auth::stalled_preamble_does_not_wedge_relay` | 5.113s |
| `phux-server::agent_detect::an_identity_only_set_gets_its_state_filled_in_by_the_detector` | 3.817s |
| `phux-server::agent_detect::deleting_the_record_hands_it_back_to_the_detector` | 3.815s |
| `phux-server::agent_detect::detector_publishes_blocked_from_a_live_prompt_box` | 3.517s |
| `phux-server::agent_events::unattached_subscriber_receives_events` | 3.014s |
| `phux-server::phux_3uv_acked_incremental::acked_incremental_converges_and_seq_is_monotonic` | 2.515s |
| `phux::bin/phux::commands::overlay::tests::wedged_tailscale_binary_is_killed_at_the_deadline` | 2.027s |

## Recent runs

| when | workflow | event | branch | result | wall | runner time |
|---|---|---|---|---|---:|---:|
| 2026-07-25 23:15 | conventional-commits | pull_request | fix/send-keys-paste-enter | success | 19s | 17s |
| 2026-07-25 22:54 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-07-25 22:54 | conventional-commits | pull_request | release-please--branches--main-- | success | 17s | 13s |
| 2026-07-25 22:54 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-07-25 22:54 | conventional-commits | pull_request | release-please--branches--main-- | success | 18s | 14s |
| 2026-07-25 22:53 | release-please | push | main | success | 52s | 45s |
| 2026-07-25 22:53 | ci | push | main | success | 15m49s | 19m30s |
| 2026-07-25 22:53 | conventional-commits | pull_request | feat/right-click-context-menus | success | 15s | 11s |
| 2026-07-25 22:53 | ci | pull_request | feat/right-click-context-menus | success | 17m28s | 21m09s |
| 2026-07-25 22:53 | conventional-commits | pull_request | feat/connector-productization | success | 13s | 11s |
| 2026-07-25 22:53 | ci | pull_request | feat/connector-productization | success | 15m13s | 20m13s |
| 2026-07-25 21:54 | ci | pull_request | release-please--branches--main-- | skipped | 3s | 0s |
| 2026-07-25 21:54 | conventional-commits | pull_request | release-please--branches--main-- | success | 16s | 13s |
| 2026-07-25 21:54 | conventional-commits | pull_request | release-please--branches--main-- | cancelled | 2s | 1s |
| 2026-07-25 21:54 | ci | pull_request | release-please--branches--main-- | skipped | 8s | 0s |
| 2026-07-25 21:54 | conventional-commits | pull_request | release-please--branches--main-- | success | 15s | 10s |
| 2026-07-25 21:53 | release-please | push | main | success | 45s | 37s |
| 2026-07-25 21:53 | observatory | push | main | success | 13m00s | 24m57s |
| 2026-07-25 21:53 | ci | push | main | success | 28m57s | 23m34s |
| 2026-07-25 21:53 | conventional-commits | pull_request | feat/relay-alpn-dialer | success | 19s | 15s |
| 2026-07-25 21:53 | ci | pull_request | feat/relay-alpn-dialer | success | 18m11s | 23m22s |
| 2026-07-25 21:34 | conventional-commits | pull_request | release-please--branches--main-- | success | 21s | 17s |
| 2026-07-25 21:34 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-07-25 21:33 | conventional-commits | pull_request | release-please--branches--main-- | cancelled | 1s | 0s |
| 2026-07-25 21:33 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-07-25 21:33 | conventional-commits | pull_request | release-please--branches--main-- | success | 18s | 14s |
| 2026-07-25 21:33 | release-please | push | main | success | 50s | 43s |
| 2026-07-25 21:33 | ci | push | main | success | 30m13s | 20m50s |
| 2026-07-25 21:30 | conventional-commits | pull_request | feat/cross-session-pane-move | success | 25s | 17s |
| 2026-07-25 21:30 | conventional-commits | pull_request | feat/cross-session-pane-move | cancelled | 7s | 4s |

---

Query the raw store directly, e.g. every recorded ci run's wall time:

```sh
git fetch origin ci-metrics && git show origin/ci-metrics:runs/2026-07.ndjson \
  | jq -r 'select(.kind == "run" and .workflow == "ci") | [.created_at, .conclusion, .duration_s] | @tsv'
```
