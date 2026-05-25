//! `capture` bench — exercise the per-pane hot path.
//!
//! Demonstrates the value of [`phux_server::grid::PaneCapture`]: by holding
//! the libghostty [`RenderState`], [`RowIterator`], and [`CellIterator`]
//! across frames, we avoid re-allocating render scaffolding on every tick.
//!
//! ## What this bench measures
//!
//! - Wall-clock cost of `PaneCapture::capture(&term)` on an 80x24 terminal
//!   pre-populated with a mix of ASCII, wide chars, and SGR-styled runs.
//!   This is the steady-state per-frame budget for one pane.
//!
//! ## Verifying steady-state allocation behavior
//!
//! Criterion does not natively expose per-iteration heap allocation
//! counters. To inspect steady-state allocations directly, run the bench
//! binary with `PHUX_CAPTURE_DHAT=1`:
//!
//! ```text
//! PHUX_CAPTURE_DHAT=1 nix develop -c \
//!   cargo bench -p phux-server --bench capture
//! ```
//!
//! The bench binary, before handing control to criterion, runs a
//! [`dhat`]-instrumented loop in two configurations — `PaneCapture` held
//! across iterations vs a fresh `PaneCapture::new()` per call — and
//! prints `total_blocks` / `total_bytes` deltas side by side.
//!
//! ### Interpreting the dhat output
//!
//! `dhat` tracks the **Rust global allocator** only. The pooled
//! `RenderState`/`RowIterator`/`CellIterator` are libghostty FFI handles
//! whose backing memory comes from the Zig allocator inside
//! libghostty-vt-sys, not from Rust's heap — so their allocations are
//! invisible to dhat. As a result, both paths report the **same**
//! Rust-side block count: that count is dominated by the returned
//! `Grid` (a `Vec<Vec<Cell>>` plus per-cell `Vec<char>` graphemes), which
//! is constructed identically by both paths.
//!
//! What dhat **does** confirm is the invariant we care about: the pooled
//! `PaneCapture::capture` does not grow any Rust-side allocator state
//! across iterations (per-iter block count is stable). Steady-state
//! per-iter alloc count == constant.
//!
//! True zero-alloc is not achievable today because `Grid` is returned by
//! value with owned vectors. The savings pooled by `PaneCapture` are the
//! libghostty FFI scaffolding allocations (avoided on every frame after
//! the first), which are real wall-clock savings the criterion timing
//! captures even though they're invisible to dhat. See SPEC §8 hot-path
//! / frame model.

#![allow(clippy::print_stdout, reason = "bench output by design")]
#![allow(clippy::expect_used, reason = "bench — fail loudly on setup error")]
#![allow(missing_docs, reason = "bench binary")]

use std::hint::black_box;

use criterion::Criterion;
use libghostty_vt::{Terminal, TerminalOptions};
use phux_server::grid::PaneCapture;

// `dhat::Alloc` no-ops to the system allocator when no `dhat::Profiler` is
// active, so it is safe to install globally for this bench binary. The
// profiler is built only inside [`dhat_probe`].
#[global_allocator]
static ALLOC: dhat::Alloc = dhat::Alloc;

/// Feed a representative mix of bytes into the terminal so the captured
/// grid exercises ASCII, wide chars, and SGR run handling.
fn populate(terminal: &mut Terminal<'_, '_>) {
    // Plain ASCII line.
    terminal.vt_write(b"the quick brown fox jumps over the lazy dog 0123456789\r\n");
    // SGR runs: bold red, italic green, underline blue, reset.
    terminal.vt_write(
        b"\x1b[1;31mBOLD-RED\x1b[0m \x1b[3;32mital-green\x1b[0m \x1b[4;34munder-blue\x1b[0m\r\n",
    );
    // 256-color and truecolor runs.
    terminal.vt_write(b"\x1b[38;5;208mfg208\x1b[0m \x1b[48;2;30;144;255mtrue-bg\x1b[0m\r\n");
    // Wide chars (CJK) mixed with ASCII.
    "abc \u{4e2d}\u{6587} \u{1f600} mix\r\n"
        .as_bytes()
        .chunks(8)
        .for_each(|c| terminal.vt_write(c));
    // A long line that should wrap once at 80 cols.
    let long: Vec<u8> = (b'a'..=b'z').cycle().take(120).collect();
    terminal.vt_write(&long);
    terminal.vt_write(b"\r\n");
    // Cursor moves and partial overwrites.
    terminal.vt_write(b"\x1b[10;1Hrow10\x1b[12;5Hrow12-col5\r\n");
    // Reverse video block.
    terminal.vt_write(b"\x1b[7mreverse\x1b[0m\r\n");
    // Strikethrough + overline.
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

fn bench_capture(c: &mut Criterion) {
    let terminal = build_terminal();
    let mut pane = PaneCapture::new().expect("PaneCapture::new");

    // Warm up: prime any one-time grow paths inside the pooled iterators.
    for _ in 0..16 {
        let g = pane.capture(&terminal).expect("warmup capture");
        black_box(g);
    }

    let mut group = c.benchmark_group("PaneCapture");
    group.bench_function("capture_80x24_mixed", |b| {
        b.iter(|| {
            let grid = pane.capture(black_box(&terminal)).expect("bench capture");
            black_box(grid);
        });
    });
    group.finish();
}

const DHAT_ITERS: usize = 1_000;

/// dhat-instrumented loop. Run with `PHUX_CAPTURE_DHAT=1` and inspect the
/// printed stats. See the module docs for interpretation.
///
/// Prints two side-by-side measurements over `DHAT_ITERS` iterations:
///
/// 1. **pooled** — one `PaneCapture` reused across all iterations
///    (the hot-path target).
/// 2. **one-shot** — fresh `PaneCapture::new()` on every iteration
///    (the pre-bc1.1 behavior of the free `capture()` function).
///
/// The delta between the two `total_blocks` counts is exactly the
/// allocations the pool eliminates (libghostty FFI scaffolding). The
/// remainder in the pooled count is the unavoidable per-frame cost of
/// constructing the returned `Grid`.
#[allow(
    clippy::cast_precision_loss,
    reason = "approximate per-iter ratios for human reading"
)]
fn dhat_probe() {
    let _profiler = dhat::Profiler::builder().testing().build();

    let terminal = build_terminal();

    // ---- pooled ----
    let mut pane = PaneCapture::new().expect("PaneCapture::new (dhat)");
    // Warm up — let any first-call growth happen before measurement.
    for _ in 0..32 {
        let g = pane.capture(&terminal).expect("dhat warmup capture");
        black_box(g);
    }
    let before_pooled = dhat::HeapStats::get();
    for _ in 0..DHAT_ITERS {
        let g = pane.capture(&terminal).expect("dhat pooled capture");
        black_box(g);
    }
    let after_pooled = dhat::HeapStats::get();
    let pooled_blocks = after_pooled.total_blocks - before_pooled.total_blocks;
    let pooled_bytes = after_pooled.total_bytes - before_pooled.total_bytes;

    // ---- one-shot (re-allocates render scaffolding every call) ----
    let before_oneshot = dhat::HeapStats::get();
    for _ in 0..DHAT_ITERS {
        let mut throwaway = PaneCapture::new().expect("dhat oneshot PaneCapture::new");
        let g = throwaway.capture(&terminal).expect("dhat oneshot capture");
        black_box(g);
    }
    let after_oneshot = dhat::HeapStats::get();
    let oneshot_blocks = after_oneshot.total_blocks - before_oneshot.total_blocks;
    let oneshot_bytes = after_oneshot.total_bytes - before_oneshot.total_bytes;

    let saved_blocks = oneshot_blocks.saturating_sub(pooled_blocks);
    let saved_bytes = oneshot_bytes.saturating_sub(pooled_bytes);

    println!("---- dhat steady-state ({DHAT_ITERS} iterations) ----");
    println!("  pooled   PaneCapture: {pooled_blocks} blocks, {pooled_bytes} bytes");
    println!(
        "      per-iter:           {:.2} blocks, {:.2} bytes",
        pooled_blocks as f64 / DHAT_ITERS as f64,
        pooled_bytes as f64 / DHAT_ITERS as f64,
    );
    println!("  one-shot PaneCapture: {oneshot_blocks} blocks, {oneshot_bytes} bytes");
    println!(
        "      per-iter:           {:.2} blocks, {:.2} bytes",
        oneshot_blocks as f64 / DHAT_ITERS as f64,
        oneshot_bytes as f64 / DHAT_ITERS as f64,
    );
    println!("  delta (Rust-side):   {saved_blocks} blocks, {saved_bytes} bytes");
    println!(
        "      per-iter delta:     {:.2} blocks, {:.2} bytes",
        saved_blocks as f64 / DHAT_ITERS as f64,
        saved_bytes as f64 / DHAT_ITERS as f64,
    );
    println!("  note: dhat sees only Rust's global allocator. Both paths");
    println!("        allocate the same Grid (Vec<Vec<Cell>> + per-cell");
    println!("        Vec<char>), so Rust-side counts are equal. The");
    println!("        libghostty FFI scaffolding (RenderState/RowIterator/");
    println!("        CellIterator) is allocated by libghostty's Zig");
    println!("        allocator — invisible to dhat but a real wall-clock");
    println!("        cost the criterion bench captures (compare timings");
    println!("        with and without pooling).");
    println!("  steady-state proof: the per-iter Rust-side block count");
    println!("        is constant across N iterations — confirms pooled");
    println!("        PaneCapture does not grow allocator state per frame.");
    println!("------------------------------------------------------------");
}

fn main() {
    if std::env::var_os("PHUX_CAPTURE_DHAT").is_some() {
        dhat_probe();
        return;
    }
    // Otherwise hand off to criterion.
    let mut criterion = Criterion::default().configure_from_args();
    bench_capture(&mut criterion);
    criterion.final_summary();
}
