//! `capture` bench — exercise the snapshot-synthesis hot path.
//!
//! Demonstrates the value of [`phux_server::grid::SnapshotSynthesizer`]:
//! by holding the libghostty [`RenderState`], [`RowIterator`], and
//! [`CellIterator`] across calls, we avoid re-allocating render
//! scaffolding on every attach.
//!
//! ## What this bench measures
//!
//! Wall-clock cost of `SnapshotSynthesizer::synthesize(&term)` on an
//! 80x24 terminal pre-populated with a mix of ASCII, wide chars, and
//! SGR-styled runs. Under [ADR-0013] this is the work the server does
//! once per client attach (and after backpressure-driven resyncs); it
//! is *not* on the steady-state forwarding path, which is a memcpy
//! plus per-client byte-stream rewrite.
//!
//! [ADR-0013]: https://github.com/phall1/phux/blob/main/ADR/0013-libghostty-bytes-on-wire.md

#![allow(clippy::print_stdout, reason = "bench output by design")]
#![allow(clippy::expect_used, reason = "bench — fail loudly on setup error")]
#![allow(missing_docs, reason = "bench binary")]

use std::hint::black_box;

use criterion::Criterion;
use libghostty_vt::{Terminal, TerminalOptions};
use phux_server::grid::SnapshotSynthesizer;

#[global_allocator]
static ALLOC: dhat::Alloc = dhat::Alloc;

/// Feed a representative mix of bytes into the terminal so the captured
/// grid exercises ASCII, wide chars, and SGR run handling.
fn populate(terminal: &mut Terminal<'_, '_>) {
    terminal.vt_write(b"the quick brown fox jumps over the lazy dog 0123456789\r\n");
    terminal.vt_write(
        b"\x1b[1;31mBOLD-RED\x1b[0m \x1b[3;32mital-green\x1b[0m \x1b[4;34munder-blue\x1b[0m\r\n",
    );
    terminal.vt_write(b"\x1b[38;5;208mfg208\x1b[0m \x1b[48;2;30;144;255mtrue-bg\x1b[0m\r\n");
    "abc \u{4e2d}\u{6587} \u{1f600} mix\r\n"
        .as_bytes()
        .chunks(8)
        .for_each(|c| terminal.vt_write(c));
    let long: Vec<u8> = (b'a'..=b'z').cycle().take(120).collect();
    terminal.vt_write(&long);
    terminal.vt_write(b"\r\n");
    terminal.vt_write(b"\x1b[10;1Hrow10\x1b[12;5Hrow12-col5\r\n");
    terminal.vt_write(b"\x1b[7mreverse\x1b[0m\r\n");
    terminal.vt_write(b"\x1b[9mstrike\x1b[0m \x1b[53moverline\x1b[0m\r\n");
}

fn build_terminal() -> Terminal<'static, 'static> {
    let mut terminal: Terminal<'static, 'static> = Terminal::new(TerminalOptions {
        cols: 80,
        rows: 24,
        max_scrollback: 1_000,
    })
    .expect("Terminal::new");
    populate(&mut terminal);
    terminal
}

fn bench_synthesize(c: &mut Criterion) {
    let terminal = build_terminal();
    let mut synth = SnapshotSynthesizer::new().expect("SnapshotSynthesizer::new");

    // Warm up.
    for _ in 0..16 {
        let snap = synth.synthesize(&terminal).expect("warmup synth");
        black_box(snap);
    }

    let mut group = c.benchmark_group("SnapshotSynthesizer");
    group.bench_function("synthesize_80x24_mixed", |b| {
        b.iter(|| {
            let snap = synth.synthesize(black_box(&terminal)).expect("bench synth");
            black_box(snap);
        });
    });
    group.finish();
}

const DHAT_ITERS: usize = 1_000;

#[allow(
    clippy::cast_precision_loss,
    reason = "approximate per-iter ratios for human reading"
)]
fn dhat_probe() {
    let _profiler = dhat::Profiler::builder().testing().build();

    let terminal = build_terminal();

    // ---- pooled ----
    let mut synth = SnapshotSynthesizer::new().expect("SnapshotSynthesizer::new (dhat)");
    for _ in 0..32 {
        let snap = synth.synthesize(&terminal).expect("dhat warmup synth");
        black_box(snap);
    }
    let before_pooled = dhat::HeapStats::get();
    for _ in 0..DHAT_ITERS {
        let snap = synth.synthesize(&terminal).expect("dhat pooled synth");
        black_box(snap);
    }
    let after_pooled = dhat::HeapStats::get();
    let pooled_blocks = after_pooled.total_blocks - before_pooled.total_blocks;
    let pooled_bytes = after_pooled.total_bytes - before_pooled.total_bytes;

    // ---- one-shot ----
    let before_oneshot = dhat::HeapStats::get();
    for _ in 0..DHAT_ITERS {
        let mut throwaway = SnapshotSynthesizer::new().expect("dhat oneshot new");
        let snap = throwaway.synthesize(&terminal).expect("dhat oneshot synth");
        black_box(snap);
    }
    let after_oneshot = dhat::HeapStats::get();
    let oneshot_blocks = after_oneshot.total_blocks - before_oneshot.total_blocks;
    let oneshot_bytes = after_oneshot.total_bytes - before_oneshot.total_bytes;

    let saved_blocks = oneshot_blocks.saturating_sub(pooled_blocks);
    let saved_bytes = oneshot_bytes.saturating_sub(pooled_bytes);

    println!("---- dhat steady-state ({DHAT_ITERS} iterations) ----");
    println!("  pooled   SnapshotSynthesizer: {pooled_blocks} blocks, {pooled_bytes} bytes");
    println!(
        "      per-iter:                  {:.2} blocks, {:.2} bytes",
        pooled_blocks as f64 / DHAT_ITERS as f64,
        pooled_bytes as f64 / DHAT_ITERS as f64,
    );
    println!("  one-shot SnapshotSynthesizer: {oneshot_blocks} blocks, {oneshot_bytes} bytes");
    println!(
        "      per-iter:                  {:.2} blocks, {:.2} bytes",
        oneshot_blocks as f64 / DHAT_ITERS as f64,
        oneshot_bytes as f64 / DHAT_ITERS as f64,
    );
    println!("  delta (Rust-side):           {saved_blocks} blocks, {saved_bytes} bytes");
    println!(
        "      per-iter delta:            {:.2} blocks, {:.2} bytes",
        saved_blocks as f64 / DHAT_ITERS as f64,
        saved_bytes as f64 / DHAT_ITERS as f64,
    );
    println!("  note: dhat tracks Rust's global allocator. The libghostty FFI");
    println!("        scaffolding (RenderState/RowIterator/CellIterator) is");
    println!("        allocated by libghostty's Zig allocator — invisible to");
    println!("        dhat but a real wall-clock cost the criterion bench");
    println!("        captures (compare timings with and without pooling).");
    println!("------------------------------------------------------------");
}

fn main() {
    if std::env::var_os("PHUX_CAPTURE_DHAT").is_some() {
        dhat_probe();
        return;
    }
    let mut criterion = Criterion::default().configure_from_args();
    bench_synthesize(&mut criterion);
    criterion.final_summary();
}
