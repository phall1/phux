//! `diagnose` — connect to a running phux server, send a probe command to
//! a fixed terminal id, and print the last few screen lines.
//!
//! Usage:
//!   cargo run -p phux --example diagnose

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

    let mut agent = match Agent::connect_uds(terminal_id.clone(), socket_path).await {
        Ok(a) => a,
        Err(e) => {
            eprintln!("Failed: {e:?}");
            return;
        }
    };

    // Issue diagnostic commands
    println!("=== ISSUING DIAGNOSTICS ===\n");

    // Check TERM via the side-effect-free ROUTE_INPUT path (no attach).
    let keys = ["echo TERM=$TERM".to_owned(), "Enter".to_owned()];
    if let Err(e) = phux_client::send_keys::send_to(socket_path, terminal_id.clone(), &keys).await {
        eprintln!("send_keys failed: {e:?}");
        return;
    }

    tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

    let state = agent.get_state().await.ok();
    if let Some(s) = state {
        println!("After 'echo TERM=$TERM':");
        for line in s.lines.iter().skip(s.lines.len().saturating_sub(5)) {
            if !line.is_empty() {
                println!("  {line}");
            }
        }
    }
}
