# phux CI dashboard

Generated 2026-08-02T06:06:54Z by the ci-metrics workflow. Do not edit —
every table is re-rendered from `runs/*.ndjson` on each update.
Machine rollup: [`site/summary.json`](site/summary.json), rendered live at
<https://phux.phall.io/ci>.

## Workflows, last 30 days

| workflow | runs | success | median | p95 | runner minutes |
|---|---:|---:|---:|---:|---:|
| ci | 326 | 57% | 13m19s | 18m27s | 3853 |
| observatory | 17 | 88% | 12m23s | 13m00s | 385 |
| stress | 35 | 43% | 5m20s | 22m37s | 325 |
| release-please | 69 | 99% | 45s | 7m37s | 170 |
| conventional-commits | 291 | 81% | 16s | 24s | 60 |

## ci jobs, last 30 days

| job | runs | median queue | median wall | p95 wall |
|---|---:|---:|---:|---:|
| test | 321 | 2s | 13m08s | 17m46s |
| check | 319 | 2s | 2m57s | 5m11s |
| detect docs-only | 323 | 2s | 5s | 8s |

## Slowest ci steps (median, last 30 days)

| job | step | median | samples |
|---|---|---:|---:|
| test | tests (unit + e2e) | 10m06s | 12 |
| check | rust checks (fmt + clippy + doc + deny) | 1m21s | 13 |
| test | Run Swatinem/rust-cache@v2 | 18s | 14 |
| check | Run Swatinem/rust-cache@v2 | 16s | 14 |
| test | agents smoke | 11s | 12 |
| check | docs-check | 10s | 13 |
| check | Run DeterminateSystems/nix-installer-action@v22 | 7s | 15 |
| test | Run DeterminateSystems/nix-installer-action@v22 | 7s | 15 |
| check | runner disk headroom | 5s | 15 |

## Cargo phases inside the lanes (median, last 30 days)

| workflow / job | phase | median | samples |
|---|---|---:|---:|
| ci / check | clippy | 1m49s | 177 |
| ci / check | doc | 10s | 177 |
| ci / check | deny | 3s | 176 |
| ci / check | fmt | 2s | 181 |
| ci / test | unit | 12m42s | 163 |
| ci / test | e2e | 11s | 160 |
| ci / test | agents-smoke | 1s | 100 |
| observatory / timings | build-dev | 11m14s | 15 |
| observatory / timings | build-release | 5m11s | 16 |
| stress / stress | stress | 17m27s | 18 |

## Cache effectiveness (last 30 days)

| workflow / job | rust-cache hit rate | samples |
|---|---:|---:|
| ci / check | 40% | 188 |
| ci / test | 43% | 186 |
| stress / stress | 17% | 18 |

## Cold build (observatory)

### dev: 11m45s (previous: 11m24s) — 542 units at `37c7d4fd7`

| slowest units | wall |
|---|---:|
| `libghostty-vt-sys build script (run)` | 127.27s |
| `phux bin "phux"` | 96.98s |
| `phux-client lib (test)` | 92.62s |
| `phux-server` | 83.51s |
| `phux-server lib (test)` | 57.34s |
| `rustls` | 51.66s |
| `phux bin "phux" (test)` | 48.13s |
| `phux-config` | 35.61s |

### release: 5m35s (previous: 4m45s) — 365 units at `37c7d4fd7`

| slowest units | wall |
|---|---:|
| `libghostty-vt-sys build script (run)` | 158.67s |
| `phux bin "phux"` | 125.79s |
| `phux-server` | 27.75s |
| `phux-mcp bin "phux-mcp"` | 22.25s |
| `regex-automata` | 22.11s |
| `phux-config` | 21.9s |
| `clap_builder` | 18.51s |
| `rustls` | 15.54s |

## Release binary size

| binary | size | previous |
|---|---:|---:|
| `phux` | 14.7 MiB | 14.6 MiB |
| `phux-mcp` | 2.1 MiB | 2.1 MiB |

## Dependency graph

- locked packages: **438** (previous: 438) — 13 workspace members, 51 direct deps
- duplicate versions: **32** (previous: 32)
- proc-macro crates: 33; build-script crates: 67

## Slowest tests (latest instrumented run, `53b3789a6`)

| test | wall |
|---|---:|
| `phux-server::terminal_actor::tests::resize_desync_then_both_shrink_does_not_overflow` | 14.314s |
| `phux-server::agent_detect::a_plain_shell_pane_never_gets_an_agent_record` | 6.015s |
| `phux-relay::relay_auth::stalled_preamble_does_not_wedge_relay` | 5.112s |
| `phux-server::agent_detect::deleting_the_record_hands_it_back_to_the_detector` | 3.813s |
| `phux-server::agent_detect::an_identity_only_set_gets_its_state_filled_in_by_the_detector` | 3.813s |
| `phux-server::agent_detect::detector_publishes_blocked_from_a_live_prompt_box` | 3.515s |
| `phux-server::server_idle_exit::without_the_flag_an_unattended_server_stays_up` | 3.013s |
| `phux-server::agent_events::unattached_subscriber_receives_events` | 3.012s |
| `phux-server::phux_3uv_acked_incremental::acked_incremental_converges_and_seq_is_monotonic` | 2.512s |
| `phux-record::golden_cast::both_containers_agree_on_the_frame_count_for_one_recording` | 2.235s |

## Recent runs

| when | workflow | event | branch | result | wall | runner time |
|---|---|---|---|---|---:|---:|
| 2026-08-02 06:06 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-08-02 06:06 | conventional-commits | pull_request | release-please--branches--main-- | cancelled | 2s | 0s |
| 2026-08-02 06:03 | conventional-commits | pull_request | fix/pi-package-arbitration | success | 17s | 13s |
| 2026-08-02 05:56 | ci | pull_request | release-please--branches--main-- | skipped | 9s | 0s |
| 2026-08-02 05:56 | conventional-commits | pull_request | release-please--branches--main-- | success | 14s | 10s |
| 2026-08-02 05:56 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-08-02 05:56 | conventional-commits | pull_request | release-please--branches--main-- | success | 20s | 11s |
| 2026-08-02 05:56 | conventional-commits | pull_request | release-please--branches--main-- | cancelled | 2s | 1s |
| 2026-08-02 05:56 | release-please | push | main | success | 50s | 42s |
| 2026-08-02 05:55 | conventional-commits | pull_request | worktree-readme-deslop | success | 20s | 17s |
| 2026-08-02 05:55 | ci | pull_request | worktree-readme-deslop | success | 11m16s | 13m28s |
| 2026-08-02 05:46 | ci | pull_request | release-please--branches--main-- | skipped | 2s | 0s |
| 2026-08-02 05:46 | conventional-commits | pull_request | release-please--branches--main-- | success | 23s | 13s |
| 2026-08-02 05:45 | ci | pull_request | release-please--branches--main-- | skipped | 10s | 0s |
| 2026-08-02 05:45 | conventional-commits | pull_request | release-please--branches--main-- | success | 21s | 17s |
| 2026-08-02 05:45 | conventional-commits | pull_request | release-please--branches--main-- | cancelled | 2s | 0s |
| 2026-08-02 05:45 | release-please | push | main | success | 1m04s | 43s |
| 2026-08-02 05:45 | ci | push | main | success | 20m47s | 13m10s |
| 2026-08-02 05:44 | ci | pull_request | release-please--branches--main-- | skipped | 2s | 0s |
| 2026-08-02 05:44 | conventional-commits | pull_request | release-please--branches--main-- | success | 19s | 15s |
| 2026-08-02 05:44 | ci | pull_request | release-please--branches--main-- | skipped | 7s | 0s |
| 2026-08-02 05:44 | conventional-commits | pull_request | release-please--branches--main-- | success | 14s | 11s |
| 2026-08-02 05:44 | stress | pull_request | release-please--branches--main-- | skipped | 2s | 0s |
| 2026-08-02 05:43 | release-please | push | main | success | 46s | 35s |
| 2026-08-02 05:43 | ci | push | main | success | 11m13s | 13m39s |
| 2026-08-02 05:33 | conventional-commits | pull_request | fix/copy-mode-resize-clamp | success | 15s | 11s |
| 2026-08-02 05:33 | ci | pull_request | fix/copy-mode-resize-clamp | success | 11m34s | 13m57s |
| 2026-08-02 05:32 | conventional-commits | pull_request | feat/ux-wave-4 | success | 24s | 15s |
| 2026-08-02 05:32 | ci | pull_request | feat/ux-wave-4 | success | 11m16s | 13m31s |
| 2026-08-02 05:24 | conventional-commits | pull_request | feat/agent-session-restore | success | 25s | 13s |

---

Query the raw store directly, e.g. every recorded ci run's wall time:

```sh
git fetch origin ci-metrics && git show origin/ci-metrics:runs/2026-08.ndjson \
  | jq -r 'select(.kind == "run" and .workflow == "ci") | [.created_at, .conclusion, .duration_s] | @tsv'
```
