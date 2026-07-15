# phux CI dashboard

Generated 2026-07-15T10:41:55Z by the ci-metrics workflow. Do not edit —
every table is re-rendered from `runs/*.ndjson` on each update.
Machine rollup: [`site/summary.json`](site/summary.json), rendered live at
<https://phux.phall.io/ci>.

## Workflows, last 30 days

| workflow | runs | success | median | p95 | runner minutes |
|---|---:|---:|---:|---:|---:|
| ci | 79 | 61% | 13m17s | 16m51s | 981 |
| stress | 6 | 50% | 6s | 21m33s | 65 |
| observatory | 2 | 100% | 11m44s | 11m44s | 47 |
| release-please | 13 | 100% | 37s | 48s | 23 |
| conventional-commits | 71 | 92% | 16s | 19s | 14 |

## ci jobs, last 30 days

| job | runs | median queue | median wall | p95 wall |
|---|---:|---:|---:|---:|
| test | 78 | 2s | 13m08s | 16m41s |
| check | 77 | 2s | 2m36s | 4m23s |
| detect docs-only | 79 | 2s | 5s | 7s |

## Slowest ci steps (median, last 30 days)

| job | step | median | samples |
|---|---|---:|---:|
| test | tests (unit + e2e) | 12m50s | 11 |
| check | rust checks (fmt + clippy + doc + deny) | 1m35s | 16 |
| test | Run Swatinem/rust-cache@v2 | 24s | 20 |
| check | Run Swatinem/rust-cache@v2 | 22s | 20 |
| check | docs-check | 9s | 18 |
| test | Run DeterminateSystems/nix-installer-action@v22 | 9s | 20 |
| check | Run DeterminateSystems/nix-installer-action@v22 | 8s | 20 |

## Cargo phases inside the lanes (median, last 30 days)

| workflow / job | phase | median | samples |
|---|---|---:|---:|
| ci / check | clippy | 1m47s | 34 |
| ci / check | doc | 14s | 34 |
| ci / check | deny | 3s | 34 |
| ci / check | fmt | 1s | 36 |
| ci / test | unit | 11m56s | 27 |
| ci / test | e2e | 8s | 26 |
| observatory / timings | build-dev | 10m39s | 2 |
| observatory / timings | build-release | 4m55s | 2 |
| stress / stress | stress | 20m31s | 1 |

## Cache effectiveness (last 30 days)

| workflow / job | rust-cache hit rate | samples |
|---|---:|---:|
| ci / check | 41% | 39 |
| ci / test | 57% | 37 |
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

## Slowest tests (latest instrumented run, `1aa849bea`)

| test | wall |
|---|---:|
| `phux-server::perf_bursty_output::synthesize_against_reference_alloc_bounded_under_full_churn` | 113.528s |
| `phux-server::terminal_actor::tests::resize_desync_then_both_shrink_does_not_overflow` | 27.805s |
| `phux-server::agent_detect::a_plain_shell_pane_never_gets_an_agent_record` | 6.018s |
| `phux-server::agent_detect::deleting_the_record_hands_it_back_to_the_detector` | 3.817s |
| `phux-server::agent_detect::an_identity_only_set_gets_its_state_filled_in_by_the_detector` | 3.815s |
| `phux-server::agent_detect::detector_publishes_blocked_from_a_live_prompt_box` | 3.516s |
| `phux-server::agent_events::unattached_subscriber_receives_events` | 3.015s |
| `phux-server::phux_3uv_acked_incremental::acked_incremental_converges_and_seq_is_monotonic` | 2.515s |
| `phux-server::l2_adversarial::test_subscribe_events_no_loss` | 2.017s |
| `phux-server::phux_0q8_no_double_emit::live_output_is_delivered_exactly_once` | 1.516s |

## Recent runs

| when | workflow | event | branch | result | wall | runner time |
|---|---|---|---|---|---:|---:|
| 2026-07-15 10:41 | conventional-commits | pull_request | swarm/input-encode-train | success | 17s | 13s |
| 2026-07-15 10:29 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-07-15 10:29 | conventional-commits | pull_request | release-please--branches--main-- | success | 15s | 12s |
| 2026-07-15 10:28 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-07-15 10:28 | conventional-commits | pull_request | release-please--branches--main-- | cancelled | 1s | 1s |
| 2026-07-15 10:28 | conventional-commits | pull_request | release-please--branches--main-- | success | 14s | 11s |
| 2026-07-15 10:28 | release-please | push | main | success | 38s | 33s |
| 2026-07-15 10:27 | conventional-commits | pull_request | swarm/spawn-placement | success | 16s | 12s |
| 2026-07-15 10:25 | conventional-commits | pull_request | swarm/foundation-train | success | 15s | 12s |
| 2026-07-15 10:25 | conventional-commits | pull_request | swarm/layout-cli | success | 16s | 13s |
| 2026-07-15 10:25 | ci | pull_request | swarm/layout-cli | success | 14m49s | 16m46s |
| 2026-07-15 10:19 | conventional-commits | pull_request | fix/client-local-focus | success | 14s | 10s |
| 2026-07-15 10:19 | conventional-commits | pull_request | fix/input-lane-ordering | success | 19s | 16s |
| 2026-07-15 10:19 | ci | pull_request | fix/client-local-focus | cancelled | 8m15s | 10m27s |
| 2026-07-15 10:19 | ci | pull_request | fix/input-lane-ordering | cancelled | 8m17s | 11m47s |
| 2026-07-15 10:19 | conventional-commits | pull_request | swarm/remove-agent-facade | success | 14s | 10s |
| 2026-07-15 10:19 | ci | pull_request | swarm/remove-agent-facade | success | 15m47s | 18m02s |
| 2026-07-15 10:19 | conventional-commits | pull_request | swarm/satellite-selectors | success | 15s | 10s |
| 2026-07-15 10:19 | ci | pull_request | swarm/satellite-selectors | success | 14m06s | 17m13s |
| 2026-07-15 10:19 | conventional-commits | pull_request | swarm/pi-parity | success | 15s | 10s |
| 2026-07-15 10:19 | ci | pull_request | swarm/pi-parity | success | 13m48s | 15m56s |
| 2026-07-15 10:18 | conventional-commits | pull_request | swarm/shared-state-tags | success | 14s | 11s |
| 2026-07-15 10:15 | conventional-commits | pull_request | swarm/layout-cli | success | 14s | 12s |
| 2026-07-15 10:15 | ci | pull_request | swarm/layout-cli | cancelled | 9m44s | 13m43s |
| 2026-07-15 10:15 | conventional-commits | pull_request | swarm/spawn-placement | success | 18s | 13s |
| 2026-07-15 10:15 | ci | pull_request | swarm/spawn-placement | cancelled | 12m19s | 16m15s |
| 2026-07-15 10:14 | conventional-commits | pull_request | swarm/mru-selector | success | 19s | 16s |
| 2026-07-15 10:14 | conventional-commits | pull_request | feat/opencode-integration | success | 13s | 11s |
| 2026-07-15 10:14 | ci | pull_request | feat/opencode-integration | success | 14m12s | 16m27s |
| 2026-07-15 10:11 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |

---

Query the raw store directly, e.g. every recorded ci run's wall time:

```sh
git fetch origin ci-metrics && git show origin/ci-metrics:runs/2026-07.ndjson \
  | jq -r 'select(.kind == "run" and .workflow == "ci") | [.created_at, .conclusion, .duration_s] | @tsv'
```
