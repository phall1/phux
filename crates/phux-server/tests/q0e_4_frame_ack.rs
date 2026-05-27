//! `phux-q0e.4` — `FRAME_ACK` handler closes the state-sync loop.
//!
//! Per ADR-0018 (lazy state synchronization) and its 2026-05-26 Addendum,
//! `FRAME_ACK` is the only thing allowed to call `mark_synced` on a
//! per-consumer `SnapshotSynthesizer`. The tick driver (phux-q0e.3) emits
//! deltas against the cached reference; the ack is what tells the actor
//! "this consumer's mirror is caught up — next tick re-diffs against the
//! just-acked reference."
//!
//! The cycle:
//!   1. tick emits `TerminalOutput { seq = N }` to the consumer,
//!   2. consumer applies the bytes, sends `FRAME_ACK { seq = N }`,
//!   3. `on_frame_ack` calls `synthesizer.mark_synced` (q0e.1 primitive),
//!      clearing the per-consumer dirty cache,
//!   4. next tick re-diffs against the just-acked reference.
//!
//! These integration tests exercise the channel-shaped path
//! (`TerminalHandle::consumer_ack`) end-to-end across the actor's
//! `select!` loop. The direct `on_frame_ack(..)` unit tests live in the
//! `terminal_actor` module's `#[cfg(test)]` block; this file focuses on
//! cross-channel routing.
//!
//! libghostty types are `!Send + !Sync`; the actor lives on a `LocalSet`
//! thread (ADR-0014). All tokio tests use `flavor = "current_thread"`
//! and `LocalSet::run_until`.
//!
//! # Known degradation (phux-l0t)
//!
//! `mark_synced` calls `Snapshot::set_dirty`, which under the libghostty
//! FFI bug in `phux-l0t` poisons subsequent `dirty()` reads with
//! `Error::InvalidValue`. The defensive `.unwrap_or(Dirty::Full)` in
//! `synthesize_incremental` (phux-q0e.1) means post-mark_synced ticks
//! degrade to "always non-empty body, effectively a full repaint." That
//! is the contract these tests pin: after an ack, the next tick still
//! emits a non-empty `TerminalOutput`. Correctness over byte-minimality
//! per ADR-0018 design until upstream libghostty fixes the FFI.

#![allow(clippy::expect_used, reason = "tests")]
#![allow(clippy::unwrap_used, reason = "tests")]
#![allow(clippy::panic, reason = "tests")]

use std::time::Duration;

use phux_protocol::ClientId;
use phux_protocol::wire::frame::FrameKind;
use phux_server::state::Outbound;
use phux_server::terminal_actor::{
    ConsumerAckRequest, ConsumerAttachRequest, ConsumerDetachRequest, DEFAULT_TICK_INTERVAL,
    TerminalActor,
};
use tokio::sync::{mpsc, oneshot};
use tokio::task::LocalSet;

/// Wire-terminal id stamped on every `TerminalOutput` frame in this
/// suite. Arbitrary; chosen to be non-trivial.
const WIRE_TID: u32 = 7;

/// Drain whatever is currently sitting on `rx` without blocking.
/// Returns the items in receive order.
fn drain<T>(rx: &mut mpsc::Receiver<T>) -> Vec<T> {
    let mut out = Vec::new();
    while let Ok(item) = rx.try_recv() {
        out.push(item);
    }
    out
}

/// Walk an `Outbound` slice and extract the `TerminalOutput` frames'
/// `(terminal_id, seq, bytes_len)`. We don't compare bodies here — the
/// q0e.1/q0e.3 tests pin synthesizer output shape; this suite cares
/// about routing + ordering + non-empty under the l0t degradation.
fn terminal_outputs(items: &[Outbound]) -> Vec<(u32, u64, usize)> {
    items
        .iter()
        .filter_map(|item| match item {
            Outbound::Frame(FrameKind::TerminalOutput {
                terminal_id,
                seq,
                bytes,
            }) => Some((*terminal_id, *seq, bytes.len())),
            _ => None,
        })
        .collect()
}

/// Advance virtual time by `n * DEFAULT_TICK_INTERVAL` plus a small
/// scheduler-yield slack so the actor's `select!` arm gets observed.
async fn advance_ticks(n: u32) {
    for _ in 0..n {
        tokio::time::advance(DEFAULT_TICK_INTERVAL).await;
        for _ in 0..8 {
            tokio::task::yield_now().await;
        }
    }
}

/// Yield several times so the actor task drains a freshly-sent channel
/// message before the test inspects observable state. The select! arm
/// runs the moment the task is polled; one yield is usually enough, a
/// small loop is bulletproof.
async fn settle() {
    for _ in 0..8 {
        tokio::task::yield_now().await;
    }
}

/// End-to-end ack roundtrip across the channel boundary. After the
/// `ConsumerAckRequest` lands and the actor processes it, the next tick
/// emits a non-empty `TerminalOutput` whose `seq` advances past the
/// pre-ack stream — proving the actor accepted the ack and re-walked
/// the per-consumer state.
#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn ack_round_trip_emits_post_ack_tick() {
    let local = LocalSet::new();
    local
        .run_until(async {
            let bundle = TerminalActor::new_with_seed(20, 5, b"hello").expect("new_with_seed");
            let handle = bundle.handle.clone();
            let token = bundle.token.clone();
            let join = tokio::task::spawn_local(bundle.actor.run());

            let client = ClientId(1);
            let (out_tx, mut out_rx) = mpsc::channel::<Outbound>(32);
            let (reply_tx, reply_rx) = oneshot::channel();
            handle
                .consumer_attach
                .send(ConsumerAttachRequest {
                    client_id: client,
                    outbound: out_tx,
                    wire_terminal_id: WIRE_TID,
                    reply: reply_tx,
                })
                .await
                .expect("send attach");
            reply_rx
                .await
                .expect("attach reply")
                .expect("attach succeeded");

            // First tick emits seq=1.
            advance_ticks(1).await;
            let pre_ack = terminal_outputs(&drain(&mut out_rx));
            assert!(
                !pre_ack.is_empty(),
                "first tick must produce a TerminalOutput",
            );
            let first_seq = pre_ack[0].1;
            assert_eq!(first_seq, 1, "first emission seq=1");

            // Send a FRAME_ACK across the channel boundary.
            handle
                .consumer_ack
                .send(ConsumerAckRequest {
                    client_id: client,
                    seq: first_seq,
                })
                .await
                .expect("send ack");
            settle().await;

            // Next tick must still emit (per the phux-l0t FFI bug, the
            // post-`mark_synced` `dirty()` degrades to `Full`, so the
            // body is non-empty even with no PTY input between ticks).
            advance_ticks(1).await;
            let post_ack = terminal_outputs(&drain(&mut out_rx));
            assert!(
                !post_ack.is_empty(),
                "post-ack tick must still emit; got 0 frames",
            );
            assert!(
                post_ack[0].1 > first_seq,
                "post-ack seq must advance past pre-ack ({} > {})",
                post_ack[0].1,
                first_seq,
            );
            assert_eq!(
                post_ack[0].0, WIRE_TID,
                "post-ack frame stamped with consumer's wire id",
            );

            token.cancel();
            tokio::time::timeout(Duration::from_secs(1), join)
                .await
                .expect("actor did not exit within 1s")
                .expect("actor task panicked");
        })
        .await;
}

/// Older/duplicate ack across the channel boundary is a silent no-op.
/// We can't read `last_acked_seq` from outside the crate, so the
/// assertion is operational: the actor stays healthy and the tick path
/// still emits afterwards. Direct field-level assertions live in
/// `terminal_actor::tests::on_frame_ack_older_or_duplicate_is_dropped`.
#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn older_and_duplicate_acks_do_not_crash_the_actor() {
    let local = LocalSet::new();
    local
        .run_until(async {
            let bundle = TerminalActor::new_with_seed(20, 5, b"hi").expect("new_with_seed");
            let handle = bundle.handle.clone();
            let token = bundle.token.clone();
            let join = tokio::task::spawn_local(bundle.actor.run());

            let client = ClientId(1);
            let (out_tx, mut out_rx) = mpsc::channel::<Outbound>(32);
            let (reply_tx, reply_rx) = oneshot::channel();
            handle
                .consumer_attach
                .send(ConsumerAttachRequest {
                    client_id: client,
                    outbound: out_tx,
                    wire_terminal_id: WIRE_TID,
                    reply: reply_tx,
                })
                .await
                .expect("send attach");
            reply_rx
                .await
                .expect("attach reply")
                .expect("attach succeeded");

            // Forward ack (seq=5) followed by older (seq=3) and
            // duplicate (seq=5). The actor must handle each cleanly.
            for seq in [5u64, 3, 5, 4] {
                handle
                    .consumer_ack
                    .send(ConsumerAckRequest {
                        client_id: client,
                        seq,
                    })
                    .await
                    .expect("send ack");
            }
            settle().await;

            // Tick must still produce output (actor not poisoned).
            advance_ticks(1).await;
            let items = terminal_outputs(&drain(&mut out_rx));
            assert!(
                !items.is_empty(),
                "actor must still emit after older/duplicate acks",
            );

            token.cancel();
            tokio::time::timeout(Duration::from_secs(1), join)
                .await
                .expect("actor did not exit within 1s")
                .expect("actor task panicked");
        })
        .await;
}

/// Sending a `ConsumerAckRequest` for a `ClientId` that was never
/// attached is a silent no-op: the actor must stay alive and the tick
/// path for *other* consumers must still work.
#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn ack_for_unregistered_consumer_is_silent_noop() {
    let local = LocalSet::new();
    local
        .run_until(async {
            let bundle = TerminalActor::new_with_seed(20, 5, b"hi").expect("new_with_seed");
            let handle = bundle.handle.clone();
            let token = bundle.token.clone();
            let join = tokio::task::spawn_local(bundle.actor.run());

            let real = ClientId(1);
            let stranger = ClientId(999);
            let (out_tx, mut out_rx) = mpsc::channel::<Outbound>(32);
            let (reply_tx, reply_rx) = oneshot::channel();
            handle
                .consumer_attach
                .send(ConsumerAttachRequest {
                    client_id: real,
                    outbound: out_tx,
                    wire_terminal_id: WIRE_TID,
                    reply: reply_tx,
                })
                .await
                .expect("send attach");
            reply_rx
                .await
                .expect("attach reply")
                .expect("attach succeeded");

            // Stray ack for a client_id that was never attached.
            handle
                .consumer_ack
                .send(ConsumerAckRequest {
                    client_id: stranger,
                    seq: 42,
                })
                .await
                .expect("send ack");
            settle().await;

            // The real consumer's tick path must still work.
            advance_ticks(1).await;
            let items = terminal_outputs(&drain(&mut out_rx));
            assert!(
                !items.is_empty(),
                "real consumer must still receive a tick after stray ack",
            );

            token.cancel();
            tokio::time::timeout(Duration::from_secs(1), join)
                .await
                .expect("actor did not exit within 1s")
                .expect("actor task panicked");
        })
        .await;
}

/// Detach then ack: silent no-op. The actor must not crash and the
/// per-consumer entry must NOT be resurrected by the late ack. We
/// can't peek at the map from outside the crate; the indirect proof
/// is that subsequent ticks emit no frames for the detached id (no
/// outbound channel to even reach, since detach drops the actor-side
/// sender; this test pins that detach + ack stays clean).
#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn ack_after_detach_is_silent_noop() {
    let local = LocalSet::new();
    local
        .run_until(async {
            let bundle = TerminalActor::new_with_seed(20, 5, b"hi").expect("new_with_seed");
            let handle = bundle.handle.clone();
            let token = bundle.token.clone();
            let join = tokio::task::spawn_local(bundle.actor.run());

            let client = ClientId(1);
            let (out_tx, mut out_rx) = mpsc::channel::<Outbound>(32);
            let (reply_tx, reply_rx) = oneshot::channel();
            handle
                .consumer_attach
                .send(ConsumerAttachRequest {
                    client_id: client,
                    outbound: out_tx,
                    wire_terminal_id: WIRE_TID,
                    reply: reply_tx,
                })
                .await
                .expect("send attach");
            reply_rx
                .await
                .expect("attach reply")
                .expect("attach succeeded");

            // Detach.
            let (det_tx, det_rx) = oneshot::channel();
            handle
                .consumer_detach
                .send(ConsumerDetachRequest {
                    client_id: client,
                    reply: det_tx,
                })
                .await
                .expect("send detach");
            det_rx.await.expect("detach reply");

            // Now ack for the just-detached id. Must be a no-op.
            handle
                .consumer_ack
                .send(ConsumerAckRequest {
                    client_id: client,
                    seq: 5,
                })
                .await
                .expect("send ack");
            settle().await;

            // No frames should arrive (entry is gone, tick has no
            // consumer to walk for this id).
            advance_ticks(3).await;
            let items = terminal_outputs(&drain(&mut out_rx));
            assert!(
                items.is_empty(),
                "detached consumer must receive zero frames; got {} ({:?})",
                items.len(),
                items,
            );

            token.cancel();
            tokio::time::timeout(Duration::from_secs(1), join)
                .await
                .expect("actor did not exit within 1s")
                .expect("actor task panicked");
        })
        .await;
}
