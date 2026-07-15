---
name: phux-terminal
description: Use phux for persistent or interactive terminal work that must survive across agent steps. Covers honest read/mutate/observe semantics, shipped watch events, bounded waits, and the full read-decide-act-observe loop; prefer a one-shot shell for independent commands.
---

# phux: persistent terminals for agents

Use phux when a process, shell state, or interactive conversation must remain
alive across steps. A phux pane is a real PTY-backed terminal owned by the
server, not a transcript created for one tool call. Agents drive it through the
`phux` CLI while a human may view the same terminal from an attached client.

For multi-pane creation, placement, MCP parity, and human attention routing,
continue with the [`phux-agent-cli` skill](../phux-agent-cli/SKILL.md).

## When phux is the right tool

Use phux for:

- a shell whose `cd`, exports, venv, or login state must persist;
- a REPL, debugger, installer, database shell, or other prompt conversation;
- a dev server or long-running job that must remain inspectable;
- a terminal a human and an agent need to observe at the same time;
- event-driven observation of output activity, lifecycle, bells, titles, or
  agent asks.

Use the normal one-shot shell tool for independent commands whose process can
exit with the call. It is simpler and cheaper.

## The honest operation classes

Do not call every headless operation "side-effect-free":

- **Reads:** `ls`, `snapshot`, and the polling performed by `wait` observe
  server state without attaching, resizing, typing, or moving focus.
- **Mutations:** `send-keys` and `run` send real input to the live PTY. They do
  not attach or resize, but they intentionally change the target program.
- **Observation streams:** shipped `watch` subscribes to pushed events without
  attaching or resizing. The CLI stream is long-lived and must be bounded by
  the caller.
- **Lifecycle:** `new`, `spawn`, placement/layout verbs, signals, and `kill`
  create or change shared resources. Confirm destructive actions first.

No headless verb changes an attached human's client-local focus. Shared layout
metadata is topology, not focus authority.

## Core commands

```sh
phux ls --json
phux new --json -s work -c "$PWD"          # create without attaching
phux snapshot --json work                   # side-effect-free viewport read
phux snapshot --json --scrollback 200 @42   # include retained history
phux send-keys @42 "python3" Enter          # real input mutation
phux run --json --timeout 120 @42 "pytest" # mutation + bounded completion
phux wait --until "ready" --timeout 60 @42  # bounded screen condition
phux watch --json @42                       # shipped JSONL event stream
phux ask @42 --id question --json "Need input?"
```

A `TARGET` may be a session (`work`), window (`work:1` or `work:editor`),
pane (`work:1.0`), local id (`@42`), satellite id (`devbox/@7`), tag set where
accepted (`#build`), or `.` for the focused session. Headless `=` is rejected:
previous-focus history belongs to an attached TUI.

Flags belong after the verb and before trailing keys/command words:

```sh
phux run --json --timeout 120 --socket /tmp/phux.sock @42 "cargo test"
phux send-keys --socket /tmp/phux.sock @42 "continue" Enter
```

## Read: snapshot the real screen

`phux snapshot --json` returns a versioned `ScreenState` with pane id,
dimensions, cursor, right-trimmed viewport `lines`, optional `scrollback`, and
optional sparse styled/semantic `cells`.

```sh
screen=$(phux snapshot --json --scrollback 100 @42)
```

A visible cursor is useful evidence that a prompt is waiting, but not proof of
a particular program state. Read the lines and use a program-specific marker.
Snapshot is safe to poll because it does not attach or resize.

## Act: choose `run` or `send-keys`

Use `run` for a discrete POSIX-shell command with a completion boundary:

```sh
set +e
result=$(phux run --json --timeout 300 @42 "cargo test")
status=$?
set -e
```

On completion, JSON reports `command`, child `exit_code`, captured `output`,
`duration_ms`, and `truncated`. The phux process mirrors the child code. Exit
`125` means phux timed out and there is no completed `RunResult`. `run` cannot
capture `exit` or `exec`, because replacing/ending the shell removes its
sentinel; use a subshell when testing a nonzero exit.

Use `send-keys` only when the interaction itself matters:

```sh
phux send-keys @42 "python3" Enter
phux send-keys @42 "print(6 * 7)" Enter
phux send-keys @42 C-c
```

Arguments are named keys or literal characters sent to the PTY. This is not a
clipboard operation. Avoid typing into a pane while a human is editing it.

## Observe: bounded wait and shipped watch

Always give `wait` a finite timeout:

```sh
phux wait --until "Listening on" --timeout 60 @42
phux wait --idle 750 --timeout 30 @42
```

`--until` matches any visible row, including command echo. Match a value that
only output produces. Exit `124` means the condition was not met in time.

`watch` is shipped and push-driven:

```sh
phux watch --json @42 >events.jsonl & watch_pid=$!
( sleep 30; kill "$watch_pid" 2>/dev/null || true ) &
wait "$watch_pid" || true
```

JSONL events include title, bell, dirty/idle, pane spawn/close, and `asked`.
Command-boundary event tags exist, but current server emission is incomplete;
do not use them as the only completion signal. Use `run` or bounded `wait` for
completion and `watch` to reduce observation latency.

A watcher ending is not proof that the terminal's work succeeded. Re-read the
screen, agent state, or inventory.

## Advisory asks and human focus

`phux ask` reports a pending human-answerable question through the existing
`asked` event. Surface its terminal, question, suggestions, and elapsed time.
The attached TUI badges it. Tell the human:

```text
Press C-a q to visit the next asking pane; press C-a Q to return once.
```

Do not send those chords into the pane or claim that the agent moved focus.
Attention navigation is local to the human's TUI. Merely visiting does not
clear attention; forwarded input does.

## The full persistent-terminal loop

```text
DISCOVER  phux ls --json; choose an explicit target
READ      phux snapshot --json TARGET
DECIDE    inspect lines/cursor/state; choose one narrow action
ACT       phux run ... or phux send-keys ...
OBSERVE   bounded phux wait and/or caller-bounded phux watch
READ      snapshot again; verify the effect rather than the echoed input
SURFACE   asked events as advisory human guidance
REPEAT    while the persistent interaction still has a reason to live
CONFIRM   before signal/kill; then verify lifecycle under a bound
```

Example:

```sh
phux new --json -s repl -c "$PWD"
phux send-keys repl "python3" Enter
phux wait --until ">>>" --timeout 20 repl
phux send-keys repl "print(6 * 7)" Enter
phux wait --until "42" --timeout 20 repl
phux snapshot --json repl
```

## Safety and lifetime

- Before a destructive signal or `kill`, display the exact target, snapshot
  relevant state, explain the loss, and obtain affirmative human confirmation.
- Sessions persist until explicitly killed or the server exits; they are not
  jobs scheduled by this skill. Re-discover them with `phux ls`.
- Short-lived CLI calls do not create a durable input-authority lease.
- The local socket access model is assumed; this skill does not configure
  remote credentials or trust.
- Output may be viewport-bounded. Request scrollback and respect `truncated`.

Runnable single-pane loops live in [`examples/agents/`](../../agents/). The
placed-fleet example there demonstrates the larger orchestration loop without
moving human focus.
