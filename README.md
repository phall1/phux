# phux

A **libghostty-backed terminal control plane.** A long-lived server
hosts terminals — spawned, observed, controlled, persisted, addressable
across hosts — and a tmux-shaped TUI rides on top as one consumer
among several.

> "Need to replace tmux with a libghostty-based multiplexer so it can
> understand KIP."
> — Mitchell Hashimoto

The TUI is what most users will see first. Sessions, windows, panes,
splits, status bar, keybindings — the vocabulary is tmux's because
it's what people know. Under that surface, the wire protocol is much
smaller and more general: terminals as first-class primitives,
collections of terminals as a lifecycle bundle, an opaque metadata
store on top. Other consumers — agent SDKs, recorders, future native
GUIs — speak the same wire and pick the tiers they care about.

Read [`VISION.md`](./VISION.md) for the long arc.

## Why it looks different from tmux underneath

- Both ends run `libghostty_vt::Terminal`. The server's is the
  canonical pane state; the client's is a local mirror used for
  rendering. Nothing in the middle re-parses VT — modern terminal
  protocols pass end-to-end (Kitty keyboard, true colour, OSC 8,
  OSC 133, images).
- The wire is asymmetric (ADR-0013): server→client *terminal content*
  is **VT bytes** (forwarded from the PTY after per-client capability
  rewriting); client→server *input* is **structured key, mouse,
  focus, and paste events** built from libghostty's own atoms.
- The protocol is layered (ADR-0015): **L1 Terminal** is the
  substrate every consumer speaks; **L2 Collection** is an optional
  lifecycle bundle; **L3 Metadata** is an opaque KV store where
  consumers (the TUI, a future GUI, an SDK) keep their own state.
  Conformance is per-tier — an agent SDK speaks L1 alone and never
  hears "session" or "window."
- Identity is federation-ready from day one (ADR-0007). `TerminalId`
  is a `LOCAL` / `SATELLITE { host, id }` tagged union; v0.1 servers
  construct `LOCAL`, v0.2+ hubs route to satellites. The wire bytes
  don't change.

## Philosophy

phux aims to be **smol**, in the [sense of the term that has become a
small-software manifesto][smol]:

> - Write programs that solve a well-defined problem.
> - Write programs that behave the way most users expect them to behave.
> - Write programs that a single person can maintain.
> - Write programs that compose with other smol tools.
> - Write programs that can be finished.

The well-defined problem is: **spawn, observe, control, persist, and
address libghostty terminals — locally or across a fleet — with
conformance tiers a consumer can target without inheriting everything
else.** The reference TUI proves the substrate is real. The substrate
is what makes phux not-tmux. See `VISION.md` for the long form and
`CONTRIBUTING.md` for the things we say no to.

[smol]: https://smol.tauri.app/

## Status

**Pre-alpha. Spec first, code second.** End-to-end attach works for a
single pre-seeded session today; most of the substrate's lifecycle,
metadata, and federation surface is not yet wired.

What is in tree today (vocabulary is still the pre-ADR-0015 names —
"pane" is the wire identity, "session/window" appear; the rename to
L1/L2/L3 names is a follow-on cascade):

- [`SPEC.md`](./SPEC.md) — normative wire protocol (currently
  pre-layering; restructure under ADR-0015 is the next big cascade).
- [`ARCHITECTURE.md`](./ARCHITECTURE.md), [`DESIGN.md`](./DESIGN.md),
  [`VISION.md`](./VISION.md), and [`ADR/`](./ADR/) — recorded
  decisions and arc.
- `phux-protocol` — length-prefixed TLV frame codec (SPEC Appendix A),
  the `HELLO` / `ATTACH` / `DETACH` / `PANE_OUTPUT` / `PANE_SNAPSHOT`
  / `INPUT_*` / `BELL` / `ERROR` / `PING` subset of the message
  catalog, and structured input types that re-export libghostty's
  atoms directly (ADR-0008).
- `phux-core` — registries (slotmaps) for the in-process domain
  objects that map to L1 terminals (currently named `Pane`) and the
  TUI's binary-split layout convention (currently still on the
  wire; demotes to L3 metadata under ADR-0015).
- `phux-server` — tokio current-thread runtime, UDS listener at
  `$XDG_RUNTIME_DIR/phux/phux.sock`, per-terminal actor (ADR-0014)
  that owns a `libghostty_vt::Terminal` and a real PTY child,
  per-terminal input encoders (ADR-0006), broadcast `PANE_OUTPUT`
  fanout, `PANE_SNAPSHOT` synthesis from `RenderState`, a per-client
  capability rewriter for outbound bytes.
- `phux-client` — UDS attach loop, raw-mode/altscreen guard, stdin
  keyboard parser, and a `libghostty_vt::Terminal` per attached
  terminal driven by `RenderState` (ADR-0013). The intended per-row
  dirty path is currently bypassed in favour of a full-screen redraw
  per frame, pending the libghostty FFI investigation tracked as
  `phux-l0t`.
- `phux-config` — TOML schema + loader with `line:col` errors,
  keybind parser/trie resolver, status `Widget` trait + time /
  session-name widgets.

What is not wired up yet: most of the L2 / L3 surface (Collection
lifecycle, metadata store, federation routing), automation, the
agent SDK, predictive local echo, mouse / bracketed-paste parsing
on the client, client-side keybinding dispatch, journaling and
crash recovery, and the full subcommand set (`new`, `ls`, `kill`,
etc. — today's binary ships `attach` and `server` only, with
tmux-style auto-spawn).

## Quickstart

```sh
nix develop          # or `direnv allow` once, then auto
just check           # type-check across the workspace
just ci              # everything CI must pass
```

The dev shell pins Rust 1.90, includes `zig_0_15` for libghostty-vt
builds, plus `nextest`, `deny`, `watch`, `insta`, `mutants`, and
`just`.

## Crate layout

| Crate            | Purpose                                                       |
|------------------|---------------------------------------------------------------|
| `phux`           | Single binary; `attach` and `server` subcommands today        |
| `phux-protocol`  | Wire types, codec, version negotiation; publish-ready         |
| `phux-core`      | Domain types: in-process terminal / collection registries     |
| `phux-server`    | Daemon: per-terminal actor, PTY supervision, output fanout    |
| `phux-client`    | TUI client: local libghostty Terminal + RenderState redraw    |
| `phux-config`    | TOML config schema + status widget contract                   |

`phux-protocol` is the only crate intended for publication; the rest
are `publish = false`. ADR-0008 records why `phux-protocol` depends on
`libghostty-vt` directly (gated behind the `server` feature so docs.rs
+ crates.io see a git-dep-free shell).

Two future crates are on the design roadmap (not yet started):

- `phux-client-sdk` — L1-only typed Rust handle for agents.
  Spawn / observe / drive terminals over the wire, no TUI vocabulary.
  The developer surface that makes the agent-first thesis real.
- `phux-client-gui` — native GUI consumer over libghostty's surface
  API. Renders terminals its own way; shares no chrome with the TUI.

## Non-goals

Documented in `CONTRIBUTING.md` and the relevant ADRs:

- **No embedded scripting language.** Commands are typed wire messages.
- **No plugin host.** Extensions are consumer-side; agents cover the
  programmatic case structurally.
- **No copy-mode reinvention.** Modern terminals do this well; we
  surface libghostty's selection APIs.
- **No homegrown crypto.** Unix socket perms + SSH (and future QUIC
  TLS) at the transport.
- **No format-template DSL.** Status widgets are typed; arbitrary
  logic lives in widget binaries the TUI runs.

## License

Dual-licensed under [MIT](./LICENSE-MIT) or
[Apache-2.0](./LICENSE-APACHE) at your option.

[lghvt-rs]: https://github.com/Uzaaft/libghostty-rs
