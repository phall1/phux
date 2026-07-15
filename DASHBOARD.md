# phux CI dashboard

Generated 2026-07-15T04:26:40Z by the ci-metrics workflow. Do not edit —
every table is re-rendered from `runs/*.ndjson` on each update.
Machine rollup: [`site/summary.json`](site/summary.json), rendered live at
<https://phux.phall.io/ci>.

## Workflows, last 30 days

| workflow | runs | success | median | p95 | runner minutes |
|---|---:|---:|---:|---:|---:|
| ci | 20 | 60% | 12m54s | 16m03s | 240 |
| conventional-commits | 16 | 94% | 16s | 19s | 3 |
| release-please | 5 | 100% | 20s | 21s | 2 |

## ci jobs, last 30 days

| job | runs | median queue | median wall | p95 wall |
|---|---:|---:|---:|---:|
| test | 20 | 2s | 12m43s | 15m54s |
| check | 20 | 2s | 2m35s | 4m02s |
| detect docs-only | 20 | 2s | 5s | 7s |

## Slowest ci steps (median, last 30 days)

| job | step | median | samples |
|---|---|---:|---:|
| test | tests (unit + e2e) | 12m41s | 13 |
| check | rust checks (fmt + clippy + doc + deny) | 1m42s | 14 |
| check | Run Swatinem/rust-cache@v2 | 22s | 16 |
| test | Run Swatinem/rust-cache@v2 | 22s | 15 |
| check | docs-check | 9s | 14 |
| check | Run DeterminateSystems/nix-installer-action@v22 | 8s | 17 |
| test | Run DeterminateSystems/nix-installer-action@v22 | 8s | 17 |

## Cargo phases inside the lanes (median, last 30 days)

| workflow / job | phase | median | samples |
|---|---|---:|---:|
| ci / check | clippy | 40s | 4 |
| ci / check | doc | 11s | 4 |
| ci / check | deny | 3s | 4 |
| ci / check | fmt | 1s | 4 |
| ci / test | unit | 11m56s | 2 |
| ci / test | e2e | 8s | 2 |

## Cache effectiveness (last 30 days)

| workflow / job | rust-cache hit rate | samples |
|---|---:|---:|
| ci / check | 60% | 5 |
| ci / test | 100% | 3 |

## Slowest tests (latest instrumented run, `b10c26f44`)

| test | wall |
|---|---:|
| `phux-server::perf_bursty_output::synthesize_against_reference_alloc_bounded_under_full_churn` | 116.798s |
| `phux-server::terminal_actor::tests::resize_desync_then_both_shrink_does_not_overflow` | 28.688s |
| `phux-server::agent_detect::a_plain_shell_pane_never_gets_an_agent_record` | 6.017s |
| `phux-server::agent_detect::deleting_the_record_hands_it_back_to_the_detector` | 3.814s |
| `phux-server::agent_detect::an_identity_only_set_gets_its_state_filled_in_by_the_detector` | 3.813s |
| `phux-server::agent_detect::detector_publishes_blocked_from_a_live_prompt_box` | 3.517s |
| `phux-server::agent_events::unattached_subscriber_receives_events` | 3.015s |
| `phux-server::phux_3uv_acked_incremental::acked_incremental_converges_and_seq_is_monotonic` | 2.514s |
| `phux-server::runtime::input_lane::tests::lane_routed_input_interleaves_with_a_large_pty_output_burst` | 2.124s |
| `phux-server::l2_adversarial::test_subscribe_events_no_loss` | 2.016s |

## Recent runs

| when | workflow | event | branch | result | wall | runner time |
|---|---|---|---|---|---:|---:|
| 2026-07-15 04:25 | release-please | push | main | success | 20s | 17s |
| 2026-07-15 04:11 | conventional-commits | pull_request | feat/ci-observability | success | 13s | 10s |
| 2026-07-15 04:11 | ci | pull_request | feat/ci-observability | success | 13m41s | 16m16s |
| 2026-07-15 04:08 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-07-15 04:08 | conventional-commits | pull_request | release-please--branches--main-- | success | 16s | 12s |
| 2026-07-15 04:08 | conventional-commits | pull_request | release-please--branches--main-- | success | 13s | 9s |
| 2026-07-15 04:08 | conventional-commits | pull_request | release-please--branches--main-- | cancelled | 1s | 0s |
| 2026-07-15 04:08 | ci | pull_request | release-please--branches--main-- | skipped | 2s | 0s |
| 2026-07-15 04:08 | release-please | push | main | success | 37s | 32s |
| 2026-07-15 04:08 | ci | push | main | success | 13m29s | 17m25s |
| 2026-07-15 03:56 | ci | pull_request | feat/ci-observability | success | 14m03s | 17m54s |
| 2026-07-15 03:56 | conventional-commits | pull_request | feat/ci-observability | success | 18s | 14s |
| 2026-07-15 03:52 | ci | pull_request | fix/mouse-encoder-size-and-scrol | success | 16m03s | 18m26s |
| 2026-07-15 03:52 | conventional-commits | pull_request | fix/mouse-encoder-size-and-scrol | success | 15s | 12s |
| 2026-07-15 03:42 | ci | pull_request | release-please--branches--main-- | skipped | 1s | 0s |
| 2026-07-15 03:42 | conventional-commits | pull_request | release-please--branches--main-- | success | 14s | 10s |
| 2026-07-15 03:42 | ci | push | main | success | 13m41s | 16m11s |
| 2026-07-15 03:42 | release-please | push | main | success | 16s | 13s |
| 2026-07-15 03:40 | ci | pull_request | feat/ci-observability | failure | 13m10s | 16m13s |
| 2026-07-15 03:40 | conventional-commits | pull_request | feat/ci-observability | success | 12s | 10s |
| 2026-07-15 03:39 | conventional-commits | pull_request | feat/ci-observability | success | 16s | 12s |
| 2026-07-15 03:39 | ci | pull_request | feat/ci-observability | cancelled | 1m11s | 1m24s |
| 2026-07-15 03:38 | conventional-commits | pull_request | feat/ci-observability | success | 17s | 14s |
| 2026-07-15 03:38 | ci | pull_request | feat/ci-observability | cancelled | 1m28s | 1m45s |
| 2026-07-15 03:36 | conventional-commits | pull_request | fix/mouse-encoder-size-and-scrol | success | 16s | 14s |
| 2026-07-15 03:36 | ci | pull_request | fix/mouse-encoder-size-and-scrol | success | 12m49s | 15m17s |
| 2026-07-15 03:30 | conventional-commits | pull_request | feat/ci-observability | success | 19s | 16s |
| 2026-07-15 03:30 | ci | pull_request | feat/ci-observability | cancelled | 8m29s | 10m55s |
| 2026-07-15 03:29 | conventional-commits | pull_request | ci/draft-release-prs | success | 16s | 12s |
| 2026-07-15 03:29 | ci | pull_request | ci/draft-release-prs | success | 12m54s | 15m26s |

---

Query the raw store directly, e.g. every recorded ci run's wall time:

```sh
git fetch origin ci-metrics && git show origin/ci-metrics:runs/2026-07.ndjson \
  | jq -r 'select(.kind == "run" and .workflow == "ci") | [.created_at, .conclusion, .duration_s] | @tsv'
```
