# phux CI dashboard

Generated 2026-07-24T11:13:57Z by the ci-metrics workflow. Do not edit —
every table is re-rendered from `runs/*.ndjson` on each update.
Machine rollup: [`site/summary.json`](site/summary.json), rendered live at
<https://phux.phall.io/ci>.

## Workflows, last 30 days

| workflow | runs | success | median | p95 | runner minutes |
|---|---:|---:|---:|---:|---:|
| ci | 164 | 62% | 13m36s | 17m52s | 2076 |
| stress | 19 | 63% | 20m09s | 23m45s | 239 |
| observatory | 8 | 88% | 12m07s | 12m42s | 190 |
| release-please | 28 | 100% | 43s | 7m03s | 70 |
| conventional-commits | 148 | 86% | 16s | 20s | 33 |

## ci jobs, last 30 days

| job | runs | median queue | median wall | p95 wall |
|---|---:|---:|---:|---:|
| test | 163 | 2s | 13m25s | 17m34s |
| check | 161 | 2s | 2m47s | 4m45s |
| detect docs-only | 164 | 2s | 5s | 7s |

## Slowest ci steps (median, last 30 days)

| job | step | median | samples |
|---|---|---:|---:|
| test | tests (unit + e2e) | 15m23s | 16 |
| check | rust checks (fmt + clippy + doc + deny) | 2m55s | 16 |
| check | runner disk headroom | 1m01s | 16 |
| test | runner disk headroom | 54s | 17 |
| check | Run Swatinem/rust-cache@v2 | 19s | 18 |
| test | Run Swatinem/rust-cache@v2 | 18s | 19 |
| test | agents smoke | 12s | 16 |
| check | docs-check | 9s | 18 |
| check | Run DeterminateSystems/nix-installer-action@v22 | 8s | 18 |
| test | Run DeterminateSystems/nix-installer-action@v22 | 8s | 19 |

## Cargo phases inside the lanes (median, last 30 days)

| workflow / job | phase | median | samples |
|---|---|---:|---:|
| ci / check | clippy | 1m59s | 88 |
| ci / check | doc | 12s | 88 |
| ci / check | deny | 4s | 88 |
| ci / check | fmt | 1s | 91 |
| ci / test | unit | 14m07s | 78 |
| ci / test | e2e | 10s | 77 |
| ci / test | agents-smoke | 1s | 18 |
| observatory / timings | build-dev | 11m06s | 7 |
| observatory / timings | build-release | 5m00s | 8 |
| stress / stress | stress | 19m15s | 10 |

## Cache effectiveness (last 30 days)

| workflow / job | rust-cache hit rate | samples |
|---|---:|---:|
| ci / check | 30% | 96 |
| ci / test | 32% | 94 |
| stress / stress | 10% | 10 |

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

## Slowest tests (latest instrumented run, `b38f2837d`)

| test | wall |
|---|---:|
| `phux-server::perf_bursty_output::synthesize_against_reference_alloc_bounded_under_full_churn` | 116.127s |
| `phux-server::terminal_actor::tests::resize_desync_then_both_shrink_does_not_overflow` | 27.365s |
| `phux-server::agent_detect::a_plain_shell_pane_never_gets_an_agent_record` | 6.016s |
| `phux-relay::relay_auth::stalled_preamble_does_not_wedge_relay` | 5.113s |
| `phux-server::agent_detect::an_identity_only_set_gets_its_state_filled_in_by_the_detector` | 3.818s |
| `phux-server::agent_detect::deleting_the_record_hands_it_back_to_the_detector` | 3.815s |
| `phux-server::agent_detect::detector_publishes_blocked_from_a_live_prompt_box` | 3.515s |
| `phux-server::agent_events::unattached_subscriber_receives_events` | 3.014s |
| `phux-server::phux_3uv_acked_incremental::acked_incremental_converges_and_seq_is_monotonic` | 2.514s |
| `phux::bin/phux::commands::overlay::tests::wedged_tailscale_binary_is_killed_at_the_deadline` | 2.022s |

## Recent runs

| when | workflow | event | branch | result | wall | runner time |
|---|---|---|---|---|---:|---:|
| 2026-07-24 10:59 | ci | pull_request | release-please--branches--main-- | skipped | 2s | 0s |
| 2026-07-24 10:59 | conventional-commits | pull_request | release-please--branches--main-- | success | 5m04s | 4m45s |
| 2026-07-24 10:58 | stress | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-07-24 10:58 | ci | pull_request | release-please--branches--main-- | skipped | 10s | 0s |
| 2026-07-24 10:58 | conventional-commits | pull_request | release-please--branches--main-- | cancelled | 40s | 36s |
| 2026-07-24 10:58 | release-please | push | main | success | 51s | 43s |
| 2026-07-24 10:55 | conventional-commits | pull_request | adr-0052-connector-productizatio | success | 16s | 14s |
| 2026-07-24 10:55 | ci | pull_request | adr-0052-connector-productizatio | success | 2m49s | 3m50s |
| 2026-07-24 10:55 | conventional-commits | pull_request | feat/relay-alpn-dialer | success | 13s | 11s |
| 2026-07-24 10:55 | ci | pull_request | feat/relay-alpn-dialer | success | 18m24s | 22m51s |
| 2026-07-24 10:52 | conventional-commits | pull_request | feat/relay-alpn-dialer | success | 20s | 15s |
| 2026-07-24 10:52 | ci | pull_request | feat/relay-alpn-dialer | cancelled | 3m27s | 6m18s |
| 2026-07-24 09:25 | stress | schedule | main | success | 20m44s | 20m41s |
| 2026-07-23 09:28 | stress | schedule | main | success | 16m36s | 16m33s |
| 2026-07-22 20:10 | stress | pull_request | release-please--branches--main-- | skipped | 8s | 0s |
| 2026-07-22 20:10 | release-please | push | main | success | 7m36s | 19m14s |
| 2026-07-22 20:10 | observatory | push | main | success | 12m07s | 22m13s |
| 2026-07-22 20:10 | ci | push | main | success | 19m37s | 24m52s |
| 2026-07-22 19:50 | ci | pull_request | release-please--branches--main-- | success | 18m59s | 24m44s |
| 2026-07-22 19:49 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-07-22 19:49 | conventional-commits | pull_request | release-please--branches--main-- | success | 15s | 12s |
| 2026-07-22 19:48 | conventional-commits | pull_request | release-please--branches--main-- | success | 18s | 15s |
| 2026-07-22 19:48 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-07-22 19:48 | stress | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-07-22 19:48 | release-please | push | main | success | 51s | 44s |
| 2026-07-22 19:48 | ci | push | main | success | 18m14s | 21m32s |
| 2026-07-22 19:31 | conventional-commits | pull_request | fix/plugin-agent-bench-phux-bin | success | 20s | 16s |
| 2026-07-22 19:31 | ci | pull_request | fix/plugin-agent-bench-phux-bin | success | 16m23s | 21m02s |
| 2026-07-22 09:31 | stress | schedule | main | success | 21m01s | 20m58s |
| 2026-07-21 14:48 | conventional-commits | pull_request | feat/oss-reference-relay | success | 14s | 11s |

---

Query the raw store directly, e.g. every recorded ci run's wall time:

```sh
git fetch origin ci-metrics && git show origin/ci-metrics:runs/2026-07.ndjson \
  | jq -r 'select(.kind == "run" and .workflow == "ci") | [.created_at, .conclusion, .duration_s] | @tsv'
```
