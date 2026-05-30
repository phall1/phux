//! Full browser end-to-end (headless Chrome): connect to a LIVE phux server
//! over WebSocket, attach, and render the seeded session's content to a canvas.
//!
//! Requires the seeded `ws_demo_server` example running on 127.0.0.1:47654
//! (the test harness starts it). The marker `PHUX_WEB_OK` printed by the seed
//! PTY must appear in the rendered grid.

use std::time::Duration;

use gloo_timers::future::sleep;
use wasm_bindgen::JsCast;
use wasm_bindgen_test::{wasm_bindgen_test, wasm_bindgen_test_configure};
use web_sys::HtmlCanvasElement;

wasm_bindgen_test_configure!(run_in_browser);

const WS_URL: &str = "ws://127.0.0.1:47654/";

#[wasm_bindgen_test]
async fn connects_to_live_server_and_renders_seed() {
    let document = web_sys::window().unwrap().document().unwrap();
    let canvas: HtmlCanvasElement = document
        .create_element("canvas")
        .unwrap()
        .dyn_into()
        .unwrap();

    let client = phux_web::client::run(WS_URL, canvas, 80, 24)
        .await
        .expect("connect to live phux server");

    // Poll until the seed PTY's marker renders (HELLO -> ATTACH -> SNAPSHOT/
    // OUTPUT all flow over the real wire), up to ~6s.
    for _ in 0..120 {
        if client
            .rows_text()
            .iter()
            .any(|row| row.contains("PHUX_WEB_OK"))
        {
            return; // rendered live content from the server — success.
        }
        sleep(Duration::from_millis(50)).await;
    }

    panic!(
        "seed marker never rendered; grid was:\n{}",
        client.rows_text().join("\n")
    );
}
