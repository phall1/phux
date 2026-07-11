//! Dedicated input lane (phux-51n6.2, ADR-0044).
//!
//! The server runs one current-thread tokio runtime on a
//! [`tokio::task::LocalSet`] (ADR-0003, ADR-0014): every per-client task and
//! every `!Send` [`libghostty_vt::Terminal`]-owning
//! `TerminalActor` time-share a single
//! core. A large PTY-output broadcast tick and a keystroke therefore compete
//! for the same thread. Commit 284fcbd bounded that contention *inside* the
//! actor's `select!` (input arm biased ahead of output, output coalesce capped
//! and yielding), but the **routing** stage — resolving the wire pane id,
//! walking the subscription set, checking the input lease (ADR-0033), and
//! `try_send`ing onto the pane actor's mailbox — still ran on the main thread,
//! behind whatever output work the runtime was mid-poll on.
//!
//! This module lifts that routing stage onto its own OS thread. Because
//! `SharedState` is `Arc<Mutex<ServerState>>` (Send + Sync) and the pane
//! actor's input mailbox sender is a `Send` `mpsc::Sender<TerminalInput>`,
//! routing needs nothing `!Send`: it never touches the `Terminal`. A keystroke
//! can thus be gated and delivered into the actor's mailbox on a second core
//! *in parallel with* an output-broadcast tick draining on the main thread,
//! rather than waiting for the runtime to yield.
//!
//! What still lives on the actor thread: the **encode** stage
//! (`TerminalActor::encode_input`). Encoding reads the
//! pane's live DEC-mode state (cursor-key application mode, keypad mode, mouse
//! tracking/format, DEC 1004 focus reporting, DEC 2004 bracketed paste) from
//! the `!Send` `Terminal`, so it cannot cross the thread boundary without a
//! `Send` mode snapshot. Moving encode onto the lane is deferred to
//! phux-51n6.6; see ADR-0044 "Deferred".
//!
//! ## Ordering and lease correctness
//!
//! * **Per-client input order** is preserved end to end: a client's read loop
//!   enqueues its `INPUT_*` frames onto the lane channel in wire order, the
//!   tokio mpsc channel is FIFO, the single lane thread drains it FIFO, and the
//!   pane mailbox it forwards into is FIFO. `K1` before `K2` stays `K1` before
//!   `K2` at the PTY.
//! * **Lease exclusion** (ADR-0033 "take the wheel") is unchanged: the lane
//!   calls the *same* `handle_terminal_input`,
//!   which re-evaluates the subscription and lease gates atomically under the
//!   state `Mutex` at delivery time. Whoever holds the wheel *when the lane
//!   routes* is honored.
//! * The one behavioral shift: a client's own `INPUT_*` frame now routes on the
//!   lane while its `ACQUIRE_INPUT` / `RELEASE_INPUT` (an L2 `COMMAND`) still
//!   runs inline on the main thread, so their relative timing is no longer
//!   strictly wire-ordered. The lease gate re-check makes every outcome safe
//!   within the fire-and-forget input contract (SPEC §12.2): a key that races
//!   just past its sender's own `RELEASE_INPUT` is delivered if the wheel is
//!   free and dropped if another client has since grabbed it — either is a
//!   legal fire-and-forget result. Cross-client exclusion never weakens.
//!
//! Only **local** pane ids are routed through the lane. Satellite-tagged ids
//! (federation-hub relay, phux-v45.4) stay on the main thread: their delivery
//! is a hub-link forward, not a mailbox `try_send`, and keeping it inline
//! avoids widening the lane's contract to the relay registry.

use tokio::sync::mpsc;

use super::handle_terminal_input;
use crate::state::{ClientId, SharedState, TerminalInput};

/// Bound on the lane's inbound queue. Input events are tiny and low-rate, so a
/// generous cap absorbs a paste-expanded burst without unbounded growth. On
/// overflow the lane drops with a `warn!`, matching the fire-and-forget
/// backpressure already applied at the pane mailbox (SPEC §9).
const INPUT_LANE_CAPACITY: usize = 1024;

/// A local `INPUT_*` event lifted off the main runtime for lease/subscription
/// gating and delivery to the owning pane actor. Every field is `Send`; the
/// `!Send` `Terminal` is never referenced here.
#[derive(Debug)]
pub(crate) struct RoutedInput {
    /// Originating client, for the subscription and lease gates.
    pub(crate) client_id: ClientId,
    /// Wire pane id. Always local (`is_local()`); satellite ids never reach
    /// the lane.
    pub(crate) terminal_id: phux_protocol::ids::TerminalId,
    /// The decoded, not-yet-encoded input event.
    pub(crate) input: TerminalInput,
    /// Frame label for logs (`"INPUT_KEY"`, ...), matching the inline path.
    pub(crate) frame_label: &'static str,
}

/// Cloneable handle a client task uses to hand input to the lane. Cloning is
/// cheap (`mpsc::Sender` clone); every clone keeps the lane thread alive.
#[derive(Clone, Debug)]
pub(crate) struct InputLaneHandle {
    tx: mpsc::Sender<RoutedInput>,
}

impl InputLaneHandle {
    /// Enqueue an input event for off-thread routing. Non-blocking: a full
    /// queue drops the event with a `warn!` (fire-and-forget, SPEC §9), a
    /// closed lane (thread gone during shutdown) drops at `debug!`.
    pub(crate) fn route(&self, routed: RoutedInput) {
        match self.tx.try_send(routed) {
            Ok(()) => {}
            Err(mpsc::error::TrySendError::Full(routed)) => {
                tracing::warn!(
                    client_id = ?routed.client_id,
                    terminal_id = ?routed.terminal_id,
                    frame_label = routed.frame_label,
                    "input lane queue full; dropping (fire-and-forget per SPEC §9)",
                );
            }
            Err(mpsc::error::TrySendError::Closed(routed)) => {
                tracing::debug!(
                    client_id = ?routed.client_id,
                    frame_label = routed.frame_label,
                    "input lane closed; dropping input",
                );
            }
        }
    }
}

/// Owns the lane's OS thread. Held for the server's lifetime; on drop it closes
/// the channel (the last non-clone sender) and joins the thread so it releases
/// its `SharedState` clone. Hand out routing capability via [`Self::handle`].
#[derive(Debug)]
pub(crate) struct InputLane {
    handle: InputLaneHandle,
    join: Option<std::thread::JoinHandle<()>>,
}

impl InputLane {
    /// A cloneable routing handle for client tasks.
    pub(crate) fn handle(&self) -> InputLaneHandle {
        self.handle.clone()
    }
}

impl Drop for InputLane {
    fn drop(&mut self) {
        // Replace our retained sender with a fresh closed one so the lane sees
        // the channel close once every client-held clone is also dropped, then
        // join. `run_async` drops the `LocalSet` before dropping the lane, which
        // *destroys* (not merely aborts) every per-client task future and so
        // releases every `InputLaneHandle` clone; only then does this join
        // observe the channel close. Dropping the lane while an aborted-but-not-
        // yet-dropped client future still holds a clone would hang the join.
        let (dead_tx, _dead_rx) = mpsc::channel(1);
        self.handle.tx = dead_tx;
        if let Some(join) = self.join.take()
            && let Err(err) = join.join()
        {
            tracing::warn!(?err, "input lane thread panicked on shutdown");
        }
    }
}

/// Spawn the dedicated input-lane thread and return its owner.
///
/// The thread blocks on the channel and, for each [`RoutedInput`], runs the
/// same [`handle_terminal_input`] the inline path uses — off the main runtime,
/// so routing never waits behind an output-broadcast tick. The thread exits
/// when the channel closes (all senders dropped), releasing its `SharedState`
/// clone.
///
/// # Errors
///
/// Returns the OS error if the lane thread cannot be spawned (a fatal
/// server-startup condition; the caller propagates it).
pub(crate) fn spawn_input_lane(state: SharedState) -> std::io::Result<InputLane> {
    let (tx, mut rx) = mpsc::channel::<RoutedInput>(INPUT_LANE_CAPACITY);
    let join = std::thread::Builder::new()
        .name("phux-input-lane".to_owned())
        .spawn(move || {
            // `blocking_recv` parks the thread with no tokio runtime on it —
            // the lane does no async work. `handle_terminal_input` is a
            // synchronous function: it takes the `std::sync::Mutex` state lock,
            // gates, and `try_send`s onto the (cross-thread) pane mailbox,
            // never awaiting. The mailbox waker fires the actor task back on
            // the main runtime.
            while let Some(routed) = rx.blocking_recv() {
                handle_terminal_input(
                    &state,
                    routed.client_id,
                    &routed.terminal_id,
                    routed.input,
                    routed.frame_label,
                );
            }
            tracing::debug!("input lane thread exiting (channel closed)");
        })?;
    Ok(InputLane {
        handle: InputLaneHandle { tx },
        join: Some(join),
    })
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use phux_protocol::input::paste::{PasteEvent, PasteTrust};
    use tokio_util::sync::CancellationToken;

    use super::*;
    use crate::terminal_actor::{MAX_PTY_COALESCE_BYTES, PaneOutput, PtyEvent, TerminalActor};

    /// A trusted paste of `bytes`. With DEC 2004 bracketed-paste off (a fresh
    /// `Terminal`'s default) the pane's paste encoder emits exactly `bytes` on
    /// the PTY writer channel, so the routed input is byte-identifiable.
    fn paste(bytes: &[u8]) -> TerminalInput {
        TerminalInput::Paste(PasteEvent {
            trust: PasteTrust::Trusted,
            data: bytes.to_vec(),
        })
    }

    /// A registered pane actor plus two subscribed clients, wired into a fresh
    /// [`SharedState`] and spawned on the caller's `LocalSet`. Input routed
    /// through the lane for `wire` and gated on `pane`'s subscription/lease is
    /// encoded by the actor and observable on `writer_rx`.
    struct Fixture {
        state: SharedState,
        wire: phux_protocol::ids::TerminalId,
        pane: phux_core::ids::TerminalId,
        client_a: ClientId,
        client_b: ClientId,
        pty_evt_tx: mpsc::UnboundedSender<PtyEvent>,
        writer_rx: mpsc::UnboundedReceiver<Vec<u8>>,
        output_rx: tokio::sync::broadcast::Receiver<PaneOutput>,
        token: CancellationToken,
    }

    /// Build the fixture and spawn the actor. Must run inside a `LocalSet`
    /// (the pane actor owns a `!Send` `Terminal`, per ADR-0014).
    fn spawn_fixture() -> Fixture {
        let bundle = TerminalActor::new(80, 24).expect("actor");
        let handle = bundle.handle.clone();
        let token = bundle.token.clone();
        let mut actor = bundle.actor;
        let (pty_evt_tx, writer_rx) = actor.install_test_pty_channels();
        let output_rx = handle.output.subscribe();

        let state = SharedState::new();
        let (wire, pane, client_a, client_b) = state.with_mut(|s| {
            let (_sid, _wid, pane) = s.seed_session("s");
            let wire = s.register_terminal_handle(pane, handle.clone(), token.clone());
            let a = s.new_client_id();
            let b = s.new_client_id();
            let (tx_a, _rx_a) = mpsc::channel(16);
            let (tx_b, _rx_b) = mpsc::channel(16);
            s.attach_default_caps(a, "s", tx_a).expect("attach a");
            s.attach_default_caps(b, "s", tx_b).expect("attach b");
            (wire, pane, a, b)
        });

        tokio::task::spawn_local(actor.run());

        Fixture {
            state,
            wire,
            pane,
            client_a,
            client_b,
            pty_evt_tx,
            writer_rx,
            output_rx,
            token,
        }
    }

    /// The lane delivers routed input to the pane's PTY writer off the main
    /// runtime, preserving per-client FIFO order: three distinct pastes routed
    /// in order arrive on the writer channel in the same order.
    #[tokio::test(flavor = "current_thread")]
    async fn lane_routes_input_to_pane_in_order() {
        let local = tokio::task::LocalSet::new();
        local
            .run_until(async {
                let mut fx = spawn_fixture();
                let lane = spawn_input_lane(fx.state.clone()).expect("spawn lane");

                for byte in [&b"a"[..], b"b", b"c"] {
                    lane.handle().route(RoutedInput {
                        client_id: fx.client_a,
                        terminal_id: fx.wire.clone(),
                        input: paste(byte),
                        frame_label: "INPUT_PASTE",
                    });
                }

                for expected in [&b"a"[..], b"b", b"c"] {
                    let got = tokio::time::timeout(Duration::from_secs(2), fx.writer_rx.recv())
                        .await
                        .expect("lane must deliver routed input to the PTY writer")
                        .expect("writer channel open");
                    assert_eq!(got, expected, "routed input must arrive in FIFO order");
                }

                fx.token.cancel();
            })
            .await;
    }

    /// Lease exclusion (ADR-0033) holds across the lane: with client B holding
    /// the wheel, client A's routed input is dropped by the lease gate on the
    /// lane thread while B's is delivered — the first (and only) byte the PTY
    /// writer sees is B's.
    #[tokio::test(flavor = "current_thread")]
    async fn lane_honors_input_lease() {
        let local = tokio::task::LocalSet::new();
        local
            .run_until(async {
                let mut fx = spawn_fixture();
                // B takes the wheel.
                fx.state
                    .with_mut(|s| s.set_input_lease(fx.pane, fx.client_b));

                let lane = spawn_input_lane(fx.state.clone()).expect("spawn lane");
                // A is blocked; its input must be dropped, not encoded.
                lane.handle().route(RoutedInput {
                    client_id: fx.client_a,
                    terminal_id: fx.wire.clone(),
                    input: paste(b"a"),
                    frame_label: "INPUT_PASTE",
                });
                // B holds the wheel; its input must be delivered.
                lane.handle().route(RoutedInput {
                    client_id: fx.client_b,
                    terminal_id: fx.wire.clone(),
                    input: paste(b"B"),
                    frame_label: "INPUT_PASTE",
                });

                let got = tokio::time::timeout(Duration::from_secs(2), fx.writer_rx.recv())
                    .await
                    .expect("B's leased input must reach the PTY writer")
                    .expect("writer channel open");
                assert_eq!(
                    got, b"B",
                    "the lease holder's input is delivered; the blocked client's is dropped",
                );
                // A's input must never arrive: nothing else is queued.
                let extra =
                    tokio::time::timeout(Duration::from_millis(200), fx.writer_rx.recv()).await;
                assert!(
                    extra.is_err(),
                    "blocked client's input must not reach the PTY writer",
                );

                fx.token.cancel();
            })
            .await;
    }

    /// Lane-routed input is not starved by a large pending PTY-output burst.
    /// Mirrors `input_interleaves_with_a_large_pty_output_burst` but drives the
    /// keystroke through the dedicated lane (a separate thread) rather than the
    /// actor's mailbox directly: the lane populates the mailbox off the main
    /// runtime, and the actor's fair `select!` (284fcbd) services it while the
    /// ~800KB burst is still draining, so the cumulative broadcast bytes seen
    /// when the input lands are far below the full burst.
    #[tokio::test(flavor = "current_thread")]
    async fn lane_routed_input_interleaves_with_a_large_pty_output_burst() {
        const CHUNK_LEN: usize = 4096;
        const CHUNK_COUNT: usize = 200;
        const BURST_BYTES: usize = CHUNK_LEN * CHUNK_COUNT;

        let local = tokio::task::LocalSet::new();
        local
            .run_until(async {
                let mut fx = spawn_fixture();
                let lane = spawn_input_lane(fx.state.clone()).expect("spawn lane");

                // Pre-queue a burst far larger than MAX_PTY_COALESCE_BYTES so it
                // spans many capped vt_writes.
                let chunk = vec![b'x'; CHUNK_LEN];
                for _ in 0..CHUNK_COUNT {
                    fx.pty_evt_tx
                        .send(PtyEvent::Bytes(chunk.clone()))
                        .expect("queue burst");
                }
                // Route ONE keystroke through the lane.
                lane.handle().route(RoutedInput {
                    client_id: fx.client_a,
                    terminal_id: fx.wire.clone(),
                    input: paste(b"k"),
                    frame_label: "INPUT_PASTE",
                });

                let got = tokio::time::timeout(Duration::from_secs(2), fx.writer_rx.recv())
                    .await
                    .expect("lane-routed input must be serviced mid-burst, not after it")
                    .expect("writer channel open");
                assert_eq!(
                    got, b"k",
                    "the lane-routed keystroke should reach the PTY writer while the burst drains",
                );

                // Cumulative broadcast bytes so far must be below the full
                // burst — the input interleaved rather than waiting it out.
                // Account Lagged-skipped frames (bounded broadcast channel)
                // toward the total so the gate cannot under-report.
                let mut emitted: usize = 0;
                loop {
                    match fx.output_rx.try_recv() {
                        Ok(PaneOutput::Live(bytes) | PaneOutput::Resync { bytes, .. }) => {
                            emitted += bytes.len();
                        }
                        Err(tokio::sync::broadcast::error::TryRecvError::Lagged(n)) => {
                            // Each lagged frame is one coalesced payload of at
                            // most MAX_PTY_COALESCE_BYTES; bound the skipped
                            // volume by that cap so the assertion stays
                            // conservative and never under-reports.
                            let skipped = usize::try_from(n).unwrap_or(usize::MAX);
                            emitted += skipped.saturating_mul(MAX_PTY_COALESCE_BYTES);
                        }
                        Err(_) => break,
                    }
                }
                fx.token.cancel();
                assert!(
                    emitted < BURST_BYTES,
                    "lane-routed input must land mid-burst: cumulative output {emitted} \
                     should be below the full burst {BURST_BYTES}",
                );
            })
            .await;
    }
}
