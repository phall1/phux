---
audience: humans, contributors
stability: stable
last-reviewed: 2026-06-17
---

# Configuration and keybindings

**TL;DR.** Config lives in `$XDG_CONFIG_HOME/phux/config.toml` (or `~/.config/phux/config.toml`); phux merges your file atop the shipped defaults, with an optional `extends` stack of shared layers in between. Customize keybindings (prefix + global chords), status bar widgets, and hooks via TOML tables.

---

## Config file location and discovery

phux loads configuration in this order:

1. **Shipped defaults** — embedded in the binary as `default.toml`
2. **Extended layers** — any files your config (or a layer) names via `extends`, in listed order (see the next section)
3. **User config** — `$XDG_CONFIG_HOME/phux/config.toml` (or `~/.config/phux/config.toml` if `$XDG_CONFIG_HOME` is not set)
4. **Override via `$PHUX_CONFIG`** — if set, this path replaces the default location (used by `phux --config`)

Later files override earlier ones, key-by-key. A key you omit keeps the default, so a phux upgrade reaches you automatically without losing your overrides.

See "Layered configs" below for the `extends` mechanics.

### Getting started

To scaffold a documented starter config:

```sh
phux config init         # creates ~/.config/phux/config.toml
                         # (refuses to overwrite; use --force to override)

phux config path         # print the resolved config path (no I/O)

phux config show         # print the effective config (defaults merged
                         # with your overrides) as canonical TOML

phux config show --default  # print the shipped defaults with comments

phux plugin link ./my-plugin/phux-plugin.toml --json
                         # add or update a plugin manifest entry

phux plugin list --json  # inspect the plugin registry

phux plugin disable example.agent-tools --json
phux plugin enable example.agent-tools --json
                         # toggle a registered plugin

phux plugin unlink example.agent-tools --json
                         # remove a registered plugin

phux config plugins --json  # legacy read path for configured manifests
phux config agents --json   # project configured plugin agent states
phux config run PLUGIN ACTION --json  # execute a configured plugin action
```

### Applying changes

There is no live-reload verb today. To apply edits to the interactive client, restart it — detach and re-attach, or relaunch `phux` — so it re-reads the file on the next start. Local config/plugin subcommands (`init`, `path`, `show`, `plugins`, `agents`, `plugin ...`, and plugin action `run`) read the file fresh on each invocation.

Reload is deliberately not automatic, even as design intent: the file is not watched, because watch-reload introduces papercuts ("saved-mid-edit, now my keybindings are gone"). An explicit reload path may land later (see `docs/consumers/tui.md` §4.3).

---

## Layered configs: `extends`

A config file may name shared layers — a team baseline, a curated distribution — with a top-level `extends` key ([ADR-0039](../ADR/0039-layered-config.md)):

```toml
extends = ["distro.toml", "minimal"]

[keybindings]
prefix = "C-b"        # your overrides win over every layer
```

Rules:

- **Order.** Layers merge in listed order, each atop the previous; your file merges last and wins per key. The shipped defaults always sit at the bottom.
- **Resolution.** An entry with a path separator or a `.toml` suffix is a path, resolved relative to the directory of the file that declares it (absolute paths pass through). A bare name `n` means `layers/n.toml` beside the declaring file — so `extends = ["minimal"]` in `~/.config/phux/config.toml` loads `~/.config/phux/layers/minimal.toml`.
- **Layers can extend layers**, up to 4 levels below your file. Cycles, missing layer files, and over-deep nesting are errors that name the offending file. A layer reachable through two branches merges once.

### Array merge: replace by default, `-append` to add

Tables merge per key across layers, but an array assignment replaces the inherited array wholesale — TOML arrays have no per-element identity to merge on. When a layer should *contribute to* a list instead of owning it, use the `-append` key suffix:

```toml
# In a distro layer or your own config:

[[plugins-append]]                      # adds to inherited [[plugins]]
manifest = "/opt/distro/phux-plugin.toml"

[status]
right-append = [{ kind = "time", format = "%H:%M" }]   # adds a widget

[[hooks.pane-exit-append]]              # adds a pane-exit hook
when   = { exit-code = "*" }
action = "noop"
```

`x-append` must hold an array and appends its elements to the stack's current `x` (creating it when absent). Setting both `x` and `x-append` in one file, appending to a non-array, or a non-array `-append` value are errors naming that file. Keybindings need no append form: `prefix-table` and `global` are tables and already merge per chord. The `-append` suffix is reserved at every level; don't end a free-form key (for example a `[theme]` slot) with it. To *drop* an inherited entry, assign the full array plainly — replacement always wins over inheritance.

---

## Schema overview

The config TOML has these main sections:

```toml
[defaults]             # Server-wide behavior: shell, history, spawning
[keybindings]          # Prefix key + prefix-table + global chords
[status]               # Status bar widget composition
[hooks]                # Event-driven actions (array-of-tables)
[[plugins]]            # Declarative plugin manifests
[theme]                # Color slots for chrome and overlays
[experimental]         # Opt-in flags subject to change
```

### Keybindings

The keybindings section has three keys:

- **`prefix`** — the key that unlocks prefix-table bindings (default: `C-a`)
- **`[keybindings.prefix-table]`** — bindings that fire after pressing the prefix (tmux-style). This is where `c` (new window), `%` (vertical split), `"` (horizontal split), `x` (kill pane), etc. live.
- **`[keybindings.global]`** — bindings that fire any time, no prefix needed. Reserved for modifiers unlikely to conflict with inner programs: `super`, `hyper`, `meta`. Empty by default.

**Chord syntax:**
- `C-a` — Control+a
- `M-a` — Meta/Alt+a  
- `S-a` or `A` — Shift+a
- `Tab`, `Enter`, `Esc` — named keys (case-sensitive)
- `F1` .. `F24` — function keys
- Punctuation with implicit Shift: `|`, `?`, `"` decompose to physical key + Shift on a US layout

**Resolution:** After pressing the prefix, the *next* keystroke is matched against `prefix-table`. If it matches, the action runs; else the keystroke goes to the pane. Global bindings are checked for every keystroke; they fire if they match, else the keystroke goes to the pane.

**Actions** are typed commands with optional parameters. See `docs/consumers/tui.md` §5.4 for the full action catalog. A bare string is shorthand for a no-parameter action:

```toml
[keybindings.prefix-table]
"x"        = "kill-pane"              # bare string
"c"        = { action = "new-window" } # same thing, inline table form
"|"        = { action = "split-pane", direction = "vertical" }
```

### Status bar widgets

The status bar is rendered entirely client-side from a list of widgets:

```toml
[status]
left   = [{ kind = "windows" }]
center = []
right  = ["session-name", { kind = "time", format = "%H:%M" }]
```

A bare string like `"session-name"` is shorthand for `{ kind = "session-name" }`. Widgets that take parameters use inline table syntax.

**Shipped widgets** (the kinds implemented today; from `docs/consumers/tui.md` §8.3):

- `"session-name"` — current session name
- `"windows"` — tab bar, one tab per window
- `{ kind = "time", format = "..." }` — wall-clock time (strftime format)

Further kinds (`cwd`, `exit`, `exec`, and more) are catalogued in `docs/consumers/tui.md` §8.3 as design intent, not yet implemented.

All are **optional**. The default ships with windows on the left and session name + time on the right. Extend by using styled variants — see `phux config show --default` for examples with custom colors and separators.

### Hooks (events and actions)

> **Status:** Schema only. The `[[hooks.<name>]]` tables parse, but the runtime that fires them is design intent (`docs/consumers/tui.md` §9); the shipped defaults define no hooks.

Hooks are event-driven actions. You could, for example, declare two `pane-exit` hooks — one for success (exit code 0), one for any other exit code:

```toml
[[hooks.pane-exit]]
when   = { exit-code = 0 }
action = "noop"

[[hooks.pane-exit]]
when   = { exit-code = "*" }
action = { kind = "notify", text = "pane {pane} exited with {exit-code}" }
```

Each `[[hooks.<name>]]` entry is an array-of-tables entry; multiple entries are allowed and evaluated in order. See `docs/consumers/tui.md` §9 for the full event and action catalog as it stabilizes.

### Plugins

> **Status:** `[[plugins]]` entries parse, `phux plugin` manages their
> lifecycle, `phux plugin list --json` / `phux config plugins --json` list
> manifests, `phux config agents --json` projects declared agent state for
> consumers, and `phux config run PLUGIN ACTION` executes action entries. Event
> hooks, panes, links, and workspace profiles are declarative provider records;
> plugin actions compose the shipped CLI surfaces such as `workspace save` and
> `workspace restore`.

Plugins are executable workflow packages declared by a `phux-plugin.toml`
manifest. The config file composes local manifests:

```toml
[[plugins]]
manifest = "/path/to/plugin/phux-plugin.toml"
enabled = true
```

`manifest` may be absolute or relative to `config.toml`. A minimal manifest:

```toml
id = "example.agent-tools"
name = "Agent Tools"
version = "0.1.0"
min_phux_version = "0.0.2"

[[actions]]
id = "summarize"
title = "Summarize pane"
contexts = ["pane"]
command = ["python3", "summarize.py"]

[[agents]]
id = "codex"
label = "Codex"
description = "Coding agent"
state = "blocked"      # unknown | idle | working | blocked
attention = "high"     # none | low | normal | high
contexts = ["workspace", "pane"]
```

The `phux plugin` verbs edit the same `[[plugins]]` array while preserving
relative manifest paths already in the file:

```sh
phux plugin link ./plugins/agent-tools/phux-plugin.toml
phux plugin validate
phux plugin disable example.agent-tools
phux plugin enable example.agent-tools
phux plugin unlink example.agent-tools
```

Run the action:

```sh
phux config run example.agent-tools summarize --json
```

Action commands execute as argv arrays from the plugin root. phux captures
stdout/stderr, exit status, duration, and timeout outcome into a
`schema_version = 1` JSON result when `--json` is set. There is no hidden shell
expansion; a manifest only gets shell behavior when it explicitly declares an
argv such as `["sh", "-c", "…"]`. The runtime inherits the phux process
environment and adds `PHUX_PLUGIN_ID`, `PHUX_PLUGIN_ACTION_ID`, and
`PHUX_PLUGIN_ROOT`.

The manifest format also accepts `[[build]]`, `[[events]]`, `[[panes]]`,
`[[links]]`, and `[[workspaces]]` entries. Agent declarations are static status
records for consumer projections: `state` normalizes to `unknown`, `idle`,
`working`, or `blocked`, and `attention` normalizes to `none`, `low`, `normal`,
or `high`. Event hooks, panes, link handlers, and workspace profiles are
provider-shaped: each entry has a plugin-local `id`, a `title`, and, where it
executes, an argv `command`, so frontends and server layers can enumerate them
without loading plugin code. Link handlers additionally declare `schemes` or
`patterns`; workspace profiles list the action, event, agent, and pane role ids
they compose. Commands are argv arrays, not shell strings. This keeps phux core
small: the plugin owns its language and files, while phux owns manifest
validation, config composition, workspace archives, and terminal control.

[`examples/plugins/provider-showcase/phux-plugin.toml`](../examples/plugins/provider-showcase/phux-plugin.toml)
is the checked-in provider fixture for event, pane, and link-handler
enumeration.

The checked-in demo package at
[`examples/plugins/agent-tools`](../examples/plugins/agent-tools/README.md)
shows the smallest useful loop: point `XDG_CONFIG_HOME` at the fixture config,
list the configured plugin, validate it with `--json`, then run its `inspect`
action. From a fresh checkout:

```sh
just plugin-demo
```

The same package carries first-party public Codex and Claude Code integration
records. Link the plugin itself with `phux plugin link` or the fixture config,
then use plugin actions for package lifecycle checks:

```sh
phux config run com.phux.demo.agent-tools validate-integrations
phux config run com.phux.demo.agent-tools link-integration
phux config run com.phux.demo.agent-tools status-integrations
phux config run com.phux.demo.agent-tools unlink-integration
```

Those actions are still external and declarative. They write plugin-local
state files, report package state as `missing`, `current`, or `outdated`, and
record either a native session id supplied by the caller or the phux session
target. They do not load an in-process plugin host, contact private services,
or require agent credentials. `smoke-integrations` runs the lifecycle against
fake public CLIs in a temporary state directory.

Two larger checked-in profiles exercise the workspace layer:

- [`examples/plugins/continuum`](../examples/plugins/continuum/phux-plugin.toml)
  declares `autosave` and `restore-latest` actions plus idle/session-change
  events that call `phux workspace save` and `phux workspace restore`.
- [`examples/plugins/agent-tools`](../examples/plugins/agent-tools/README.md)
  declares an `agent-bench` workspace whose `launch-bench`, `list-bench`, and
  `drive-bench` actions create role sessions, report status, and route keys to
  the selected role through `phux send-keys`.

---

## Three concrete examples

### Example 1: Rebind the prefix from Ctrl-A to Ctrl-B

The shipped default is `C-a` to avoid conflicts with readline and screen. To change it, edit `~/.config/phux/config.toml`:

```toml
[keybindings]
prefix = "C-b"
```

Then restart the client (detach and re-attach, or relaunch `phux`). Every prefix-table binding (`c`, `%`, `x`, etc.) now fires after `Ctrl-B`.

Or use `Ctrl-Space`:

```toml
[keybindings]
prefix = "C-Space"
```

### Example 2: Switch the clock to a 12-hour format

Suppose you want the right status bar to show the session name and a 12-hour clock. Edit your config:

```toml
[status]
right = [
  "session-name",
  { kind = "time", format = " %I:%M %p" }
]
```

Restart the client to apply it. The status bar now shows the session name and a 12-hour time on the right. For styling (color, bold, underline), use the styled widget forms in `docs/consumers/tui.md` §8.2.

### Example 3: Customize the prefix-table to use Vim-style bindings

The shipped defaults use `h/j/k/l` for directional focus (Vim-style) and `c` for new window. Suppose you want to remap to HJKL (uppercase) for resize, and add a binding for splitting horizontally with `-`:

```toml
[keybindings]
prefix = "C-a"

[keybindings.prefix-table]
# Include the shipped defaults (or override as needed).
# This example shows the resize bindings:
"H" = { action = "resize-pane", direction = "left",  amount = 5 }
"J" = { action = "resize-pane", direction = "down",  amount = 5 }
"K" = { action = "resize-pane", direction = "up",    amount = 5 }
"L" = { action = "resize-pane", direction = "right", amount = 5 }
"-" = { action = "split-pane", direction = "horizontal" }
```

Your file overrides the matching keys in the shipped defaults; all other bindings remain active. Restart the client to apply it. Now `Ctrl-A H` resizes the pane left by 5 columns, and `Ctrl-A -` splits horizontally.

---

## Links and next steps

**Dive deeper:**
- **Action catalog and widget types** → [`docs/consumers/tui.md`](./consumers/tui.md) (§5.4 for actions, §8 for widgets)
- **Full schema** → the `phux-config` crate (`crates/phux-config/src/schema.rs`)
- **Lifecycle and event types** → [`docs/consumers/tui.md`](./consumers/tui.md) §9

**Common workflows:**
- **Getting started** → [`docs/QUICKSTART.md`](./QUICKSTART.md)
- **Understanding the TUI model** → [`docs/consumers/tui.md`](./consumers/tui.md) §2–3
- **Reference** → `phux config show --default` (the shipped config with comments)
