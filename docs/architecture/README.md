---
audience: contributors, agents
stability: stable
last-reviewed: 2026-07-09
---

# Architecture reference

**TL;DR.** Internal structure of phux — process model, threading,
transport, crate graph, data model, state sync, rendering, and the quality
bar. Not normative (the wire spec is); not user-facing (the consumer docs
are). What you read to understand how phux is built.

---

## Files

| File | Owns |
|---|---|
| [process-model.md](./process-model.md) | Per-user server, single process, current-thread runtime; supervision (ADR-0003, ADR-0014) |
| [threading.md](./threading.md) | `!Send`/`!Sync` constraints, LocalSet, why this matters for libghostty |
| [transport.md](./transport.md) | The Transport trait, UDS today, QUIC + SSH-stdio later (ADR-0007) |
| [crate-graph.md](./crate-graph.md) | Crate dependency edges and the protocol-core independence (ADR-0011) |
| [data-model.md](./data-model.md) | Sessions, windows, terminals, layouts as in-process types — distinct from wire shape |
| [state-sync.md](./state-sync.md) | What happens on attach: snapshots, replay, scrollback policy (ADR-0018) |
| [render-layering.md](./render-layering.md) | ratatui chrome over libghostty pane interiors (ADR-0020) |
| [predictive-echo.md](./predictive-echo.md) | Client-side prediction loop and reconciliation |
| [verification.md](./verification.md) | The test and performance quality bar: unit, integration, golden snapshots, hot-path discipline, allocation budget |
| [module-structure.md](./module-structure.md) | Per-crate module layout as it exists in tree today |

## Scratch (not in the published set)

These files exist in the tree but are marked `stability: scratch` and are
not part of the published architecture docs. Read them as working notes,
not as design of record.

- `DIAGRAM.md` — a one-glance system shape sketch (PTY, server libghostty,
  transport, client libghostty, TUI).
- `l2-server-design.md` — a design for an L2 collection tier, superseded by
  [ADR-0030](../../ADR/0030-engine-delegated-wire-and-projection-consumers.md):
  there is no L2 tier. Group lifecycle is L3 metadata plus a single atomic
  L1 batch operation.

## What's not here

- Wire bytes — that's [`../spec/`](../spec/).
- TUI surfaces — that's [`../consumers/tui.md`](../consumers/tui.md).
- Decisions — that's [`../../ADR/`](../../ADR/). Architecture docs
  describe what the code is; ADRs explain why it's that shape.

## When this directory is wrong

Code is the implementation; these documents are the intended design. If
they diverge, file an issue. Either the code drifted or the design did.
Both happen; the response is to reconcile, not to let either rot.
</content>
</invoke>
