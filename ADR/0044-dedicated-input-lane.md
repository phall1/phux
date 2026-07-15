---
audience: contributors
stability: stable
last-reviewed: 2026-07-11
---

# 0044 — Dedicated input lane

**TL;DR.** Input routing (lease/subscription gating and pane-mailbox delivery) moves off the single current-thread runtime onto its own OS thread, so a keystroke is gated and delivered in parallel with an output-broadcast tick instead of waiting for the runtime to yield. This extends ADR-0003/ADR-0014: the `!Send` `Terminal` actor stays put; only the `Send` routing stage crosses the thread boundary.

Status: Accepted
Date: 2026-07-11

## Context

ADR-0003 gives the server one current-thread tokio runtime; ADR-0014 puts
every per-client task and every `!Send` `Terminal`-owning pane actor on one
`LocalSet`. They time-share a single core. A large PTY-output broadcast tick
and an inbound keystroke therefore contend for the same thread.

Commit 284fcbd bounded that contention *inside* the pane actor's `select!`
(input arm biased ahead of output; output coalesce capped at 48KB and
yielding). But the **routing** stage that runs *before* the mailbox — resolve
the wire pane id, walk the subscription set, check the input lease (ADR-0033),
`try_send` onto the actor's mailbox — still executed on the main thread, behind
whatever output work the runtime was mid-poll on. The keystroke's route waited
for a yield point.

The input encode stage (wire event → PTY bytes) reads the pane's live DEC-mode
state from the `!Send` `Terminal`, so it cannot leave the actor thread. But
routing touches none of that.

## Decision

Spawn one dedicated OS thread — the input lane — at server start. Each per-client
read loop, on decoding a **local** `INPUT_*` frame or `ROUTE_INPUT` command,
hands a `Send` `RoutedInput` (client id, wire pane id, decoded event, authority
policy, and for commands a reply sender) to the lane over a bounded channel
instead of routing inline. The lane blocks on that channel and runs the same
existing subscription/lease or headless lease handler off the main runtime.
The pane actor, its `Terminal`, and encode are unchanged and unmoved.

This is sound because the routing path is fully `Send`: `SharedState` is
`Arc<Mutex<ServerState>>`, and the pane mailbox is a `Send`
`mpsc::Sender<TerminalInput>`. Making it compile required one wart fix — the
dead `UnsubscribeFromEventsRequest` carried a `*const Sender<Outbound>` (raw
pointer, `!Send`) that poisoned the whole state tree; it is now a `usize`
address with identical identity semantics.

Satellite-tagged pane ids stay on the main thread (their delivery is a hub-link
relay, not a mailbox send). Tests that drive `handle_client` directly pass
`None` and route inline — identical behavior, on-thread. Local `ROUTE_INPUT`
waits for the lane's correlated result before its command acknowledgement is
emitted, preserving its existing error and lease-check semantics.

## Why

- **Root, not per-`select!`.** 284fcbd made the actor fair once input is in the
  mailbox; this makes the mailbox fill off the contended thread. The two
  compose: the lane populates the mailbox in parallel with output, the actor's
  biased `select!` then services it within one 48KB parse.
- **The `!Send` constraint is respected, not fought.** The `Terminal` never
  moves; only the provably-`Send` routing crosses the boundary.
- **Lease and ordering correctness are preserved by construction** (below), so
  no new correctness surface is opened.

## Tradeoffs

- **Route/encode split across two threads.** Encode still runs on the actor
  thread; the lane only routes. The win is the route half plus the foundation
  for moving encode later (see Deferred).
- **One extra OS thread** for the server's lifetime. It parks on the channel
  (no busy-wait) and joins on shutdown.
- **A same-client input-vs-lease timing shift.** A client's `INPUT_*` now
  routes on the lane while its `ACQUIRE_INPUT`/`RELEASE_INPUT` (an L2 `COMMAND`)
  still runs inline, so their relative order is no longer strictly wire-ordered.
  The lease gate is re-evaluated atomically under the state `Mutex` at delivery,
  so every outcome stays legal under the fire-and-forget input contract
  (SPEC §12.2): a key racing just past its sender's own `RELEASE_INPUT` is
  delivered if the wheel is free and dropped if another client grabbed it.
  Cross-client exclusion never weakens.
- **Per-client input order is unchanged**: the read loop enqueues `INPUT_*` and
  local `ROUTE_INPUT` in wire order, the channel and lane are FIFO, and the
  mailbox is FIFO. Both input surfaces share this one queue.

## Deferred

Moving **encode** onto the lane (phux-51n6.6) needs a `Send` snapshot of the
pane's encoder-relevant DEC modes (cursor-key/keypad application, mouse
tracking/format, DEC 1004, DEC 2004), published by the actor after each output
batch and consumed by the lane's encoders via their explicit setters instead of
`set_options_from_terminal`. That is a larger change with its own correctness
surface (a missed mode silently mis-encodes input) and wants an equivalence
test against the live-terminal encode. It is out of scope here.

## Alternatives

- **Make the whole runtime multi-threaded.** Rejected: the `Terminal` is
  `!Send` (ADR-0003/0014); a work-stealing pool cannot host the actor.
- **A second full tokio runtime.** Unnecessary: routing is synchronous
  (non-blocking `try_send`, no await), so a plain thread with `blocking_recv`
  is simpler and has no runtime-in-runtime hazards.
- **Only strengthen the actor `select!` priority.** Already done (284fcbd); it
  cannot address contention *before* the mailbox, which is on the main thread.
- **Mirror the lease/subscription state into a separate `Send` structure the
  lane owns.** Rejected: two sources of truth for the lease is exactly the
  correctness surface this ADR avoids by reusing `handle_terminal_input` under
  the one `Mutex`.
