# phux CI dashboard

Generated 2026-07-20T23:19:08Z by the ci-metrics workflow. Do not edit —
every table is re-rendered from `runs/*.ndjson` on each update.
Machine rollup: [`site/summary.json`](site/summary.json), rendered live at
<https://phux.phall.io/ci>.

## Workflows, last 30 days

| workflow | runs | success | median | p95 | runner minutes |
|---|---:|---:|---:|---:|---:|
| ci | 149 | 61% | 13m36s | 17m25s | 1881 |
| stress | 12 | 67% | 20m45s | 23m45s | 161 |
| observatory | 6 | 83% | 11m56s | 12m38s | 144 |
| release-please | 25 | 100% | 42s | 54s | 50 |
| conventional-commits | 137 | 85% | 15s | 20s | 25 |

## ci jobs, last 30 days

| job | runs | median queue | median wall | p95 wall |
|---|---:|---:|---:|---:|
| test | 148 | 2s | 13m22s | 17m11s |
| check | 146 | 2s | 2m43s | 4m34s |
| detect docs-only | 149 | 2s | 5s | 7s |

## Slowest ci steps (median, last 30 days)

| job | step | median | samples |
|---|---|---:|---:|
| test | tests (unit + e2e) | 15m32s | 17 |
| check | rust checks (fmt + clippy + doc + deny) | 2m55s | 18 |
| check | runner disk headroom | 1m09s | 6 |
| test | runner disk headroom | 51s | 6 |
| check | Run Swatinem/rust-cache@v2 | 18s | 18 |
| test | Run Swatinem/rust-cache@v2 | 18s | 18 |
| test | agents smoke | 12s | 10 |
| check | docs-check | 9s | 18 |
| check | Run DeterminateSystems/nix-installer-action@v22 | 8s | 18 |
| test | Run DeterminateSystems/nix-installer-action@v22 | 8s | 18 |

## Cargo phases inside the lanes (median, last 30 days)

| workflow / job | phase | median | samples |
|---|---|---:|---:|
| ci / check | clippy | 1m59s | 80 |
| ci / check | doc | 13s | 80 |
| ci / check | deny | 4s | 80 |
| ci / check | fmt | 1s | 83 |
| ci / test | unit | 14m04s | 70 |
| ci / test | e2e | 10s | 69 |
| ci / test | agents-smoke | 1s | 10 |
| observatory / timings | build-dev | 11m06s | 5 |
| observatory / timings | build-release | 5m00s | 6 |
| stress / stress | stress | 20m31s | 6 |

## Cache effectiveness (last 30 days)

| workflow / job | rust-cache hit rate | samples |
|---|---:|---:|
| ci / check | 30% | 86 |
| ci / test | 34% | 83 |
| stress / stress | 17% | 6 |

## Cold build (observatory)

### dev: 11m22s (previous: 11m28s) — 520 units at `220695682`

| slowest units | wall |
|---|---:|
| `libghostty-vt-sys build script (run)` | 102.83s |
| `phux-server lib (test)` | 94.68s |
| `phux bin "phux"` | 76.04s |
| `phux-client lib (test)` | 67.99s |
| `phux-server` | 56.99s |
| `rustls` | 42.52s |
| `phux-server test "spawn_terminal" (test)` | 36.25s |
| `phux-server test "hub_relay_federation" (test)` | 35.65s |

### release: 5m24s (previous: 5m11s) — 359 units at `220695682`

| slowest units | wall |
|---|---:|
| `libghostty-vt-sys build script (run)` | 163.93s |
| `phux bin "phux"` | 111.39s |
| `phux-server` | 24.3s |
| `phux-mcp bin "phux-mcp"` | 23.01s |
| `regex-automata` | 20.69s |
| `phux-config` | 18.11s |
| `rustls` | 15.22s |
| `clap_builder` | 14.91s |

## Release binary size

| binary | size | previous |
|---|---:|---:|
| `phux` | 12.9 MiB | 12.8 MiB |
| `phux-mcp` | 2.1 MiB | 2.1 MiB |

## Dependency graph

- locked packages: **432** (previous: 432) — 11 workspace members, 48 direct deps
- duplicate versions: **32** (previous: 32)
- proc-macro crates: 33; build-script crates: 67

## Slowest tests (latest instrumented run, `9abf6a495`)

| test | wall |
|---|---:|
| `phux-server::perf_bursty_output::synthesize_against_reference_alloc_bounded_under_full_churn` | 113.163s |
| `phux-server::terminal_actor::tests::resize_desync_then_both_shrink_does_not_overflow` | 27.633s |
| `phux-server::agent_detect::a_plain_shell_pane_never_gets_an_agent_record` | 6.017s |
| `phux-server::agent_detect::deleting_the_record_hands_it_back_to_the_detector` | 3.816s |
| `phux-server::agent_detect::an_identity_only_set_gets_its_state_filled_in_by_the_detector` | 3.815s |
| `phux-server::agent_detect::detector_publishes_blocked_from_a_live_prompt_box` | 3.516s |
| `phux-server::agent_events::unattached_subscriber_receives_events` | 3.018s |
| `phux-server::phux_3uv_acked_incremental::acked_incremental_converges_and_seq_is_monotonic` | 2.515s |
| `phux::bin/phux::commands::overlay::tests::wedged_tailscale_binary_is_killed_at_the_deadline` | 2.022s |
| `phux-server::l2_adversarial::test_subscribe_events_no_loss` | 2.015s |

## Recent runs

| when | workflow | event | branch | result | wall | runner time |
|---|---|---|---|---|---:|---:|
| 2026-07-20 23:10 | stress | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-07-20 23:10 | release-please | push | main | success | 8m33s | 19m45s |
| 2026-07-20 22:52 | conventional-commits | pull_request | release-please--branches--main-- | success | 16s | 12s |
| 2026-07-20 22:52 | ci | pull_request | release-please--branches--main-- | success | 17m52s | 22m05s |
| 2026-07-20 22:51 | conventional-commits | pull_request | release-please--branches--main-- | cancelled | 1s | 0s |
| 2026-07-20 22:51 | conventional-commits | pull_request | release-please--branches--main-- | success | 15s | 11s |
| 2026-07-20 22:51 | ci | pull_request | release-please--branches--main-- | cancelled | 42s | 1m05s |
| 2026-07-20 22:51 | release-please | push | main | success | 48s | 42s |
| 2026-07-20 22:51 | ci | push | main | success | 16m45s | 20m54s |
| 2026-07-20 10:17 | stress | schedule | main | success | 20m45s | 20m41s |
| 2026-07-20 08:57 | observatory | schedule | main | success | 12m56s | 25m23s |
| 2026-07-19 09:08 | stress | schedule | main | success | 23m51s | 23m48s |
| 2026-07-18 08:52 | stress | schedule | main | success | 5m20s | 5m17s |
| 2026-07-18 03:23 | conventional-commits | pull_request | ci/sync-install-surface-releasin | success | 18s | 15s |
| 2026-07-18 03:23 | ci | pull_request | ci/sync-install-surface-releasin | success | 18m27s | 21m43s |
| 2026-07-18 03:22 | ci | pull_request | release-please--branches--main-- | success | 17m31s | 22m48s |
| 2026-07-17 09:14 | stress | schedule | main | success | 22m37s | 22m34s |
| 2026-07-16 09:20 | stress | schedule | main | success | 23m45s | 23m42s |
| 2026-07-15 20:42 | release-please | push | main | success | 21s | 18s |
| 2026-07-15 20:42 | ci | push | main | success | 15m45s | 20m43s |
| 2026-07-15 20:24 | conventional-commits | pull_request | ci/runner-disk-headroom | success | 19s | 14s |
| 2026-07-15 20:24 | ci | pull_request | ci/runner-disk-headroom | success | 16m52s | 20m45s |
| 2026-07-15 20:22 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-07-15 20:22 | conventional-commits | pull_request | release-please--branches--main-- | success | 13s | 10s |
| 2026-07-15 20:22 | conventional-commits | pull_request | release-please--branches--main-- | success | 18s | 14s |
| 2026-07-15 20:22 | conventional-commits | pull_request | release-please--branches--main-- | cancelled | 2s | 1s |
| 2026-07-15 20:22 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-07-15 20:21 | release-please | push | main | success | 45s | 38s |
| 2026-07-15 20:21 | ci | push | main | success | 13m34s | 17m20s |
| 2026-07-15 20:04 | conventional-commits | pull_request | train/wave2-2026-07-15 | success | 16s | 12s |

---

Query the raw store directly, e.g. every recorded ci run's wall time:

```sh
git fetch origin ci-metrics && git show origin/ci-metrics:runs/2026-07.ndjson \
  | jq -r 'select(.kind == "run" and .workflow == "ci") | [.created_at, .conclusion, .duration_s] | @tsv'
```
