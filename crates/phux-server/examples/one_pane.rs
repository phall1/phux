//! `one_pane` — spawn a real shell in a real PTY, pipe its output into a
//! `libghostty_vt::Terminal`, and emit `TERMINAL_OUTPUT` frame summaries to
//! stderr.
//!
//! This is the end-to-end PTY → Terminal → bytes-on-wire smoke test for
//! phux under ADR-0013:
//!
//!   $SHELL ── PTY ──> reader thread ──mpsc──> tokio task
//!                                              │
//!                                              ▼
//!                                       `Terminal::vt_write` (canonical)
//!                                              │
//!                                              ▼
//!                                    `downsample::rewrite_bytes`
//!                                       (per-client capability tier)
//!                                              │
//!                                              ▼
//!                                  `FrameKind::TerminalOutput { … }`
//!                                       (encoded; logged to stderr)
//!
//! No IPC, no connected client: the goal is "real bytes flowing through a
//! real PTY into a real Terminal, with valid `TERMINAL_OUTPUT` frames falling
//! out the other side." Useful for hand-eyeballing the byte stream.
//!
//! ## Stdin → PTY
//!
//! Stdin is bridged through a dedicated `std::thread` (portable-pty's API
//! is blocking, and the workspace tokio build does *not* enable `io-std`,
//! so we can't use `tokio::io::stdin()`). Each line read on stdin is
//! written to the PTY master verbatim with a trailing `\r` to simulate
//! Enter.
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

use bytes::BytesMut;
use libghostty_vt::{Terminal, TerminalOptions};
use phux_protocol::caps::ColorSupport;
use phux_protocol::wire::frame::FrameKind;
use phux_server::downsample;
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
    cmd.env("PS1", "$ ");
    let mut child = pair.slave.spawn_command(cmd)?;

    drop(pair.slave);

    // ─── 3. Master reader (sync) → tokio (async) bridge ────────────────
    let mut reader = pair.master.try_clone_reader()?;
    let writer = Arc::new(Mutex::new(pair.master.take_writer()?));

    let (pty_tx, mut pty_rx) = mpsc::unbounded_channel::<PtyEvent>();

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

    // ─── 4. Stdin → PTY bridge ──────────────────────────────────────────
    let stdin_writer = Arc::clone(&writer);
    let stdin_thread = std::thread::spawn(move || {
        let stdin = std::io::stdin();
        let mut locked = stdin.lock();
        let mut line = String::new();
        loop {
            line.clear();
            match locked.read_line(&mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
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

    // ─── 5. Terminal (canonical state owner) ───────────────────────────
    let mut terminal = Terminal::new(TerminalOptions {
        cols: 80,
        rows: 24,
        max_scrollback: 10_000,
    })?;

    // Pretend we have one TrueColor client. Real fanout would loop over
    // every attached client; this spike just demonstrates the path.
    let client_caps = ColorSupport::default();

    // ─── 6. Main select! loop ──────────────────────────────────────────
    // Coalesce PTY bytes between transport writes (SPEC §8.1: server SHOULD
    // batch). 16ms gives ~60 Hz emission cap; the cap is the rate-limit, not
    // the latency floor — short bursts emit on next tick.
    let mut tick = interval(Duration::from_millis(16));
    tick.tick().await;

    let mut total_bytes: u64 = 0;
    let mut total_frames: u64 = 0;
    let mut pending: Vec<u8> = Vec::new();
    let mut seq: u64 = 0;
    let terminal_id = phux_protocol::ids::TerminalId::local(1);
    let started = Instant::now();
    let mut child_exit_status: Option<portable_pty::ExitStatus> = None;
    let mut saw_eof = false;

    eprintln!(
        "[one_pane] shell={shell} cols=80 rows=24 — emitting TERMINAL_OUTPUT frames every 16ms (stderr)"
    );

    loop {
        tokio::select! {
            evt = pty_rx.recv() => {
                match evt {
                    Some(PtyEvent::Bytes(chunk)) => {
                        total_bytes += chunk.len() as u64;
                        terminal.vt_write(&chunk);
                        pending.extend_from_slice(&chunk);
                    }
                    Some(PtyEvent::Eof) | None => {
                        saw_eof = true;
                    }
                }
            }
            _ = tick.tick() => {
                if !pending.is_empty() {
                    let rewritten = downsample::rewrite_bytes(&pending, client_caps);
                    let frame = FrameKind::TerminalOutput {
                        terminal_id: terminal_id.clone(),
                        seq,
                        bytes: rewritten,
                    };
                    let mut enc = BytesMut::new();
                    frame.encode(&mut enc);
                    total_frames += 1;
                    eprintln!(
                        "[one_pane] frame {total_frames:>4} seq={seq} pane={terminal_id} \
                         body_bytes={body} encoded={enc} preview={preview}",
                        body = pending.len(),
                        enc = enc.len(),
                        preview = preview(&pending),
                    );
                    pending.clear();
                    seq = seq.saturating_add(1);
                }
            }
        }

        if child_exit_status.is_none()
            && let Some(status) = child.try_wait()?
        {
            child_exit_status = Some(status);
        }

        if child_exit_status.is_some() && saw_eof {
            break;
        }
    }

    // Final flush so trailing bytes aren't lost.
    if !pending.is_empty() {
        let rewritten = downsample::rewrite_bytes(&pending, client_caps);
        let frame = FrameKind::TerminalOutput {
            terminal_id: terminal_id.clone(),
            seq,
            bytes: rewritten,
        };
        let mut enc = BytesMut::new();
        frame.encode(&mut enc);
        total_frames += 1;
        eprintln!(
            "[one_pane] frame {total_frames:>4} seq={seq} pane={terminal_id} (final flush) body_bytes={body} encoded={enc}",
            body = pending.len(),
            enc = enc.len(),
        );
    }

    drop(writer);
    drop(pair.master);

    let _ = pty_reader_thread.join();
    drop(stdin_thread);

    let elapsed = started.elapsed();
    let status_repr = child_exit_status
        .as_ref()
        .map_or_else(|| "<unknown>".to_string(), |s| format!("{s:?}"));
    eprintln!("[one_pane] ─── summary ───");
    eprintln!("[one_pane] child exit:   {status_repr}");
    eprintln!("[one_pane] elapsed:      {elapsed:.2?}");
    eprintln!("[one_pane] bytes read:   {total_bytes}");
    eprintln!("[one_pane] frames:       {total_frames}");

    Ok(())
}

/// Render up to the first 40 printable ASCII chars of `bytes` for
/// at-a-glance stderr logging. Non-printable bytes render as `.`.
fn preview(bytes: &[u8]) -> String {
    let mut s = String::new();
    for b in bytes.iter().take(40) {
        if (0x20..=0x7e).contains(b) {
            s.push(*b as char);
        } else {
            s.push('.');
        }
    }
    if bytes.len() > 40 {
        s.push_str("...");
    }
    s
}
