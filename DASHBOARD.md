# phux CI dashboard

Generated 2026-08-01T23:14:25Z by the ci-metrics workflow. Do not edit —
every table is re-rendered from `runs/*.ndjson` on each update.
Machine rollup: [`site/summary.json`](site/summary.json), rendered live at
<https://phux.phall.io/ci>.

## Workflows, last 30 days

| workflow | runs | success | median | p95 | runner minutes |
|---|---:|---:|---:|---:|---:|
| ci | 265 | 58% | 13m37s | 18m24s | 3258 |
| observatory | 16 | 88% | 12m07s | 12m56s | 360 |
| stress | 33 | 45% | 6m13s | 22m37s | 325 |
| release-please | 53 | 98% | 45s | 7m42s | 143 |
| conventional-commits | 237 | 82% | 16s | 22s | 50 |

## ci jobs, last 30 days

| job | runs | median queue | median wall | p95 wall |
|---|---:|---:|---:|---:|
| test | 262 | 2s | 13m26s | 17m49s |
| check | 260 | 2s | 3m16s | 5m14s |
| detect docs-only | 265 | 2s | 5s | 8s |

## Slowest ci steps (median, last 30 days)

| job | step | median | samples |
|---|---|---:|---:|
| test | tests (unit + e2e) | 14m19s | 15 |
| check | rust checks (fmt + clippy + doc + deny) | 2m40s | 19 |
| check | runner disk headroom | 38s | 19 |
| test | runner disk headroom | 27s | 19 |
| check | Run Swatinem/rust-cache@v2 | 19s | 19 |
| test | Run Swatinem/rust-cache@v2 | 17s | 19 |
| check | docs-check | 13s | 19 |
| test | agents smoke | 12s | 15 |
| check | Run DeterminateSystems/nix-installer-action@v22 | 9s | 19 |
| test | Run DeterminateSystems/nix-installer-action@v22 | 9s | 19 |
| check | formula-check | 5s | 19 |

## Cargo phases inside the lanes (median, last 30 days)

| workflow / job | phase | median | samples |
|---|---|---:|---:|
| ci / check | clippy | 1m55s | 145 |
| ci / check | doc | 11s | 145 |
| ci / check | deny | 4s | 144 |
| ci / check | fmt | 2s | 148 |
| ci / test | unit | 13m24s | 132 |
| ci / test | e2e | 10s | 129 |
| ci / test | agents-smoke | 1s | 70 |
| observatory / timings | build-dev | 11m06s | 14 |
| observatory / timings | build-release | 5m11s | 15 |
| stress / stress | stress | 17m27s | 18 |

## Cache effectiveness (last 30 days)

| workflow / job | rust-cache hit rate | samples |
|---|---:|---:|
| ci / check | 32% | 155 |
| ci / test | 37% | 153 |
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

## Slowest tests (latest instrumented run, `974876a6e`)

| test | wall |
|---|---:|
| `phux-server::perf_bursty_output::synthesize_against_reference_alloc_bounded_under_full_churn` | 108.354s |
| `phux-server::terminal_actor::tests::resize_desync_then_both_shrink_does_not_overflow` | 27.483s |
| `phux-server::agent_detect::a_plain_shell_pane_never_gets_an_agent_record` | 6.019s |
| `phux-relay::relay_auth::stalled_preamble_does_not_wedge_relay` | 5.114s |
| `phux-server::agent_detect::an_identity_only_set_gets_its_state_filled_in_by_the_detector` | 3.818s |
| `phux-server::agent_detect::deleting_the_record_hands_it_back_to_the_detector` | 3.816s |
| `phux-server::agent_detect::detector_publishes_blocked_from_a_live_prompt_box` | 3.518s |
| `phux-record::golden_cast::both_containers_agree_on_the_frame_count_for_one_recording` | 3.466s |
| `phux-server::agent_events::unattached_subscriber_receives_events` | 3.018s |
| `phux-server::server_idle_exit::without_the_flag_an_unattended_server_stays_up` | 3.016s |

## Recent runs

| when | workflow | event | branch | result | wall | runner time |
|---|---|---|---|---|---:|---:|
| 2026-08-01 23:13 | conventional-commits | pull_request | dev | success | 19s | 13s |
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
| 2026-07-30 09:34 | stress | schedule | main | success | 19m27s | 19m20s |
| 2026-07-29 09:42 | stress | schedule | main | success | 22m14s | 22m04s |
| 2026-07-29 09:37 | conventional-commits | pull_request | release-please--branches--main-- | success | 16s | 14s |
| 2026-07-29 09:37 | ci | pull_request | release-please--branches--main-- | skipped | 7s | 0s |
| 2026-07-29 09:36 | ci | pull_request | release-please--branches--main-- | skipped | 2s | 0s |
| 2026-07-29 09:36 | conventional-commits | pull_request | release-please--branches--main-- | success | 18s | 14s |
| 2026-07-29 09:36 | stress | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-07-29 09:36 | release-please | push | main | success | 1m00s | 41s |
| 2026-07-29 09:36 | ci | push | main | success | 33m01s | 52m29s |
| 2026-07-29 09:15 | conventional-commits | pull_request | fix/snapshot-mouse-modes | success | 17s | 14s |
| 2026-07-29 09:15 | ci | pull_request | fix/snapshot-mouse-modes | success | 20m30s | 24m41s |
| 2026-07-28 09:39 | stress | schedule | main | failure | 2m39s | 2m35s |
| 2026-07-27 20:38 | stress | pull_request | release-please--branches--main-- | skipped | 11s | 0s |
| 2026-07-27 20:37 | release-please | push | main | success | 8m03s | 20m11s |
| 2026-07-27 20:37 | observatory | push | main | success | 12m46s | 23m55s |
| 2026-07-27 20:37 | ci | push | main | success | 17m26s | 22m12s |
| 2026-07-27 20:20 | ci | pull_request | release-please--branches--main-- | success | 17m18s | 22m03s |
| 2026-07-27 20:19 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-07-27 20:19 | conventional-commits | pull_request | release-please--branches--main-- | success | 18s | 14s |

---

Query the raw store directly, e.g. every recorded ci run's wall time:

```sh
git fetch origin ci-metrics && git show origin/ci-metrics:runs/2026-08.ndjson \
  | jq -r 'select(.kind == "run" and .workflow == "ci") | [.created_at, .conclusion, .duration_s] | @tsv'
```
