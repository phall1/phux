---
name: phux-agent-cli
description: Wrap the phux CLI as a programmable terminal-control surface for an agent — enumerate sessions, read panes, send input, and run commands using the stable --json contracts and exit-code mirroring. Use when you are scripting phux from another program/agent and need machine-parseable I/O and a read+act+wait loop, rather than the prose "when to use phux" guidance.
---

# phux as a programmable surface: the JSON CLI playbook

This is the **machine-facing** companion to the `phux-terminal` skill.
`phux-terminal` answers *when* to reach for a persistent terminal and
walks the `run`/`wait` verbs in prose. This skill answers *how to wrap
the CLI in code*: which verbs emit stable JSON, how to parse it, how exit
codes compose, and the read+act+wait loop an agent runs. **Read
`phux-terminal` first** — this doc does not repeat its when-to-use
guidance or its verb reference.

Runnable, tested versions of everything below live in
[`examples/agents/`](../../agents/) (`01`–`04` plus `agent_loop.py`).
Start there if you want to copy working code.

## The CLI is the surface (no library)

There is no phux client SDK you link against. Every interaction is a
`phux <verb>` subprocess call; the JSON verbs print a versioned contract
to stdout (ADR-0022). Any language that spawns processes and parses JSON
can drive a real terminal. The one-per-user server is implicit — omit
`--socket` and you hit it.

## The stable JSON contracts

Each carries a `schema_version` so you can pin or branch.

```jsonc
// phux ls --json     -> sessions on the server
{ "schema_version": 1,
  "sessions": [ { "name": "build", "windows": 1, "attached": false } ] }

// phux snapshot NAME --json   -> a pane's viewport (side-effect-free read)
{ "schema_version": 1, "pane": 1, "cols": 80, "rows": 24,
  "cursor": { "x": 2, "y": 10, "visible": true },
  "lines": ["$ cargo build", "   Compiling …", "$"] }

// phux run NAME "CMD" --json  -> a finished command's result
{ "command": "cargo test", "exit_code": 0,
  "output": "test result: ok. 42 passed", "duration_ms": 5130,
  "truncated": false }

// phux wait NAME --until TEXT --json  -> the final ScreenState (same shape
//                                        as snapshot) once the condition is met
```

`snapshot.lines` is the viewport, top to bottom, right-trimmed; `cursor`
tells you whether a prompt is waiting (`null` when off-viewport/hidden).

## Flag placement (the one parsing trap)

Every verb's first positional is a `TARGET` selector — a session (`build`),
a window (`build:1`), a pane (`build:1.0`), an opaque id (`@42`), or `.`/`=`
for the focused / last-focused session. A session-wide target resolves to
its focused pane; a `:window.pane` target hits exactly that pane.

`--socket`, `--json`, `--timeout`, `--until`, `--idle` are **per-verb**
flags and must come **after the verb but before its trailing positional
args** (`send-keys`/`run` slurp everything after the target):

```sh
phux ls --json                          # ok
phux run --json --timeout 30 build "cargo test"   # flags BEFORE the command
phux send-keys --socket P build "cargo test" Enter # --socket BEFORE the keys
```

`phux --socket P ls` (flag before the verb) is rejected — `--socket`
is not a top-level flag.

## Exit codes are part of the contract

- `phux run` **mirrors the command's own exit code** into its process
  exit, so it composes in shell control flow and `subprocess` checks:

  ```sh
  phux run build "make" && phux run build "make install"   # chains
  ```

  In code, treat a non-zero `run` as data, not an exception (read
  `exit_code` from the JSON; don't let your subprocess wrapper raise).
- `phux wait` exits `0` when the condition is met, **`124` on
  `--timeout`** — use it to bound a poll instead of an exception.
- `phux run` itself returns **`125`** (not a mirrored child code) when it
  gives up waiting for its own sentinel — distinguishes "phux timed out"
  from "the child exited 125".
- `phux ls` / `snapshot` exit non-zero when no server is running.

## The read+act+wait loop, in code

```python
phux = Phux(binary, socket, session)   # thin subprocess wrapper

# discrete command: ACT+WAIT in one call, exit code as data
r = phux.run("cargo build")            # -> {"exit_code": 0, ...}

# interactive: READ -> DECIDE -> ACT -> WAIT
phux.send_keys("./configure", "Enter")
phux.wait_until("Proceed?")            # block until prompt is on screen
scr = phux.snapshot()                  # READ lines/cursor, decide
phux.send_keys("y", "Enter")           # ACT
phux.wait_until("result=49")           # WAIT on OUTPUT, not your echoed key
```

See [`examples/agents/agent_loop.py`](../../agents/agent_loop.py) for the
full `Phux` wrapper (one method per verb) and
[`04-read-act-wait-loop.sh`](../../agents/04-read-act-wait-loop.sh) for
the shell version.

## Two correctness rules for honest matching

- **Wait on output, not on your own keystrokes.** `wait --until` matches
  *any* visible line, including the shell's echo of the command you typed.
  Match on text that appears only in output — e.g. a value the program
  *computes* (`result=49`), never a literal in the command itself.
- **Don't assume the pane's shell is POSIX.** The seed pane runs the
  user's `$SHELL` (maybe zsh/fish). If you send a snippet that relies on
  sh semantics (`read -p`, `[ ... ]`), wrap it in `sh -c '...'`. `run`
  already assumes a POSIX shell for its `$?` sentinel.

## Boundaries (v0)

- All four verbs are side-effect-free: `snapshot`/`wait` read via
  `GET_SCREEN`, and `send-keys`/`run` route input by pane id (`ROUTE_INPUT`)
  without attaching or resizing the pane. They still *type into a live
  pane*, so avoid firing them at a pane a human is mid-keystroke in.
- `run`/`snapshot` output is viewport-bounded; `truncated: true` means it
  scrolled past the viewport (full capture awaits scrollback).
- `run` cannot capture a command that *replaces* the shell (`exit`,
  `exec`) — that kills the pane. Use a subshell (`sh -c 'exit 7'`) to
  observe a non-zero code without killing the session.
