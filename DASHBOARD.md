# phux CI dashboard

Generated 2026-07-26T00:06:35Z by the ci-metrics workflow. Do not edit —
every table is re-rendered from `runs/*.ndjson` on each update.
Machine rollup: [`site/summary.json`](site/summary.json), rendered live at
<https://phux.phall.io/ci>.

## Workflows, last 30 days

| workflow | runs | success | median | p95 | runner minutes |
|---|---:|---:|---:|---:|---:|
| ci | 204 | 59% | 13m29s | 18m14s | 2451 |
| stress | 20 | 65% | 18m05s | 23m45s | 257 |
| observatory | 10 | 80% | 12m07s | 12m56s | 215 |
| release-please | 37 | 97% | 44s | 7m03s | 76 |
| conventional-commits | 187 | 83% | 16s | 21s | 39 |

## ci jobs, last 30 days

| job | runs | median queue | median wall | p95 wall |
|---|---:|---:|---:|---:|
| test | 202 | 2s | 13m19s | 17m51s |
| check | 200 | 2s | 2m43s | 5m11s |
| detect docs-only | 204 | 2s | 5s | 7s |

## Slowest ci steps (median, last 30 days)

| job | step | median | samples |
|---|---|---:|---:|
| test | tests (unit + e2e) | 13m03s | 12 |
| check | rust checks (fmt + clippy + doc + deny) | 2m52s | 12 |
| test | runner disk headroom | 1m09s | 13 |
| check | runner disk headroom | 1m00s | 13 |
| test | Run Swatinem/rust-cache@v2 | 22s | 13 |
| check | Run Swatinem/rust-cache@v2 | 17s | 13 |
| test | agents smoke | 12s | 12 |
| check | docs-check | 10s | 13 |
| check | Run DeterminateSystems/nix-installer-action@v22 | 8s | 13 |
| test | Run DeterminateSystems/nix-installer-action@v22 | 8s | 13 |

## Cargo phases inside the lanes (median, last 30 days)

| workflow / job | phase | median | samples |
|---|---|---:|---:|
| ci / check | clippy | 1m59s | 105 |
| ci / check | doc | 11s | 105 |
| ci / check | deny | 4s | 105 |
| ci / check | fmt | 1s | 108 |
| ci / test | unit | 14m04s | 95 |
| ci / test | e2e | 10s | 94 |
| ci / test | agents-smoke | 1s | 35 |
| observatory / timings | build-dev | 11m06s | 8 |
| observatory / timings | build-release | 5m01s | 9 |
| stress / stress | stress | 19m15s | 11 |

## Cache effectiveness (last 30 days)

| workflow / job | rust-cache hit rate | samples |
|---|---:|---:|
| ci / check | 29% | 115 |
| ci / test | 33% | 113 |
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

## Slowest tests (latest instrumented run, `9b2d8a25b`)

| test | wall |
|---|---:|
| `phux-server::perf_bursty_output::synthesize_against_reference_alloc_bounded_under_full_churn` | 112.398s |
| `phux-server::terminal_actor::tests::resize_desync_then_both_shrink_does_not_overflow` | 28.001s |
| `phux-server::agent_detect::a_plain_shell_pane_never_gets_an_agent_record` | 6.016s |
| `phux-relay::relay_auth::stalled_preamble_does_not_wedge_relay` | 5.113s |
| `phux-server::agent_detect::deleting_the_record_hands_it_back_to_the_detector` | 3.815s |
| `phux-server::agent_detect::an_identity_only_set_gets_its_state_filled_in_by_the_detector` | 3.815s |
| `phux-server::agent_detect::detector_publishes_blocked_from_a_live_prompt_box` | 3.516s |
| `phux-server::agent_events::unattached_subscriber_receives_events` | 3.014s |
| `phux-server::phux_3uv_acked_incremental::acked_incremental_converges_and_seq_is_monotonic` | 2.515s |
| `phux::bin/phux::commands::overlay::tests::wedged_tailscale_binary_is_killed_at_the_deadline` | 2.019s |

## Recent runs

| when | workflow | event | branch | result | wall | runner time |
|---|---|---|---|---|---:|---:|
| 2026-07-26 00:06 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-07-26 00:05 | conventional-commits | pull_request | release-please--branches--main-- | success | 22s | 17s |
| 2026-07-26 00:05 | conventional-commits | pull_request | release-please--branches--main-- | cancelled | 2s | 0s |
| 2026-07-26 00:05 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-07-26 00:05 | release-please | push | main | success | 45s | 38s |
| 2026-07-26 00:05 | conventional-commits | pull_request | feat/agent-detection-fixtures | success | 21s | 18s |
| 2026-07-25 23:51 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-07-25 23:51 | conventional-commits | pull_request | release-please--branches--main-- | success | 13s | 10s |
| 2026-07-25 23:51 | conventional-commits | pull_request | release-please--branches--main-- | success | 19s | 15s |
| 2026-07-25 23:51 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-07-25 23:51 | conventional-commits | pull_request | release-please--branches--main-- | cancelled | 2s | 0s |
| 2026-07-25 23:50 | release-please | push | main | success | 46s | 40s |
| 2026-07-25 23:50 | ci | push | main | success | 15m15s | 19m44s |
| 2026-07-25 23:36 | conventional-commits | pull_request | feat/agent-detection-manifests | success | 22s | 18s |
| 2026-07-25 23:36 | ci | pull_request | feat/agent-detection-manifests | success | 14m43s | 19m32s |
| 2026-07-25 23:16 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-07-25 23:16 | conventional-commits | pull_request | release-please--branches--main-- | success | 13s | 10s |
| 2026-07-25 23:15 | conventional-commits | pull_request | release-please--branches--main-- | success | 20s | 16s |
| 2026-07-25 23:15 | ci | pull_request | release-please--branches--main-- | skipped | 2s | 0s |
| 2026-07-25 23:15 | conventional-commits | pull_request | release-please--branches--main-- | cancelled | 2s | 0s |
| 2026-07-25 23:15 | release-please | push | main | success | 42s | 37s |
| 2026-07-25 23:15 | ci | push | main | success | 14m58s | 18m54s |
| 2026-07-25 23:15 | conventional-commits | pull_request | fix/send-keys-paste-enter | success | 19s | 17s |
| 2026-07-25 23:15 | ci | pull_request | fix/send-keys-paste-enter | success | 15m13s | 19m52s |
| 2026-07-25 22:54 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-07-25 22:54 | conventional-commits | pull_request | release-please--branches--main-- | success | 17s | 13s |
| 2026-07-25 22:54 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-07-25 22:54 | conventional-commits | pull_request | release-please--branches--main-- | success | 18s | 14s |
| 2026-07-25 22:53 | release-please | push | main | success | 52s | 45s |
| 2026-07-25 22:53 | ci | push | main | success | 15m49s | 19m30s |

---

Query the raw store directly, e.g. every recorded ci run's wall time:

```sh
git fetch origin ci-metrics && git show origin/ci-metrics:runs/2026-07.ndjson \
  | jq -r 'select(.kind == "run" and .workflow == "ci") | [.created_at, .conclusion, .duration_s] | @tsv'
```
