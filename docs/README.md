---
audience: contributors, agents
stability: stable
last-reviewed: 2026-05-28
---

# docs/

**TL;DR.** The doc tree. Concepts and consumer-shaped guides live as
single files; the wire spec and the architecture description are split
per concept under their own subdirectories.

**Start here:** [`CONVENTIONS.md`](./CONVENTIONS.md) defines the doc
system itself (frontmatter, TL;DR rule, ADR template, CI gates) — read
before adding or moving content.

---

## Layout

| Path | Owns |
|---|---|
| [CONVENTIONS.md](./CONVENTIONS.md) | The doc system itself: layers, frontmatter, TL;DR rule, ADR template, CI gates |
| [CONCEPTS.md](./CONCEPTS.md) | The canonical "what is phux" — terminals as substrate, layered protocol, libghostty foundation, federation arc |
| [QUICKSTART.md](./QUICKSTART.md) | Run the dev shell, attach a terminal, exercise the demo |
| [vision.md](./vision.md) | The long arc — agents, federation, the "smol" thesis |
| [operations.md](./operations.md) | Errors, logging, telemetry, security boundaries |
| [spec/](./spec/) | Normative wire — proto / L1 / L2 / L3 / appendices, versioned with `phux-protocol` |
| [architecture/](./architecture/) | Process model, threading, transport, crate graph, state replay |
| [consumers/](./consumers/) | One file per consumer surface — `tui.md`, `sdk.md` |

[`../ADR/`](../ADR/) holds decision records — one decision per file,
Nygard template, strict `Status:` vocabulary. Start at
[`../ADR/README.md`](../ADR/README.md) for the index.

## What's not here

- Code-level documentation lives in `crates/*/src/` as rustdoc.
  `cargo doc --workspace --all-features` is the rendered view.
- Scratch research lives in [`../research/`](../research/) with
  `stability: scratch`. Ratified findings move into ADRs or one of the
  reference subdirs above.
