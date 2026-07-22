# phux CI dashboard

Generated 2026-07-22T20:09:49Z by the ci-metrics workflow. Do not edit —
every table is re-rendered from `runs/*.ndjson` on each update.
Machine rollup: [`site/summary.json`](site/summary.json), rendered live at
<https://phux.phall.io/ci>.

## Workflows, last 30 days

| workflow | runs | success | median | p95 | runner minutes |
|---|---:|---:|---:|---:|---:|
| ci | 158 | 62% | 13m37s | 17m45s | 2018 |
| stress | 15 | 67% | 20m45s | 23m45s | 202 |
| observatory | 7 | 86% | 12m25s | 12m42s | 168 |
| release-please | 26 | 100% | 42s | 54s | 50 |
| conventional-commits | 143 | 86% | 16s | 20s | 27 |

## ci jobs, last 30 days

| job | runs | median queue | median wall | p95 wall |
|---|---:|---:|---:|---:|
| test | 157 | 2s | 13m25s | 17m24s |
| check | 155 | 2s | 2m47s | 4m43s |
| detect docs-only | 158 | 2s | 5s | 7s |

## Slowest ci steps (median, last 30 days)

| job | step | median | samples |
|---|---|---:|---:|
| test | tests (unit + e2e) | 15m23s | 17 |
| check | rust checks (fmt + clippy + doc + deny) | 2m55s | 17 |
| check | runner disk headroom | 1m00s | 13 |
| test | runner disk headroom | 54s | 13 |
| check | Run Swatinem/rust-cache@v2 | 18s | 18 |
| test | Run Swatinem/rust-cache@v2 | 18s | 18 |
| test | agents smoke | 12s | 15 |
| check | docs-check | 9s | 18 |
| check | Run DeterminateSystems/nix-installer-action@v22 | 8s | 18 |
| test | Run DeterminateSystems/nix-installer-action@v22 | 8s | 18 |

## Cargo phases inside the lanes (median, last 30 days)

| workflow / job | phase | median | samples |
|---|---|---:|---:|
| ci / check | clippy | 1m57s | 86 |
| ci / check | doc | 12s | 86 |
| ci / check | deny | 4s | 86 |
| ci / check | fmt | 1s | 89 |
| ci / test | unit | 14m04s | 76 |
| ci / test | e2e | 10s | 75 |
| ci / test | agents-smoke | 1s | 16 |
| observatory / timings | build-dev | 11m06s | 6 |
| observatory / timings | build-release | 5m01s | 7 |
| stress / stress | stress | 19m27s | 8 |

## Cache effectiveness (last 30 days)

| workflow / job | rust-cache hit rate | samples |
|---|---:|---:|
| ci / check | 30% | 93 |
| ci / test | 33% | 90 |
| stress / stress | 13% | 8 |

## Cold build (observatory)

### dev: 11m27s (previous: 11m22s) — 520 units at `c34009cfb`

| slowest units | wall |
|---|---:|
| `libghostty-vt-sys build script (run)` | 123.46s |
| `phux-server lib (test)` | 91.46s |
| `phux bin "phux"` | 73.96s |
| `phux-client lib (test)` | 65.86s |
| `phux-server` | 56.43s |
| `rustls` | 48.41s |
| `phux-server test "spawn_terminal" (test)` | 35.92s |
| `quinn-proto` | 35.03s |

### release: 5m07s (previous: 5m24s) — 359 units at `c34009cfb`

| slowest units | wall |
|---|---:|
| `libghostty-vt-sys build script (run)` | 158.3s |
| `phux bin "phux"` | 102.9s |
| `phux-mcp bin "phux-mcp"` | 21.12s |
| `phux-server` | 20.93s |
| `regex-automata` | 19.23s |
| `rustls` | 18.4s |
| `phux-config` | 17.64s |
| `clap_builder` | 16.19s |

## Release binary size

| binary | size | previous |
|---|---:|---:|
| `phux` | 12.9 MiB | 12.9 MiB |
| `phux-mcp` | 2.1 MiB | 2.1 MiB |

## Dependency graph

- locked packages: **432** (previous: 432) — 11 workspace members, 48 direct deps
- duplicate versions: **32** (previous: 32)
- proc-macro crates: 33; build-script crates: 67

## Slowest tests (latest instrumented run, `e79a58e34`)

| test | wall |
|---|---:|
| `phux-server::perf_bursty_output::synthesize_against_reference_alloc_bounded_under_full_churn` | 114.533s |
| `phux-server::terminal_actor::tests::resize_desync_then_both_shrink_does_not_overflow` | 27.411s |
| `phux-server::agent_detect::a_plain_shell_pane_never_gets_an_agent_record` | 6.020s |
| `phux-server::agent_detect::an_identity_only_set_gets_its_state_filled_in_by_the_detector` | 3.815s |
| `phux-server::agent_detect::deleting_the_record_hands_it_back_to_the_detector` | 3.813s |
| `phux-server::agent_detect::detector_publishes_blocked_from_a_live_prompt_box` | 3.516s |
| `phux-server::agent_events::unattached_subscriber_receives_events` | 3.014s |
| `phux-server::phux_3uv_acked_incremental::acked_incremental_converges_and_seq_is_monotonic` | 2.514s |
| `phux::bin/phux::commands::overlay::tests::wedged_tailscale_binary_is_killed_at_the_deadline` | 2.017s |
| `phux-server::l2_adversarial::test_subscribe_events_no_loss` | 2.016s |

## Recent runs

| when | workflow | event | branch | result | wall | runner time |
|---|---|---|---|---|---:|---:|
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
| 2026-07-21 14:48 | ci | pull_request | feat/oss-reference-relay | success | 18m08s | 23m10s |
| 2026-07-21 14:47 | conventional-commits | pull_request | feat/relay-alpn-dialer | success | 18s | 13s |
| 2026-07-21 14:47 | ci | pull_request | feat/relay-alpn-dialer | success | 15m50s | 19m27s |
| 2026-07-21 09:31 | stress | schedule | main | success | 20m09s | 20m06s |
| 2026-07-21 07:36 | conventional-commits | pull_request | adr-0052-connector-productizatio | success | 17s | 15s |
| 2026-07-21 07:36 | ci | pull_request | adr-0052-connector-productizatio | success | 2m35s | 4m04s |
| 2026-07-20 23:10 | stress | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-07-20 23:10 | release-please | push | main | success | 8m33s | 19m45s |
| 2026-07-20 23:10 | observatory | push | main | success | 12m42s | 24m27s |
| 2026-07-20 23:10 | ci | push | main | success | 17m45s | 22m41s |
| 2026-07-20 22:52 | conventional-commits | pull_request | release-please--branches--main-- | success | 16s | 12s |
| 2026-07-20 22:52 | ci | pull_request | release-please--branches--main-- | success | 17m52s | 22m05s |
| 2026-07-20 22:51 | conventional-commits | pull_request | release-please--branches--main-- | cancelled | 1s | 0s |
| 2026-07-20 22:51 | conventional-commits | pull_request | release-please--branches--main-- | success | 15s | 11s |
| 2026-07-20 22:51 | ci | pull_request | release-please--branches--main-- | cancelled | 42s | 1m05s |
| 2026-07-20 22:51 | release-please | push | main | success | 48s | 42s |
| 2026-07-20 22:51 | ci | push | main | success | 16m45s | 20m54s |
| 2026-07-20 10:17 | stress | schedule | main | success | 20m45s | 20m41s |

---

Query the raw store directly, e.g. every recorded ci run's wall time:

```sh
git fetch origin ci-metrics && git show origin/ci-metrics:runs/2026-07.ndjson \
  | jq -r 'select(.kind == "run" and .workflow == "ci") | [.created_at, .conclusion, .duration_s] | @tsv'
```
