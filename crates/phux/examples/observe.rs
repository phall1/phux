//! `observe` — connect to a running phux server and dump the current
//! screen state (grid + scrollback) for a fixed terminal id.
//!
//! Usage:
//!   cargo run -p phux --example observe

#![allow(
    clippy::print_stdout,
    clippy::print_stderr,
    clippy::expect_used,
    clippy::unwrap_used,
    reason = "standalone diagnostic harness, not library code"
)]

use phux_client::agent::Agent;
use phux_protocol::ids::TerminalId;
use std::path::Path;

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let socket_path = Path::new("/tmp/phux-phall/phux.sock");
    let terminal_id = TerminalId::local(1);

    let mut agent = match Agent::connect_uds(terminal_id, socket_path).await {
        Ok(a) => a,
        Err(e) => {
            eprintln!("Failed: {e:?}");
            return;
        }
    };

    let state = match agent.get_state().await {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Failed: {e:?}");
            return;
        }
    };

    println!("Dimensions: {}x{}", state.cols, state.rows);
    if let Some(c) = state.cursor {
        println!("Cursor: ({}, {})", c.x, c.y);
    }
    println!("Total lines: {}", state.lines.len());

    println!("\n=== ALL GRID LINES ===");
    for (idx, line) in state.lines.iter().enumerate() {
        if !line.is_empty() {
            println!("[{idx:2}] {line}");
        }
    }

    println!("\n=== SCROLLBACK ===");
    println!("Scrollback lines: {}", state.scrollback.len());
    for (idx, line) in state.scrollback.iter().take(5).enumerate() {
        println!("[SB {idx}] {line}");
    }
}
