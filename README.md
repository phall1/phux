# ci-metrics — the CI observability data branch

Machine-written store, single writer: the `ci-metrics` workflow on `main`
(see ADR-0047 there). Layout:

- `runs/<YYYY-MM>.ndjson` — one JSON record per line: completed workflow
  runs (per-job/per-step wall times from the Actions API) plus the records
  each run's `ci-metrics-*` artifacts carried (cargo phase timings,
  cold-build timelines, binary sizes, dependency stats).
- `DASHBOARD.md` — rendered rollup. Regenerated on every sweep; never edit.
- `site/summary.json` — compact rollup fetched live by phux.phall.io/ci.

Query examples live at the bottom of DASHBOARD.md. Old monthly shards may
be pruned; the dashboard only reads the trailing 30 days.
