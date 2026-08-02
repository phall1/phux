# phux CI dashboard

Generated 2026-08-02T06:40:07Z by the ci-metrics workflow. Do not edit —
every table is re-rendered from `runs/*.ndjson` on each update.
Machine rollup: [`site/summary.json`](site/summary.json), rendered live at
<https://phux.phall.io/ci>.

## Workflows, last 30 days

| workflow | runs | success | median | p95 | runner minutes |
|---|---:|---:|---:|---:|---:|
| ci | 336 | 57% | 13m15s | 18m27s | 3929 |
| observatory | 18 | 89% | 12m23s | 13m00s | 410 |
| stress | 35 | 43% | 5m20s | 22m37s | 325 |
| release-please | 72 | 99% | 45s | 7m37s | 172 |
| conventional-commits | 301 | 81% | 16s | 24s | 62 |

## ci jobs, last 30 days

| job | runs | median queue | median wall | p95 wall |
|---|---:|---:|---:|---:|
| test | 330 | 2s | 13m02s | 17m40s |
| check | 328 | 2s | 2m49s | 5m05s |
| detect docs-only | 331 | 2s | 5s | 8s |

## Slowest ci steps (median, last 30 days)

| job | step | median | samples |
|---|---|---:|---:|
| test | tests (unit + e2e) | 9m57s | 14 |
| check | rust checks (fmt + clippy + doc + deny) | 1m20s | 15 |
| test | Run Swatinem/rust-cache@v2 | 18s | 17 |
| check | Run Swatinem/rust-cache@v2 | 17s | 17 |
| test | agents smoke | 11s | 14 |
| check | docs-check | 10s | 15 |
| check | Run DeterminateSystems/nix-installer-action@v22 | 7s | 17 |
| test | Run DeterminateSystems/nix-installer-action@v22 | 6s | 17 |

## Cargo phases inside the lanes (median, last 30 days)

| workflow / job | phase | median | samples |
|---|---|---:|---:|
| ci / check | clippy | 1m48s | 182 |
| ci / check | doc | 10s | 182 |
| ci / check | deny | 3s | 181 |
| ci / check | fmt | 2s | 187 |
| ci / test | unit | 12m40s | 168 |
| ci / test | e2e | 11s | 165 |
| ci / test | agents-smoke | 1s | 105 |
| observatory / timings | build-dev | 11m06s | 16 |
| observatory / timings | build-release | 5m13s | 17 |
| stress / stress | stress | 17m27s | 18 |

## Cache effectiveness (last 30 days)

| workflow / job | rust-cache hit rate | samples |
|---|---:|---:|
| ci / check | 40% | 194 |
| ci / test | 43% | 192 |
| stress / stress | 17% | 18 |

## Cold build (observatory)

### dev: 11m02s (previous: 11m45s) — 543 units at `6c7d65e8e`

| slowest units | wall |
|---|---:|
| `libghostty-vt-sys build script (run)` | 121.35s |
| `phux bin "phux"` | 90.05s |
| `phux-client lib (test)` | 88.53s |
| `phux-server` | 77.84s |
| `phux-server lib (test)` | 59.62s |
| `rustls` | 45.79s |
| `phux bin "phux" (test)` | 44.64s |
| `phux-config` | 34.72s |

### release: 5m34s (previous: 5m35s) — 366 units at `6c7d65e8e`

| slowest units | wall |
|---|---:|
| `libghostty-vt-sys build script (run)` | 147.37s |
| `phux bin "phux"` | 135.53s |
| `phux-server` | 28.16s |
| `phux-config` | 24.01s |
| `phux-mcp bin "phux-mcp"` | 22.81s |
| `regex-automata` | 21.31s |
| `rustls` | 14.95s |
| `clap_builder` | 13.03s |

## Release binary size

| binary | size | previous |
|---|---:|---:|
| `phux` | 14.8 MiB | 14.7 MiB |
| `phux-mcp` | 2.1 MiB | 2.1 MiB |

## Dependency graph

- locked packages: **438** (previous: 438) — 13 workspace members, 52 direct deps
- duplicate versions: **32** (previous: 32)
- proc-macro crates: 33; build-script crates: 67

## Slowest tests (latest instrumented run, `6c7d65e8e`)

| test | wall |
|---|---:|
| `phux-server::terminal_actor::tests::resize_desync_then_both_shrink_does_not_overflow` | 14.398s |
| `phux-server::agent_detect::a_plain_shell_pane_never_gets_an_agent_record` | 6.015s |
| `phux-relay::relay_auth::stalled_preamble_does_not_wedge_relay` | 5.247s |
| `phux-server::agent_detect::an_identity_only_set_gets_its_state_filled_in_by_the_detector` | 3.813s |
| `phux-server::agent_detect::deleting_the_record_hands_it_back_to_the_detector` | 3.811s |
| `phux-server::agent_detect::detector_publishes_blocked_from_a_live_prompt_box` | 3.513s |
| `phux-server::server_idle_exit::without_the_flag_an_unattended_server_stays_up` | 3.012s |
| `phux-server::agent_events::unattached_subscriber_receives_events` | 3.012s |
| `phux-server::phux_3uv_acked_incremental::acked_incremental_converges_and_seq_is_monotonic` | 2.511s |
| `phux-record::golden_cast::both_containers_agree_on_the_frame_count_for_one_recording` | 2.219s |

## Recent runs

| when | workflow | event | branch | result | wall | runner time |
|---|---|---|---|---|---:|---:|
| 2026-08-02 06:32 | conventional-commits | pull_request | fix/adr-0066-collision | success | 22s | 18s |
| 2026-08-02 06:31 | conventional-commits | pull_request | feat/cache-preserving-agent-awar | success | 15s | 11s |
| 2026-08-02 06:24 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-08-02 06:24 | conventional-commits | pull_request | release-please--branches--main-- | success | 15s | 10s |
| 2026-08-02 06:24 | conventional-commits | pull_request | release-please--branches--main-- | cancelled | 24s | 14s |
| 2026-08-02 06:24 | conventional-commits | pull_request | release-please--branches--main-- | cancelled | 2s | 0s |
| 2026-08-02 06:24 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-08-02 06:23 | release-please | push | main | success | 45s | 38s |
| 2026-08-02 06:23 | observatory | push | main | success | 12m23s | 24m40s |
| 2026-08-02 06:23 | ci | push | main | success | 16m08s | 16m22s |
| 2026-08-02 06:12 | release-please | push | main | success | 24s | 20s |
| 2026-08-02 06:12 | ci | push | main | success | 14m56s | 12m45s |
| 2026-08-02 06:10 | conventional-commits | pull_request | feat/agent-session-restore | success | 20s | 17s |
| 2026-08-02 06:10 | ci | pull_request | feat/agent-session-restore | success | 12m56s | 16m20s |
| 2026-08-02 06:08 | conventional-commits | pull_request | feat/agent-session-restore | success | 16s | 13s |
| 2026-08-02 06:08 | ci | pull_request | feat/agent-session-restore | cancelled | 2m36s | 4m36s |
| 2026-08-02 06:08 | conventional-commits | pull_request | chore/consolidate-homebrew-tap | success | 17s | 13s |
| 2026-08-02 06:08 | ci | pull_request | chore/consolidate-homebrew-tap | success | 10m10s | 12m20s |
| 2026-08-02 06:07 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-08-02 06:07 | conventional-commits | pull_request | release-please--branches--main-- | success | 21s | 11s |
| 2026-08-02 06:06 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-08-02 06:06 | conventional-commits | pull_request | release-please--branches--main-- | success | 24s | 13s |
| 2026-08-02 06:06 | conventional-commits | pull_request | release-please--branches--main-- | cancelled | 2s | 0s |
| 2026-08-02 06:06 | release-please | push | main | success | 56s | 44s |
| 2026-08-02 06:06 | ci | push | main | cancelled | 6m28s | 0s |
| 2026-08-02 06:03 | conventional-commits | pull_request | fix/pi-package-arbitration | success | 17s | 13s |
| 2026-08-02 06:03 | ci | pull_request | fix/pi-package-arbitration | success | 11m38s | 13m51s |
| 2026-08-02 05:56 | ci | pull_request | release-please--branches--main-- | skipped | 9s | 0s |
| 2026-08-02 05:56 | conventional-commits | pull_request | release-please--branches--main-- | success | 14s | 10s |
| 2026-08-02 05:56 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |

---

Query the raw store directly, e.g. every recorded ci run's wall time:

```sh
git fetch origin ci-metrics && git show origin/ci-metrics:runs/2026-08.ndjson \
  | jq -r 'select(.kind == "run" and .workflow == "ci") | [.created_at, .conclusion, .duration_s] | @tsv'
```
