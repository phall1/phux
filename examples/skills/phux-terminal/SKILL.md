---
name: phux-terminal
description: Drive persistent, interactive terminal sessions via the phux CLI instead of one-off shells. Use when work needs session state across steps (a long-lived shell, a REPL, an activated venv, a running dev server), an interactive/TUI program (installer prompts, gdb, vim, ssh password), or watching a long-running process — i.e. anything a stateless one-shot `bash` call handles badly.
---

# phux: a persistent terminal for agents

A normal shell tool is **stateless** — each call is a fresh process, output
on exit, no live session. phux gives you a **persistent control plane over
real terminals**: long-lived panes with state, structured screen reads, and
structured input. You drive it entirely through the `phux` binary; no TTY,
no tmux.

This is a v0 surface (ADR-0022). It already supports the read+act loop;
richer verbs (`run`, `watch`) are landing.

## When to use phux vs a one-shot shell

Use phux when **state must persist across your steps** or the program is
**interactive**:
- a shell where `cd`/`export`/venv must survive to the next command
- a REPL (python, node, psql) you converse with
- a TUI / prompting program (installers, `gdb`, `ssh` password, menus)
- a long-running process you start, then check on later

Keep using your normal shell tool for **independent one-shot commands** —
it's lower overhead.

## Core verbs

```sh
phux ls --json                       # sessions on the running server (JSON)
phux new -s NAME                     # create a session (auto-starts a server)
phux snapshot NAME --json            # read the focused pane as structured JSON
phux send-keys NAME KEY...           # send input to the focused pane
phux kill NAME                       # destroy a session/pane
```

`--socket PATH` overrides the UDS. **Flags must precede `send-keys`' trailing
keys** (`phux send-keys --socket P NAME ...`), or they get parsed as keys.

### Reading the screen — `snapshot`

```jsonc
$ phux snapshot work --json
{ "schema_version": 1, "pane": 1, "cols": 80, "rows": 24,
  "cursor": { "x": 7, "y": 2, "visible": true },
  "lines": ["$ cargo build", "   Compiling …", "$"] }
```

`lines` is the viewport, top to bottom, right-trimmed. Parse `lines` to see
what's on screen; use `cursor` to tell whether a prompt is waiting.

### Sending input — `send-keys`

Each argument is a **named key** or a **literal string** (sent char by
char), tmux-style:

```sh
phux send-keys work "cargo test" Enter      # type a command and run it
phux send-keys work C-c                      # interrupt
phux send-keys work "print(2+2)" Enter       # talk to a REPL
phux send-keys work Up Up Enter              # history recall
```

Named keys: `Enter` `Tab` `Escape` `Space` `BSpace` `Up` `Down` `Left`
`Right` `Home` `End`, `C-<x>` (control), `M-<x>` (alt). Anything else is a
literal string.

## The read+act loop

```sh
phux new -s work
phux send-keys work "make build" Enter
# ... poll until it settles ...
phux snapshot work --json     # read result; check for errors / the prompt
phux send-keys work "make test" Enter
```

Until `phux wait`/`run` land, "wait for it to finish" means: `snapshot` in a
loop until the prompt returns (cursor back at a prompt line) or the output
stops changing. Don't busy-spin; sleep briefly between snapshots.

## Gotchas (v0)

- **`snapshot` is side-effect-free** — it reads the server's own grid
  (`GET_SCREEN`), so it never attaches or resizes the pane and is safe to
  poll, even against a pane a human is using.
- **`send-keys` attaches transiently**, which can resize the pane to 80x24
  for a moment (it self-heals): the server only accepts input from an
  attached client. Avoid firing it against a pane a human is mid-keystroke
  in. A side-effect-free input route is coming.
- **No command-exit yet.** You can't get a command's exit code directly; read
  the screen and infer (or run `echo $?` and snapshot). `phux run` will fix
  this.
- The server is **one per user**; sessions persist until killed or the
  machine reboots — that's the point. Use `phux ls` to find them again.
