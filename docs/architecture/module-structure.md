---
audience: contributors, agents
stability: evolving
last-reviewed: 2026-05-27
---

# Module structure

**TL;DR.** Per-crate module trees as they exist in tree today, with
notes on what's deliberately absent. New modules should land in the
shape that fits the crate; do not retrofit older layouts onto new work.
The render-layering split inside `phux-client` is documented separately
in [`render-layering.md`](./render-layering.md).

---

What is in tree today. New modules land in the shape that fits the
crate; do not retrofit older layouts onto new work.

## `phux-protocol`

```
src/
  lib.rs              — re-exports, top-level docs, PROTOCOL_VERSION
  ids.rs              — SessionId, WindowId, PaneId, ClientId, FrameId
  input/              — INPUT_* event types (SPEC §9)
    key.rs, mouse.rs, focus.rs, paste.rs, mod.rs
  wire/               — TLV codec (SPEC Appendix A)
    frame.rs          — FrameKind + length-prefix framing
    encode.rs, decode.rs, field.rs, info.rs, error.rs
```

The `input` and `wire` modules are gated behind the `server` cargo
feature so the no-feature shell compiles without `libghostty-vt`.
See `lib.rs` for the docs.rs / crates.io rationale. The pre-ADR-0013
`diff/` module and its companion `wire/diff.rs` have been deleted;
`PANE_OUTPUT` and `PANE_SNAPSHOT` carry VT bytes directly. A small
amount of stale doc-comment text inside `wire/field.rs` still mentions
`DiffOp` and is scheduled for cleanup.

## `phux-core`

```
src/
  lib.rs              — re-exports
  ids.rs              — typed slotmap keys
  registry.rs         — Registry: SlotMaps + cascading deletes
  session.rs          — Session
  window.rs           — Window + binary split-tree LayoutNode
  pane.rs             — Pane (pure metadata; no PTY, no Terminal)
```

No `selector.rs` or `config/` here yet — selectors are unimplemented;
config lives in its own crate.

## `phux-server`

```
src/
  lib.rs              — re-exports of ServerRuntime, ServerState, PaneActor, ...
  runtime.rs          — tokio current-thread + UDS listener + accept loop;
                        spawns per-client tasks on a LocalSet
  state.rs            — ServerState, SharedState, AttachedClient, ClientId,
                        PaneInput, Outbound
  pane_actor.rs       — PaneActor: owns the pane's libghostty Terminal (!Send,
                        in RefCell on the LocalSet), per-pane input encoders,
                        PTY reader/writer threads, broadcast PANE_OUTPUT fanout,
                        snapshot synthesis on demand (ADR-0014)
  grid.rs             — SnapshotSynthesizer: walks the canonical Terminal via
                        RenderState and emits a self-contained vt_replay_bytes
                        sequence for PANE_SNAPSHOT (per-row SGR deltas +
                        graphemes + cursor restore + DECSCUSR)
  downsample.rs       — per-client capability rewrite of outbound VT bytes
                        (truecolor → 256/16, OSC 8 / image / KIP gating)
  id_bridge.rs        — core SessionId <-> wire SessionId (u32)
  telemetry.rs        — tracing setup; opt-in tokio-console behind a feature
  input/              — server-side encoders bridging wire input -> PTY bytes;
                        each pane owns its own PerPane{Key,Mouse,Focus,Paste}
                        encoder, refreshed from Terminal state per encode
    key.rs, mouse.rs, focus.rs, paste.rs, mod.rs
```

No `pty/`, `journal/`, `command.rs`, or `hooks.rs` yet — these are
future work; their absence here is intentional, not drift. PTY supervision
today lives inside `pane_actor.rs` (two `std::thread`s bridging blocking
`portable_pty` I/O to the async actor over `mpsc` channels).

## `phux-client`

Under ADR-0013 the client owns a `libghostty_vt::Terminal` per
attached pane and uses `RenderState` to drive redraw. The hand-rolled
`mirror/` module from earlier drafts has been deleted.

```
src/
  lib.rs              — re-exports of attach::run
  attach/
    mod.rs            — public run(socket, target); ties everything together
    connection.rs     — UDS transport, length-prefixed frame I/O
    driver.rs         — tokio::select! lifecycle, RawModeGuard RAII for
                        outer terminal state (raw mode + altscreen, restored
                        on any exit)
    render.rs         — PaneRenderer: feeds PANE_OUTPUT bytes into the local
                        Terminal and walks RenderState rows to emit cursor
                        positioning + per-cell SGR deltas + graphemes. Uses
                        per-row dirty bits to skip unchanged rows.
    input.rs          — StdinParser: keyboard + UTF-8 + escape sequences;
                        configurable keybinding chords
```

What this tree does NOT contain yet, deliberately:

- Mouse / bracketed-paste parsing on the client (keyboard only in v0).
- Predictive local echo (see [`predictive-echo.md`](./predictive-echo.md)
  for the design that lives on top of the mirror Terminal).
- `VIEWPORT_RESIZE` routing end-to-end (frame exists; SIGWINCH handler
  not yet wired).
- Full client-side command coverage for every docs/consumers/tui.md keybinding action.

See `../../research/2026-05-25-libghostty-renderstate.md` for the renderer
contract these modules implement.

The two-renderer split inside `phux-client/src/render/` is described in
[`render-layering.md`](./render-layering.md).

## `phux-client-core`

The ratatui-free pane-interior substrate (`layout`, `multi_pane`,
`predict`), extracted from `phux-client` under phux-0fv. It declares no
`ratatui` dependency, so the chrome boundary (ADR-0020) is enforced by
the compiler rather than a grep guard. `phux-client` depends on it and
re-exports its modules so consumers keep `phux_client::{layout,
multi_pane, predict}` paths. See [`crate-graph.md`](./crate-graph.md)
and [`render-layering.md`](./render-layering.md).

## `phux-config`

```
src/
  lib.rs              — parse_str + re-exports
  schema.rs           — typed TOML schema (Config, KeybindingsCfg, ...)
  loader.rs           — XDG resolution + agent round-trip
  keybind.rs          — keybind parser + trie resolver
  error.rs            — ConfigError with line:col spans
  widget/             — StatusWidget trait + registry
    mod.rs
    widgets/time.rs, widgets/session_name.rs, widgets/mod.rs
```

## `phux` (binary)

```
src/main.rs           — clap subcommand dispatch:
                          `phux attach [SESSION] [--socket PATH]`
                          `phux server  [--session NAME] [--socket PATH]`
                        Auto-spawns a detached `phux server` if the socket
                        doesn't exist when `attach` is invoked (25 ms poll,
                        2 s timeout). Opt-in cargo features: `dhat-heap`
                        (binary), and `tokio-console` via `phux-server`.
```

The wider subcommand surface in docs/consumers/tui.md §1 (`new`, `ls`, `windows`,
`panes`, `kill`, `send`, `capture`, `config`, `messages`, `version`,
`help`) is not yet wired.
