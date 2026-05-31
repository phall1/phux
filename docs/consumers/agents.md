---
audience: consumers, contributors, agents
stability: evolving
last-reviewed: 2026-05-29
---

# The phux agent CLI

**TL;DR.** The phux **agent CLI surface**: the structured subcommands
an AI agent drives without a TTY — `phux ls / snapshot / send-keys / run / wait`,
plus `new` to create a session. Each read verb has a `--json` machine shape
over the same client-side selector grammar the TUI uses, and reads are
side-effect-free (no attach, no resize). This file documents the per-verb
JSON contracts, the read-act-wait loop, and the exit-code semantics each
verb mirrors. For universal agent instructions, see [`../../AGENTS.md`](../../AGENTS.md).

---

## 0. What this is, what this isn't

This document is the **agent-facing CLI surface**, parallel to the
[TUI's product surface](./tui.md) and the
[MCP adapter](./mcp.md). Every consumer projects the one
source-of-truth libghostty `Terminal`
([ADR-0022](../../ADR/0022-tool-for-agents.md)); the TUI projects it to
VT bytes (it renders, like tmux), while agents project it to
**structured data** — cells, OSC-133 marks, command results.

There are three agent surfaces, and they nest:

- **This CLI** is the canonical, stable agent contract
  ([ADR-0022](../../ADR/0022-tool-for-agents.md)): the verbs and their
  `--json` shapes are what an agent depends on.
- [`mcp.md`](./mcp.md) is a thin adapter that wraps these same
  `phux-client` functions name-for-name over JSON-RPC stdio.
- [`sdk.md`](./sdk.md) is the future typed-Rust L1 handle
  (forward-looking, not yet shipped).

All three are unprivileged consumers
([ADR-0017](../../ADR/0017-tui-not-protocol-privileged.md)); none holds a
protocol-level privilege. The wire underneath stays additive and
versioned, normative under [`../spec/`](../spec/).

The selector grammar is owned by [`tui.md`](./tui.md) §3 — this file
links there rather than restating the table (the doc system's "one fact,
one home" rule). The decision rationale lives in
[ADR-0022](../../ADR/0022-tool-for-agents.md); client-side selector
resolution in
[ADR-0021](../../ADR/0021-control-plane-commands.md).

**Side-effect-free against a live pane.** `snapshot`, `run`,
`send-keys`, and `wait` neither attach nor resize the target pane: the
reads issue the `GET_SCREEN` control command (the server walks its own
grid), and input rides `ROUTE_INPUT` (events route to a pane by id). So
an agent can drive — or just watch — a pane a human is also attached to,
without disturbing that human's view.

---

## 1. The structured CLI surface (verb catalog)

phux is one binary; the verbs below are its agent-facing subcommands.
[`tui.md`](./tui.md) §1 has the full CLI table (which marks
`snapshot`/`send-keys`/`run`/`wait`/`kill` as shipped; its `ls` row still
reads `spec-only` even though the verb ships today); this section zooms
into the agent verbs and their JSON. Exit codes are collected in §4.2.

- **`phux ls [--json] [--socket P]`** — list sessions via `GET_STATE`.
  Does *not* auto-start a server (like `tmux ls`): with none running it
  reports as much and exits non-zero. `--json` emits `SessionListJson`
  (§3.1).
- **`phux snapshot [TARGET] [--json] [--scrollback[=N]] [--cells]
  [--socket P]`** — side-effect-free pane read via `GET_SCREEN`. `TARGET`
  is optional (defaults to the focused/last session). `--json` emits
  `ScreenState` (§3.2); without it, a boxed text view.
- **`phux send-keys TARGET KEYS... [--socket P]`** — route named keys or
  literal strings to one resolved pane by id (`ROUTE_INPUT`). `TARGET` is
  required. No JSON. `KEYS` are tmux-shaped: named keys (`Enter`, `Tab`,
  `Escape`, `Up`, `C-c`, `M-x`) or a literal string sent character by
  character.
- **`phux run TARGET CMD... [--timeout SECS] [--json] [--socket P]`** —
  run a command in a pane and capture its exit code, output, and
  duration via printed sentinels (assumes a POSIX shell: sh/bash/zsh).
  `TARGET` is required. `--json` emits `RunResult` (§3.3). The exit code
  mirrors the child (§4.2). **Gotcha:** flags MUST precede `CMD`, or
  clap's `trailing_var_arg` swallows them into the command line.
- **`phux wait [TARGET] [--until TEXT] [--idle MS] [--timeout SECS]
  [--json] [--socket P]`** — poll the side-effect-free screen read until
  a condition holds. `--until` takes precedence over `--idle`; with
  neither, it settles on idle. `--json` emits the final `ScreenState`.
  Exit 0 when the condition is met, 124 on timeout. **Gotchas:** flags
  must precede `TARGET`; and `--until` matches *any* visible row,
  including the shell's echo of the command you just typed — match on
  text that appears only in command *output*, never the command itself.
- **`phux new [-s NAME] [-c CWD] [-- COMMAND...] [--json] [--socket P]`** —
  create a **new** session. Without `--json` it creates *and attaches*:
  an explicit `-s NAME` that already exists is an error (like tmux's
  duplicate-session refusal); an omitted name is auto-assigned the
  smallest free numeric name (tmux-style); a server is auto-spawned if
  none is running. With `--json` it creates the session **without
  attaching** (no attach, no resize) via the `CREATE_SESSION` control
  command, whose create is atomic server-side — no `GET_STATE`→attach
  race — then prints the seed pane id as JSON and exits. `--json`
  **requires an explicit `-s NAME`** and errors if that name is already
  in use (create-only, never create-or-attach). Shape in §3.4.

**Socket precedence (once, for every verb).** The `--socket` argument
wins, then the `PHUX_SOCKET` environment variable, then the daemon
default: `$XDG_RUNTIME_DIR/phux/phux.sock`, falling back to
`/tmp/phux-$UID/phux.sock`. Per-verb rows above just say "see §1".

---

## 2. Targeting: the selector grammar

One grammar, every targeted command — `kill`, `snapshot`, `wait`,
`send-keys`, and `run` all share `TARGET`. It is resolved
**client-side** against a `GET_STATE` snapshot
([ADR-0021](../../ADR/0021-control-plane-commands.md)); the server never
parses a selector.

The full grammar table and CLI examples live in [`tui.md`](./tui.md) §3.
In one line, the forms are: `.` (current), `=` (last), `name` (session),
`name:N` / `name:tag` (window), `name:N.M` (pane), and `@N` (opaque id).

A selector that names several panes (a whole session or window) narrows
to a **single** pane: the focused pane when it is among the matches, else
the first in snapshot order (the `pick_target_pane` tiebreak the MCP
tools share). Optionality differs per verb: `snapshot` and `wait` default
`TARGET` to the last-focused session; `send-keys` and `run` require it.

---

## 3. JSON contracts (the per-verb machine shapes)

Each `--json` verb emits a versioned, plain-data struct from `phux-core`
or `phux-client`. These structs *are* the stable agent contract
([ADR-0022](../../ADR/0022-tool-for-agents.md)); the wire underneath
stays additive and versioned. Each struct carries its own
`schema_version`, tracked independently.

### 3.1 `SessionListJson` — `phux ls --json`

Defined in `crates/phux-core/src/session_list.rs`
(`LS_SCHEMA_VERSION = 1`). Shape, name-sorted:

```json
{
  "schema_version": 1,
  "sessions": [
    { "name": "work", "windows": 3, "attached": true }
  ]
}
```

`windows` is the window **count**; `attached` is a bool — whether any
client is attached. **Cross-surface gotcha:** the MCP `phux_ls` tool
([`mcp.md`](./mcp.md) §3.1) surfaces the raw wire fields
`window_count` / `attached_client_count`; the CLI's `--json` projects
them to `windows` / `attached`. The two surfaces do not share identical
keys — don't carry a parser across them.

### 3.2 `ScreenState` — `phux snapshot --json` (and `phux wait --json`)

Defined in `crates/phux-core/src/screen.rs` (`SCHEMA_VERSION = 3`). The
same struct the server returns from `GET_SCREEN`, not an agents-specific
shape. Fields:

| Field | Type | Meaning |
|---|---|---|
| `schema_version` | u32 | Contract version (currently `3`); the pin/branch signal. |
| `pane` | u32 | Wire-local id of the captured pane. |
| `cols`, `rows` | u16 | Grid dimensions. |
| `cursor` | `Option<{x,y,visible}>` | Viewport-relative, zero-based; `None` when the cursor is not viewport-resident (scrollback or hidden). |
| `lines` | `Vec<String>` | Viewport rows, top to bottom, right-trimmed. |
| `scrollback` | `Vec<String>` | History rows above the viewport, oldest first; empty unless requested. |
| `cells` | `Option<Vec<CellInfo>>` | Per-cell marks + styles; present only with `--cells`. |

**`scrollback` is tri-state** (and load-bearing — mirrors
[`mcp.md`](./mcp.md) §3.2): flag **absent** → viewport only;
`--scrollback` or `--scrollback=0` → **all** retained history;
`--scrollback N` → the most-recent `N` rows. On the wire this is
`None` / `Some(0)` (all) / `Some(n)`.

**`--cells`** populates `cells` with a sparse `Vec<CellInfo>` — only
cells carrying a non-default style or an OSC-133 mark, in row-major
order, skipping the right half of double-width glyphs. Each `CellInfo`
is `{ col, row, semantic?, style }`:

- `semantic` is `SemanticContent` — `Input` (typed input) or `Prompt`
  (shell prompt). `Output` is the default for every cell and is collapsed
  to absence, so `semantic` is `Some` only for marked input vs prompt.
- `style` is `CellStyle`: nine SGR booleans (`bold`, `faint`, `italic`,
  `underline`, `blink`, `inverse`, `invisible`, `strikethrough`,
  `overline`) plus `fg` / `bg`, each a `CellColor` tagged enum with
  `kind` of `default`, `palette` (`{ index }`), or `rgb` (`{ r, g, b }`).
  The tag distinguishes "terminal default" from "explicitly black".

**Back-compat.** `scrollback` and `cells` are `#[serde(default)]` (and
`cells` is `skip_serializing_if` `None`), so a `cells = None` snapshot
serializes to exactly the pre-cells shape, and an older consumer reading
a newer payload ignores extra keys. `schema_version` is the bump signal.

### 3.3 `RunResult` — `phux run --json` (on completion)

Defined in `crates/phux-client/src/run.rs`:

```json
{
  "command": "cargo test",
  "exit_code": 0,
  "output": "...",
  "duration_ms": 8123,
  "truncated": false
}
```

- `exit_code` (i32) is the child's `$?`, parsed out of a printed
  sentinel (`run` brackets the command with `BEGIN`/`RC` markers — it
  does not rely on shell integration).
- `output` is the rows between the `BEGIN` and `RC` markers.
- `duration_ms` (u64) is wall-clock from submit to sentinel-seen,
  **including poll latency** — an upper bound on the child's runtime, not
  a precise measurement.
- `truncated` is `true` when the `BEGIN` marker had scrolled out of the
  viewport, so `output` is best-effort visible context; a full capture
  needs scrollback (phux-o1v).

**On timeout, `run --json` emits no JSON.** `RunOutcome::TimedOut`
carries the command, elapsed time, and last screen internally, but the
CLI's `--json` path serializes only the completed `RunResult`. The
timeout signal is the **exit code** (125 — see §4.2), printed alongside a
stderr diagnostic. An agent must read the exit code here and must *not*
expect an `outcome: "timed_out"` body — that shape exists in the MCP
`phux_run` tool ([`mcp.md`](./mcp.md) §3.4), not in the CLI's `--json`
output.

### 3.4 `phux new --json`

`phux new --json -s NAME` emits a small fixed object naming the created
session and its seed pane's wire-local id, then exits `0` without
attaching:

```json
{ "session": "NAME", "terminal_id": 2 }
```

It is create-only: `--json` requires an explicit `-s NAME` and errors
(exit `1`) if that name is already in use. Unlike the versioned
`ScreenState` / `RunResult` / `SessionListJson` shapes, this is a flat
ad-hoc object with no `schema_version`.

---

## 4. The read-act-wait loop + exit-code mirroring

### 4.1 The loop

The canonical agent pattern is **read → act → wait → read**: snapshot the
pane, send input or run a command, wait for the result to land, snapshot
again. A worked example in `sh`:

```sh
phux send-keys build "cargo test" Enter
phux wait build --until "test result:" --timeout 120
phux snapshot build --json --scrollback 200 > out.json
```

When you only want a command's exit code and output, the one-shot
`phux run` is the higher-level alternative — it brackets the command with
sentinels and mirrors `$?`:

```sh
phux run build "cargo test" --json
```

The contrast: `run` is "I want the exit code"; `send-keys` + `wait` is
"I'm driving an interactive or long-lived program." Because `run` mirrors
the child's code (§4.2), `phux run ... && next` composes like a shell
([ADR-0022](../../ADR/0022-tool-for-agents.md) §3).

### 4.2 Exit-code mirroring

Exit codes are **not uniform across verbs** — this is the load-bearing
table:

| Verb | Exit codes |
|---|---|
| `ls` | `0` ok; `1` no server / unexpected result. |
| `snapshot` | `0` ok; `1` failure (no server, serialize error, resolve miss). |
| `send-keys` | `0` ok; `1` failure (no server / refused / miss). |
| `run` | the child's own code clamped to `0..=255` (negative or `>255` saturate to `255`); `125` when phux gave up waiting for the sentinel (`--timeout`); `1` for no server / refused target / other. |
| `wait` | `0` condition met; `124` on `--timeout`; `1` no server / parse / read error. |
| `new` | `0` ok; `1` duplicate `-s` name / failure. |
| `kill` | `0` ok; `1` selector miss / no server / parse; `2` server-side refusal. |

**Why `run` uses 125, not 124.** `run` mirrors the child's own code into
`0..=255`, and `124` is a code real commands produce — notably GNU
`timeout`. So `run` reserves `125` (the wrapper-failure convention, as
used by `env` and `timeout`) for "phux itself gave up," keeping it
distinct from a child that legitimately exited `124`. `wait`, which wraps
nothing, uses `124` for its own timeout. `kill` is a control-plane verb
(not strictly an agent read) but shares `TARGET`; its `0`/`1`/`2` triad
is listed for completeness.

---

## 5. Relationship to the other agent surfaces

The CLI verbs here are the stable contract. The [MCP adapter](./mcp.md)
exposes them name-for-name (`phux_ls` ↔ `ls`, and
`phux_snapshot` / `phux_send_keys` / `phux_run` / `phux_wait` ↔ the
matching subcommands) over the same `phux-client` functions — same
client-side resolution, same tiebreaks. [`sdk.md`](./sdk.md) is the
future typed-Rust L1 handle. All three are unprivileged consumers
([ADR-0017](../../ADR/0017-tui-not-protocol-privileged.md)); the wire
underneath stays additive and versioned under [`../spec/`](../spec/)
([ADR-0022](../../ADR/0022-tool-for-agents.md)).
