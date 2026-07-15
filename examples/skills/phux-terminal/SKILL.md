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

This is a v0 surface (ADR-0022). It supports the full read+act+wait loop
today (`ls` / `snapshot` / `send-keys` / `run` / `wait` / `new`); a
push-driven `watch` is still landing.

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
phux snapshot TARGET --json          # read a pane as structured JSON
phux send-keys TARGET KEY...         # send input to a pane
phux run TARGET "CMD"                # run a command, get its exit code + output
phux wait TARGET --until TEXT        # block until the screen shows TEXT
phux kill TARGET                     # destroy a session/window/pane
```

`TARGET` is a selector: a session name (`work`), a window (`work:1` or
`work:editor`), a pane (`work:1.0`), an opaque id (`@42`), or `.` for the
focused session. Headless `=` is unsupported because no attached-client focus
history exists. A session-wide target resolves to its focused pane; a `:window.pane` target hits exactly that pane. `--socket PATH`
overrides the UDS. **Flags must precede `send-keys`' trailing keys**
(`phux send-keys --socket P TARGET ...`), or they get parsed as keys.

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

### Running a command — `run`

For "run this and tell me the exit code," reach for `run` instead of
`send-keys` + polling. It submits the command, waits for it to finish, and
reports structured results — and the `phux` process **exits with the
command's own code**, so it composes like a shell:

```jsonc
$ phux run build "cargo test" --json
{ "command": "cargo test", "exit_code": 0,
  "output": "test result: ok. 42 passed; 0 failed",
  "duration_ms": 5130, "truncated": false }
```

```sh
phux run build "make" && phux run build "make install"   # chains on success
```

`run` assumes a **POSIX shell** (sh/bash/zsh): it appends a sentinel to read
`$?`. It can't capture a command that *replaces* the shell (`exit`, `exec`)
— that kills the session. If `truncated` is true, the output scrolled past
the viewport (full capture awaits scrollback support).

### Waiting on a condition — `wait`

When you've started something with `send-keys` and want to block until it's
ready (exit 0 when met, 124 on `--timeout`):

```sh
phux wait server --until "Listening on"     # a line appears
phux wait repl   --idle 750                  # screen stops changing for 750ms
phux wait job    --until DONE --timeout 60   # ...or give up after 60s
```

`--until` matches **any visible line, including the echo of a command you
just typed** — so match on text that only appears in *output*, not in the
command itself.

## The read+act loop

```sh
phux new -s work
phux run work "make build"    # blocks, returns exit code
phux run work "make test"     # only if you chain; or check exit codes
```

Prefer `run` for discrete commands (you get the exit code for free). Drop to
`send-keys` + `wait`/`snapshot` for interactive programs, REPLs, or
backgrounded work where there's no single "command done" moment.

## Gotchas (v0)

- **`snapshot` is side-effect-free** — it reads the server's own grid
  (`GET_SCREEN`), so it never attaches or resizes the pane and is safe to
  poll, even against a pane a human is using.
- **`send-keys`/`run` are side-effect-free too.** They resolve the target to
  a pane id client-side and route input by id (`ROUTE_INPUT`), so they
  neither attach nor resize the pane. Still, they *type into a live pane* —
  avoid firing them at a pane a human is mid-keystroke in.
- **`run` output is viewport-bounded.** If a command prints more than fits on
  screen, `output` is the visible tail and `truncated` is true. Full capture
  awaits scrollback support.
- The server is **one per user**; sessions persist until killed or the
  machine reboots — that's the point. Use `phux ls` to find them again.

## Driving phux from code

When you're scripting phux from another program rather than running verbs
by hand, see the `phux-agent-cli` skill — it covers the stable `--json`
contracts, flag placement, exit-code mirroring, and a `Phux` wrapper
class. Runnable, tested examples live in
[`examples/agents/`](../../agents/).
