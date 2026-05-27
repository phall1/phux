# Design — Reference TUI

This document describes the **reference TUI consumer's product surface**:
the things a tmux-shaped phux user sees and configures. It is not
normative — `SPEC.md` is. Where this document conflicts with `SPEC.md`,
the spec wins; file an issue.

The TUI is one consumer of the phux wire among several
([ADR-0017](./ADR/0017-tui-not-protocol-privileged.md)). Other
consumers — an agent SDK, a future native GUI — get their own design
docs. The TUI's specialness is that it ships in tree as the
human-facing reference; nothing on the wire exists for it alone.

For the long arc, read [`VISION.md`](./VISION.md). For the wire
protocol, see [`SPEC.md`](./SPEC.md). For internal structure,
[`ARCHITECTURE.md`](./ARCHITECTURE.md). This document is everything
between: how a user invokes the TUI, configures it, binds keys to it,
reads its status output, and extends it.

## How the TUI's user model maps to the substrate

The user-facing vocabulary is tmux's because it's what people know.
Under the hood, each TUI concept maps to one or more substrate
concepts from [ADR-0015](./ADR/0015-protocol-layering.md):

| TUI vocabulary | Substrate mapping |
|---|---|
| Session | L2 Collection (`CollectionId`, named lifecycle bundle of Terminals) |
| Window | TUI convention. An entry in a layout-tree blob stored in L3 metadata, keyed by `phux.tui.layout/v1` under the Collection that contains the Session. |
| Pane | L1 Terminal (`TerminalId`) referenced from a leaf of the TUI's layout tree. |
| Layout (split tree) | TUI convention. The shape stored in the L3 metadata blob above. ADR-0012's "binary split, not n-ary" still governs *this tree*; it is no longer a wire concept. |
| Active pane / window focus | TUI convention. Per-client, persisted in TUI metadata if the client wants it to come back on reattach. |
| Status bar / hooks / keybindings | TUI-local. Not on the wire. |
| Mouse routing (click-to-focus, drag-to-resize) | TUI-local. The wire carries `INPUT_MOUSE`; what to do with it is the TUI's call. |

A consumer that doesn't want this vocabulary doesn't have to learn
it. The substrate doesn't carry it. The TUI design that follows is
*the TUI's* design.

---

## 1. CLI surface

phux is a single binary with subcommands. The naked invocation —
`phux` — is the common case: attach to the user's server, lazily
spawning it if it isn't running.

> **Status (2026-05-26):** today's binary ships only `attach` and
> `server`. The naked `phux` invocation errors with a usage hint
> instead of attaching to the last session. Auto-spawn from `attach`
> works (the client forks itself as `phux server` if the socket is
> missing, polls 25 ms / 2 s). The rest of the table below is design
> intent.

```
phux                          # attach to default session, autostart server
phux attach [session]         # attach explicitly; session optional      shipped
phux server  [--session N]    # run server in foreground (incl. for SSH) shipped (no --stdio yet)
phux new [-s NAME] [-c CWD] [--] [COMMAND...]
                              # create a session                         spec-only
phux ls                       # list sessions (alias: list-sessions)     spec-only
phux windows [-s SESSION]     # list windows                             spec-only
phux panes [-w WINDOW]        # list panes                               spec-only
phux kill TARGET              # kill session/window/pane by selector     spec-only
phux send TARGET KEYS...      # send keys to a pane (scripting)          spec-only
phux capture TARGET           # dump pane grid (for piping/scripting)    spec-only
phux config [show|edit|path]  # config inspection                        spec-only
phux messages                 # recent server-emitted messages           spec-only
phux version                  # print version                            spec-only
phux help [COMMAND]                                                       spec-only
```

All subcommands accept `--target` / `-t` consistently where applicable.
Output is human-readable by default and JSON with `--json` where it
makes sense (`ls`, `windows`, `panes`, `capture`, `config show`,
`server status`).

---

## 2. The user model

Three nouns. Same as tmux. Don't reinvent vocabulary that users already
know.

- **Session** — top-level container. Named. Persists across client
  disconnects. Lives until explicitly killed or until the server exits.
- **Window** — tab within a session. Numbered from 0 within its session;
  optionally named.
- **Pane** — leaf in a window's layout. One PTY, one terminal grid, one
  shell or command.

A **client** is an attached frontend (TUI or GUI). Clients are
transient; they are not part of the session model. The protocol exposes
`ClientId` only for the duration of a connection.

---

## 3. Selectors

A selector identifies a session, window, or pane. Selectors appear in
CLI arguments, keybinding actions, and hook arguments.

| Selector              | Meaning                                          |
|-----------------------|--------------------------------------------------|
| `.`                   | current — the client's focused pane/window/session |
| `name`                | session by name                                  |
| `name:N`              | session `name`, window index `N`                 |
| `name:N.M`            | session `name`, window `N`, pane index `M`       |
| `name:tag`            | session `name`, window whose name is `tag`       |
| `@N`                  | opaque ID (pane/window/session) — stable for the |
|                       | server's lifetime                                |
| `=`                   | last (most recently focused)                     |

The CLI infers what kind of selector is expected from the command. When
ambiguity matters, prefer the most specific form. Example:

```sh
phux kill work:edit.2         # second pane in window "edit" of session "work"
phux send @42 "ls\n"          # by stable ID
phux kill =                   # kill last-focused (within whatever the command targets)
```

---

## 4. Configuration

### 4.1 File location

Config is read in order, later files overriding earlier:

1. `$XDG_CONFIG_HOME/phux/config.toml` (or `~/.config/phux/config.toml`)
2. `$PHUX_CONFIG` if set, replacing the above (used by `phux --config`)

Runtime and persistent state are split. The Unix socket lives in the
runtime dir (where it's expected to disappear on reboot); persistent
state lives in the state dir.

```
$XDG_RUNTIME_DIR/phux/phux.sock     # SOCK_STREAM, parent dir mode 0o700
                                    #   (fallback: /tmp/phux-$UID/phux.sock)

$XDG_STATE_HOME/phux/               # design intent; not yet implemented
├── server.pid
├── log/
│   └── server.log                  # tracing output, rotated daily
└── journal/
    └── <pane_id>.log               # per-pane PTY journal, capped ring
                                    #   (default 10 MiB)
```

Today only the socket is real (see
[`phux-server::runtime::default_socket_path`](./crates/phux-server/src/runtime.rs)).
The state-dir layout matches what
[`ARCHITECTURE.md`](./ARCHITECTURE.md#process-model) describes; both
docs treat it as the destination shape.

### 4.2 Format

Config is **TOML**. We picked TOML over KDL because:

- TOML is ubiquitous in the Rust ecosystem and the broader tooling
  world; every developer and every LLM-based assistant reads and writes
  it fluently. KDL's syntactic niceties are real but don't outweigh
  TOML's leverage in an environment where humans *and* agents are both
  expected to edit configuration routinely.
- Our config tree is shallow enough that TOML's idioms
  (`[table]`, `[[array.of.tables]]`, inline tables for parameterized
  values) cover it cleanly. We avoid the deep nesting that makes TOML
  awkward.

A minimal config:

```toml
[defaults]
shell          = "/bin/zsh"
history-limit  = 50000
refresh-rate   = 60

[keybindings]
prefix = "ctrl+space"

# Bindings under the prefix.
# An action is either a bare string (no parameters) or an inline
# table whose `action` field names the action and remaining fields
# pass parameters.
[keybindings.prefix-table]
"c"        = { action = "new-pane", direction = "horizontal" }
"v"        = { action = "new-pane", direction = "vertical" }
"x"        = "kill-pane"
"n"        = "new-window"
"tab"      = "next-window"
"h"        = { action = "focus-pane", direction = "left" }
"j"        = { action = "focus-pane", direction = "down" }
"k"        = { action = "focus-pane", direction = "up" }
"l"        = { action = "focus-pane", direction = "right" }
"d"        = "detach"
"shift+r"  = "rename-window"

# Global table: bindings that fire without a prefix.
# Empty by default; opt in to hyper/super combos if your outer
# terminal forwards them.
[keybindings.global]
# "hyper+left" = { action = "focus-pane", direction = "left" }

[status]
left   = ["session"]
center = ["windows"]
right  = [{ kind = "clock", format = "%H:%M" }]

[[hooks.pane-exit]]
when   = { exit-code = 0 }
action = "noop"

[[hooks.pane-exit]]
when   = { exit-code = "*" }
action = { kind = "notify", text = "pane {pane} exited with {exit-code}" }

[theme]
fg = "#cdd6f4"
bg = "#1e1e2e"
```

**Experimental knobs** live under `[experimental]`. Today the only key
is `predictive-echo` (boolean, default `false`), which engages Mosh-class
predictive local echo in `phux attach` — a client-side guess for the
next keystroke, rendered with an underline, that is reconciled when the
server's authoritative output arrives. The flag is opt-in because the
safe-prediction set is intentionally narrow (printable ASCII and
end-of-line backspace only) and the wider rollout will widen it in
follow-ups; anything under `[experimental]` may be renamed or removed
without a SemVer bump.

```toml
[experimental]
predictive-echo = true
```

### 4.3 Reloading

Config reloads are explicit, not automatic. `phux config reload` re-reads
the config file and applies it server-wide. We do not watch the file
because watch-reload introduces a class of "saved-mid-edit, now my
keybindings are gone" papercuts.

---

## 5. Keybindings

### 5.1 The model

We support two binding tables, both always present:

- **Prefix table** (`[keybindings.prefix-table]`): bindings that fire
  after the prefix key has been pressed. This is tmux's familiar model.
- **Global table** (`[keybindings.global]`): bindings that fire any
  time. Reserved for combinations unlikely to conflict with inner
  programs — in practice, ones using `super`, `hyper`, or `meta`
  modifiers.

```toml
[keybindings]
prefix = "ctrl+space"

[keybindings.global]
"hyper+left"  = { action = "focus-pane", direction = "left" }
"hyper+right" = { action = "focus-pane", direction = "right" }

[keybindings.prefix-table]
"c" = { action = "new-pane", direction = "horizontal" }
# ...
```

The global table is empty by default — no global bindings ship out of
the box because we cannot assume the user's outer terminal forwards
hyper/super at all. Users on Ghostty can opt in.

### 5.2 The dispatcher

Bindings invoke **actions**. Actions are typed `Command`s from `SPEC.md`
§11 plus a small set of client-side actions (detach, message-prompt,
copy-selection-to-clipboard). They are *not* shell strings; they are
named identifiers with typed parameters.

To shell out, use the explicit `run` action:

```toml
[keybindings.prefix-table]
"g"        = { action = "run", command = "lazygit" }          # in a new pane
"shift+g"  = { action = "run", command = "git status", in = "." }  # in current pane
```

### 5.3 Defaults

The defaults ship with `prefix="ctrl+space"`. We chose `ctrl+space`
because:

- It does not conflict with readline (`C-a` begin-line, `C-b` back-char,
  `C-e` end-line are all common).
- It does not conflict with screen (`C-a`).
- It does not conflict with vim (`C-w` is window, but `C-Space` is
  free or used as completion which we tolerate).
- It is two physical keys, no Greek-key chord.

Users with strong opinions override it in one line of config.

### 5.4 Action catalog (initial)

| Action               | Parameters                            |
|----------------------|---------------------------------------|
| `new-session`        | `name`, `cwd`, `command`              |
| `new-window`         | `cwd`, `command`                      |
| `new-pane`           | `direction`, `target`, `cwd`, `command` |
| `kill-pane`          | `target?`                             |
| `kill-window`        | `target?`                             |
| `kill-session`       | `target?`                             |
| `rename-window`      | `target?`, `prompt?`                  |
| `rename-session`     | `target?`, `prompt?`                  |
| `focus-pane`         | `direction` or `target`               |
| `focus-window`       | `direction` or `index` or `target`    |
| `move-pane`          | `target`, `position`                  |
| `resize-pane`        | `direction`, `amount`                 |
| `next-window`        |                                       |
| `previous-window`    |                                       |
| `detach`             |                                       |
| `run`                | `command`, `in?` (pane to run in)     |
| `message`            | `text`                                |
| `command-prompt`     | (interactive command entry)           |
| `noop`               |                                       |

---

## 6. Layout

### 6.1 The tree

A window's layout is a **binary split tree**: each interior node is a
split (horizontal or vertical) with a single `ratio` in `(0, 1)` and
exactly two children; leaves are panes. Three-way and N-way splits
are represented as nested binary splits. See ADR-0012 for the closed
decision behind this shape and SPEC §10.3 for the wire form.

```
window: split(vertical, ratio = 0.5)
        ├── pane #0
        └── split(horizontal, ratio = 0.33)
            ├── pane #1
            └── pane #2
```

(The first ratio gives pane #0 the top half of the window; the second
gives pane #1 the left third of the bottom half.)

Tabbed layout nodes are reserved for `SPEC.md` v0.2.

### 6.2 Resize behavior

> **Status:** Design intent. Not yet implemented as of 2026-05-25.
> The layout tree, split, kill-pane, and directional focus shipped in
> `phux-byc.2`; viewport-driven re-flow, minimum-size freezing, and
> the `resize-pane` command have no tickets filed yet.

When the client viewport (or server-aggregated viewport for multi-client
sessions) resizes, split ratios are preserved and dimensions are
redistributed proportionally. A leaf that hits its minimum size
(`min_cols = 2`, `min_rows = 1` for the inner content; chrome is per
client) freezes; remaining space redistributes among non-frozen leaves.

This is tmux's behavior. It's what users expect.

### 6.3 Resize commands

> **Status:** Design intent. Not yet implemented as of 2026-05-25.
> No `resize-pane` action wired into the dispatcher; no ticket filed.

`resize-pane direction=right amount=5` moves the boundary between the
focused pane and its right neighbor by 5 columns toward the right,
giving the focused pane more width. Negative amounts shrink.

Resize commands modify the relevant interior node's `ratio` (not
absolute sizes). After a subsequent window resize, the new ratio is
preserved.

---

## 7. Mouse

> **Status:** Partial. Input types and the per-pane wire encoder ship
> in `phux-protocol::input::mouse` and `phux-server/src/input/mouse.rs`
> (per ADR-0006 / ADR-0008). The routing described below — click-to-
> focus, drag-on-border to resize, scroll-wheel fallthrough, per-pane
> `set-pane mouse off` — is **not yet implemented** as of 2026-05-25.
> No tickets filed.

Mouse handling is enabled by default. The defaults:

| Event                    | Action                                |
|--------------------------|---------------------------------------|
| Click in pane            | Focus the pane                        |
| Click on pane border     | (no-op; reserved for future)          |
| Drag on pane border      | Resize the boundary                   |
| Scroll wheel in pane     | If the inner program has mouse mode,  |
|                          | forward; else scroll pane scrollback  |
| Right-click              | Pass through to inner program         |
| Click on status bar slot | Slot-defined; default no-op           |

Mouse handling is configurable per-pane: `set-pane mouse off` for a pane
that wants raw bytes (e.g. a TUI that does its own mouse handling).

We do not ship copy-mode mouse selection — see §11.

---

## 8. Status bar

### 8.1 Architecture: widget-first from day one

The status bar is **rendered entirely client-side**. A GUI client may
ignore it and render its own chrome; the TUI client composes it from
widgets and draws it on the bottom row of the outer terminal.

Every slot's contents are a list of **widgets**. A widget is a typed
thing that produces styled text. The default config looks short because
a bare string is shorthand for a no-parameters widget:

```toml
[status]
left   = ["session"]                                    # → [{ kind = "session" }]
center = ["windows"]
right  = [{ kind = "clock", format = "%H:%M" }]
```

There are three categories of widgets:

1. **Server facts.** The server already publishes session names, window
   lists, focused pane, cwd (via OSC 7), last command exit (via OSC
   133). These are widget kinds (`session`, `windows`, `cwd`, `exit`,
   etc.) backed by data the server pushes anyway.
2. **Client-local widgets.** Things derivable on the client without
   server help: `clock`, `mode`, `key-indicator` (last key chord).
3. **`exec` widgets.** The client runs the named program on the
   configured interval and renders its stdout (parsed for SGR if it
   contains ANSI). These run per-client; a clipboard daemon, a battery
   percentage, etc.

```toml
right = [
    { kind = "exec", command = "~/.local/bin/battery", interval = "30s" },
    { kind = "text", value = " | " },
    { kind = "clock", format = "%H:%M" },
]
```

### 8.2 Why widget-first

The scoping decision in `CONTRIBUTING.md` is that we will not ship a
status bar *DSL* — no `if/else` mini-language, no format-template
expression engine. The widget system gets us extensibility without
becoming a template interpreter: arbitrary logic lives in `exec`
widgets, which are real programs in real languages, supervised by the
client. The widget contract itself is small and typed.

This shape costs us almost nothing on day one (the default config is
three names in three lists), and means we never have to do an
architectural revision to grow a status bar plugin story.

### 8.3 Built-in widget kinds

| Kind            | Parameters                                                   |
|-----------------|--------------------------------------------------------------|
| `session`       | `format?` (default: `"{name}"`)                              |
| `window`        | `format?` (default: `"{name}"`)                              |
| `windows`       | `active-mark?`, `inactive-format?`, `active-format?`         |
| `pane`          | `format?`                                                    |
| `cwd`           | `format?`, `truncate?` (chars)                               |
| `exit`          | `format?` (last command exit code, OSC 133)                  |
| `clock`         | `format` (strftime)                                          |
| `host`          | `format?`                                                    |
| `mode`          | `format?` (current input mode)                               |
| `key-indicator` | shows the last key/chord pressed; reserved for v0.2          |
| `text`          | `value` (literal styled text)                                |
| `spacer`        | flexible expanding space; no parameters                      |
| `exec`          | `command`, `interval?` (default `5s`), `parse-ansi?` (true)  |

Every widget kind accepts a `style` table with optional `fg`, `bg`,
`bold`, `italic`, `underline` keys.

### 8.4 Refresh and ordering

- Server-fact widgets re-render on the relevant server event (window
  rename, focus change, OSC 7/133).
- Client-local widgets with no interval re-render only on event.
  `clock` re-renders every minute by default; `interval` overrides.
- `exec` widgets re-render every `interval`. The client batches
  re-renders to once per frame (max ~60 Hz).
- Slot contents render left-to-right with no implicit separator. Use
  `text` widgets for separators.

### 8.5 What the status bar is not

- Not multi-row. One row, bottom of the outer terminal. If you need
  more, dedicate a pane.
- Not themable via a styling engine. Per-widget `style` tables only.
- Not server-rendered. Every client owns its chrome. This is what
  enables a future GUI client with native chrome to coexist with the
  TUI client trivially.

---

## 9. Hooks

> **Status:** Design intent. Config parsing for `[[hooks.<name>]]`
> entries ships in `phux-config` (see `schema.rs`); the server-side
> dispatcher that actually fires hooks on real events is **not yet
> implemented** as of 2026-05-25. No tickets filed.

Hooks fire at named events. Each hook in the config is an
array-of-tables (TOML `[[hooks.<name>]]`) of `{ when, action }` pairs.

```toml
[[hooks.after-new-pane]]
when   = { cwd-startswith = "/Users/phall/work" }
action = { kind = "message", text = "in work tree" }

[[hooks.pane-exit]]
when   = { exit-code = 0 }
action = "noop"

[[hooks.pane-exit]]
when   = { exit-code = "*" }
action = { kind = "run", command = "say 'pane exited'" }
```

The hook system is intentionally small:

- **Match clauses** (`when = { key = value }`) are exact-string or
  simple glob matches (`"*"`). No regex; no expression language.
- **First match wins** per hook event. Subsequent entries don't fire.
- **Async by default.** Hook actions fire and the server moves on. Sync
  hooks (where the result blocks the trigger) are reserved for v0.2.

Hook points (initial):

| Hook                  | Fires after / on                         |
|-----------------------|------------------------------------------|
| `after-new-session`   | session creation                         |
| `after-new-window`    | window creation                          |
| `after-new-pane`      | pane creation, before exec               |
| `after-kill-pane`     | pane removed from layout                 |
| `pane-exit`           | inner process exit                       |
| `client-attached`     | client attach completed                  |
| `client-detached`     | client detach (any reason)               |
| `focus-changed`       | any client changes focus                 |
| `output-silenced`     | configurable silence threshold elapsed   |
| `output-active`       | first byte after a silence               |

---

## 10. Recording and playback

> **Status:** Design — implementation pending a ticket filed during the
> ADR-0013 follow-up sweep. Neither `phux capture --record` nor
> `phux play` exists in the crates today, but the underlying
> mechanism is now mechanical: under ADR-0013 the pane content on the
> wire *is* the byte stream we would want to record.

`phux capture --record TARGET --out FILE.cast` records a pane's session
to an [asciinema] v3-compatible file. v3 is a strict superset of v2 in
the features we need; players that only know v2 read v3 with reduced
fidelity rather than failing.

The record path is a tee on the server's outbound `PANE_OUTPUT` byte
stream for the target pane, wrapped in asciinema timing metadata.
There is no diff-to-bytes conversion step — the bytes are already
what we need.

Replay is `phux play FILE.cast` — a thin wrapper that streams the
recorded bytes into a new pane (via `INPUT_RAW`, where the server's
canonical `Terminal` parses them like any other PTY output). We do
not ship a full player; the ecosystem has plenty.

We do not record per-keystroke timing client-side; recordings reflect
output as the server emitted it. This matches what users expect and
keeps the recording infrastructure server-local.

[asciinema]: https://asciinema.org/

---

## 11. Things we explicitly do not ship

Repeating from `CONTRIBUTING.md` because the design decisions here lean
on these:

- **No embedded scripting language.** No tmux-style `if-shell`, no
  format-template DSL with conditionals. Templates are interpolation
  only.
- **No copy-mode reimplementation.** No vi/emacs cursor mode, no
  search, no in-grid selection. We expose grid state and stay out of
  the OS clipboard's way. Modern terminals (Ghostty, kitty, wezterm,
  iTerm2) handle selection well; we delegate.
- **No multi-row status bar, no widgets, no themes-as-config.** The
  status bar is one row. Themes are color slots, not a styling engine.
- **No plugin system on day one.** Hooks are typed events. Extensions
  shell out.
- **No homegrown crypto.** Transport is the right layer; SSH and Unix
  socket perms cover it.

---

## 12. Defaults table

The shipped defaults, in one place:

| Setting                       | Default                                  |
|-------------------------------|------------------------------------------|
| Shell                         | `$SHELL`, fallback `/bin/sh`             |
| History limit per pane        | 50 000 lines                             |
| Pane refresh rate cap         | 60 Hz                                    |
| Backpressure threshold        | 32 unacked frames                        |
| Journal size cap (per pane)   | 10 MiB ring                              |
| Prefix key                    | `ctrl+space`                             |
| Pane on PTY exit              | close                                    |
| Mouse                         | on                                       |
| Status bar                    | `{session}` / `{windows}` / `{date %H:%M}` |
| Activity / silence thresholds | activity off; silence 2 min when enabled |
| Resize on attach              | aggregate min bounding box per session   |
| Cursor blink                  | follow inner program request             |

---

## 13. First-time use

A new user, fresh install, no config file:

```sh
$ phux
# spawns server, creates session "default" with one window/one pane
# running $SHELL in $PWD
# attaches the client and renders
# status bar shows "default | 0:shell | 21:14"
# prefix is ctrl+space (advertised once in a startup message)
$ ctrl+space c    # new pane horizontally
$ ctrl+space d    # detach
$ phux            # re-attach to "default"; full state replayed
```

Discoverability: at startup the first time, the client prints one
non-intrusive message to the status bar:

```
phux 0.1 — prefix ctrl+space, ? for help, d to detach
```

That message disappears after 5 seconds or any keystroke, whichever
first.

Beyond that, `?` after the prefix opens a popup listing every binding.
The popup is rendered server-side (a temporary overlay pane) so
keyboard users and GUI clients both see the same list.

---

## 14. Out of scope, but on the radar

These are not in v0.1 but the design accommodates them so they don't
require breaking changes:

- **Resilient remote transport** (zmosh-style UDP/SSP). Hooks into the
  `Transport` abstraction in `SPEC.md` §4.
- **Native GUI client** (libghostty surface). Talks the same protocol
  as the TUI client — the client's `libghostty_vt::Terminal` already
  parses `PANE_OUTPUT` bytes locally (ADR-0013); a GUI client swaps
  the TUI's `RenderState`-to-VT renderer for a `RenderState`-to-GPU
  renderer and reuses everything else.
- **Multi-user shared sessions.** Today's protocol already supports
  multiple clients per session; ACL and identity will be a future
  authenticated transport addition.
- **Tabbed layouts** (nested tab containers). `SPEC.md` §10.3 reserves
  the `TABBED` layout node.
- **Image protocols** (sixel, kitty graphics). Under ADR-0013 these
  ride on the `PANE_OUTPUT` byte stream like any other VT sequence;
  per-client gating happens in the server's capability rewriter
  (SPEC §6.2). The `Sixel` / `KittyGraphics` / `Iterm2` capability
  bits already exist; the work is in the rewriter, not the wire
  format.
- **tmux control mode (CC) frontend.** Optional adapter that would let
  a CC-aware terminal (iTerm2 today; Ghostty when 1.4+ binds its
  parser to the GUI) render phux Terminals as native splits of that
  terminal. The native byte-stream protocol (ADR-0013) stays primary
  and strictly more capable; CC is one possible alternative consumer,
  not a roadmap commitment. Per
  [ADR-0017](./ADR/0017-tui-not-protocol-privileged.md) the
  reference TUI has no protocol-level privilege, so a CC adapter
  picks its tier set (typically L1+L3) the same way the native TUI
  does. The earlier `CC_FRONTEND` capability bit in `SPEC.md` §6.2
  is **reclaimed** under ADR-0017; no capability bit is needed.
