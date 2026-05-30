//! phux-web binary entry (Trunk builds this).
//!
//! On load: read the server URL from `?ws=…` (defaulting to
//! `ws://<host>/session`) and attach the terminal into `#phux-term`.

use wasm_bindgen::JsValue;

fn main() {
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
    phux_web::start(ws_url, "phux-term".to_owned(), 80, 24).await
}

/// Extract a `ws=` query parameter, decoding the `:` and `/` a browser escapes.
fn ws_from_query(search: &str) -> Option<String> {
    search
        .trim_start_matches('?')
        .split('&')
        .find_map(|kv| kv.strip_prefix("ws="))
        .map(|v| {
            v.replace("%3A", ":")
                .replace("%2F", "/")
                .replace("%2f", "/")
        })
}
