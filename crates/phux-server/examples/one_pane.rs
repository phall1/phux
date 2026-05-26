//! `one_pane` — spawn a real shell in a real PTY, pipe its output into a
//! `libghostty_vt::Terminal`, and stream `DiffOp[]` summaries to stderr.
//!
//! This is the first end-to-end PTY → Terminal → diff smoke test for phux
//! (phux-bc1.2). It validates the architectural seam *with real bytes*:
//!
//!   $SHELL ── PTY ──> reader thread ──mpsc──> tokio task
//!                                              │
//!                                              ▼
//!                                       `Terminal::vt_write`
//!                                              │
//!                                       `PaneCapture::capture`
//!                                              │
//!                                       `compute_diff(prev, next)`
//!                                              │
//!                                              ▼
//!                                          eprintln!
//!
//! No IPC, no protocol wire — that's E3 (phux-byc). The point is "real
//! bytes flowing through a real PTY into a real Terminal, with frame
//! diffs falling out the other side."
//!
//! ## Stdin → PTY
//!
//! Stdin is bridged through a dedicated `std::thread` (portable-pty's API
//! is blocking, and the workspace tokio build does *not* enable `io-std`,
//! so we can't use `tokio::io::stdin()`). Each line read on stdin is
//! written to the PTY master verbatim with a trailing `\r` to simulate
//! Enter — that's how the user feeds e.g. `echo hi; exit` into the shell.
//!
//! ## Run
//!
//!     nix develop -c cargo run -p phux-server --example one_pane
//!     echo 'echo hi; exit' | nix develop -c cargo run -p phux-server --example one_pane

#![allow(clippy::print_stderr, reason = "spike binary — stderr IS the output")]
#![allow(clippy::expect_used, reason = "spike binary — fail loudly")]
#![allow(
    clippy::too_many_lines,
    reason = "single-file spike, kept linear on purpose"
)]

use std::{
    io::{BufRead, Read, Write},
    sync::{Arc, Mutex},
    time::Duration,
};

use libghostty_vt::{Terminal, TerminalOptions};
use phux_protocol::{DiffOp, Grid, compute_diff};
use phux_server::grid::PaneCapture;
use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use tokio::{
    sync::mpsc,
    time::{Instant, interval},
};

/// Chunks of bytes streamed from the PTY master reader thread.
enum PtyEvent {
    /// A chunk of bytes read from the PTY master.
    Bytes(Vec<u8>),
    /// The PTY reader hit EOF (or errored). Either way: child is going away.
    Eof,
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // ─── 1. Open a PTY pair ────────────────────────────────────────────
    let pty_system = native_pty_system();
    let pair = pty_system.openpty(PtySize {
        rows: 24,
        cols: 80,
        pixel_width: 0,
        pixel_height: 0,
    })?;

    // ─── 2. Spawn $SHELL on the slave side ─────────────────────────────
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string());
    let mut cmd = CommandBuilder::new(&shell);
    cmd.env("TERM", "ghostty");
    // Make the prompt deterministic-ish for the smoke test; not strictly
    // required — the diff stream is interesting either way.
    cmd.env("PS1", "$ ");
    let mut child = pair.slave.spawn_command(cmd)?;

    // The slave fd lives in the child; drop our handle so the child is the
    // sole owner. Otherwise the master never sees EOF when the child exits.
    drop(pair.slave);

    // ─── 3. Master reader (sync) → tokio (async) bridge ────────────────
    let mut reader = pair.master.try_clone_reader()?;
    // Wrap the master writer in Arc<Mutex<_>> so both the stdin-bridge
    // thread and (potentially) the main task can write to it.
    let writer = Arc::new(Mutex::new(pair.master.take_writer()?));

    let (pty_tx, mut pty_rx) = mpsc::unbounded_channel::<PtyEvent>();

    // Dedicated OS thread: blocking read loop → unbounded async channel.
    let pty_reader_thread = std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => {
                    let _ = pty_tx.send(PtyEvent::Eof);
                    break;
                }
                Ok(n) => {
                    if pty_tx.send(PtyEvent::Bytes(buf[..n].to_vec())).is_err() {
                        // Receiver dropped — main task exited. Stop reading.
                        break;
                    }
                }
                Err(e) => {
                    eprintln!("[one_pane] pty read error: {e}");
                    let _ = pty_tx.send(PtyEvent::Eof);
                    break;
                }
            }
        }
    });

    // ─── 4. Stdin → PTY bridge (also blocking, also on its own thread) ─
    // Same rationale as the reader: tokio in this workspace doesn't ship
    // `io-std`, and even if it did, BufRead-on-stdin is fine here.
    let stdin_writer = Arc::clone(&writer);
    let stdin_thread = std::thread::spawn(move || {
        let stdin = std::io::stdin();
        let mut locked = stdin.lock();
        let mut line = String::new();
        loop {
            line.clear();
            match locked.read_line(&mut line) {
                Ok(0) | Err(_) => break, // stdin closed or errored
                Ok(_) => {
                    // Strip the trailing '\n' the OS gave us and append '\r'
                    // so the shell sees Enter the way it would from a tty.
                    let trimmed = line.trim_end_matches('\n').trim_end_matches('\r');
                    let mut out = trimmed.as_bytes().to_vec();
                    out.push(b'\r');
                    let Ok(mut w) = stdin_writer.lock() else {
                        break;
                    };
                    if w.write_all(&out).is_err() {
                        break;
                    }
                    if w.flush().is_err() {
                        break;
                    }
                }
            }
        }
    });

    // ─── 5. Terminal + PaneCapture ─────────────────────────────────────
    let mut terminal = Terminal::new(TerminalOptions {
        cols: 80,
        rows: 24,
        max_scrollback: 10_000,
    })?;
    let mut capture = PaneCapture::new()?;
    let mut prev: Grid = capture.capture(&terminal)?;

    // ─── 6. Main select! loop ──────────────────────────────────────────
    let mut tick = interval(Duration::from_millis(16));
    // Skip the immediate first tick so we don't emit a no-op frame before
    // anything has been written.
    tick.tick().await;

    let mut total_bytes: u64 = 0;
    let mut total_ops: u64 = 0;
    let mut frame_count: u64 = 0;
    let started = Instant::now();
    let mut child_exit_status: Option<portable_pty::ExitStatus> = None;
    let mut saw_eof = false;

    eprintln!("[one_pane] shell={shell} cols=80 rows=24 — streaming diffs every 16ms (stderr)");

    loop {
        tokio::select! {
            // PTY bytes (or EOF) → vt_write
            evt = pty_rx.recv() => {
                match evt {
                    Some(PtyEvent::Bytes(chunk)) => {
                        total_bytes += chunk.len() as u64;
                        terminal.vt_write(&chunk);
                    }
                    Some(PtyEvent::Eof) | None => {
                        saw_eof = true;
                    }
                }
            }
            // Frame tick → capture + diff + report
            _ = tick.tick() => {
                let next = capture.capture(&terminal)?;
                let diff = compute_diff(&prev, &next);
                if !diff.ops.is_empty() {
                    frame_count += 1;
                    total_ops += diff.ops.len() as u64;
                    eprintln!(
                        "[one_pane] frame {frame_count:>4} ops={:>3} cursor=({:>2},{:>2}) summary={}",
                        diff.ops.len(),
                        diff.cursor.row,
                        diff.cursor.col,
                        summarize(&diff.ops),
                    );
                }
                prev = next;
            }
        }

        // Has the child exited? portable-pty's try_wait is non-blocking.
        if child_exit_status.is_none()
            && let Some(status) = child.try_wait()?
        {
            child_exit_status = Some(status);
        }

        // Exit when *both* the child has reaped *and* the reader has
        // drained to EOF. This avoids losing the last few hundred bytes
        // of pre-exit output (e.g. the prompt redraw, exit message).
        if child_exit_status.is_some() && saw_eof {
            break;
        }
    }

    // One last capture so we don't drop the trailing frame between the
    // final tick and exit.
    let final_grid = capture.capture(&terminal)?;
    let final_diff = compute_diff(&prev, &final_grid);
    if !final_diff.ops.is_empty() {
        frame_count += 1;
        total_ops += final_diff.ops.len() as u64;
        eprintln!(
            "[one_pane] frame {frame_count:>4} ops={:>3} (final flush) summary={}",
            final_diff.ops.len(),
            summarize(&final_diff.ops),
        );
    }

    // ─── 7. Shutdown ───────────────────────────────────────────────────
    // Drop the master writer so the stdin bridge thread's write_all fails
    // and it can exit cleanly (it'll still be parked on read_line until
    // the user hits ctrl-D — that's a known limitation of a one-shot
    // smoke test, not a leak).
    drop(writer);
    drop(pair.master);

    // The reader thread will have already exited (it sent Eof). Join it
    // so we surface any panic.
    let _ = pty_reader_thread.join();
    // Don't join the stdin thread — it may be blocked on read_line and
    // there's no portable way to interrupt it. The process is exiting
    // anyway; the OS will reap it.
    drop(stdin_thread);

    let elapsed = started.elapsed();
    let status_repr = child_exit_status
        .as_ref()
        .map_or_else(|| "<unknown>".to_string(), |s| format!("{s:?}"));
    eprintln!("[one_pane] ─── summary ───");
    eprintln!("[one_pane] child exit:   {status_repr}");
    eprintln!("[one_pane] elapsed:      {elapsed:.2?}");
    eprintln!("[one_pane] bytes read:   {total_bytes}");
    eprintln!("[one_pane] frames w/ops: {frame_count}");
    eprintln!("[one_pane] total ops:    {total_ops}");

    Ok(())
}

/// One-line classification of what's in a diff slice — enough to eyeball
/// whether the stream looks alive without dumping every `CellRun`.
fn summarize(ops: &[DiffOp]) -> String {
    let mut cell_runs = 0usize;
    let mut clears = 0usize;
    let mut other = 0usize;
    for op in ops {
        match op {
            DiffOp::CellRun { .. } => cell_runs += 1,
            DiffOp::Clear { .. } => clears += 1,
            // `DiffOp` is `#[non_exhaustive]` per phux-429 / SPEC §8.3.
            // Cursor lives on the PANE_DIFF frame, not in the op stream.
            _ => other += 1,
        }
    }
    format!("cellruns={cell_runs} clears={clears} other={other}")
}
