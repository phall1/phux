---
audience: humans, contributors
stability: stable
last-reviewed: 2026-05-31
---

# Configuration and keybindings

**TL;DR.** Config lives in `$XDG_CONFIG_HOME/phux/config.toml` (or `~/.config/phux/config.toml`); phux merges your file atop the shipped defaults. Customize keybindings (prefix + global chords), status bar widgets, and hooks via TOML tables.

---

## Config file location and discovery

phux loads configuration in this order:

1. **Shipped defaults** — embedded in the binary as `default.toml`
2. **User config** — `$XDG_CONFIG_HOME/phux/config.toml` (or `~/.config/phux/config.toml` if `$XDG_CONFIG_HOME` is not set)
3. **Override via `$PHUX_CONFIG`** — if set, this path replaces the default location (used by `phux --config`)

Later files override earlier ones, key-by-key. A key you omit keeps the default, so a phux upgrade reaches you automatically without losing your overrides.

### Getting started

To scaffold a documented starter config:

```sh
phux config init         # creates ~/.config/phux/config.toml
                         # (refuses to overwrite; use --force to override)

phux config path         # print the resolved config path (no I/O)

phux config show         # print the effective config (defaults merged
                         # with your overrides) as canonical TOML

phux config show --default  # print the shipped defaults with comments
```

### Reloading

Config reloads are **explicit**, not automatic. After editing your config file, run:

```sh
phux config reload       # re-read the config file and apply server-wide
```

We do not watch the file because auto-reload introduces papercuts ("saved-mid-edit, now my keybindings are gone"). Explicit reload is safer.

---

## Schema overview

The config TOML has five main sections:

```toml
[defaults]             # Server-wide behavior: shell, history, spawning
[keybindings]          # Prefix key + prefix-table + global chords
[status]               # Status bar widget composition
[hooks]                # Event-driven actions (array-of-tables)
[theme]                # Color slots for chrome and overlays
[experimental]         # Opt-in flags subject to change
```

### Keybindings

The keybindings section has three keys:

- **`prefix`** — the key that unlocks prefix-table bindings (default: `C-a`)
- **`[keybindings.prefix-table]`** — bindings that fire after pressing the prefix (tmux-style). This is where `c` (new window), `|` (vertical split), `x` (kill pane), etc. live.
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

**Built-in widgets** (from `docs/consumers/tui.md` §8):

- `"session-name"` — current session name
- `"windows"` — tab bar, one tab per window
- `{ kind = "time", format = "..." }` — wall-clock time (strftime format)
- `"cwd"` — current working directory from OSC 7
- `"exit"` — last command's exit code (from OSC 133)

All are **optional**. The default ships with windows on the left and session name + time on the right. Extend by using styled variants — see `phux config show --default` for examples with custom colors and separators.

### Hooks (events and actions)

Hooks are event-driven actions. The shipped defaults show two `pane-exit` hooks — one for success (exit code 0), one for any other exit code:

```toml
[[hooks.pane-exit]]
when   = { exit-code = 0 }
action = "noop"

[[hooks.pane-exit]]
when   = { exit-code = "*" }
action = { kind = "notify", text = "pane {pane} exited with {exit-code}" }
```

Each `[[hooks.<name>]]` entry is an array-of-tables entry; multiple entries are allowed and evaluated in order. See `docs/consumers/tui.md` §9 for the full event and action catalog as it stabilizes.

---

## Three concrete examples

### Example 1: Rebind the prefix from Ctrl-A to Ctrl-B

The shipped default is `C-a` to avoid conflicts with readline and screen. To change it, edit `~/.config/phux/config.toml`:

```toml
[keybindings]
prefix = "C-b"
```

Then run `phux config reload`. Every prefix-table binding (`c`, `|`, `x`, etc.) now fires after `Ctrl-B`.

Or use `Ctrl-Space`:

```toml
[keybindings]
prefix = "C-Space"
```

### Example 2: Add a clock widget to the right status bar

Suppose you want to add the current working directory and use a 12-hour format. Edit your config:

```toml
[status]
right = [
  "session-name",
  "cwd",
  { kind = "time", format = " %I:%M %p" }
]
```

Reload:

```sh
phux config reload
```

The status bar now shows session, working directory, and time on the right. For styling (color, bold, underline), use the styled widget forms in `docs/consumers/tui.md` §8.2.

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

Your file overrides the matching keys in the shipped defaults; all other bindings remain active. Reload:

```sh
phux config reload
```

Now `Ctrl-A H` resizes the pane left by 5 columns, and `Ctrl-A -` splits horizontally.

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
