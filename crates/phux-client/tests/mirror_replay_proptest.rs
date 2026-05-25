//! End-to-end replay-invariant proptest.
//!
//! Generates random VT byte sequences, feeds them to `libghostty_vt::Terminal`,
//! captures the server-side authoritative pre/post grids via
//! `phux_server::grid::capture`, computes the diff with
//! `phux_protocol::compute_diff`, and asserts that applying the same diff to
//! a client-side [`DiffMirror`] initialised at the pre-state yields the same
//! post-state byte-for-byte.
//!
//! This is the same round-trip `phux-server/examples/diff_spike.rs` asserts
//! at the protocol layer, but using the client's `DiffMirror` as the applier.

#![allow(clippy::unwrap_used, reason = "tests")]
#![allow(clippy::expect_used, reason = "tests")]
#![allow(clippy::panic, reason = "tests assert via panic")]

use libghostty_vt::{Terminal, TerminalOptions};
use phux_client::DiffMirror;
use phux_protocol::compute_diff;
use phux_server::grid;
use proptest::prelude::*;

/// Build a short, mostly-printable byte stream with occasional control
/// sequences, biased toward shapes the diff algorithm exercises (text,
/// resets, newlines, clears, cursor moves).
fn arb_vt_bytes() -> impl Strategy<Value = Vec<u8>> {
    // Each "chunk" is either a printable run or a small VT directive.
    let printable = proptest::collection::vec(0x20u8..0x7Eu8, 1..8);
    let chunk = prop_oneof![
        printable.prop_map(|v| v),
        Just(b"\r\n".to_vec()),
        Just(b"\n".to_vec()),
        Just(b"\x1b[H".to_vec()),
        Just(b"\x1b[2J".to_vec()),
        Just(b"\x1b[1;31m".to_vec()),
        Just(b"\x1b[1;32m".to_vec()),
        Just(b"\x1b[4m".to_vec()),
        Just(b"\x1b[0m".to_vec()),
        Just(b"\x1b[K".to_vec()),
        (1u8..6u8, 1u8..10u8).prop_map(|(r, c)| {
            // CUP — cursor position (1-indexed).
            format!("\x1b[{r};{c}H").into_bytes()
        }),
    ];
    proptest::collection::vec(chunk, 1..6).prop_map(|chunks| chunks.into_iter().flatten().collect())
}

proptest! {
    // Keep the case count modest: each case spins up a libghostty Terminal,
    // which is not free. 64 cases is enough to find regressions and still
    // fits the 1m default test budget comfortably.
    #![proptest_config(ProptestConfig { cases: 64, ..ProptestConfig::default() })]

    #[test]
    fn client_mirror_matches_server_grid_after_diff(
        pre_bytes in arb_vt_bytes(),
        post_bytes in arb_vt_bytes(),
    ) {
        // Small grid keeps capture cheap and exposes wrap / scroll behavior
        // sooner.
        let rows: u16 = 6;
        let cols: u16 = 12;

        let mut terminal = Terminal::new(TerminalOptions {
            cols,
            rows,
            max_scrollback: 64,
        }).expect("Terminal::new");

        // Feed the "pre" sequence and capture G0.
        terminal.vt_write(&pre_bytes);
        let g0 = grid::capture(&terminal).expect("capture g0");

        // Feed more and capture G1.
        terminal.vt_write(&post_bytes);
        let g1 = grid::capture(&terminal).expect("capture g1");

        // Compute the canonical diff and replay on the client mirror.
        let ops = compute_diff(&g0, &g1);

        let mut mirror = DiffMirror::new(rows, cols);
        // Seed the mirror with G0 via the snapshot path — same as a real
        // client attaching after the server has already drawn into the
        // pane.
        mirror.ingest_snapshot(&g0, 0);
        mirror.apply(&ops);

        prop_assert_eq!(
            &mirror.grid, &g1,
            "client mirror diverged from server grid after diff replay",
        );
    }
}
