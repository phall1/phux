# phux

A terminal multiplexer built on [libghostty-vt][lghvt-rs].

> "Need to replace tmux with a libghostty-based multiplexer so it can
> understand KIP."
> — Mitchell Hashimoto

phux is in the shape of tmux — a long-lived server, attaching clients,
sessions of windows of panes — but the architecture underneath is different:

- The server owns each pane's terminal state as a `libghostty_vt::Terminal`.
- The wire protocol carries **structured cell-level diffs**, not VT byte
  streams.
- Clients (TUI or native GUI) render diffs directly; nothing re-parses VT
  along the way.
- Input is carried as **semantic key events**, so the kitty keyboard
  protocol and friends pass through cleanly to inner programs — the thing
  every existing multiplexer fights to do and none quite manages.

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

**Pre-alpha. Spec first, code second.** Today this repo contains:

- [`SPEC.md`](./SPEC.md) — normative wire protocol.
- [`ARCHITECTURE.md`](./ARCHITECTURE.md) — internal design.
- [`ADR/`](./ADR/) — recorded design decisions.
- Empty crate skeletons.

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
| `phux`             | Single binary; subcommands for server, attach, new, ls, kill   |
| `phux-protocol`    | Wire types, codec, version negotiation                         |
| `phux-core`        | Domain types: Session, Window, Pane, Layout                    |
| `phux-server`      | Daemon: PTYs, terminal grids, IPC, diff emission               |
| `phux-client`      | TUI client: composes pane grids + chrome, emits VT             |

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
