---
audience: humans, contributors
stability: evolving
last-reviewed: 2026-07-09
---

# herdr — the phux starter distribution

**TL;DR.** herdr is a curated config layer, lazyvim-style: opinionated
keybindings, a status-bar lineup, a theme, and the demo plugin set
(continuum autosave/restore, agent-tools bench) wired in via
`-append`. Install with `phux config init --distro herdr`; your config
extends the layer, so herdr updates keep reaching you and your own
overrides always win.

---

## Install

```sh
phux config init --distro herdr
```

That scaffolds `~/.config/phux/config.toml` with one active line:

```toml
extends = ["/absolute/path/to/distros/herdr/herdr.toml"]
```

followed by the fully-commented shipped defaults. Nothing is copied out
of herdr: the layer file stays authoritative, so pulling a newer phux
checkout updates the distro for every config that extends it.

Already have a config? Add the `extends` line yourself (top-level, any
config file). See docs/CONFIG.md "Layered configs" for the mechanics.

The bundled name `herdr` resolves through, in order: `$PHUX_DISTROS_DIR`,
`$XDG_DATA_HOME/phux/distros` (default `~/.local/share/phux/distros`),
then the repo checkout's `distros/` directory. A path (`--distro
./distros/herdr` or `--distro ./distros/herdr/herdr.toml`) works from
anywhere.

## What you get

- **Which-key-first prefix table.** The popup delay drops to 400 ms;
  press the prefix and hesitate to see every continuation, including the
  herdr additions: `Space` opens the command palette, `|` / `-` split
  along the axis the chord draws, `Tab` hops to the next window. All
  shipped phux bindings remain.
- **Command palette as the hub.** Actions contributed by the wired
  plugins (continuum saves, agent-tools bench helpers) appear in the
  palette (`prefix Space` or `prefix :`) as `plugin:` rows.
- **Status lineup.** Window tabs on the left with a blue active tab,
  contextual help hints center, session name and clock right.
- **Theme.** A cool blue accent with warm amber attention chrome — the
  slot names are documented in docs/consumers/tui.md section 4.4.
- **Session naming.** Auto-created sessions take the launch directory's
  basename instead of `default`.
- **Plugins, additively.** `[[plugins-append]]` wires
  `examples/plugins/continuum` and `examples/plugins/agent-tools`
  without erasing plugin entries from your own config or another layer.
  To drop them, assign a plain `plugins = [...]` in your config —
  replacement wins over inherited appends.

## Overriding herdr

Your `config.toml` merges on top of the layer, key by key:

```toml
extends = ["/absolute/path/to/distros/herdr/herdr.toml"]

[keybindings]
which-key-delay-ms = 800        # slow the popup back down

[theme]
accent = "magenta"              # replace one slot, keep the rest
```

`phux config show` prints the effective result of the whole stack.

## Layout notes

Relative `manifest` paths in a layer resolve against the layer file's
own directory, which is what lets herdr reference the sibling
`examples/plugins/` tree. If you copy `distros/herdr/` out of a phux
checkout, copy those two plugin directories too and update the
`[[plugins-append]]` paths in `herdr.toml`.
