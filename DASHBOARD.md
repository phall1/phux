# phux CI dashboard

Generated 2026-07-15T10:06:53Z by the ci-metrics workflow. Do not edit —
every table is re-rendered from `runs/*.ndjson` on each update.
Machine rollup: [`site/summary.json`](site/summary.json), rendered live at
<https://phux.phall.io/ci>.

## Workflows, last 30 days

| workflow | runs | success | median | p95 | runner minutes |
|---|---:|---:|---:|---:|---:|
| ci | 61 | 62% | 13m15s | 16m52s | 758 |
| stress | 6 | 50% | 6s | 21m33s | 65 |
| observatory | 2 | 100% | 11m44s | 11m44s | 47 |
| release-please | 11 | 100% | 36s | 44s | 22 |
| conventional-commits | 51 | 92% | 16s | 20s | 10 |

## ci jobs, last 30 days

| job | runs | median queue | median wall | p95 wall |
|---|---:|---:|---:|---:|
| test | 60 | 2s | 13m04s | 16m41s |
| check | 59 | 2s | 2m37s | 4m23s |
| detect docs-only | 61 | 2s | 5s | 7s |

## Slowest ci steps (median, last 30 days)

| job | step | median | samples |
|---|---|---:|---:|
| test | tests (unit + e2e) | 12m58s | 14 |
| check | rust checks (fmt + clippy + doc + deny) | 3m20s | 15 |
| test | Run Swatinem/rust-cache@v2 | 20s | 20 |
| check | Run Swatinem/rust-cache@v2 | 19s | 19 |
| check | docs-check | 9s | 17 |
| check | Run DeterminateSystems/nix-installer-action@v22 | 8s | 19 |
| test | Run DeterminateSystems/nix-installer-action@v22 | 8s | 20 |

## Cargo phases inside the lanes (median, last 30 days)

| workflow / job | phase | median | samples |
|---|---|---:|---:|
| ci / check | clippy | 2m08s | 20 |
| ci / check | doc | 14s | 20 |
| ci / check | deny | 3s | 20 |
| ci / check | fmt | 1s | 22 |
| ci / test | unit | 11m57s | 17 |
| ci / test | e2e | 8s | 16 |
| observatory / timings | build-dev | 10m39s | 2 |
| observatory / timings | build-release | 4m55s | 2 |
| stress / stress | stress | 20m31s | 1 |

## Cache effectiveness (last 30 days)

| workflow / job | rust-cache hit rate | samples |
|---|---:|---:|
| ci / check | 36% | 25 |
| ci / test | 35% | 23 |
| stress / stress | 0% | 1 |

## Cold build (observatory)

### dev: 10m48s (previous: 10m40s) — 519 units at `0abd5a5ed`

| slowest units | wall |
|---|---:|
| `libghostty-vt-sys build script (run)` | 118.77s |
| `phux-server lib (test)` | 89.01s |
| `phux bin "phux"` | 67.92s |
| `phux-client lib (test)` | 59.91s |
| `phux-server` | 53.93s |
| `rustls` | 44.92s |
| `phux-server test "hub_relay_federation" (test)` | 33.62s |
| `phux-server test "spawn_terminal" (test)` | 33.28s |

### release: 5m01s (previous: 4m55s) — 358 units at `0abd5a5ed`

| slowest units | wall |
|---|---:|
| `libghostty-vt-sys build script (run)` | 156.92s |
| `phux bin "phux"` | 98.53s |
| `phux-server` | 22.45s |
| `regex-automata` | 20.4s |
| `phux-mcp bin "phux-mcp"` | 20.0s |
| `phux-config` | 17.06s |
| `rustls` | 17.04s |
| `clap_builder` | 16.5s |

## Release binary size

| binary | size | previous |
|---|---:|---:|
| `phux` | 12.6 MiB | 12.6 MiB |
| `phux-mcp` | 2.0 MiB | 2.0 MiB |

## Dependency graph

- locked packages: **431** (previous: 431) — 11 workspace members, 47 direct deps
- duplicate versions: **32** (previous: 32)
- proc-macro crates: 33; build-script crates: 67

## Slowest tests (latest instrumented run, `7bbe0884b`)

| test | wall |
|---|---:|
| `phux-server::perf_bursty_output::synthesize_against_reference_alloc_bounded_under_full_churn` | 87.514s |
| `phux-server::terminal_actor::tests::resize_desync_then_both_shrink_does_not_overflow` | 23.735s |
| `phux-server::agent_detect::a_plain_shell_pane_never_gets_an_agent_record` | 6.017s |
| `phux-server::agent_detect::an_identity_only_set_gets_its_state_filled_in_by_the_detector` | 3.818s |
| `phux-server::agent_detect::deleting_the_record_hands_it_back_to_the_detector` | 3.815s |
| `phux-server::agent_detect::detector_publishes_blocked_from_a_live_prompt_box` | 3.516s |
| `phux-server::agent_events::unattached_subscriber_receives_events` | 3.015s |
| `phux-server::phux_3uv_acked_incremental::acked_incremental_converges_and_seq_is_monotonic` | 2.515s |
| `phux-server::l2_adversarial::test_subscribe_events_no_loss` | 2.016s |
| `phux-server::phux_0q8_no_double_emit::live_output_is_delivered_exactly_once` | 1.516s |

## Recent runs

| when | workflow | event | branch | result | wall | runner time |
|---|---|---|---|---|---:|---:|
| 2026-07-15 10:06 | conventional-commits | pull_request | fix/watch-dirty-idle-starvation | success | 16s | 12s |
| 2026-07-15 10:03 | conventional-commits | pull_request | feat/dialout-connector-adr-spike | success | 17s | 13s |
| 2026-07-15 10:03 | conventional-commits | pull_request | feat/overlay-reachability-first- | success | 20s | 16s |
| 2026-07-15 10:00 | conventional-commits | pull_request | feat/opencode-integration | success | 24s | 22s |
| 2026-07-15 09:56 | conventional-commits | pull_request | release-please--branches--main-- | success | 19s | 15s |
| 2026-07-15 09:56 | ci | pull_request | release-please--branches--main-- | skipped | 7s | 0s |
| 2026-07-15 09:56 | conventional-commits | pull_request | fix/client-local-focus | success | 14s | 10s |
| 2026-07-15 09:56 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-07-15 09:56 | conventional-commits | pull_request | release-please--branches--main-- | cancelled | 2s | 1s |
| 2026-07-15 09:56 | conventional-commits | pull_request | release-please--branches--main-- | success | 18s | 12s |
| 2026-07-15 09:56 | conventional-commits | pull_request | fix/headless-watch-idle | success | 17s | 12s |
| 2026-07-15 09:56 | release-please | push | main | success | 41s | 35s |
| 2026-07-15 09:48 | conventional-commits | pull_request | swarm/remove-agent-facade | success | 18s | 15s |
| 2026-07-15 09:48 | ci | pull_request | swarm/remove-agent-facade | success | 16m07s | 18m11s |
| 2026-07-15 09:48 | conventional-commits | pull_request | swarm/shared-state-tags | success | 12s | 10s |
| 2026-07-15 09:48 | conventional-commits | pull_request | fix/client-local-focus | success | 13s | 10s |
| 2026-07-15 09:48 | ci | pull_request | fix/client-local-focus | cancelled | 8m35s | 12m27s |
| 2026-07-15 09:48 | conventional-commits | pull_request | swarm/mcp-registration | success | 14s | 11s |
| 2026-07-15 09:48 | ci | pull_request | swarm/mcp-registration | success | 1m56s | 2m27s |
| 2026-07-15 09:45 | ci | pull_request | swarm/shared-state-tags | cancelled | 3m44s | 7m13s |
| 2026-07-15 09:44 | conventional-commits | pull_request | swarm/mcp-registration | success | 16s | 12s |
| 2026-07-15 09:44 | conventional-commits | pull_request | swarm/remove-agent-facade | success | 17s | 11s |
| 2026-07-15 09:44 | conventional-commits | pull_request | swarm/shared-state-tags | success | 15s | 11s |
| 2026-07-15 09:44 | ci | pull_request | swarm/mcp-registration | success | 1m46s | 2m34s |
| 2026-07-15 09:44 | ci | pull_request | swarm/remove-agent-facade | cancelled | 3m47s | 7m14s |
| 2026-07-15 09:41 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-07-15 09:41 | conventional-commits | pull_request | release-please--branches--main-- | success | 13s | 11s |
| 2026-07-15 09:41 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-07-15 09:41 | conventional-commits | pull_request | release-please--branches--main-- | cancelled | 1s | 1s |
| 2026-07-15 09:41 | conventional-commits | pull_request | release-please--branches--main-- | success | 18s | 14s |

---

Query the raw store directly, e.g. every recorded ci run's wall time:

```sh
git fetch origin ci-metrics && git show origin/ci-metrics:runs/2026-07.ndjson \
  | jq -r 'select(.kind == "run" and .workflow == "ci") | [.created_at, .conclusion, .duration_s] | @tsv'
```
