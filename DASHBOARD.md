# phux CI dashboard

Generated 2026-08-02T04:49:18Z by the ci-metrics workflow. Do not edit —
every table is re-rendered from `runs/*.ndjson` on each update.
Machine rollup: [`site/summary.json`](site/summary.json), rendered live at
<https://phux.phall.io/ci>.

## Workflows, last 30 days

| workflow | runs | success | median | p95 | runner minutes |
|---|---:|---:|---:|---:|---:|
| ci | 305 | 57% | 13m37s | 18m24s | 3670 |
| observatory | 16 | 88% | 12m07s | 12m56s | 360 |
| stress | 33 | 45% | 6m13s | 22m37s | 325 |
| release-please | 63 | 98% | 45s | 7m36s | 149 |
| conventional-commits | 274 | 81% | 16s | 23s | 57 |

## ci jobs, last 30 days

| job | runs | median queue | median wall | p95 wall |
|---|---:|---:|---:|---:|
| test | 300 | 2s | 13m26s | 17m49s |
| check | 298 | 2s | 3m15s | 5m12s |
| detect docs-only | 302 | 2s | 5s | 8s |

## Slowest ci steps (median, last 30 days)

| job | step | median | samples |
|---|---|---:|---:|
| test | tests (unit + e2e) | 14m42s | 12 |
| check | rust checks (fmt + clippy + doc + deny) | 1m38s | 13 |
| check | runner disk headroom | 49s | 14 |
| test | runner disk headroom | 45s | 14 |
| check | Run Swatinem/rust-cache@v2 | 23s | 13 |
| test | Run Swatinem/rust-cache@v2 | 22s | 13 |
| check | docs-check | 12s | 13 |
| test | agents smoke | 11s | 12 |
| check | Run DeterminateSystems/nix-installer-action@v22 | 9s | 14 |
| test | Run DeterminateSystems/nix-installer-action@v22 | 8s | 14 |

## Cargo phases inside the lanes (median, last 30 days)

| workflow / job | phase | median | samples |
|---|---|---:|---:|
| ci / check | clippy | 1m51s | 165 |
| ci / check | doc | 10s | 165 |
| ci / check | deny | 4s | 164 |
| ci / check | fmt | 2s | 168 |
| ci / test | unit | 12m59s | 151 |
| ci / test | e2e | 10s | 148 |
| ci / test | agents-smoke | 1s | 89 |
| observatory / timings | build-dev | 11m06s | 14 |
| observatory / timings | build-release | 5m11s | 15 |
| stress / stress | stress | 17m27s | 18 |

## Cache effectiveness (last 30 days)

| workflow / job | rust-cache hit rate | samples |
|---|---:|---:|
| ci / check | 38% | 175 |
| ci / test | 42% | 173 |
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

## Slowest tests (latest instrumented run, `c18ce108c`)

| test | wall |
|---|---:|
| `phux-server::terminal_actor::tests::resize_desync_then_both_shrink_does_not_overflow` | 14.235s |
| `phux-server::agent_detect::a_plain_shell_pane_never_gets_an_agent_record` | 6.015s |
| `phux-relay::relay_auth::stalled_preamble_does_not_wedge_relay` | 5.113s |
| `phux-server::agent_detect::deleting_the_record_hands_it_back_to_the_detector` | 3.814s |
| `phux-server::agent_detect::an_identity_only_set_gets_its_state_filled_in_by_the_detector` | 3.813s |
| `phux-server::agent_detect::detector_publishes_blocked_from_a_live_prompt_box` | 3.514s |
| `phux-server::server_idle_exit::without_the_flag_an_unattended_server_stays_up` | 3.013s |
| `phux-server::agent_events::unattached_subscriber_receives_events` | 3.013s |
| `phux-server::phux_3uv_acked_incremental::acked_incremental_converges_and_seq_is_monotonic` | 2.512s |
| `phux-record::golden_cast::both_containers_agree_on_the_frame_count_for_one_recording` | 2.221s |

## Recent runs

| when | workflow | event | branch | result | wall | runner time |
|---|---|---|---|---|---:|---:|
| 2026-08-02 04:48 | conventional-commits | pull_request | release-please--branches--main-- | success | 16s | 13s |
| 2026-08-02 04:48 | release-please | push | main | success | 21s | 13s |
| 2026-08-02 04:48 | ci | push | main | cancelled | 22s | 0s |
| 2026-08-02 04:48 | ci | pull_request | release-please--branches--main-- | cancelled | 56s | 1m33s |
| 2026-08-02 04:37 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-08-02 04:37 | conventional-commits | pull_request | release-please--branches--main-- | success | 20s | 10s |
| 2026-08-02 04:36 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-08-02 04:36 | conventional-commits | pull_request | release-please--branches--main-- | success | 17s | 13s |
| 2026-08-02 04:36 | conventional-commits | pull_request | release-please--branches--main-- | cancelled | 2s | 1s |
| 2026-08-02 04:36 | release-please | push | main | success | 54s | 41s |
| 2026-08-02 04:36 | ci | pull_request | release-please--branches--main-- | skipped | 7s | 0s |
| 2026-08-02 04:36 | conventional-commits | pull_request | release-please--branches--main-- | success | 18s | 14s |
| 2026-08-02 04:35 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-08-02 04:35 | conventional-commits | pull_request | release-please--branches--main-- | success | 20s | 16s |
| 2026-08-02 04:35 | conventional-commits | pull_request | release-please--branches--main-- | cancelled | 2s | 0s |
| 2026-08-02 04:35 | release-please | push | main | success | 56s | 48s |
| 2026-08-02 04:35 | ci | push | main | cancelled | 1m05s | 0s |
| 2026-08-02 04:34 | conventional-commits | pull_request | ci-arm-runners-evict-bursty | success | 20s | 11s |
| 2026-08-02 04:34 | ci | pull_request | ci-arm-runners-evict-bursty | success | 14m24s | 18m40s |
| 2026-08-02 04:31 | conventional-commits | pull_request | chore/beads-reconcile-receipt | success | 18s | 14s |
| 2026-08-02 04:31 | ci | pull_request | chore/beads-reconcile-receipt | success | 16m45s | 20m35s |
| 2026-08-02 04:27 | release-please | push | main | success | 29s | 25s |
| 2026-08-02 04:27 | ci | push | main | success | 16m13s | 21m09s |
| 2026-08-02 04:18 | conventional-commits | pull_request | feat/ux-wave-3 | success | 18s | 13s |
| 2026-08-02 04:18 | ci | pull_request | feat/ux-wave-3 | success | 17m42s | 20m34s |
| 2026-08-02 04:10 | conventional-commits | pull_request | chore/beads-reconcile | success | 16s | 12s |
| 2026-08-02 04:10 | ci | pull_request | chore/beads-reconcile | success | 17m03s | 20m57s |
| 2026-08-02 04:07 | conventional-commits | pull_request | feat/ux-wave-3 | success | 14s | 10s |
| 2026-08-02 04:07 | ci | pull_request | feat/ux-wave-3 | cancelled | 11m50s | 15m45s |
| 2026-08-02 03:54 | ci | pull_request | release-please--branches--main-- | skipped | 2s | 0s |

---

Query the raw store directly, e.g. every recorded ci run's wall time:

```sh
git fetch origin ci-metrics && git show origin/ci-metrics:runs/2026-08.ndjson \
  | jq -r 'select(.kind == "run" and .workflow == "ci") | [.created_at, .conclusion, .duration_s] | @tsv'
```
