---
audience: consumers, contributors, agents
stability: evolving
last-reviewed: 2026-05-30
---

# The phux web client

**TL;DR.** `phux-web` is a browser consumer of the wire (ADR-0025), written in
Rust→WASM. It speaks the exact `phux-protocol` codec and renders with the exact
`libghostty-vt` engine (`ghostty-vt.wasm`, driven from Rust via `phux-vt-web`) —
not a JS terminal, not a reimplementation. It connects to the server's WebSocket
transport, attaches to a `default` session, and paints one terminal to a
`<canvas>`; keystrokes flow back as `INPUT_KEY`. Verified end-to-end in headless
Chrome. Single-terminal today; splits/layout are out of scope (that's the TUI).

---

## What it is

A peer consumer alongside the reference TUI and the (forthcoming) agent SDK
(ADR-0017): same wire, different projection. The TUI projects the terminal to VT
bytes on a real tty; the web client projects it to a canvas grid in a browser.

Three crates make it up:

| Crate (`clients/`) | Role |
|---|---|
| `phux-vt-web` | Loads `ghostty-vt.wasm` and drives its `libghostty-vt` C ABI from Rust: create a `Terminal`, `write` VT bytes, `resize`, read a styled `Grid` (cells + fg/bg). |
| `phux-web` | The client: a WebSocket `Session` over the engine + a `<canvas>` renderer + keyboard input, plus the Trunk `main`. |

The engine module is self-contained — its only import is `env.log`, and it ships
its own allocator — so it loads via the WebAssembly JS API with no zig linked
into the Rust wasm binary (ADR-0025).

## The flow

1. Open a WebSocket to the server (`PHUX_WS_ADDR`); one binary message carries one
   encoded `FrameKind` (the same wire UDS uses).
2. Send `HELLO`, then `ATTACH` (`CreateIfMissing` the `default` session).
3. On `TERMINAL_SNAPSHOT` / `TERMINAL_OUTPUT`, `vt_write` the bytes into the
   engine terminal and repaint the grid; ack output with `FRAME_ACK`.
4. On `keydown`, map `KeyboardEvent.code` to a `PhysicalKey`, build a `KeyEvent`,
   and send `INPUT_KEY` for the attached terminal.

## Running it

```sh
# Build the deployable app (dist/: index.html + wasm + JS glue).
cd clients/phux-web && trunk build --release

# A standalone seeded server to point it at:
PHUX_WS_ADDR=127.0.0.1:47654 cargo run -p phux-server --example ws_demo_server

# Serve dist/ and open it with the server URL:
#   index.html?ws=ws://127.0.0.1:47654/
```

The engine wasm is a build artifact: run `scripts/build-vt-wasm.sh` (needs zig)
to (re)generate `clients/phux-vt-web/vendor/ghostty-vt.wasm` before building.

## Scope and limits

- **Single terminal.** No splits, windows, or layout chrome — that's the TUI's
  job. The web client mirrors one terminal.
- **Text + color.** The canvas renderer paints grapheme cells with fg/bg.
  Images/sixel (which the engine *does* parse) are a future renderer pass.
- **Engine boundary copies.** Bytes cross two wasm linear memories (the Rust
  client and `ghostty-vt.wasm`); fine for terminal traffic.

## Verification

- `phux-vt-web` — `wasm-pack test --node`: drives the real engine, reads the grid
  back, decodes a truecolor cell.
- `phux-web` — `wasm-pack test --node`: a real `TERMINAL_OUTPUT` frame
  (round-tripped through the codec) feeds the engine and acks.
- Renderer + full client — `wasm-pack test --headless --chrome`: engine→grid→
  canvas pixel test, and a live connect-to-server-and-render end-to-end test
  against `ws_demo_server`.
- Server side — `phux-server` `ws_attach` test: a real client does
  `HELLO`→`ATTACH` over WebSocket and receives `ATTACHED` + `TERMINAL_SNAPSHOT`.
