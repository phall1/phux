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

    // Issue diagnostic commands
    println!("=== ISSUING DIAGNOSTICS ===\n");
    
    // Check TERM
    agent.send_keys_str("echo TERM=$TERM").await.ok();
    agent.send_keys_str("Enter").await.ok();
    
    tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
    
    let state = agent.get_state().await.ok();
    if let Some(s) = state {
        println!("After 'echo TERM=$TERM':");
        for line in s.lines.iter().skip(s.lines.len().saturating_sub(5)) {
            if !line.is_empty() {
                println!("  {}", line);
            }
        }
    }
}
