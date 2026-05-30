---
audience: consumers, contributors, agents
stability: evolving
last-reviewed: 2026-05-30
---

# The phux web client

**TL;DR.** `phux-web` is a browser consumer of the wire ([ADR-0025]), written in
Rust→WASM. It speaks the exact `phux-protocol` codec and renders with the exact
`libghostty-vt` engine (`ghostty-vt.wasm`, driven from Rust via `phux-vt-web`) —
not a JS terminal, not a reimplementation. It connects to the server's WebSocket
transport, attaches to a `default` session, and paints one terminal (with a
blinking cursor) to a `<canvas>`; keystrokes flow back as `INPUT_KEY`. Verified
end-to-end in headless Chrome. Single-terminal today; splits/layout are out of
scope (that's the TUI).

---

## What it is

A peer consumer alongside the reference TUI and the (forthcoming) agent SDK
([ADR-0017]): same wire, different projection. The TUI projects the terminal to
VT bytes on a real tty; the web client projects it to a canvas grid in a browser.

Two crates make it up, plus one vendored artifact:

| Piece (`clients/`) | Language | Role |
|---|---|---|
| `ghostty-vt.wasm` | Zig (ghostty's) | The **VT engine** itself — parses escape codes, holds the grid + cursor. A vendored build artifact, not phux code. |
| `phux-vt-web` | Rust | A safe **driver** over the engine's C ABI: make a `Terminal`, `write` VT bytes, read a styled `Grid` (cells + fg/bg + cursor). Depends on nothing phux. |
| `phux-web` | Rust | The **client**: a WebSocket `Session` over the engine + a `<canvas>` renderer (cursor blink) + keyboard, plus the `#[wasm_bindgen]` `start()` entry. |

## The two-wasm architecture

The non-obvious part: **there are two wasm modules, one nested inside the
other.** The engine module is self-contained — its only import is `env.log`, and
it ships its own allocator — so `phux-vt-web` loads it through the plain
`WebAssembly` JS API, with **no Zig linked into the Rust wasm binary**. Linking
them would mean sharing one wasm linear memory between two toolchains; instead
the Rust module runs the engine as a **sibling instance** and copies bytes across
the boundary ([ADR-0025]).

```text
phux_web_bg.wasm  (Rust client)
   └─ embeds, then instantiates ──►  ghostty-vt.wasm  (Zig engine)
        two wasm instances live in the tab; bytes cross the boundary
```

## Dependency chain (build time)

```text
ghostty (Zig source)
   │  zig build -Demit-lib-vt        (scripts/build-vt-wasm.sh)
   ▼
ghostty-vt.wasm  ── vendored into clients/phux-vt-web/vendor/
   │  include_bytes!                 (baked into the Rust binary)
   ▼
phux-vt-web ──┐
              ├──►  phux-web  ──wasm-pack build──►  phux_web_bg.wasm + phux_web.js
phux-protocol ┤                                          │
web-sys ──────┘                                          ▼  (e.g. consumed by phux-site)
                                              <import init, { start }>
```

`phux-protocol` is the same wire codec the server uses — it became wasm-safe in
[ADR-0024] (the wire owns its input atoms, so the codec no longer pulls in
libghostty on the client).

## Runtime flow

1. Browser loads `phux_web.js` (glue) + `phux_web_bg.wasm` (the Rust client).
2. The client boots and instantiates the embedded `ghostty-vt.wasm` (the engine).
3. Open a WebSocket to the server (`PHUX_WS_ADDR`); one binary message carries one
   encoded `FrameKind` — the same wire UDS uses.
4. Send `HELLO`, then `ATTACH` (`CreateIfMissing` the `default` session).
5. On `TERMINAL_SNAPSHOT` / `TERMINAL_OUTPUT`, `vt_write` the bytes into the
   engine and repaint the grid; ack output with `FRAME_ACK`. A 530 ms interval
   toggles the cursor blink.
6. On `keydown`, map `KeyboardEvent.code` to a `PhysicalKey`, build a `KeyEvent`,
   and send `INPUT_KEY` for the attached terminal.

## Building

Two steps, because of the two modules. Run inside the phux **nix devshell**,
which provides the entire toolchain (`nix develop`):

```sh
# 1. Build the engine artifact (once; regenerate when ghostty bumps). Needs zig.
scripts/build-vt-wasm.sh
#    → clients/phux-vt-web/vendor/ghostty-vt.wasm   (gitignored)

# 2. Build the client to a web package.
cd clients/phux-web && wasm-pack build --target web --release --out-dir pkg
#    → pkg/phux_web.js + pkg/phux_web_bg.wasm  (~6 MB; engine included)
```

`build.rs` in `phux-vt-web` fails with a clear message if step 1 hasn't run.

### Build dependencies

| Tool | Version | Provided by | For |
|---|---|---|---|
| Rust | 1.90.0 | `rust-toolchain.toml` (targets `wasm32-unknown-unknown`) | both crates |
| Zig | 0.15.x | nix devshell (`zig_0_15`) | building `ghostty-vt.wasm` from ghostty |
| `wasm-pack` | 0.15 | nix devshell | packaging the client |
| `wasm-bindgen` / `wasm-bindgen-cli` | **`=0.2.121`** | crate pin + nix devshell | bindings — the crate pin **must** equal the CLI version |
| `binaryen` (`wasm-opt`) | nixpkgs | nix devshell | release size optimization |
| `chromedriver` + Chrome | matched majors | nix devshell + system Chrome | `--headless --chrome` tests only |

The ghostty checkout should be pinned to the same revision `libghostty-vt-sys`
uses (see the native crates' `Cargo.toml`), so the browser engine matches the
one native phux links.

## Running it locally

```sh
# A standalone seeded server to point a build at:
PHUX_WS_ADDR=127.0.0.1:47654 cargo run -p phux-server --example ws_demo_server
```

Then serve the `pkg/` output and call `start("ws://127.0.0.1:47654/", canvasId,
cols, rows)`. The phux-site repo wires this into a `<PhuxTerminal>` island; see
its `scripts/build-client.sh` for the copy-the-artifact step.

## Scope and limits

- **Single terminal.** No splits, windows, or layout chrome — that's the TUI's
  job. The web client mirrors one terminal.
- **Text + color + cursor.** The canvas renderer paints grapheme cells with
  fg/bg and a blinking block cursor. Images/sixel (which the engine *does* parse)
  are a future renderer pass.
- **Engine boundary copies.** Bytes cross two wasm linear memories (the Rust
  client and `ghostty-vt.wasm`); fine for terminal traffic.

## Verification

- `phux-vt-web` — `wasm-pack test --node`: drives the real engine, reads the grid
  back, decodes a truecolor cell + the cursor.
- `phux-web` — `wasm-pack test --node`: a real `TERMINAL_OUTPUT` frame
  (round-tripped through the codec) feeds the engine and acks.
- Renderer + full client — `wasm-pack test --headless --chrome`: engine→grid→
  canvas pixel test, and a live connect-to-server-and-render end-to-end test
  against `ws_demo_server`.
- Server side — `phux-server` `ws_attach` test: a real client does
  `HELLO`→`ATTACH` over WebSocket and receives `ATTACHED` + `TERMINAL_SNAPSHOT`.

[ADR-0017]: ../../ADR/0017-tui-not-protocol-privileged.md
[ADR-0024]: ../../ADR/0024-wire-owns-input-atoms.md
[ADR-0025]: ../../ADR/0025-browser-web-client.md
