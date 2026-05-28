---
audience: contributors, agents
stability: evolving
last-reviewed: 2026-05-27
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

The happy path (single keypress, server echoes the same grapheme back):

```
User      Client                                          Server                          PTY/Shell
 │         │                                                │                                │
 │ key 'a' │                                                │                                │
 ├────────►│                                                │                                │
 │         │ 1. predict: paint 'a' at cursor in overlay     │                                │
 │         │    (epoch = N, style = dim/underline)          │                                │
 │         │ 2. INPUT_KEY {pane, KeyEvent('a', …)}          │                                │
 │         ├───────────────────────────────────────────────►│                                │
 │         │                                                │ 3. libghostty Encoder → 0x61   │
 │         │                                                │ 4. write to PTY                │
 │         │                                                ├───────────────────────────────►│
 │         │                                                │                                │ 5. shell
 │         │                                                │                                │    echoes
 │         │                                                │ 6. feed bytes to canonical     │◄─┐
 │         │                                                │    libghostty Terminal         │  │
 │         │                                                │◄───────────────────────────────┘  │
 │         │ 7. PANE_OUTPUT {pane, seq=K, bytes=0x61}       │                                   │
 │         │◄───────────────────────────────────────────────┤                                   │
 │         │ 8. vt_write bytes into mirror Terminal         │                                   │
 │         │ 9. reconcile: prediction at (row,col,'a')      │                                   │
 │         │    matches mirror at (row,col,'a') → CONFIRM,  │                                   │
 │         │    drop overlay entry                          │                                   │
 │         │                                                │                                   │
 │         │ 10. FRAME_ACK {pane, seq=K} ──────────────────►│                                   │
 │         │                                                │                                   │

  Contradiction path (e.g. user is in vim normal mode):
 │         │ 7'. PANE_OUTPUT bytes do NOT place 'a' at      │                                   │
 │         │     cursor (cursor moves instead, no insert)   │                                   │
 │         │ 8'. reconcile: prediction CONTRADICTED         │                                   │
 │         │     drop overlay entry; redraw cell from       │                                   │
 │         │     mirror                                     │                                   │

  Timeout path (server silent, no confirming output ever arrives):
 │         │ -. epoch N has lived > predict_ttl_ms without  │                                   │
 │         │    a confirming PANE_OUTPUT → KILL prediction, │                                   │
 │         │    redraw cell from mirror                     │                                   │
```

Three properties hold:

1. **The mirror is authoritative.** Predictions are an overlay drawn on
   top at render time; they never mutate the mirror. A bug in the
   predictor cannot corrupt the user's visible grid past the next
   redraw.
2. **Reconciliation runs on `PANE_OUTPUT` arrival**, not on
   `FRAME_ACK`. The ack is a server-side flow-control signal (SPEC
   §12.2); it carries no rendering meaning. This means predictive echo
   continues to function correctly even if a future minor version
   reshapes the ack protocol.
3. **Epochs + TTL are the safety net.** If the server is silent
   (network dead, app not echoing, app crashed), predictions don't
   accumulate forever; they age out and the displayed cell falls back
   to the mirror's truth.
