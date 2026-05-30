//! phux-web — the phux browser client.
//!
//! Renders terminals to a `<canvas>` using the ghostty-vt engine (via
//! [`phux_vt_web`]), and (subsequent milestones) speaks the phux wire over a
//! WebSocket and routes keyboard input back. This module currently provides the
//! canvas renderer over the engine's styled [`Grid`].

#![deny(missing_docs)]

pub mod client;
pub mod session;

pub use session::{Outcome, Session};

use phux_vt_web::{Grid, Rgb};
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;
use web_sys::CanvasRenderingContext2d;

/// JS entry point: connect to `ws_url` and render the attached terminal into the
/// canvas element with id `canvas_id`, sized `cols`×`rows`.
///
/// # Errors
/// Fails if the canvas element is missing or the connection can't be set up.
#[wasm_bindgen]
pub async fn start(
    ws_url: String,
    canvas_id: String,
    cols: u16,
    rows: u16,
) -> Result<(), JsValue> {
    let canvas = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.get_element_by_id(&canvas_id))
        .ok_or_else(|| JsValue::from_str("canvas element not found"))?
        .dyn_into::<web_sys::HtmlCanvasElement>()?;
    client::run(&ws_url, canvas, cols, rows).await
}

/// Auto-start when served as a page: read the server URL from `?ws=…`
/// (defaulting to `ws://<host>/session`) and attach into `#phux-term`.
#[wasm_bindgen(start)]
pub fn main() {
    wasm_bindgen_futures::spawn_local(async {
        if let Err(err) = auto_start().await {
            web_sys::console::error_1(&err);
        }
    });
}

async fn auto_start() -> Result<(), JsValue> {
    let window = web_sys::window().ok_or_else(|| JsValue::from_str("no window"))?;
    let location = window.location();
    let search = location.search().unwrap_or_default();
    let ws_url = ws_from_query(&search)
        .unwrap_or_else(|| format!("ws://{}/session", location.host().unwrap_or_default()));
    start(ws_url, "phux-term".to_owned(), 80, 24).await
}

/// Extract a `ws=` query parameter, decoding the `:` and `/` a browser escapes.
fn ws_from_query(search: &str) -> Option<String> {
    search
        .trim_start_matches('?')
        .split('&')
        .find_map(|kv| kv.strip_prefix("ws="))
        .map(|v| v.replace("%3A", ":").replace("%2F", "/").replace("%2f", "/"))
}

/// Cell geometry + font for the canvas renderer. A monospace cell grid: every
/// cell is `cell_w`×`cell_h` device pixels.
#[derive(Clone, Debug)]
pub struct Metrics {
    /// Cell width in pixels.
    pub cell_w: f64,
    /// Cell height in pixels.
    pub cell_h: f64,
    /// CSS font string, e.g. `"14px monospace"`.
    pub font: String,
}

impl Default for Metrics {
    fn default() -> Self {
        Self {
            cell_w: 8.0,
            cell_h: 16.0,
            font: "14px monospace".to_owned(),
        }
    }
}

/// Paint a [`Grid`] onto a 2D canvas context: a background rect per cell, then
/// the glyph in its resolved foreground. Cells fall back to the grid defaults.
pub fn render(ctx: &CanvasRenderingContext2d, grid: &Grid, m: &Metrics) {
    ctx.set_font(&m.font);
    ctx.set_text_baseline("top");

    let cols = usize::from(grid.cols);
    for row in 0..usize::from(grid.rows) {
        for col in 0..cols {
            let Some(cell) = grid.cells.get(row * cols + col) else {
                continue;
            };
            let x = col as f64 * m.cell_w;
            let y = row as f64 * m.cell_h;

            let bg = cell.bg.unwrap_or(grid.default_bg);
            ctx.set_fill_style_str(&css(bg));
            ctx.fill_rect(x, y, m.cell_w, m.cell_h);

            if cell.ch != ' ' && cell.ch != '\0' {
                let fg = cell.fg.unwrap_or(grid.default_fg);
                ctx.set_fill_style_str(&css(fg));
                let mut buf = [0u8; 4];
                let _ = ctx.fill_text(cell.ch.encode_utf8(&mut buf), x, y);
            }
        }
    }
}

fn css(c: Rgb) -> String {
    format!("rgb({} {} {})", c.r, c.g, c.b)
}
