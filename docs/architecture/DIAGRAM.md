---
audience: humans, contributors, agents
stability: stable
last-reviewed: 2026-05-31
---

# System shape diagram

**TL;DR.** phux is a libghostty-backed terminal control plane. The canonical terminal state lives server-side; clients attach over a wire abstraction and maintain local mirrors for rendering. This diagram shows the full path from PTY through to screen.

---

```
┌──────────────┐
│              │
│     PTY      │  Shell process (PTY child)
│   (child)    │
│              │
└────────┬─────┘
         │
         │ VT bytes (canonical input)
         │
         ▼
┌────────────────────────────────────┐
│   SERVER (single per user)         │
│                                    │
│  ┌──────────────────────────────┐  │
│  │  libghostty Terminal         │  │
│  │  (canonical state)           │  │
│  │                              │  │
│  │  - PTY supervisor            │  │
│  │  - Per-terminal actor        │  │
│  │  - Grid state + parse tree   │  │
│  └──────────────────────────────┘  │
│                                    │
└────────┬─────────────────────┬─────┘
         │                     │
         │ L1 HELLO           │ Per-terminal actor
         │ (capabilities)      │ supervision via tokio
         │                     │
         ▼                     ▼
    ┌─────────────────────────────────────┐
    │   Transport Trait (async)           │
    │                                     │
    │  Abstraction over:                  │
    │  ┌─────────────────────────────┐    │
    │  │ UnixSocketTransport (v0.1)  │    │
    │  │ QuicTransport (v0.2+)       │    │
    │  │ SshStdioTransport (v0.2+)   │    │
    │  └─────────────────────────────┘    │
    │                                     │
    │  Direction split:                   │
    │  ← VT bytes (PTY output)            │
    │  → Structured input events          │
    │  ← Terminal state updates           │
    └──────┬────────────────────────┬─────┘
           │                        │
    Input events                Output bytes
    (Key, Mouse, Focus,       (TERMINAL_OUTPUT,
     Paste, Resize)            bell, title, cwd)
           │                        │
           ▼                        ▼
┌────────────────────────────────────────┐
│   CLIENT (separate process per attach) │
│                                        │
│  ┌──────────────────────────────────┐  │
│  │  libghostty Terminal             │  │
│  │  (local mirror)                  │  │
│  │                                  │  │
│  │  - Async I/O, unbuffered recv    │  │
│  │  - Per-client RenderState cache  │  │
│  │  - Predictive local echo queue   │  │
│  │  - Grid + full parse tree        │  │
│  └──────────────────────────────────┘  │
│                                        │
│  ┌──────────────────────────────────┐  │
│  │  ratatui Chrome                  │  │
│  │  (TUI decoration)                │  │
│  │                                  │  │
│  │  - Status bar                    │  │
│  │  - Layout tree (panes/splits)    │  │
│  │  - Keybindings, hooks            │  │
│  │  - L3 metadata rendering         │  │
│  └──────────────────────────────────┘  │
│                                        │
└────────────┬─────────────────────┬─────┘
             │                     │
        Render                Keybind
        (once/frame)          input
             │                     │
             ▼                     ▼
         Terminal Screen      (keyboard, mouse)
         (rasterized)
```

---

## Key invariants

### Canonical state (server)

The server's `libghostty_vt::Terminal` is the single source of truth:
- Canonical grid (full parsed state from PTY bytes)
- Full parse tree (color, styles, hyperlinks, etc.)
- PTY supervision (per-terminal actor pattern)
- Session/window/pane collections (L2)

### Local mirror (client)

The client maintains its own `libghostty_vt::Terminal` as a local mirror for rendering. It is **never the source of truth**:
- Render text and styles to screen via ratatui
- Cache `RenderState` (what was painted last frame)
- Reconcile predictions when server state arrives
- Scroll-back search
- Clipboard (grid available locally)

Both terminals are **identical instances** of the same libghostty parser. No re-encoding in the middle.

### Transport abstraction

The wire sits behind an async `Transport` trait:

```rust
trait Transport: Send {
    async fn send(&mut self, msg: FrameBytes) -> Result<()>;
    async fn recv(&mut self) -> Result<FrameBytes>;
}
```

Implementations:
- **UnixSocketTransport**: `$XDG_RUNTIME_DIR/phux/phux.sock` (v0.1)
- **QuicTransport**: QUIC with connection migration (v0.2+, ADR-0007)
- **SshStdioTransport**: SSH process stdin/stdout (v0.2+, satellites)

No code names a concrete transport; all I/O is trait-bound.

### Per-terminal actor (server)

Each terminal runs as a tokio `current-thread` task under supervision:
- Reads from its PTY via async I/O
- Feeds bytes to the canonical Terminal
- Broadcasts state snapshots + deltas to all attached clients
- Accepts structured input events from clients; encodes them back to VT bytes for the PTY

### Per-client RenderState (client)

A per-frame `RenderState` cache tracks what was painted last frame. When new server state arrives:

1. Client's local Terminal is updated with new bytes
2. Changed cells get re-rendered
3. Unchanged cells are skipped (zero-copy cell references)
4. Predictive echoes are reconciled (dropped if server already has the byte)

This is the hot path for rendering; see `render-layering.md` for details.

### Data direction

- **PTY → Server → Wire → Client**: VT bytes (terminal output)
- **Client → Wire → Server → PTY**: Structured input events (key, mouse, focus, paste)
- **Server ↔ Client**: State snapshots (attach), deltas (every frame)

The wire is **asymmetric**: one direction is bytes, the other is structured events. Core invariant from ADR-0013 (libghostty bytes on the wire).

---

## Scopes

| Scope | Lives | Carries |
|---|---|---|
| **Session** | Server (L2 wire) | Named terminal bundle, lifecycle |
| **Window** | TUI client (L3 metadata) | Layout tree, focus, pane arrangement |
| **Terminal** | Server (L1 wire) | PTY, canonical grid, events |
| **Pane** | TUI client (L3 metadata) | Viewport into a Terminal, split geometry |

The wire defines terminals (PTY + grid + events). The TUI defines sessions, windows, and panes as one way to arrange those terminals. An agent SDK speaks only L1. A headless server speaks only L2. The TUI speaks all three. No concept is duplicated.

---

## Cold-read digest

1. **Start left**: PTY emits VT bytes.
2. **Move right to Server**: Server's libghostty Terminal is canonical.
3. **Across the Transport**: Wire abstraction carries bytes one way, structured events the other.
4. **Right side**: Client's libghostty Terminal mirrors the server's state for rendering.
5. **Rightmost**: ratatui chrome (TUI) decorates the terminal grid with status bar, layout, keybindings.

The system **cannot degrade modern terminal features** (Kitty keyboard, true color, hyperlinks, pixel-precision mouse) because both ends use the same libghostty parser and nothing re-encodes in the middle.

---

## See also

- [`docs/CONCEPTS.md`](../CONCEPTS.md) — full mental model
- [`docs/architecture/transport.md`](./transport.md) — Transport trait details
- [`docs/architecture/process-model.md`](./process-model.md) — server/client lifecycle
- [`docs/architecture/render-layering.md`](./render-layering.md) — client-side rendering cache
- [`ADR-0013`](../../ADR/0013-libghostty-bytes-on-wire.md) — why libghostty bytes, not re-encoded
- [`ADR-0007`](../../ADR/0007-mosh-class-transport-and-satellites.md) — Transport trait design
