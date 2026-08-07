# phux CI dashboard

Generated 2026-08-07T13:27:21Z by the ci-metrics workflow. Do not edit —
every table is re-rendered from `runs/*.ndjson` on each update.
Machine rollup: [`site/summary.json`](site/summary.json), rendered live at
<https://phux.phall.io/ci>.

## Workflows, last 30 days

| workflow | runs | success | median | p95 | runner minutes |
|---|---:|---:|---:|---:|---:|
| ci | 477 | 53% | 10m26s | 18m14s | 5159 |
| observatory | 31 | 94% | 12m23s | 13m14s | 725 |
| stress | 53 | 38% | 11s | 22m15s | 367 |
| release-please | 102 | 97% | 48s | 7m47s | 279 |
| conventional-commits | 428 | 80% | 17s | 25s | 88 |
| release | 3 | 100% | 8m02s | 8m02s | 61 |

## ci jobs, last 30 days

| job | runs | median queue | median wall | p95 wall |
|---|---:|---:|---:|---:|
| test | 467 | 2s | 10m24s | 17m24s |
| check | 469 | 2s | 2m33s | 5m25s |
| detect docs-only | 472 | 2s | 5s | 9s |

## Slowest ci steps (median, last 30 days)

| job | step | median | samples |
|---|---|---:|---:|
| test | tests (unit + e2e) | 9m13s | 13 |
| check | rust checks (fmt + clippy + doc + deny) | 4m20s | 14 |
| test | Run Swatinem/rust-cache@v2 | 15s | 17 |
| check | Run Swatinem/rust-cache@v2 | 14s | 17 |
| check | docs-check | 11s | 15 |
| test | agents smoke | 10s | 13 |
| check | Run DeterminateSystems/nix-installer-action@v22 | 7s | 17 |
| check | runner disk headroom | 7s | 17 |
| test | Run DeterminateSystems/nix-installer-action@v22 | 7s | 17 |
| test | runner disk headroom | 7s | 17 |

## Cargo phases inside the lanes (median, last 30 days)

| workflow / job | phase | median | samples |
|---|---|---:|---:|
| ci / check | clippy | 1m49s | 262 |
| ci / check | doc | 10s | 262 |
| ci / check | deny | 3s | 260 |
| ci / check | fmt | 2s | 271 |
| ci / test | unit | 11m45s | 242 |
| ci / test | e2e | 19s | 234 |
| ci / test | agents-smoke | 1s | 170 |
| observatory / timings | build-dev | 10m55s | 29 |
| observatory / timings | build-release | 5m29s | 30 |
| stress / stress | stress | 7m04s | 24 |

## Cache effectiveness (last 30 days)

| workflow / job | rust-cache hit rate | samples |
|---|---:|---:|
| ci / check | 43% | 280 |
| ci / test | 43% | 279 |
| stress / stress | 13% | 24 |

## Cold build (observatory)

### dev: 9m48s (previous: 10m06s) — 552 units at `8821f4145`

| slowest units | wall |
|---|---:|
| `libghostty-vt-sys build script (run)` | 256.0s |
| `phux-server lib (test)` | 143.28s |
| `phux bin "phux"` | 107.15s |
| `phux-server` | 97.63s |
| `phux-client lib (test)` | 88.83s |
| `phux bin "phux" (test)` | 61.98s |
| `phux-server-testkit` | 37.65s |
| `phux-config` | 35.81s |

### release: 6m51s (previous: 6m37s) — 368 units at `8821f4145`

| slowest units | wall |
|---|---:|
| `libghostty-vt-sys build script (run)` | 207.03s |
| `phux bin "phux"` | 142.52s |
| `phux-server` | 37.04s |
| `phux-config` | 29.1s |
| `phux-mcp bin "phux-mcp"` | 23.77s |
| `rustls` | 16.11s |
| `phux-server-testkit` | 14.48s |
| `phux-client` | 14.24s |

## Release binary size

| binary | size | previous |
|---|---:|---:|
| `phux` | 16.4 MiB | 16.4 MiB |
| `phux-mcp` | 2.1 MiB | 2.1 MiB |

## Dependency graph

- locked packages: **441** (previous: 441) — 15 workspace members, 53 direct deps
- duplicate versions: **33** (previous: 33)
- proc-macro crates: 33; build-script crates: 67

## Slowest tests (latest instrumented run, `1db6d7c8f`)

| test | wall |
|---|---:|
| `phux::config_plugin_actions::config_run_timeout_returns_125_and_json_timeout` | 1.011s |
| `phux::bin/phux::commands::attach::tests::reconnect_probe_distinguishes_gone_live_and_dead_sockets` | 0.309s |
| `phux::bin/phux::commands::update::tests::a_full_update_installs_verifies_and_hands_off` | 0.274s |
| `phux::bin/phux::commands::update::tests::a_tampered_archive_is_refused_and_nothing_is_replaced` | 0.272s |
| `phux::bin/phux::commands::update::tests::dry_run_downloads_and_verifies_but_installs_nothing` | 0.260s |
| `phux::bin/phux::commands::overlay::tests::wedged_tailscale_binary_is_killed_at_the_deadline` | 0.256s |
| `phux::bin/phux::commands::update::tests::rollback_restores_the_previous_release_and_hands_off_again` | 0.235s |
| `phux::plugin_demo_package::integration_template_actions_are_local_and_validated` | 0.231s |
| `phux::bin/phux::commands::agent::shim::tests::wrapper_never_relaunches_claude_after_a_mid_session_crash` | 0.219s |
| `phux::service_login_shell_e2e::service_managed_pane_resolves_a_profile_provided_command` | 0.054s |

## Recent runs

| when | workflow | event | branch | result | wall | runner time |
|---|---|---|---|---|---:|---:|
| 2026-08-07 13:26 | conventional-commits | pull_request | rc/one-zero-candidate-pass | success | 15s | 12s |
| 2026-08-07 13:13 | conventional-commits | pull_request | rc/one-zero-candidate-pass | failure | 17s | 13s |
| 2026-08-07 13:13 | ci | pull_request | rc/one-zero-candidate-pass | failure | 9m22s | 10m20s |
| 2026-08-07 13:10 | conventional-commits | pull_request | rc/one-zero-candidate-pass | failure | 21s | 18s |
| 2026-08-07 13:10 | ci | pull_request | rc/one-zero-candidate-pass | cancelled | 2m32s | 3m50s |
| 2026-08-07 08:18 | stress | schedule | main | success | 6m33s | 6m29s |
| 2026-08-07 07:08 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-08-07 07:08 | conventional-commits | pull_request | release-please--branches--main-- | success | 29s | 26s |
| 2026-08-07 07:07 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-08-07 07:07 | stress | pull_request | release-please--branches--main-- | skipped | 8s | 0s |
| 2026-08-07 07:07 | conventional-commits | pull_request | release-please--branches--main-- | success | 20s | 17s |
| 2026-08-07 07:07 | ci | push | main | success | 10m52s | 16m33s |
| 2026-08-07 07:07 | release-please | push | main | success | 42s | 35s |
| 2026-08-07 06:51 | conventional-commits | pull_request | docs-ios-consumer-contract | success | 14s | 11s |
| 2026-08-07 06:51 | ci | pull_request | docs-ios-consumer-contract | success | 13m15s | 18m24s |
| 2026-08-06 09:43 | stress | schedule | main | success | 6m49s | 6m45s |
| 2026-08-05 15:12 | stress | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-08-05 15:12 | release-please | push | main | success | 8m48s | 22m51s |
| 2026-08-05 15:12 | ci | push | main | success | 11m33s | 17m43s |
| 2026-08-05 15:12 | observatory | push | main | success | 13m55s | 25m53s |
| 2026-08-05 15:12 | ci | pull_request | release-please--branches--main-- | success | 11m16s | 16m51s |
| 2026-08-05 09:40 | stress | schedule | main | success | 7m05s | 6m57s |
| 2026-08-04 19:52 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-08-04 19:52 | conventional-commits | pull_request | release-please--branches--main-- | success | 14s | 11s |
| 2026-08-04 19:51 | stress | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-08-04 19:51 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-08-04 19:51 | conventional-commits | pull_request | release-please--branches--main-- | success | 23s | 14s |
| 2026-08-04 19:51 | release-please | push | main | success | 1m08s | 53s |
| 2026-08-04 19:51 | ci | push | main | success | 7m19s | 9m47s |
| 2026-08-04 19:48 | ci | pull_request | fix/relay-spec-status-current | success | 2m14s | 2m56s |

---

Query the raw store directly, e.g. every recorded ci run's wall time:

```sh
git fetch origin ci-metrics && git show origin/ci-metrics:runs/2026-08.ndjson \
  | jq -r 'select(.kind == "run" and .workflow == "ci") | [.created_at, .conclusion, .duration_s] | @tsv'
```
