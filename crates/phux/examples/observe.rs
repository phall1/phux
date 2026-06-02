use std::path::Path;
use phux_client::agent::Agent;
use phux_protocol::ids::TerminalId;

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let socket_path = Path::new("/tmp/phux-phall/phux.sock");
    let terminal_id = TerminalId::local(1);

    let mut agent = match Agent::connect_uds(terminal_id, socket_path).await {
        Ok(a) => a,
        Err(e) => {
            eprintln!("Failed: {:?}", e);
            return;
        }
    };

    let state = match agent.get_state().await {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Failed: {:?}", e);
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
            println!("[{:2}] {}", idx, line);
        }
    }

    println!("\n=== SCROLLBACK ===");
    println!("Scrollback lines: {}", state.scrollback.len());
    for (idx, line) in state.scrollback.iter().take(5).enumerate() {
        println!("[SB {}] {}", idx, line);
    }
}
