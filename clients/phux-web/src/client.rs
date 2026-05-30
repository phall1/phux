//! Browser glue: a WebSocket to the phux server, a `<canvas>`, and the keyboard,
//! all driving a [`Session`]. This is the only part that touches the DOM/WS; the
//! protocol logic lives in [`crate::session`].

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use phux_protocol::input::key::{KeyAction, KeyEvent, ModSet, PhysicalKey};
use phux_protocol::wire::frame::FrameKind;
use phux_vt_web::Vt;
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;
use web_sys::{
    BinaryType, CanvasRenderingContext2d, HtmlCanvasElement, KeyboardEvent, MessageEvent, WebSocket,
};

use crate::{Metrics, render};

/// Connect to a phux server over WebSocket and render the attached terminal
/// into the given canvas, routing keyboard input back. Resolves once wired up;
/// the handlers then run for the connection's lifetime.
///
/// # Errors
/// Fails if the engine can't load, the canvas has no 2D context, or the
/// WebSocket can't be opened.
pub async fn run(
    ws_url: &str,
    canvas: HtmlCanvasElement,
    cols: u16,
    rows: u16,
) -> Result<Client, JsValue> {
    let vt = Vt::load().await?;
    let ctx: CanvasRenderingContext2d = canvas
        .get_context("2d")?
        .ok_or_else(|| JsValue::from_str("no 2D context"))?
        .dyn_into()?;

    let ws = WebSocket::new(ws_url)?;
    ws.set_binary_type(BinaryType::Arraybuffer);

    let app = Rc::new(RefCell::new(App {
        session: crate::Session::new(&vt, cols, rows),
        ws: ws.clone(),
        canvas,
        ctx,
        metrics: Metrics::default(),
        cursor_on: Cell::new(true),
    }));

    // On open: send HELLO + ATTACH.
    {
        let app = Rc::clone(&app);
        let onopen = Closure::<dyn FnMut()>::new(move || {
            let a = app.borrow();
            a.send(a.session.handshake());
        });
        ws.set_onopen(Some(onopen.as_ref().unchecked_ref()));
        onopen.forget();
    }

    // On message: decode one frame, drive the session, ack, repaint.
    {
        let app = Rc::clone(&app);
        let onmessage = Closure::<dyn FnMut(MessageEvent)>::new(move |e: MessageEvent| {
            let buf = js_sys::Uint8Array::new(&e.data()).to_vec();
            let Ok((frame, _rest)) = FrameKind::decode(&buf) else {
                return;
            };
            let mut a = app.borrow_mut();
            let outcome = a.session.on_frame(frame);
            if !outcome.send.is_empty() {
                a.send(outcome.send);
            }
            if outcome.render {
                a.paint();
            }
        });
        ws.set_onmessage(Some(onmessage.as_ref().unchecked_ref()));
        onmessage.forget();
    }

    // Keyboard: each keydown becomes an INPUT_KEY for the attached terminal.
    {
        let app = Rc::clone(&app);
        let document = web_sys::window()
            .and_then(|w| w.document())
            .ok_or_else(|| JsValue::from_str("no document"))?;
        let onkey = Closure::<dyn FnMut(KeyboardEvent)>::new(move |e: KeyboardEvent| {
            let Some(event) = key_event_from_browser(&e) else {
                return;
            };
            let a = app.borrow();
            if let Some(frame) = a.session.key_frame(event) {
                let _ = a.ws.send_with_u8_array(&frame);
                e.prevent_default();
            }
        });
        document.add_event_listener_with_callback("keydown", onkey.as_ref().unchecked_ref())?;
        onkey.forget();
    }

    // Cursor blink: toggle the phase and repaint on a fixed cadence.
    {
        let app = Rc::clone(&app);
        let window = web_sys::window().ok_or_else(|| JsValue::from_str("no window"))?;
        let blink = Closure::<dyn FnMut()>::new(move || {
            let a = app.borrow();
            a.cursor_on.set(!a.cursor_on.get());
            a.paint();
        });
        window.set_interval_with_callback_and_timeout_and_arguments_0(
            blink.as_ref().unchecked_ref(),
            530,
        )?;
        blink.forget();
    }

    Ok(Client { app })
}

/// A live connection handle. The event handlers run for the connection's
/// lifetime; this lets a caller (or test) inspect the rendered grid.
pub struct Client {
    app: Rc<RefCell<App>>,
}

impl Client {
    /// The current styled grid as one `String` per row (for inspection/tests).
    #[must_use]
    pub fn rows_text(&self) -> Vec<String> {
        let grid = self.app.borrow().session.grid();
        let cols = usize::from(grid.cols);
        grid.cells
            .chunks(cols.max(1))
            .map(|row| row.iter().map(|c| c.ch).collect::<String>())
            .collect()
    }
}

struct App {
    session: crate::Session,
    ws: WebSocket,
    canvas: HtmlCanvasElement,
    ctx: CanvasRenderingContext2d,
    metrics: Metrics,
    /// Cursor blink phase; toggled by an interval in `run`.
    cursor_on: Cell<bool>,
}

impl App {
    fn send(&self, frames: Vec<Vec<u8>>) {
        for f in frames {
            let _ = self.ws.send_with_u8_array(&f);
        }
    }

    fn paint(&self) {
        let grid = self.session.grid();
        // Keep the canvas sized to the grid (handles server-side resizes).
        let w = u32::from(grid.cols) * (self.metrics.cell_w as u32);
        let h = u32::from(grid.rows) * (self.metrics.cell_h as u32);
        if self.canvas.width() != w {
            self.canvas.set_width(w);
        }
        if self.canvas.height() != h {
            self.canvas.set_height(h);
        }
        render(&self.ctx, &grid, &self.metrics, self.cursor_on.get());
    }
}

/// Map a browser `KeyboardEvent` to a wire `KeyEvent`. Returns `None` for
/// modifier-only keydowns (which carry no terminal input on their own).
fn key_event_from_browser(e: &KeyboardEvent) -> Option<KeyEvent> {
    let key = code_to_physical_key(&e.code());

    let mut mods = ModSet::empty();
    if e.ctrl_key() {
        mods |= ModSet::CTRL;
    }
    if e.shift_key() {
        mods |= ModSet::SHIFT;
    }
    if e.alt_key() {
        mods |= ModSet::ALT;
    }
    if e.meta_key() {
        mods |= ModSet::SUPER;
    }

    // `key()` is the produced character; carry it as text for printable keys
    // (single char, no Ctrl/Meta). Named keys ("Enter", "Shift", …) are >1 char.
    let produced = e.key();
    if produced == "Shift" || produced == "Control" || produced == "Alt" || produced == "Meta" {
        return None;
    }
    let text =
        (produced.chars().count() == 1 && !e.ctrl_key() && !e.meta_key()).then_some(produced);

    Some(KeyEvent {
        action: KeyAction::Press,
        key,
        mods,
        consumed_mods: ModSet::empty(),
        composing: false,
        text,
        unshifted_codepoint: None,
    })
}

/// Map a W3C `KeyboardEvent.code` to libghostty's physical-key discriminant.
/// `KeyA`–`KeyZ` and `Digit0`–`Digit9` map arithmetically; the rest by name.
fn code_to_physical_key(code: &str) -> PhysicalKey {
    use PhysicalKey as K;

    if let Some(c) = code.strip_prefix("Key").and_then(|s| s.chars().next())
        && c.is_ascii_uppercase()
    {
        return PhysicalKey::try_from(20 + (c as u32 - u32::from(b'A'))).unwrap_or(K::Unidentified);
    }
    if let Some(d) = code.strip_prefix("Digit").and_then(|s| s.chars().next())
        && d.is_ascii_digit()
    {
        return PhysicalKey::try_from(6 + (d as u32 - u32::from(b'0'))).unwrap_or(K::Unidentified);
    }

    match code {
        "Enter" | "NumpadEnter" => K::Enter,
        "Backspace" => K::Backspace,
        "Tab" => K::Tab,
        "Space" => K::Space,
        "Escape" => K::Escape,
        "ArrowUp" => K::ArrowUp,
        "ArrowDown" => K::ArrowDown,
        "ArrowLeft" => K::ArrowLeft,
        "ArrowRight" => K::ArrowRight,
        "Home" => K::Home,
        "End" => K::End,
        "PageUp" => K::PageUp,
        "PageDown" => K::PageDown,
        "Delete" => K::Delete,
        "Insert" => K::Insert,
        "Minus" => K::Minus,
        "Equal" => K::Equal,
        "Period" => K::Period,
        "Comma" => K::Comma,
        "Slash" => K::Slash,
        "Semicolon" => K::Semicolon,
        "Quote" => K::Quote,
        "Backslash" => K::Backslash,
        "BracketLeft" => K::BracketLeft,
        "BracketRight" => K::BracketRight,
        "Backquote" => K::Backquote,
        _ => K::Unidentified,
    }
}
