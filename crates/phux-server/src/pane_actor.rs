//! Per-pane actor that owns a `libghostty_vt::Terminal` and serves snapshot
//! requests, input writes, and PTY output fanout (`phux-byc.8`).
//!
//! See ADR-0014 for the placement rationale. In short: `Terminal` is
//! `!Send + !Sync`, so it can't live behind a `tokio::spawn` future. It
//! lives inside a `spawn_local` task that runs on the server's existing
//! current-thread runtime via a `LocalSet`. All cross-task coordination
//! flows through channel handles ([`PaneHandle`]) that are `Send` —
//! the actor itself never crosses a thread boundary.
//!
//! # Surface for `phux-byc.8`
//!
//! Only the snapshot-request branch of the actor is load-bearing for this
//! ticket. The PTY read path (`portable-pty`) and the input write path
//! land in `phux-byc.5` once the PTY pump exists; the channels are
//! created and stored on [`PaneHandle`] so the surrounding plumbing
//! (ATTACH handler, subscribers) can be wired through end-to-end now.
//!
//! # Why `bytes::Bytes` for the output broadcast
//!
//! `tokio::sync::broadcast::Sender` requires `Clone` payloads (every
//! subscriber receives a copy of the same value). `bytes::Bytes` is the
//! standard cheap-clone byte buffer in the tokio ecosystem; `Vec<u8>`
//! would also work but at the cost of a full clone per subscriber.

use std::cell::RefCell;

use bytes::Bytes;
use libghostty_vt::{Terminal, TerminalOptions};
use tokio::sync::{broadcast, mpsc, oneshot};
use tracing::{debug, warn};

use crate::grid::{SnapshotBytes, SnapshotSynthesizer};
use crate::state::PaneInput;

/// Default depth of the per-pane input mailbox.
///
/// Small on purpose: keystrokes are tiny and the server drains them in
/// the same event loop. A backed-up channel here would mean the actor
/// has stalled, which is its own bug to investigate.
pub const DEFAULT_INPUT_MAILBOX: usize = 64;

/// Default capacity of the per-pane output broadcast channel.
///
/// Bytes fan out to subscribed clients. Sized for "burst tolerance" —
/// a busy pane can emit a few dozen frames in a short window before a
/// slow subscriber falls behind and gets a `RecvError::Lagged`.
pub const DEFAULT_OUTPUT_BROADCAST: usize = 256;

/// Request for the pane's current `vt_replay_bytes` snapshot.
///
/// Sent by the ATTACH handler on the per-client task; the actor walks
/// its `Terminal` via [`SnapshotSynthesizer`] and replies on the
/// oneshot.
#[derive(Debug)]
pub struct SnapshotRequest {
    /// Channel the actor uses to ship the synthesized snapshot back.
    /// Dropping the sender on the receiver side is benign — the actor
    /// just discards the reply.
    pub reply: oneshot::Sender<SnapshotBytes>,
}

/// Cross-task handle to a [`PaneActor`].
///
/// `PaneHandle` is `Send + Clone`: per-client tasks clone it freely to
/// request snapshots, send input, or subscribe to the output broadcast.
/// The actor itself (which owns the `!Send` `Terminal`) lives on the
/// `LocalSet` and never crosses a thread boundary.
#[derive(Debug, Clone)]
pub struct PaneHandle {
    /// Sender for input events (keys, mouse, etc.). Drained by the
    /// actor and written to the PTY. Stubbed for `phux-byc.8` — the
    /// actor logs and discards input until `phux-byc.5` lands the
    /// PTY pump.
    pub input: mpsc::Sender<PaneInput>,
    /// Sender for snapshot requests. The ATTACH handler uses this to
    /// build `PANE_SNAPSHOT` frames.
    pub snapshot: mpsc::Sender<SnapshotRequest>,
    /// Output broadcast channel; subscribers receive every PTY byte
    /// chunk forwarded by the actor. Empty for `phux-byc.8` since the
    /// PTY pump is not yet implemented — kept on the handle so the
    /// ATTACH handler can subscribe the client now and start receiving
    /// the moment `phux-byc.5` lands.
    pub output: broadcast::Sender<Bytes>,
    /// Pane viewport width in cells at construction time. Placeholder
    /// until `VIEWPORT_RESIZE` (`phux-4hp`) wires through; the actor
    /// is the eventual owner of `Terminal::set_size`.
    pub cols: u16,
    /// Pane viewport height in cells at construction time. Same
    /// placeholder story as [`Self::cols`].
    pub rows: u16,
}

/// Per-pane actor. Owns the `Terminal` and serves the channels exposed
/// via [`PaneHandle`].
///
/// `Terminal<'static, 'static>` because we use [`Terminal::new`] (NULL
/// allocator) — the lifetime parameters degenerate to `'static`. A
/// future custom allocator path would tie this to the surrounding
/// arena's lifetime; not needed for `phux-byc.8`.
///
/// `Terminal` and `SnapshotSynthesizer` are stashed inside `RefCell` so
/// the `select!` arms (which conceptually borrow `&mut self`) can each
/// take what they need without fighting the borrow checker over
/// disjoint field access.
pub struct PaneActor {
    terminal: RefCell<Terminal<'static, 'static>>,
    synth: RefCell<SnapshotSynthesizer<'static>>,
    input_rx: mpsc::Receiver<PaneInput>,
    snapshot_rx: mpsc::Receiver<SnapshotRequest>,
    /// Kept on the actor so `phux-byc.5`'s PTY-read branch can publish
    /// bytes here without restructuring the actor; for `phux-byc.8` it
    /// has no driver yet. Held to keep the channel alive for any
    /// already-subscribed clients (subscribers see `Closed` when the
    /// last sender drops; we don't want to spuriously close a channel
    /// that the PTY pump hasn't been wired into yet).
    #[allow(dead_code, reason = "phux-byc.5 PTY pump will publish to this")]
    output_tx: broadcast::Sender<Bytes>,
    shutdown: oneshot::Receiver<()>,
    cols: u16,
    rows: u16,
}

/// Errors surfaced while constructing a [`PaneActor`].
#[derive(Debug, thiserror::Error)]
pub enum PaneActorError {
    /// Libghostty refused to allocate a [`Terminal`].
    #[error("Terminal::new failed: {0}")]
    Terminal(#[from] libghostty_vt::Error),
    /// Failed to allocate the [`SnapshotSynthesizer`].
    #[error("SnapshotSynthesizer::new failed: {0}")]
    Synth(#[from] crate::grid::SynthesisError),
}

/// Bundle returned from [`PaneActor::new`]: the actor itself plus a
/// shutdown sender that fires the actor's exit branch when dropped or
/// signaled.
#[must_use]
pub struct PaneActorBundle {
    /// The actor; pass to `tokio::task::spawn_local`.
    pub actor: PaneActor,
    /// Cross-task handle to the actor.
    pub handle: PaneHandle,
    /// Drop or `send(())` to shut the actor down cleanly.
    pub shutdown: oneshot::Sender<()>,
}

impl std::fmt::Debug for PaneActor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PaneActor")
            .field("cols", &self.cols)
            .field("rows", &self.rows)
            .finish_non_exhaustive()
    }
}

impl std::fmt::Debug for PaneActorBundle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PaneActorBundle")
            .field("actor", &self.actor)
            .field("handle", &self.handle)
            .finish_non_exhaustive()
    }
}

impl PaneActor {
    /// Build a fresh actor of the given dimensions.
    ///
    /// The `Terminal` is allocated via libghostty's default allocator
    /// (NULL alloc → `'static` lifetimes). `max_scrollback` defaults to
    /// `10_000` — a tmux-style mid-range value. Scrollback negotiation
    /// per ATTACH viewport metrics is deferred to `phux-byc.5`.
    ///
    /// Returns a [`PaneActorBundle`] rather than `Self` because the
    /// caller needs the `PaneHandle` + shutdown sender from the same
    /// allocation site as the actor itself.
    #[allow(clippy::new_ret_no_self, reason = "bundle-shaped constructor")]
    pub fn new(cols: u16, rows: u16) -> Result<PaneActorBundle, PaneActorError> {
        let terminal = Terminal::new(TerminalOptions {
            cols,
            rows,
            max_scrollback: 10_000,
        })?;
        let synth = SnapshotSynthesizer::new()?;

        let (input_tx, input_rx) = mpsc::channel(DEFAULT_INPUT_MAILBOX);
        let (snapshot_tx, snapshot_rx) = mpsc::channel(DEFAULT_INPUT_MAILBOX);
        let (output_tx, _output_rx_seed) = broadcast::channel(DEFAULT_OUTPUT_BROADCAST);
        let (shutdown_tx, shutdown_rx) = oneshot::channel();

        let actor = Self {
            terminal: RefCell::new(terminal),
            synth: RefCell::new(synth),
            input_rx,
            snapshot_rx,
            output_tx: output_tx.clone(),
            shutdown: shutdown_rx,
            cols,
            rows,
        };
        let handle = PaneHandle {
            input: input_tx,
            snapshot: snapshot_tx,
            output: output_tx,
            cols,
            rows,
        };
        Ok(PaneActorBundle {
            actor,
            handle,
            shutdown: shutdown_tx,
        })
    }

    /// Test-only constructor: write `bytes` into the actor's `Terminal`
    /// before the actor starts running. Useful for unit tests that want
    /// the snapshot path to return non-trivial content without wiring
    /// up a PTY pump (`phux-byc.5` lands the real PTY source).
    ///
    /// Returns the bundle so callers can spawn the actor immediately.
    #[cfg(test)]
    pub fn new_with_seed(
        cols: u16,
        rows: u16,
        bytes: &[u8],
    ) -> Result<PaneActorBundle, PaneActorError> {
        let bundle = Self::new(cols, rows)?;
        bundle.actor.terminal.borrow_mut().vt_write(bytes);
        Ok(bundle)
    }

    /// Synthesize a snapshot of the current `Terminal` state. Exposed
    /// for tests that want to drive the synthesis path synchronously
    /// without going through the actor's `select!` loop.
    fn synthesize(&self) -> Result<SnapshotBytes, crate::grid::SynthesisError> {
        let terminal = self.terminal.borrow();
        let mut synth = self.synth.borrow_mut();
        synth.synthesize(&terminal)
    }

    /// Run the actor's event loop until shutdown.
    ///
    /// For `phux-byc.8`, only the snapshot-request and shutdown branches
    /// are exercised. The input-recv branch records-and-discards
    /// (mirroring `state.rs`'s pre-existing input log behavior); the
    /// PTY read branch is absent entirely until `phux-byc.5` lands the
    /// `portable-pty` integration. When that ticket lands, the
    /// `select!` here gains a `_ = self.pty.read() => { ... }` arm.
    #[allow(
        clippy::future_not_send,
        reason = "ADR-0014: PaneActor owns !Send Terminal; lives on LocalSet"
    )]
    pub async fn run(mut self) {
        debug!(cols = self.cols, rows = self.rows, "PaneActor started");
        loop {
            tokio::select! {
                biased;
                // Shutdown wins over other branches so a `drop(handle)`
                // path can terminate the actor without racing pending
                // requests.
                _ = &mut self.shutdown => {
                    debug!("PaneActor shutdown signal");
                    return;
                }
                Some(req) = self.snapshot_rx.recv() => {
                    let snap = match self.synthesize() {
                        Ok(s) => s,
                        Err(err) => {
                            warn!(error = %err, "snapshot synthesis failed; replying with empty");
                            SnapshotBytes {
                                cols: self.cols,
                                rows: self.rows,
                                bytes: Vec::new(),
                            }
                        }
                    };
                    // If the requester dropped the receiver, just discard
                    // the reply. Not an error: the client probably
                    // disconnected mid-attach.
                    let _ = req.reply.send(snap);
                }
                Some(input) = self.input_rx.recv() => {
                    // Stub: phux-byc.5 will translate `PaneInput` into
                    // PTY bytes via the libghostty input encoders + the
                    // pane's mode bits, then `terminal.vt_write` the
                    // local echo / `pty.write` the actual bytes. For
                    // now we just log; the per-pane input log on
                    // ServerState records the same data for
                    // inspectability.
                    debug!(?input, "PaneActor received input (stubbed)");
                }
                // No-output-side-driver: the broadcast sender is held
                // by the actor but nothing pumps PTY bytes into it for
                // byc.8. The `output_tx` exists so the ATTACH handler
                // can subscribe clients now; the moment the PTY pump
                // lands, subscriptions become live.
                else => break,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Direct synchronous test: snapshot-of-blank-Terminal yields the
    /// expected reset preamble. Doesn't spawn the actor; exercises the
    /// synthesis helper directly.
    #[test]
    fn synthesize_blank_pane_returns_reset_preamble() {
        let bundle = PaneActor::new(80, 24).expect("new");
        let snap = bundle.actor.synthesize().expect("synthesize");
        assert_eq!(snap.cols, 80);
        assert_eq!(snap.rows, 24);
        assert!(snap.bytes.starts_with(b"\x1b[!p\x1b[2J\x1b[H"));
    }

    /// Synchronous test: seed bytes flow through to the synthesized
    /// snapshot. Exercises [`PaneActor::new_with_seed`].
    #[test]
    fn synthesize_seeded_pane_carries_visible_text() {
        let bundle = PaneActor::new_with_seed(20, 5, b"hello").expect("new_with_seed");
        let snap = bundle.actor.synthesize().expect("synthesize");
        // The seeded text round-trips through the synthesizer.
        let body = String::from_utf8_lossy(&snap.bytes);
        assert!(
            body.contains("hello"),
            "synthesized bytes should contain seeded text, got: {body:?}"
        );
    }

    /// Async test: the actor responds to `SnapshotRequest` over the
    /// `LocalSet` and ships back the same bytes the synchronous
    /// synthesizer would.
    #[tokio::test(flavor = "current_thread")]
    async fn actor_responds_to_snapshot_request_on_localset() {
        let local = tokio::task::LocalSet::new();
        local
            .run_until(async {
                let bundle = PaneActor::new_with_seed(20, 5, b"hi there").expect("new_with_seed");
                let handle = bundle.handle.clone();
                let _shutdown_tx = bundle.shutdown; // keep alive so the actor doesn't exit early
                tokio::task::spawn_local(bundle.actor.run());

                let (reply_tx, reply_rx) = oneshot::channel();
                handle
                    .snapshot
                    .send(SnapshotRequest { reply: reply_tx })
                    .await
                    .expect("send snapshot request");
                let snap = reply_rx.await.expect("snapshot reply");
                assert_eq!(snap.cols, 20);
                assert_eq!(snap.rows, 5);
                let body = String::from_utf8_lossy(&snap.bytes);
                assert!(
                    body.contains("hi there"),
                    "actor-synthesized bytes should contain seeded text"
                );
            })
            .await;
    }

    /// The actor stops promptly when the shutdown oneshot fires, even
    /// if input/snapshot channels stay open.
    #[tokio::test(flavor = "current_thread")]
    async fn actor_exits_on_shutdown_signal() {
        let local = tokio::task::LocalSet::new();
        local
            .run_until(async {
                let bundle = PaneActor::new(20, 5).expect("new");
                let handle = bundle.handle.clone();
                let shutdown_tx = bundle.shutdown;
                let join = tokio::task::spawn_local(bundle.actor.run());

                shutdown_tx.send(()).expect("send shutdown");
                // The actor must terminate quickly. Wrap in a tiny
                // timeout so a hang surfaces as a test failure, not a
                // hung CI job.
                tokio::time::timeout(std::time::Duration::from_millis(500), join)
                    .await
                    .expect("actor did not exit within 500ms")
                    .expect("actor task panicked");

                // After shutdown, `handle` still works as a value but
                // the actor is gone. Sending a snapshot request will
                // succeed at the channel level (mailbox has slack) but
                // the reply will never arrive.
                let (reply_tx, reply_rx) = oneshot::channel();
                let _ = handle
                    .snapshot
                    .try_send(SnapshotRequest { reply: reply_tx });
                drop(reply_rx);
            })
            .await;
    }
}
