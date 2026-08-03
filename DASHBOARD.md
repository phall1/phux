# phux CI dashboard

Generated 2026-08-03T00:31:51Z by the ci-metrics workflow. Do not edit —
every table is re-rendered from `runs/*.ndjson` on each update.
Machine rollup: [`site/summary.json`](site/summary.json), rendered live at
<https://phux.phall.io/ci>.

## Workflows, last 30 days

| workflow | runs | success | median | p95 | runner minutes |
|---|---:|---:|---:|---:|---:|
| ci | 384 | 55% | 11m50s | 18m24s | 4323 |
| observatory | 22 | 91% | 12m23s | 12m56s | 507 |
| stress | 39 | 38% | 4m27s | 22m37s | 334 |
| release-please | 83 | 98% | 46s | 7m37s | 199 |
| conventional-commits | 350 | 79% | 16s | 24s | 70 |

## ci jobs, last 30 days

| job | runs | median queue | median wall | p95 wall |
|---|---:|---:|---:|---:|
| test | 378 | 2s | 11m30s | 17m34s |
| check | 376 | 2s | 2m37s | 5m05s |
| detect docs-only | 379 | 2s | 5s | 8s |

## Slowest ci steps (median, last 30 days)

| job | step | median | samples |
|---|---|---:|---:|
| test | tests (unit + e2e) | 9m34s | 12 |
| check | rust checks (fmt + clippy + doc + deny) | 2m56s | 14 |
| check | Run Swatinem/rust-cache@v2 | 16s | 15 |
| test | Run Swatinem/rust-cache@v2 | 15s | 15 |
| test | agents smoke | 11s | 12 |
| check | docs-check | 10s | 14 |
| check | runner disk headroom | 8s | 15 |
| test | Run DeterminateSystems/nix-installer-action@v22 | 7s | 15 |
| test | runner disk headroom | 7s | 15 |
| check | Run DeterminateSystems/nix-installer-action@v22 | 6s | 15 |

## Cargo phases inside the lanes (median, last 30 days)

| workflow / job | phase | median | samples |
|---|---|---:|---:|
| ci / check | clippy | 1m47s | 209 |
| ci / check | doc | 10s | 209 |
| ci / check | deny | 3s | 208 |
| ci / check | fmt | 2s | 215 |
| ci / test | unit | 12m21s | 194 |
| ci / test | e2e | 11s | 190 |
| ci / test | agents-smoke | 1s | 130 |
| observatory / timings | build-dev | 11m02s | 20 |
| observatory / timings | build-release | 5m23s | 21 |
| stress / stress | stress | 17m27s | 19 |

## Cache effectiveness (last 30 days)

| workflow / job | rust-cache hit rate | samples |
|---|---:|---:|
| ci / check | 43% | 223 |
| ci / test | 45% | 221 |
| stress / stress | 16% | 19 |

## Cold build (observatory)

### dev: 10m55s (previous: 11m26s) — 537 units at `a0e1ac329`

| slowest units | wall |
|---|---:|
| `libghostty-vt-sys build script (run)` | 178.08s |
| `phux-server lib (test)` | 103.89s |
| `phux bin "phux"` | 96.46s |
| `phux-client lib (test)` | 81.86s |
| `phux-server` | 80.46s |
| `phux bin "phux" (test)` | 52.93s |
| `phux-config` | 35.22s |
| `phux-server test "spawn_terminal" (test)` | 31.02s |

### release: 6m12s (previous: 5m58s) — 366 units at `a0e1ac329`

| slowest units | wall |
|---|---:|
| `libghostty-vt-sys build script (run)` | 182.98s |
| `phux bin "phux"` | 136.08s |
| `phux-server` | 28.78s |
| `phux-config` | 24.23s |
| `phux-mcp bin "phux-mcp"` | 23.29s |
| `rustls` | 14.45s |
| `regex-automata` | 11.89s |
| `ring build script (run)` | 11.18s |

## Release binary size

| binary | size | previous |
|---|---:|---:|
| `phux` | 15.4 MiB | 15.3 MiB |
| `phux-mcp` | 2.1 MiB | 2.1 MiB |

## Dependency graph

- locked packages: **438** (previous: 438) — 13 workspace members, 52 direct deps
- duplicate versions: **32** (previous: 32)
- proc-macro crates: 33; build-script crates: 67

## Slowest tests (latest instrumented run, `a0e1ac329`)

| test | wall |
|---|---:|
| `phux-record::golden_cast::golden_cast_renders_gif_and_apng_and_frame_counts_agree` | 4.066s |
| `phux-server::terminal_actor::tests::resize_desync_then_both_shrink_does_not_overflow` | 3.958s |
| `phux-server::terminal_actor::tests::xtwinops_size_queries_answered_from_resized_geometry` | 1.457s |
| `phux-server::phux_3uv_acked_incremental::acked_incremental_converges_and_seq_is_monotonic` | 1.312s |
| `phux-server::agent_detect::a_plain_shell_pane_never_gets_an_agent_record` | 1.215s |
| `phux-relay::relay_auth::bad_consumer_bearer_rejected_per_stream_tunnel_survives` | 1.147s |
| `phux-relay::relay_auth::enrolled_token_admits_tunnel_and_serves_consumers` | 1.146s |
| `phux-server::agent_detect::deleting_the_record_hands_it_back_to_the_detector` | 1.114s |
| `phux-server::agent_detect::an_identity_only_set_gets_its_state_filled_in_by_the_detector` | 1.114s |
| `phux-server::server_self_exit::server_without_clients_does_not_self_exit_on_seed_pane_death` | 1.013s |

## Recent runs

| when | workflow | event | branch | result | wall | runner time |
|---|---|---|---|---|---:|---:|
| 2026-08-03 00:30 | stress | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-08-03 00:29 | release-please | push | main | failure | 1m36s | 2m14s |
| 2026-08-03 00:13 | ci | pull_request | release-please--branches--main-- | skipped | 2s | 0s |
| 2026-08-03 00:13 | conventional-commits | pull_request | release-please--branches--main-- | success | 14s | 12s |
| 2026-08-03 00:12 | ci | pull_request | release-please--branches--main-- | skipped | 2s | 0s |
| 2026-08-03 00:12 | conventional-commits | pull_request | release-please--branches--main-- | cancelled | 2s | 0s |
| 2026-08-03 00:12 | conventional-commits | pull_request | release-please--branches--main-- | success | 19s | 14s |
| 2026-08-03 00:12 | release-please | push | main | success | 58s | 46s |
| 2026-08-03 00:12 | observatory | push | main | success | 12m23s | 25m33s |
| 2026-08-03 00:12 | ci | push | main | success | 13m12s | 18m03s |
| 2026-08-02 23:36 | conventional-commits | pull_request | feat/ux-wave-8 | success | 26s | 16s |
| 2026-08-02 23:36 | ci | pull_request | feat/ux-wave-8 | success | 13m01s | 17m07s |
| 2026-08-02 23:36 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-08-02 23:36 | conventional-commits | pull_request | release-please--branches--main-- | success | 16s | 13s |
| 2026-08-02 23:35 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-08-02 23:35 | conventional-commits | pull_request | release-please--branches--main-- | success | 14s | 10s |
| 2026-08-02 23:35 | conventional-commits | pull_request | release-please--branches--main-- | cancelled | 2s | 0s |
| 2026-08-02 23:35 | release-please | push | main | success | 42s | 38s |
| 2026-08-02 23:35 | observatory | push | main | success | 12m41s | 25m50s |
| 2026-08-02 23:35 | ci | push | main | success | 12m56s | 18m09s |
| 2026-08-02 23:30 | conventional-commits | pull_request | feat/ux-wave-8 | success | 20s | 10s |
| 2026-08-02 23:30 | ci | pull_request | feat/ux-wave-8 | cancelled | 5m46s | 9m47s |
| 2026-08-02 23:30 | conventional-commits | pull_request | chore/zig-0.16-libghostty-bump | success | 17s | 11s |
| 2026-08-02 23:30 | conventional-commits | pull_request | chore/zig-0.16-libghostty-bump | cancelled | 15s | 4s |
| 2026-08-02 23:30 | ci | pull_request | chore/zig-0.16-libghostty-bump | success | 14m05s | 18m53s |
| 2026-08-02 23:24 | conventional-commits | pull_request | chore/zig-0.16-libghostty-bump | success | 19s | 16s |
| 2026-08-02 23:24 | ci | pull_request | chore/zig-0.16-libghostty-bump | cancelled | 6m40s | 11m18s |
| 2026-08-02 22:30 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-08-02 22:30 | conventional-commits | pull_request | release-please--branches--main-- | success | 15s | 12s |
| 2026-08-02 22:30 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |

---

Query the raw store directly, e.g. every recorded ci run's wall time:

```sh
git fetch origin ci-metrics && git show origin/ci-metrics:runs/2026-08.ndjson \
  | jq -r 'select(.kind == "run" and .workflow == "ci") | [.created_at, .conclusion, .duration_s] | @tsv'
```
