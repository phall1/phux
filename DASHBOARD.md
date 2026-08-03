# phux CI dashboard

Generated 2026-08-03T08:50:10Z by the ci-metrics workflow. Do not edit —
every table is re-rendered from `runs/*.ndjson` on each update.
Machine rollup: [`site/summary.json`](site/summary.json), rendered live at
<https://phux.phall.io/ci>.

## Workflows, last 30 days

| workflow | runs | success | median | p95 | runner minutes |
|---|---:|---:|---:|---:|---:|
| ci | 444 | 53% | 10m59s | 18m24s | 4889 |
| observatory | 26 | 92% | 12m23s | 12m56s | 603 |
| stress | 43 | 35% | 11s | 22m15s | 334 |
| release-please | 94 | 97% | 47s | 7m42s | 228 |
| conventional-commits | 402 | 80% | 17s | 25s | 82 |
| release | 3 | 100% | 8m02s | 8m02s | 61 |

## ci jobs, last 30 days

| job | runs | median queue | median wall | p95 wall |
|---|---:|---:|---:|---:|
| test | 435 | 2s | 10m43s | 17m34s |
| check | 436 | 2s | 2m35s | 5m17s |
| detect docs-only | 439 | 2s | 5s | 8s |

## Slowest ci steps (median, last 30 days)

| job | step | median | samples |
|---|---|---:|---:|
| test | tests (unit + e2e) | 8m45s | 8 |
| check | rust checks (fmt + clippy + doc + deny) | 4m02s | 17 |
| check | Run Swatinem/rust-cache@v2 | 15s | 19 |
| test | Run Swatinem/rust-cache@v2 | 14s | 19 |
| check | docs-check | 11s | 17 |
| test | agents smoke | 11s | 8 |
| check | Run DeterminateSystems/nix-installer-action@v22 | 7s | 19 |
| check | runner disk headroom | 7s | 19 |
| test | Run DeterminateSystems/nix-installer-action@v22 | 7s | 19 |
| test | runner disk headroom | 6s | 19 |

## Cargo phases inside the lanes (median, last 30 days)

| workflow / job | phase | median | samples |
|---|---|---:|---:|
| ci / check | clippy | 1m47s | 245 |
| ci / check | doc | 10s | 245 |
| ci / check | deny | 3s | 243 |
| ci / check | fmt | 2s | 252 |
| ci / test | unit | 11m55s | 224 |
| ci / test | e2e | 13s | 217 |
| ci / test | agents-smoke | 1s | 154 |
| observatory / timings | build-dev | 11m02s | 24 |
| observatory / timings | build-release | 5m26s | 25 |
| stress / stress | stress | 17m27s | 19 |

## Cache effectiveness (last 30 days)

| workflow / job | rust-cache hit rate | samples |
|---|---:|---:|
| ci / check | 44% | 260 |
| ci / test | 44% | 259 |
| stress / stress | 16% | 19 |

## Cold build (observatory)

### dev: 6m48s (previous: 8m09s) — 542 units at `12bbc8878`

| slowest units | wall |
|---|---:|
| `libghostty-vt-sys build script (run)` | 129.61s |
| `phux-server lib (test)` | 94.08s |
| `phux bin "phux"` | 87.47s |
| `phux-client lib (test)` | 72.62s |
| `phux-server` | 69.85s |
| `phux bin "phux" (test)` | 52.92s |
| `phux-config` | 30.75s |
| `phux-server-testkit` | 26.18s |

### release: 6m07s (previous: 6m05s) — 367 units at `12bbc8878`

| slowest units | wall |
|---|---:|
| `libghostty-vt-sys build script (run)` | 184.63s |
| `phux bin "phux"` | 129.52s |
| `phux-server` | 30.25s |
| `phux-config` | 25.24s |
| `phux-mcp bin "phux-mcp"` | 21.96s |
| `rustls` | 13.15s |
| `ring build script (run)` | 12.41s |
| `regex-automata` | 12.38s |

## Release binary size

| binary | size | previous |
|---|---:|---:|
| `phux` | 15.5 MiB | 15.5 MiB |
| `phux-mcp` | 2.1 MiB | 2.1 MiB |

## Dependency graph

- locked packages: **439** (previous: 439) — 14 workspace members, 52 direct deps
- duplicate versions: **32** (previous: 32)
- proc-macro crates: 33; build-script crates: 67

## Slowest tests (latest instrumented run, `52174fb31`)

| test | wall |
|---|---:|
| `phux-server::runtime::attach::tests::prepare_attach_rejects_pane_source_count_before_registration` | 1.694s |
| `phux-record::golden_cast::golden_cast_renders_gif_and_apng_and_frame_counts_agree` | 1.463s |
| `phux-server::terminal_actor::tests::xtwinops_size_queries_answered_from_resized_geometry` | 1.447s |
| `phux-server::phux_3uv_acked_incremental::acked_incremental_converges_and_seq_is_monotonic` | 1.378s |
| `phux-client::sidebar_branch_e2e::sidebar_branch_line_derives_from_attached_snapshot_cwd` | 1.335s |
| `phux-server::agent_detect::a_plain_shell_pane_never_gets_an_agent_record` | 1.214s |
| `phux-server::attach_cwd_snapshot::attached_snapshot_carries_pane_cwd` | 1.191s |
| `phux-server::agent_detect::deleting_the_record_hands_it_back_to_the_detector` | 1.112s |
| `phux-server::agent_detect::an_identity_only_set_gets_its_state_filled_in_by_the_detector` | 1.112s |
| `phux-server::server_idle_exit::connecting_disarms_the_idle_clock` | 1.063s |

## Recent runs

| when | workflow | event | branch | result | wall | runner time |
|---|---|---|---|---|---:|---:|
| 2026-08-03 08:49 | conventional-commits | pull_request | refactor/serverstate-decompositi | success | 19s | 15s |
| 2026-08-03 07:24 | conventional-commits | pull_request | feat/negotiated-libghostty-codec | success | 44s | 41s |
| 2026-08-03 07:24 | ci | pull_request | feat/negotiated-libghostty-codec | failure | 12m01s | 17m31s |
| 2026-08-03 07:10 | conventional-commits | pull_request | feat/negotiated-libghostty-codec | success | 18s | 14s |
| 2026-08-03 07:10 | ci | pull_request | feat/negotiated-libghostty-codec | failure | 10m58s | 17m29s |
| 2026-08-03 06:42 | conventional-commits | pull_request | feat/negotiated-libghostty-codec | success | 21s | 11s |
| 2026-08-03 06:42 | ci | pull_request | feat/negotiated-libghostty-codec | failure | 10m01s | 15m42s |
| 2026-08-03 06:14 | conventional-commits | pull_request | feat/negotiated-libghostty-codec | success | 17s | 14s |
| 2026-08-03 06:14 | ci | pull_request | feat/negotiated-libghostty-codec | cancelled | 20m16s | 26m00s |
| 2026-08-03 06:01 | conventional-commits | pull_request | feat/negotiated-libghostty-codec | success | 18s | 14s |
| 2026-08-03 06:01 | ci | pull_request | feat/negotiated-libghostty-codec | failure | 9m13s | 14m56s |
| 2026-08-03 05:47 | conventional-commits | pull_request | feat/negotiated-libghostty-codec | success | 22s | 12s |
| 2026-08-03 05:47 | ci | pull_request | feat/negotiated-libghostty-codec | failure | 10m00s | 15m26s |
| 2026-08-03 05:40 | conventional-commits | pull_request | feat/negotiated-libghostty-codec | success | 17s | 14s |
| 2026-08-03 05:40 | ci | pull_request | feat/negotiated-libghostty-codec | cancelled | 7m00s | 9m08s |
| 2026-08-03 05:35 | stress | pull_request | release-please--branches--main-- | skipped | 7s | 0s |
| 2026-08-03 05:35 | release-please | push | main | success | 8m32s | 21m22s |
| 2026-08-03 05:35 | ci | push | main | success | 10m24s | 15m28s |
| 2026-08-03 05:35 | observatory | push | main | success | 12m16s | 21m12s |
| 2026-08-03 05:24 | ci | pull_request | release-please--branches--main-- | success | 10m21s | 15m07s |
| 2026-08-03 05:08 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-08-03 05:08 | conventional-commits | pull_request | release-please--branches--main-- | success | 24s | 14s |
| 2026-08-03 05:07 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-08-03 05:07 | conventional-commits | pull_request | release-please--branches--main-- | success | 22s | 19s |
| 2026-08-03 05:07 | release-please | push | main | success | 1m05s | 47s |
| 2026-08-03 05:07 | ci | push | main | success | 7m38s | 9m18s |
| 2026-08-03 04:59 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-08-03 04:59 | conventional-commits | pull_request | release-please--branches--main-- | success | 20s | 11s |
| 2026-08-03 04:58 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-08-03 04:58 | conventional-commits | pull_request | release-please--branches--main-- | cancelled | 2s | 1s |

---

Query the raw store directly, e.g. every recorded ci run's wall time:

```sh
git fetch origin ci-metrics && git show origin/ci-metrics:runs/2026-08.ndjson \
  | jq -r 'select(.kind == "run" and .workflow == "ci") | [.created_at, .conclusion, .duration_s] | @tsv'
```
