---
audience: contributors, agents
stability: evolving
last-reviewed: 2026-06-06
---

# Predictive local echo

**TL;DR.** The Mosh-style "paint under your finger" property, layered
on top of the client's mirror Terminal so it works uniformly over any
transport. Grapheme-level predictions in a sparse overlay tagged by
epoch; the mirror stays authoritative; reconciliation runs on
`PANE_OUTPUT`, never on `FRAME_ACK`. Designed, not yet implemented.

---

> **Status:** Design intent. Not yet implemented as of 2026-05-26.
> The mirror `Terminal` and `RenderState` redraw path landed with
> ADR-0013; the overlay layer below is the next step.

Predictive local echo is the Mosh property users actually feel on a
slow link: the cell paints under your finger, the network round-trip
catches up later. We implement it as a **client feature** layered on
top of the local mirror `Terminal`, not as a transport feature, so it
works uniformly over UDS, SSH-stdio, and a future QUIC transport.

## Mechanism

Three structures inside the client, all per attached pane:

- **Mirror Terminal** — `libghostty_vt::Terminal` fed by `PANE_OUTPUT`.
  Authoritative for the user's visible grid. Predictions never modify it.
- **Prediction overlay** — a sparse `(row, col) -> PredictedCell` map
  drawn on top of the mirror at render time. Cells are styled (dim /
  underline) until the server confirms them.
- **Epoch counter** — monotonic id tagging each prediction with the
  network state at the time it was made. Predictions older than a TTL
  with no confirming `PANE_OUTPUT` are killed (treated as wrong).

phux's structured-input choice (ADR-0006 / ADR-0008) means the client
cannot byte-predict the way Mosh does: the libghostty `Encoder` lives
server-side, so the client doesn't know whether the user's `'a'` will
hit the PTY as `0x61` (insertable text) or be swallowed by the inner
program (vim normal mode, less, etc.). v0.1 therefore predicts at the
**grapheme level** — "if the cursor is plausibly in insertable-text
context, the next visible cell will be this grapheme." Conservative by
default; matches Mosh's safety posture. A future v0.2 enlargement may
add a parallel client-side encoder for richer predictions, with the
extra divergence risk that implies.

## Sequence

The happy path: a single keypress the server echoes back unchanged.

```
User      Client                                          Server
 │ key 'a' │                                                │
 ├────────►│                                                │
 │         │ 1. predict: paint 'a' at cursor in overlay     │
 │         │    (epoch = N, style = dim/underline)          │
 │         │ 2. INPUT_KEY {pane, KeyEvent('a', …)}          │
 │         ├───────────────────────────────────────────────►│ encode → PTY → shell echoes
 │         │ 3. PANE_OUTPUT {pane, seq=K, bytes=0x61}        │
 │         │◄───────────────────────────────────────────────┤
 │         │ 4. vt_write into mirror; reconcile: prediction  │
 │         │    at (row,col,'a') matches mirror → CONFIRM,   │
 │         │    drop overlay entry                           │
```

A prediction the mirror later contradicts (e.g. the user is in vim
normal mode, so the cursor moves instead of inserting) is dropped on
the next `PANE_OUTPUT` and the cell is redrawn from the mirror. A
prediction that ages past `predict_ttl_ms` with no confirming
`PANE_OUTPUT` is killed the same way.

Two invariants make this safe: the mirror is authoritative, so a
predictor bug cannot corrupt the visible grid past the next redraw; and
reconciliation keys off `PANE_OUTPUT`, not `FRAME_ACK` — the ack is a
flow-control signal (SPEC §12.2) with no rendering meaning, so reshaping
it in a later minor version does not affect echo.
