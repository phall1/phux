# examples/agents — driving phux end to end from the CLI

Runnable, self-documenting scripts that show an agent operating phux
**purely through the `phux` binary** — no client library, no TTY, no
tmux. The CLI *is* the agent surface (ADR-0022); anything that can spawn
a process and read JSON can drive a real terminal this way.

Each script stands up its own throwaway server on a private socket (see
[`lib.sh`](./lib.sh)) so running one never touches the user's real
one-per-user server. **A production agent skips all of that** and just
runs `phux <verb>` against the user's existing server, with no
`--socket`.

## The loop these examples teach

```
loop:
  READ  the screen     ->  phux snapshot --json   (side-effect-free)
  DECIDE what to do     ->  parse `lines` / `cursor`
  ACT   send input      ->  phux send-keys ...     (or phux run for a discrete command)
  WAIT  for the effect  ->  phux wait --until / --idle   (bounded by --timeout)
```

`run` collapses ACT+WAIT into one call for discrete commands (it returns
the exit code); drop to `send-keys` + `wait` for REPLs, TUIs, prompts, and
backgrounded work where there is no single "command done" moment.

## The scripts

| File | Verbs shown | Teaches |
|---|---|---|
| [`01-ls-and-snapshot.sh`](./01-ls-and-snapshot.sh) | `ls`, `ls --json`, `snapshot`, `snapshot --json` | The two side-effect-free reads: enumerate sessions, read a pane. |
| [`02-run-and-exit-codes.sh`](./02-run-and-exit-codes.sh) | `run`, `run --json` | Discrete commands; exit-code mirroring; `&&`/`||` chaining. |
| [`03-send-keys-and-wait.sh`](./03-send-keys-and-wait.sh) | `send-keys`, `wait --until`, `wait --idle`, `--timeout` | Structured input; blocking on a condition; driving a REPL. |
| [`04-read-act-wait-loop.sh`](./04-read-act-wait-loop.sh) | all of the above | The full read+act+wait loop against an interactive prompt. |
| [`agent_loop.py`](./agent_loop.py) | all of the above | The same loop as code: subprocess calls + JSON parsing, no phux library. |

## Running them

```sh
bash examples/agents/01-ls-and-snapshot.sh
python3 examples/agents/agent_loop.py
```

They locate a `phux` binary in this order: `$PHUX`, then a `phux` on
`PATH`, then `target/debug/phux` (building it via `nix develop -c cargo
build -p phux` if missing — the dev shell provides the zig toolchain
libghostty-vt needs). The first build can be slow; subsequent runs reuse
it.

Only the bash scripts depend on `bash` + `python3` (used purely to parse
JSON in the output-extraction snippets). `phux` itself needs neither.

## Two gotchas the scripts bake in

- **`--socket` is a per-subcommand flag** and must precede a verb's
  trailing positional args (`phux send-keys --socket P NAME KEY...`, not
  `phux --socket P send-keys ...`). The `phux()` wrapper in `lib.sh`
  inserts it in the right place.
- **`wait --until` matches the echo of the command you just typed**, not
  only its output. Match on text that appears *only* in output — the
  examples wait on a value the program *computes at runtime*
  (`result=49`), which never appears in the command's own source.

## See also

- [`../skills/phux-terminal/SKILL.md`](../skills/phux-terminal/SKILL.md) —
  when to reach for phux over a one-shot shell, and the `run`/`wait`
  surface in prose.
- [`../skills/phux-agent-cli/SKILL.md`](../skills/phux-agent-cli/SKILL.md) —
  the JSON-driven CLI-wrapping playbook that points at these scripts.
- [`../../docs/consumers/tui.md`](../../docs/consumers/tui.md) — the full
  CLI shape.
- [`../../ADR/0022-tool-for-agents.md`](../../ADR/0022-tool-for-agents.md) —
  why the CLI is the agent surface.
