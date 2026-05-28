---
audience: contributors, agents
stability: evolving
last-reviewed: 2026-05-27
---

# Data model

**TL;DR.** The in-process types that the server manipulates: sessions,
windows, panes, layouts, attached clients. Pure-data `phux-core::Registry`
on slotmaps with generational keys; I/O state lives separately on
`phux-server::ServerState`. Distinct from the wire shape; the bridge
crosses at `IdBridge`.

---

The server is a graph of long-lived nodes with stable identity. The
domain (`phux-core`) uses one `SlotMap` per node type rather than
`Rc<RefCell<>>` because:

- Stable IDs are exactly what the wire protocol needs anyway.
- Cross-references ("this client's active terminal") become an ID, not
  a borrowed reference — no aliasing problem.
- Deletion is `O(1)` and slotmap's generational keys catch
  use-after-free in tests.

The shape splits in two: the *domain* (pure data, in `phux-core`) and
the *attached-client + I/O* state (in `phux-server`). The split is
deliberate; see ADR-0008 and the crate-graph note above.

```rust
// phux-core::registry::Registry — domain only, no I/O.
// Pre-ADR-0015 vocabulary; layer mapping in parens.
pub struct Registry {
    sessions: SlotMap<SessionId, Session>,    // L2 Collection (kind of —
                                              //   currently bundles windows too)
    windows:  SlotMap<WindowId,  Window>,     // demotes to TUI L3 metadata
    panes:    SlotMap<PaneId,    Pane>,       // L1 Terminal
}

pub struct Session  { id, name, windows: Vec<WindowId>, active: Option<WindowId> }
pub struct Window   { id, session, panes: Vec<PaneId>, layout: Option<LayoutNode>, active: Option<PaneId> }
pub struct Pane     { id, window, dims, cwd, title }
// LayoutNode is a binary split tree of PaneId leaves. Under ADR-0017
// this whole tree (LayoutNode + Window + active pane focus) demotes
// from a wire-protocol concept to a TUI-consumer convention stored
// in L3 metadata. ADR-0012's "binary split, not n-ary" decision
// continues to apply *to the TUI's tree*, not to the wire.
```

The PTY handle and `libghostty_vt::Terminal` for a pane are NOT fields
of `Pane`. They are server-side concerns and will hang off `PaneId` in
side tables in `phux-server` once PTY supervision lands. Keeping `Pane`
free of I/O is what lets `phux-core` stay `forbid(unsafe_code)` and
ship without an async runtime.

```rust
// phux-server::state::ServerState — domain + clients + I/O.
pub struct ServerState {
    pub registry:        Registry,
    pub attached:        HashMap<ClientId, AttachedClient>,
    pub pane_subscribers: HashMap<PaneId, Vec<ClientId>>,
    // Per-pane input log; merge point for multi-client keystrokes.
    pane_inputs:         HashMap<PaneId, Vec<PaneInput>>,
    // Core SessionId (slotmap key, generational) <-> wire SessionId (u32).
    pub session_id_bridge: IdBridge,
    next_client_id:      u64,
}

pub struct AttachedClient {
    pub id:      ClientId,           // server-assigned, monotonic
    pub session: phux_core::SessionId,
    pub tx:      tokio::sync::mpsc::Sender<OutboundFrame>,
}
```

Session name lookup goes through `Registry::sessions()` rather than a
side index — it is O(N) in session count, which is fine: session count
is small (single digits typical, double digits worst-case) and the
extra index would have to be kept consistent across cascading deletes.
