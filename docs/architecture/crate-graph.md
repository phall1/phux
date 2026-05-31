---
audience: contributors, agents
stability: evolving
last-reviewed: 2026-05-30
---

# Crate dependency graph

**TL;DR.** The crate edges that hold phux together and the two
load-bearing boundaries: `phux-core` and `phux-protocol` never depend on
each other, and `phux-protocol` re-exports libghostty atoms directly.
Plus how each crate participates in the L1/L2/L3 wire layering from
ADR-0015.

---

```
                ┌──────────────────────────────────┐
                │              phux                │  binary; subcommands
                └─┬──────────┬───────────┬─────────┘
                  │          │           │
            ┌─────▼───┐  ┌───▼────┐  ┌───▼────┐
            │ server  │  │ client │  │ config │   client = chrome (ratatui)
            └──┬────┬─┘  └─┬──┬──┬─┘  └────────┘         + attach loop
               │    │      │  │  └────────┐
               │    │      │  │      ┌────▼───────┐  pane-interior substrate:
               │    │      │  │      │ client-core│  layout, multi-pane,
               │    │      │  │      └────┬───────┘  predict — NO ratatui
               │    │      │  └───────────┤          dep (ADR-0020, phux-0fv)
       ┌───────▼─┐  │   ┌──▼──────────────▼─┐    │
       │  core   │  └──►│     protocol      │──► libghostty-vt ◄┘
       └─────────┘      │ (codec, input     │     (client also links;
                        │  events, wire     │      runs a local Terminal
                        │  envelopes)       │      per attached pane —
                        └───────────────────┘      ADR-0013)
```

Two boundaries are load-bearing:

1. **`phux-core` and `phux-protocol` do not depend on each other.** Core
   holds the in-process domain (slotmap keys with generational tags,
   layout tree, registry). Protocol holds the wire shape (`u32`-wide
   IDs, length-prefixed TLV, libghostty-derived input/style atoms).
   The two ID spaces meet in `phux-server::id_bridge::IdBridge` and
   nowhere else; this isolates wire stability from in-process
   refactors and vice versa. See ADR-0011 for the full rationale.
2. **`phux-protocol` depends on `libghostty-vt` directly** (ADR-0008,
   gated by the `server` cargo feature). The protocol crate re-exports
   libghostty's input and style atoms instead of mirroring them. The
   default-features-off shell exists so `crates.io`/`docs.rs` see a
   git-dep-free surface (libghostty-vt is a git-only dep today).

`server` and `client` both depend on `protocol`. Both also depend on
`libghostty-vt` directly: the server's `Terminal` is the canonical
state for each pane and drives the structured-input encoders
(ADR-0006, ADR-0008); the client's `Terminal` is a local replica fed
by `PANE_OUTPUT` bytes for the panes that client has attached, with
`RenderState` providing per-row dirty tracking for efficient redraw.
See ADR-0013 and `../../research/2026-05-25-libghostty-renderstate.md`
for the renderer-side contract on both ends.

`phux-config` is a sibling of `core` and is consumed by the binary and
the client.

3. **`phux-client` is split from `phux-client-core` so the ratatui
   boundary is compiler-enforced** (ADR-0020, phux-0fv). The chrome
   toolkit (`ratatui`) lives only in `phux-client`; the pane-interior
   substrate — layout math, multi-pane composition, predictive echo —
   lives in `phux-client-core`, which declares no `ratatui` dependency.
   A `use ratatui` in the substrate cannot resolve, so it fails to
   compile. The attach loop stays in `phux-client` because it
   composites chrome over panes and depends on both. `phux-client`
   re-exports `phux_client_core::{layout, multi_pane, predict}` so
   consumers keep stable `phux_client::…` paths.

## Browser client crates (standalone wasm workspace)

The browser client lives under `clients/` as its **own cargo workspace**,
excluded from the native `cargo build --workspace` / CI (`exclude =
["clients"]` in the root manifest). It targets `wasm32-unknown-unknown` only
and never builds the native binary.

```
                    ghostty (Zig) ──zig build──► ghostty-vt.wasm
                                                      │ include_bytes!
   phux-protocol ──┐                                  ▼
   (default feats, ├──────────────►  phux-web ◄── phux-vt-web (engine driver)
    wasm-safe)     │   wasm-pack          │
   web-sys ────────┘                      ▼
                                  phux_web_bg.wasm + phux_web.js
```

- **`clients/phux-vt-web`** — a safe Rust driver over `ghostty-vt.wasm` (the
  VT engine, built from ghostty's Zig). Loaded as a **separate** wasm instance,
  not linked in (ADR-0025). Depends on nothing phux.
- **`clients/phux-web`** — the browser client: `phux-vt-web` (engine) +
  `phux-protocol` (the wire codec, **default-features** so it's libghostty-free
  and wasm-safe per ADR-0024) + `web-sys` (WebSocket/canvas/keyboard).

This is the one place `phux-protocol`'s default-features-off shell pays off: the
web client compiles the codec to wasm without the `server` feature's
libghostty-vt git dep. Full architecture + build steps: [the web client
consumer doc](../consumers/web.md).

## Protocol layering and this implementation

[ADR-0015](../../ADR/0015-protocol-layering.md) layers the wire into three
tiers plus two orthogonal cross-cuts. Mapping each onto code currently
in tree:

| Layer | Concept | Implemented in tree as | Status |
|---|---|---|---|
| **L1** | Terminal: PTY + libghostty `Terminal` + identity + I/O + snapshot + event stream | `PaneActor` in `phux-server::pane_actor`; wire `PaneId` and the `PANE_OUTPUT` / `PANE_SNAPSHOT` / `INPUT_*` / `BELL` / `OSC_EVENT` (currently spec-only) messages | shipped under pre-layering vocabulary; rename to `TerminalId` is ADR-0016 |
| **L2** | Collection: named lifecycle bundle of Terminals | `phux-core::Session` plus the session/window registries on `ServerState`; the "session" CLI noun | partial — session lifecycle today bundles windows AND terminals; ADR-0015 splits Collection (bundle of terminals) from the TUI's "windows" presentation |
| **L3** | Opaque metadata KV scoped to Terminal / Collection / global | not yet implemented — closest analog is the in-memory window/layout state on `ServerState` | spec-only |

Cross-cuts:

- **Federation** ([ADR-0007](../../ADR/0007-mosh-class-transport-and-satellites.md)) — addressing scheme. The wire's `SessionId` already has a `LOCAL` / `SATELLITE` tag union per the ADR; `TerminalId` (ADR-0016) extends the same shape to every identity. Today's server constructs `LOCAL` only.
- **Automation** — server-side rules subscribing to L1 events. Not yet implemented; an optional service when it lands.

A consumer's tier set is declared at HELLO time. Today's `phux-client`
is an L1+L2+L3-equivalent TUI consumer. A future `phux-client-sdk`
will be L1-only; a future native GUI consumer will be L1+L3 with its
own metadata schema. The reference TUI is **not** protocol-privileged
([ADR-0017](../../ADR/0017-tui-not-protocol-privileged.md)) — the wire
carries nothing that exists for it alone.

The cascades that align the in-tree implementation with this layering
are queued, not landed: rename `PaneId` → `TerminalId` workspace-wide;
split `phux-server` so L1 (terminal supervision) is mountable without
the L2/L3 services; reify L3 as a real KV store; demote `LayoutNode`,
`WindowId`, `WINDOW_*`, `LAYOUT_CHANGED`, `FOCUS_CHANGED` from the
wire into the TUI's L3 metadata conventions.

## Wire bytes: implementation participation

Wire bytes are normative in [`../spec/L1.md`](../spec/L1.md). This
document describes how phux's *implementation* participates.

The protocol is asymmetric. Server-to-client *terminal content* is a
stream of VT bytes (`PANE_OUTPUT { pane_id, seq, bytes }` today; under
ADR-0016 the message will be `TERMINAL_OUTPUT { terminal_id, ... }`);
the server forwards what the PTY emitted, after a per-client capability
rewrite. Client-to-server *input* is structured (`INPUT_KEY`,
`INPUT_MOUSE`, `INPUT_FOCUS`, `INPUT_PASTE`, `INPUT_RAW`), built from
libghostty's input atoms per ADR-0006 / ADR-0008. Lifecycle and
commands stay structured. See [`../spec/L1.md`](../spec/L1.md) for the
wire shape and ADR-0013 for the bytes-on-wire rationale.

The shape follows libghostty's interface: `Terminal::vt_write(&[u8])`
is the **only** way to feed grid content into a `Terminal`, and
structured readout (`grid_ref()`, `mode()`, `cursor_*`, `RenderState`)
is the only way to draw from one. Carrying bytes on the wire means
each end can keep a `Terminal` as its source of truth and the
protocol stops mirroring libghostty's grid model in a parallel
structure. Per-client capability downsampling moves from a per-cell
operation to a server-side VT byte-stream rewriter (SGR rewriting for
truecolor → 256-color → 16-color, OSC 8 stripping, image-protocol
gating, kitty-keyboard gating) sitting between the canonical PTY
stream and each subscribed client's send queue.
