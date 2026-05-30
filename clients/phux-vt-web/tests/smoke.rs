//! Drives the real ghostty-vt.wasm engine through the Rust driver under node:
//! create a terminal, feed VT bytes, read the grid back as text.

use phux_vt_web::Vt;
use wasm_bindgen_test::wasm_bindgen_test;

#[wasm_bindgen_test]
fn engine_wasm_is_embedded() {
    assert!(phux_vt_web::engine_wasm_len() > 1_000_000);
}

#[wasm_bindgen_test]
async fn writes_and_reads_back_text() {
    let vt = Vt::load().await.expect("load ghostty-vt engine");
    let term = vt.terminal(20, 5);

    term.write(b"Hello, phux");
    let rows = term.rows_text();

    assert!(!rows.is_empty(), "expected rows, got none");
    assert!(
        rows[0].starts_with("Hello, phux"),
        "row 0 should contain the written text; got {:?}",
        rows[0],
    );
}

#[wasm_bindgen_test]
async fn handles_cursor_movement_and_overwrite() {
    let vt = Vt::load().await.expect("load ghostty-vt engine");
    let term = vt.terminal(20, 5);

    // Write, carriage-return to column 0, overwrite the first char.
    term.write(b"world\r");
    term.write(b"W");
    let rows = term.rows_text();
    assert_eq!(rows[0], "World", "CR + overwrite; got {:?}", rows[0]);
}
