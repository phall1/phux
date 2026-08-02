# phux CI dashboard

Generated 2026-08-02T01:06:34Z by the ci-metrics workflow. Do not edit —
every table is re-rendered from `runs/*.ndjson` on each update.
Machine rollup: [`site/summary.json`](site/summary.json), rendered live at
<https://phux.phall.io/ci>.

## Workflows, last 30 days

| workflow | runs | success | median | p95 | runner minutes |
|---|---:|---:|---:|---:|---:|
| ci | 274 | 58% | 13m37s | 18m24s | 3365 |
| observatory | 16 | 88% | 12m07s | 12m56s | 360 |
| stress | 33 | 45% | 6m13s | 22m37s | 325 |
| release-please | 55 | 98% | 45s | 7m42s | 144 |
| conventional-commits | 245 | 82% | 16s | 22s | 52 |

## ci jobs, last 30 days

| job | runs | median queue | median wall | p95 wall |
|---|---:|---:|---:|---:|
| test | 271 | 2s | 13m27s | 17m51s |
| check | 269 | 2s | 3m17s | 5m12s |
| detect docs-only | 274 | 2s | 5s | 8s |

## Slowest ci steps (median, last 30 days)

| job | step | median | samples |
|---|---|---:|---:|
| test | tests (unit + e2e) | 14m40s | 14 |
| check | rust checks (fmt + clippy + doc + deny) | 1m48s | 18 |
| check | runner disk headroom | 56s | 18 |
| test | runner disk headroom | 55s | 17 |
| test | Run Swatinem/rust-cache@v2 | 23s | 17 |
| check | Run Swatinem/rust-cache@v2 | 20s | 18 |
| check | docs-check | 13s | 18 |
| test | agents smoke | 11s | 14 |
| check | Run DeterminateSystems/nix-installer-action@v22 | 8s | 18 |
| test | Run DeterminateSystems/nix-installer-action@v22 | 8s | 17 |
| check | formula-check | 5s | 18 |

## Cargo phases inside the lanes (median, last 30 days)

| workflow / job | phase | median | samples |
|---|---|---:|---:|
| ci / check | clippy | 1m53s | 150 |
| ci / check | doc | 11s | 150 |
| ci / check | deny | 4s | 149 |
| ci / check | fmt | 2s | 153 |
| ci / test | unit | 13m22s | 137 |
| ci / test | e2e | 10s | 134 |
| ci / test | agents-smoke | 1s | 75 |
| observatory / timings | build-dev | 11m06s | 14 |
| observatory / timings | build-release | 5m11s | 15 |
| stress / stress | stress | 17m27s | 18 |

## Cache effectiveness (last 30 days)

| workflow / job | rust-cache hit rate | samples |
|---|---:|---:|
| ci / check | 34% | 160 |
| ci / test | 38% | 158 |
| stress / stress | 17% | 18 |

## Cold build (observatory)

### dev: 11m24s (previous: 7m50s) — 541 units at `02dd58643`

| slowest units | wall |
|---|---:|
| `libghostty-vt-sys build script (run)` | 131.05s |
| `phux-client lib (test)` | 92.59s |
| `phux bin "phux"` | 90.2s |
| `phux-server` | 82.24s |
| `phux-server lib (test)` | 50.6s |
| `rustls` | 50.38s |
| `phux bin "phux" (test)` | 45.07s |
| `phux-config` | 34.65s |

### release: 4m45s (previous: 5m25s) — 365 units at `02dd58643`

| slowest units | wall |
|---|---:|
| `libghostty-vt-sys build script (run)` | 121.34s |
| `phux bin "phux"` | 118.25s |
| `phux-server` | 23.48s |
| `phux-config` | 21.09s |
| `phux-mcp bin "phux-mcp"` | 20.13s |
| `regex-automata` | 18.52s |
| `rustls` | 12.94s |
| `clap_builder` | 12.29s |

## Release binary size

| binary | size | previous |
|---|---:|---:|
| `phux` | 14.6 MiB | 14.5 MiB |
| `phux-mcp` | 2.1 MiB | 2.1 MiB |

## Dependency graph

- locked packages: **438** (previous: 438) — 13 workspace members, 51 direct deps
- duplicate versions: **32** (previous: 32)
- proc-macro crates: 33; build-script crates: 67

## Slowest tests (latest instrumented run, `b5706f81b`)

| test | wall |
|---|---:|
| `phux-server::perf_bursty_output::synthesize_against_reference_alloc_bounded_under_full_churn` | 111.652s |
| `phux-server::terminal_actor::tests::resize_desync_then_both_shrink_does_not_overflow` | 27.799s |
| `phux-server::agent_detect::a_plain_shell_pane_never_gets_an_agent_record` | 6.016s |
| `phux-relay::relay_auth::stalled_preamble_does_not_wedge_relay` | 5.116s |
| `phux-server::agent_detect::deleting_the_record_hands_it_back_to_the_detector` | 3.817s |
| `phux-server::agent_detect::an_identity_only_set_gets_its_state_filled_in_by_the_detector` | 3.817s |
| `phux-record::golden_cast::both_containers_agree_on_the_frame_count_for_one_recording` | 3.716s |
| `phux-server::agent_detect::detector_publishes_blocked_from_a_live_prompt_box` | 3.517s |
| `phux-server::agent_events::unattached_subscriber_receives_events` | 3.018s |
| `phux-server::server_idle_exit::without_the_flag_an_unattended_server_stays_up` | 3.015s |

## Recent runs

| when | workflow | event | branch | result | wall | runner time |
|---|---|---|---|---|---:|---:|
| 2026-08-02 01:06 | ci | pull_request | release-please--branches--main-- | skipped | 2s | 0s |
| 2026-08-02 01:06 | conventional-commits | pull_request | release-please--branches--main-- | success | 17s | 14s |
| 2026-08-02 01:05 | ci | pull_request | release-please--branches--main-- | skipped | 6s | 0s |
| 2026-08-02 01:05 | conventional-commits | pull_request | release-please--branches--main-- | success | 18s | 14s |
| 2026-08-02 01:05 | conventional-commits | pull_request | release-please--branches--main-- | cancelled | 2s | 1s |
| 2026-08-02 01:05 | release-please | push | main | success | 49s | 37s |
| 2026-08-02 00:48 | conventional-commits | pull_request | fix/context-menu-resize-hittest | success | 23s | 13s |
| 2026-08-02 00:48 | ci | pull_request | fix/context-menu-resize-hittest | success | 16m56s | 20m33s |
| 2026-08-02 00:47 | conventional-commits | pull_request | fix/pty-writer-resilience | success | 22s | 17s |
| 2026-08-02 00:47 | ci | pull_request | fix/pty-writer-resilience | success | 17m10s | 20m46s |
| 2026-08-02 00:44 | conventional-commits | pull_request | chore/beads-sync | success | 21s | 16s |
| 2026-08-02 00:44 | ci | pull_request | chore/beads-sync | success | 19m31s | 23m04s |
| 2026-08-01 23:31 | ci | pull_request | release-please--branches--main-- | skipped | 0s | 0s |
| 2026-08-01 23:31 | conventional-commits | pull_request | release-please--branches--main-- | success | 14s | 11s |
| 2026-08-01 23:31 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-08-01 23:31 | conventional-commits | pull_request | release-please--branches--main-- | success | 12s | 9s |
| 2026-08-01 23:31 | release-please | push | main | success | 49s | 38s |
| 2026-08-01 23:31 | ci | push | main | success | 17m06s | 21m28s |
| 2026-08-01 23:13 | conventional-commits | pull_request | dev | success | 19s | 13s |
| 2026-08-01 23:13 | ci | pull_request | dev | success | 16m59s | 20m57s |
| 2026-08-01 16:27 | conventional-commits | pull_request | chore/beads-sync | success | 15s | 11s |
| 2026-08-01 16:27 | ci | pull_request | chore/beads-sync | success | 16m10s | 20m01s |
| 2026-08-01 10:10 | conventional-commits | pull_request | dev | success | 18s | 14s |
| 2026-08-01 10:10 | ci | pull_request | dev | failure | 13m25s | 16m10s |
| 2026-08-01 09:42 | conventional-commits | pull_request | dev | success | 17s | 13s |
| 2026-08-01 09:42 | ci | pull_request | dev | failure | 13m37s | 16m09s |
| 2026-08-01 09:35 | conventional-commits | pull_request | dev | failure | 17s | 13s |
| 2026-08-01 09:35 | ci | pull_request | dev | cancelled | 7m12s | 12m45s |
| 2026-08-01 09:10 | stress | schedule | main | failure | 6m39s | 6m36s |
| 2026-07-31 09:49 | stress | schedule | main | failure | 4m27s | 4m24s |

---

Query the raw store directly, e.g. every recorded ci run's wall time:

```sh
git fetch origin ci-metrics && git show origin/ci-metrics:runs/2026-08.ndjson \
  | jq -r 'select(.kind == "run" and .workflow == "ci") | [.created_at, .conclusion, .duration_s] | @tsv'
```
