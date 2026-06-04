//! Off-loop stdout writer (phux-fysb).
//!
//! The attach `tokio::select!` loop renders synchronously: every
//! `paint_full_frame`/`render_at` ends in `out.flush()`. When `out` is the
//! real tty that flush BLOCKS until the terminal drains, and because the
//! paint happens *inside* the `biased` `conn.recv()` select arm, a slow
//! terminal starves the stdin/signal arms — the client wedges (Ctrl-C and
//! detach stop working). Multi-pane re-attach makes it worse: the
//! `paint_full_frame` burst is ~N× a single pane's bytes, so it crosses the
//! wedge threshold a single pane never reaches.
//!
//! [`StdoutSink`] breaks that coupling. It is a `Write` that the driver uses
//! as `out`: writes accumulate in an in-memory buffer, and `flush()` ships
//! the accumulated bytes to a dedicated OS thread that owns the real stdout
//! and does the blocking write off the runtime thread. The select loop never
//! blocks on the terminal, so input/signals are always serviced.
//!
//! Backpressure is bounded and lossless-at-the-frame-level: if the writer
//! falls far enough behind that the queued backlog exceeds [`CAP_BYTES`], the
//! sink DROPS the stale backlog and sets a `needs_resync` flag. The driver
//! polls that flag and repaints the latest state from scratch
//! (`paint_full_frame` is self-contained — an `ED2` clear + full redraw — so
//! it supersedes every dropped diff). The result under a sustained-slow sink:
//! the user sees the newest full frame as fast as the terminal can absorb it,
//! intermediate diffs are dropped, memory stays bounded, and the loop never
//! blocks.

use std::collections::VecDeque;
use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;

/// Backlog cap in bytes before the sink drops the stale queue and forces a
/// resync. A single full-screen repaint is ~20–30 KB even at large multi-pane
/// sizes, so this is comfortably above one legitimate frame (a healthy
/// terminal never trips it) yet bounds memory under a stuck/slow sink.
const CAP_BYTES: usize = 256 * 1024;

/// Shared producer/consumer state behind the lock.
struct QueueState {
    /// Complete-`flush()` byte buffers, written to the tty in order.
    chunks: VecDeque<Vec<u8>>,
    /// Running total of `chunks` byte lengths (cheap cap check).
    bytes: usize,
    /// Set by [`WriterHandle::shutdown_and_join`]; tells the writer to drain
    /// and exit.
    shutdown: bool,
}

struct Shared {
    queue: Mutex<QueueState>,
    cv: Condvar,
}

/// The `Write` the driver threads through `main_loop` as `out`.
///
/// `write*` only appends to `pending` (never blocks, never locks). `flush`
/// is the ship point: it moves `pending` into the shared queue and wakes the
/// writer thread.
pub(super) struct StdoutSink {
    shared: Arc<Shared>,
    /// Driver-polled: set when the backlog overflowed and stale frames were
    /// dropped, so the driver repaints the latest state. Cloned so the driver
    /// can hold a reader independent of the `&mut StdoutSink` borrow.
    pub(super) needs_resync: Arc<AtomicBool>,
    pending: Vec<u8>,
}

impl Write for StdoutSink {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.pending.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        self.pending.extend_from_slice(buf);
        Ok(())
    }

    fn flush(&mut self) -> io::Result<()> {
        if self.pending.is_empty() {
            return Ok(());
        }
        let chunk = std::mem::take(&mut self.pending);
        {
            let mut q = self
                .shared
                .queue
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if q.bytes.saturating_add(chunk.len()) > CAP_BYTES {
                // Writer is behind. Drop the stale backlog AND this chunk (a
                // diff that would corrupt the screen if applied after the gap)
                // and ask the driver to repaint the latest state from scratch.
                q.chunks.clear();
                q.bytes = 0;
                self.needs_resync.store(true, Ordering::Release);
            } else {
                q.bytes += chunk.len();
                q.chunks.push_back(chunk);
            }
        }
        self.shared.cv.notify_one();
        Ok(())
    }
}

/// Owns the writer thread; used to drain + stop it cleanly on attach exit.
pub(super) struct WriterHandle {
    shared: Arc<Shared>,
    join: Option<JoinHandle<()>>,
}

impl WriterHandle {
    /// Stop the writer and join it. DROPS any queued backlog rather than
    /// draining it: every attach-exit path leaves the alt screen (the reset in
    /// `exit_after_detach` / `RawModeGuard::Drop`), which discards the
    /// alt-screen content the backlog was painting — so draining it to a slow
    /// terminal would just make detach hang for no visible benefit. The writer
    /// finishes at most the one chunk it is mid-write on, then exits; the
    /// direct reset write that follows is therefore not garbled by a queued
    /// frame. Call this BEFORE the reset writes on every exit path.
    pub(super) fn shutdown_and_join(mut self) {
        {
            let mut q = self
                .shared
                .queue
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            q.shutdown = true;
            q.chunks.clear();
            q.bytes = 0;
        }
        self.shared.cv.notify_one();
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

/// Spawn the stdout writer thread and return the sink + its handle.
pub(super) fn spawn_stdout_writer() -> (StdoutSink, WriterHandle) {
    spawn_writer_into(io::stdout())
}

/// As [`spawn_stdout_writer`] but writes to an arbitrary sink — the seam tests
/// use to drive a deliberately-slow inner writer and prove `flush()` stays
/// non-blocking regardless of how slow the terminal is.
#[allow(
    clippy::expect_used,
    reason = "thread spawn failure at attach start is fatal and unrecoverable"
)]
fn spawn_writer_into<W: Write + Send + 'static>(inner: W) -> (StdoutSink, WriterHandle) {
    let shared = Arc::new(Shared {
        queue: Mutex::new(QueueState {
            chunks: VecDeque::new(),
            bytes: 0,
            shutdown: false,
        }),
        cv: Condvar::new(),
    });
    let writer_shared = Arc::clone(&shared);
    let join = std::thread::Builder::new()
        .name("phux-stdout".to_owned())
        .spawn(move || writer_loop(&writer_shared, inner))
        .expect("spawn phux-stdout writer thread");
    let sink = StdoutSink {
        shared: Arc::clone(&shared),
        needs_resync: Arc::new(AtomicBool::new(false)),
        pending: Vec::with_capacity(8192),
    };
    (
        sink,
        WriterHandle {
            shared,
            join: Some(join),
        },
    )
}

/// Drain the queue to `out`, blocking on the sink off the runtime thread.
/// Exits once `shutdown` is set AND the queue is empty (so a clean shutdown
/// flushes every queued chunk first; `shutdown_and_join` clears the backlog so
/// this exits promptly).
fn writer_loop<W: Write>(shared: &Shared, mut out: W) {
    loop {
        let chunks: Vec<Vec<u8>> = {
            let mut q = shared
                .queue
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            while q.chunks.is_empty() && !q.shutdown {
                q = shared
                    .cv
                    .wait(q)
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
            }
            if q.chunks.is_empty() && q.shutdown {
                break;
            }
            q.bytes = 0;
            q.chunks.drain(..).collect()
        };
        for chunk in &chunks {
            if out.write_all(chunk).is_err() {
                return;
            }
        }
        let _ = out.flush();
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, reason = "tests")]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    /// A sink that sleeps on every write — stands in for a terminal so slow it
    /// would wedge the select loop if `flush()` blocked on it.
    struct SlowSink {
        per_write: Duration,
    }
    impl Write for SlowSink {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            std::thread::sleep(self.per_write);
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn flush_does_not_block_on_a_slow_sink() {
        // The writer thread sleeps 50ms per chunk; the select-loop-side
        // `flush()` must still return ~instantly. This is the core of the
        // phux-fysb fix: render never blocks on the terminal.
        let (mut sink, handle) = spawn_writer_into(SlowSink {
            per_write: Duration::from_millis(50),
        });
        let mut worst = Duration::ZERO;
        for i in 0..20u32 {
            sink.write_all(format!("frame-{i}\n").as_bytes())
                .expect("write");
            let t0 = Instant::now();
            sink.flush().expect("flush");
            worst = worst.max(t0.elapsed());
        }
        // 20 frames behind a 50ms/chunk sink is ~1s of writer work, but each
        // flush returned in well under one chunk-time. Generous bound to stay
        // robust on a loaded CI box; the unfixed (direct-stdout) path would
        // see flushes of ~50ms+ each.
        assert!(
            worst < Duration::from_millis(25),
            "flush blocked on the slow sink: worst={worst:?}"
        );
        handle.shutdown_and_join();
    }

    #[test]
    fn flush_never_blocks_and_ships_in_order() {
        let (mut sink, handle) = spawn_stdout_writer();
        // write+flush a few frames; flush must return immediately.
        for i in 0..5u8 {
            sink.write_all(&[i]).expect("write");
            sink.flush().expect("flush");
        }
        // Nothing dropped (well under the cap), resync not set.
        assert!(!sink.needs_resync.load(Ordering::Acquire));
        handle.shutdown_and_join();
    }

    #[test]
    fn overflow_drops_backlog_and_sets_resync() {
        // Drive the queue past CAP_BYTES WITHOUT a draining writer by building
        // the shared state directly (no thread), exercising the sink's flush
        // backpressure branch deterministically.
        let shared = Arc::new(Shared {
            queue: Mutex::new(QueueState {
                chunks: VecDeque::new(),
                bytes: 0,
                shutdown: false,
            }),
            cv: Condvar::new(),
        });
        let mut sink = StdoutSink {
            shared: Arc::clone(&shared),
            needs_resync: Arc::new(AtomicBool::new(false)),
            pending: Vec::new(),
        };
        // Queue just under the cap.
        sink.write_all(&vec![0u8; CAP_BYTES - 1]).expect("write");
        sink.flush().expect("flush");
        assert!(!sink.needs_resync.load(Ordering::Acquire));
        assert_eq!(shared.queue.lock().expect("lock").chunks.len(), 1);
        // The next chunk trips the cap: backlog dropped, resync set.
        sink.write_all(&[1u8, 2, 3]).expect("write");
        sink.flush().expect("flush");
        assert!(sink.needs_resync.load(Ordering::Acquire));
        let (chunks_empty, bytes) = {
            let q = shared.queue.lock().expect("lock");
            (q.chunks.is_empty(), q.bytes)
        };
        assert!(chunks_empty, "stale backlog dropped on overflow");
        assert_eq!(bytes, 0);
    }

    #[test]
    fn empty_flush_is_a_noop() {
        let (mut sink, handle) = spawn_stdout_writer();
        sink.flush().expect("flush"); // no pending bytes
        assert!(!sink.needs_resync.load(Ordering::Acquire));
        handle.shutdown_and_join();
    }
}
