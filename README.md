# phux

A terminal multiplexer built on [libghostty-vt][lghvt-rs].

> "Need to replace tmux with a libghostty-based multiplexer so it can
> understand KIP."
> — Mitchell Hashimoto

phux is in the shape of tmux — a long-lived server, attaching clients,
sessions of windows of panes — but the architecture underneath is different:

- Both ends run `libghostty_vt::Terminal`. The server's is the
  canonical pane state; the client's is a local mirror used for
  rendering.
- The wire is asymmetric (ADR-0013): server→client *pane content* is
  **VT bytes** (forwarded from the PTY after per-client capability
  rewriting); client→server *input* is **structured key, mouse, focus,
  and paste events** built from libghostty's own atoms.
- Carrying input as semantic events — not raw VT — is what lets the
  kitty keyboard protocol and friends pass through cleanly to inner
  programs. Carrying pane content as bytes — not a parallel cell-diff
  model — is what keeps phux from re-implementing libghostty's grid.

The result is a multiplexer where the *outer composition* (panes, splits,
status, chrome) is cleanly separated from the *terminal emulation* (which
isn't ours — we lean on libghostty for that), and where a future native
GUI client falls out of the same protocol that the TUI uses.

## Philosophy

phux aims to be **smol**, in the [sense of the term that has become a
small-software manifesto][smol]:

> - Write programs that solve a well-defined problem.
> - Write programs that behave the way most users expect them to behave.
> - Write programs that a single person can maintain.
> - Write programs that compose with other smol tools.
> - Write programs that can be finished.

The well-defined problem is: **persistent, multiplexed terminal sessions,
correctly carrying modern terminal protocols end-to-end.** Not a window
manager. Not a config DSL. Not a plugin host. The scope is fixed by
design; the things we say no to are listed in
[`CONTRIBUTING.md`](./CONTRIBUTING.md).

[smol]: https://smol.tauri.app/

## Status

**Pre-alpha. Spec first, code second.** End-to-end attach works for a
single pre-seeded session/window/pane; most of the SPEC's session
graph, command, and event surface is not yet wired.

What is wired up today:

- [`SPEC.md`](./SPEC.md) — normative wire protocol.
- [`ARCHITECTURE.md`](./ARCHITECTURE.md), [`DESIGN.md`](./DESIGN.md), and
  [`ADR/`](./ADR/) — recorded decisions.
- `phux-protocol` — length-prefixed TLV frame codec (SPEC Appendix A),
  the `HELLO` / `ATTACH` / `DETACH` / `PANE_OUTPUT` / `PANE_SNAPSHOT` /
  `INPUT_*` / `BELL` / `ERROR` / `PING` subset of the message catalog,
  and structured input types that re-export libghostty's atoms directly
  (ADR-0008).
- `phux-core` — `SessionId` / `WindowId` / `PaneId` registries
  (slotmaps) and the binary split-tree window layout (ADR-0012).
- `phux-server` — tokio current-thread runtime, UDS listener at
  `$XDG_RUNTIME_DIR/phux/phux.sock`, per-pane actor (ADR-0014) that
  owns a `libghostty_vt::Terminal` and a real PTY child, per-pane
  input encoders (ADR-0006), broadcast `PANE_OUTPUT` fanout,
  `PANE_SNAPSHOT` synthesis from `RenderState`, a per-client capability
  rewriter for outbound bytes, and an `IdBridge` between core slotmap
  keys and wire `u32` IDs.
- `phux-client` — UDS attach loop, raw-mode/altscreen guard, stdin
  keyboard parser, and a `libghostty_vt::Terminal` per attached pane
  with `RenderState`-driven per-row dirty redraw (ADR-0013).
- `phux-config` — TOML schema + loader with `line:col` errors, keybind
  parser/trie resolver, status `Widget` trait + time/session-name
  widgets.

What is not wired up yet: most of the command/event surface (sessions,
windows, layout, focus, OSC events, hooks), `VIEWPORT_RESIZE` routing,
predictive local echo, mouse / bracketed-paste parsing on the client,
client-side keybinding dispatch, journaling and crash recovery, and
the full subcommand set (`new`, `ls`, `kill`, etc. — today's binary
ships `attach` and `server` only, with tmux-style auto-spawn).

## Quickstart

```sh
nix develop          # or `direnv allow` once, then auto
just check           # type-check across the workspace
just ci              # everything CI must pass
```

The dev shell pins Rust 1.90, includes `zig_0_15` for libghostty-vt builds,
plus `nextest`, `deny`, `watch`, `insta`, `mutants`, and `just`.

## Crate layout

| Crate              | Purpose                                                        |
|--------------------|----------------------------------------------------------------|
| `phux`             | Single binary; `attach` and `server` subcommands today         |
| `phux-protocol`    | Wire types, codec, version negotiation; publish-ready          |
| `phux-core`        | Domain types: Session, Window, Pane, binary-split Layout       |
| `phux-server`      | Daemon: per-pane actor, PTY supervision, `PANE_OUTPUT` fanout  |
| `phux-client`      | TUI client: local libghostty Terminal + per-row dirty redraw   |
| `phux-config`      | TOML config schema + status widget contract                    |

`phux-protocol` is the only crate intended for publication; the rest are
`publish = false`. ADR-0008 records why `phux-protocol` depends on
`libghostty-vt` directly (gated behind the `server` feature so docs.rs +
crates.io see a git-dep-free shell).

A future `phux-client-gui` (libghostty-surface-based) plugs into the same
protocol.

## Non-goals

We have decided *against* several things on purpose. They are documented
in [`CONTRIBUTING.md`](./CONTRIBUTING.md) and the relevant ADRs:

- **No embedded scripting language.** Commands are typed IPC messages.
- **No plugin system on day one.** Hooks are typed events; integrations
  shell out.
- **No copy-mode reinvention.** Modern terminals do this well; we expose
  grid state and stay out of the way.
- **No homegrown crypto.** Unix socket perms + SSH at the transport.

## License

Dual-licensed under [MIT](./LICENSE-MIT) or
[Apache-2.0](./LICENSE-APACHE) at your option.

[lghvt-rs]: https://github.com/Uzaaft/libghostty-rs
