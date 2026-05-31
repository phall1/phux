---
audience: contributors, agents
stability: stable
last-reviewed: 2026-05-28
---

# docs/architecture/

**TL;DR.** Internal structure of phux — process model, threading,
transport, crate graph, data model, state replay, observability. Not
normative (the wire spec is); not user-facing (the consumer docs are).
What you read to understand *how* phux is built.

---

## Files

| File | Owns |
|---|---|
| [DIAGRAM.md](./DIAGRAM.md) | System shape: PTY, server libghostty, transport, client libghostty, TUI — one-glance overview |
| [process-model.md](./process-model.md) | Per-user server, single process, current-thread runtime; supervision (ADR-0003, ADR-0014) |
| [threading.md](./threading.md) | `!Send`/`!Sync` constraints, LocalSet, why this matters for libghostty |
| [transport.md](./transport.md) | The Transport trait, UDS today, QUIC + SSH-stdio later (ADR-0007) |
| [crate-graph.md](./crate-graph.md) | Crate dependency edges and the protocol↔core independence (ADR-0011) |
| [data-model.md](./data-model.md) | Sessions, windows, terminals, layouts as in-process types — distinct from wire shape |
| [state-replay.md](./state-replay.md) | What happens on attach: snapshots, replay, scrollback policy (ADR-0018) |
| [render-layering.md](./render-layering.md) | ratatui chrome over libghostty pane interiors (ADR-0020) |
| [predictive-echo.md](./predictive-echo.md) | Client-side prediction loop and reconciliation |
| [testing.md](./testing.md) | Test strategy: unit, integration, fixtures, golden snapshots |
| [performance.md](./performance.md) | Hot-path discipline, allocation budget, what `samply` tells us |
| [module-structure.md](./module-structure.md) | Per-crate module layout as it exists in tree today |

## Status

Content for the files above landed under ticket phux-4uw, which split
the former root `ARCHITECTURE.md`. The root file now retains only the
operations sections (error model, logging, security), pending their
move to `docs/operations.md` under ticket phux-ea3.

## What's not here

- Wire bytes — that's [`../spec/`](../spec/).
- TUI surfaces — that's [`../consumers/tui.md`](../consumers/tui.md).
- Decisions — that's [`../../ADR/`](../../ADR/). Architecture docs
  describe *what* the code is; ADRs explain *why* it's that shape.

## When this directory is wrong

Code is the implementation; these documents are the *intended* design. If
they diverge, file an issue. Either the code drifted or the design did.
Both happen; the response is to reconcile, not to let either rot.
