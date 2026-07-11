//! In-browser render test (headless Chrome): drive the real ghostty-vt engine,
//! read the styled grid, paint it to a real canvas, and read a pixel back.

use phux_vt_web::Vt;
use phux_web::{Metrics, render};
use wasm_bindgen::JsCast;
use wasm_bindgen_test::{wasm_bindgen_test, wasm_bindgen_test_configure};
use web_sys::{CanvasRenderingContext2d, HtmlCanvasElement};

wasm_bindgen_test_configure!(run_in_browser);

#[wasm_bindgen_test]
async fn renders_engine_grid_to_canvas() {
    let vt = Vt::load().await.expect("load ghostty-vt engine");
    let term = vt.terminal(4, 2);
    // A red-background space at cell (0,0).
    term.write(b"\x1b[48;2;255;0;0m \x1b[0m");
    let grid = term.grid();

    let document = web_sys::window().unwrap().document().unwrap();
    let canvas: HtmlCanvasElement = document
        .create_element("canvas")
        .unwrap()
        .dyn_into()
        .unwrap();
    canvas.set_width(64);
    canvas.set_height(64);
    let ctx: CanvasRenderingContext2d = canvas
        .get_context("2d")
        .unwrap()
        .unwrap()
        .dyn_into()
        .unwrap();

    let m = Metrics {
        cell_w: 10.0,
        cell_h: 16.0,
        font: "14px monospace".to_owned(),
    };
    render(&ctx, &grid, &m, false);

    // Sample a pixel inside cell (0,0): it should be the red background.
    let pixel = ctx.get_image_data(3, 3, 1, 1).unwrap();
    let d = pixel.data();
    assert!(
        d[0] > 200 && d[1] < 60 && d[2] < 60,
        "cell(0,0) should render red bg; got rgba({},{},{},{})",
        d[0],
        d[1],
        d[2],
        d[3],
    );
}
