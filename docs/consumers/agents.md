---
audience: consumers, contributors, agents
stability: evolving
last-reviewed: 2026-06-06
---

# The phux agent CLI

**TL;DR.** The structured CLI surface an AI agent drives without a TTY:
`phux ls / snapshot / send-keys / run / wait / watch / ask`, plus `new` to
create a session, `plugin` to manage configured plugin manifests, `config run`
to invoke configured plugin actions, `workspace` to map git worktrees before
spawning agent panes, and `satellite` to manage hub-side federation entries.
This file is the agent contract. Per ADR-0030, the structured agent state —
cells, command results, semantic events — is a local projection over the shared
engine, and the CLI plus its versioned JSON schemas are what an agent depends
on, not a structured wire tier. It documents each verb, its JSON shape, the
read-act-wait loop, and the exit codes each verb mirrors.

---

## 0. The thesis: structured agent state is a projection

phux does not own terminal semantics; libghostty does, and both ends of the
wire run that engine
([ADR-0013](../../ADR/0013-libghostty-bytes-on-wire.md)). It follows that any
structured view of a terminal — a cell grid, an OSC-133 command-boundary
stream, a command's captured output — is computed by a consumer from the
engine it already has, not transmitted as a second model on the wire
([ADR-0030](../../ADR/0030-engine-delegated-wire-and-projection-consumers.md)).

So the agent contract is **not** a structured wire protocol. It is this CLI and
the versioned JSON schemas its `--json` verbs emit. The wire carries opaque
terminal bytes plus lifecycle and metadata; the structured shapes below are a
local projection an agent reads through the CLI
([ADR-0022](../../ADR/0022-tool-for-agents.md): agents are a projection, the
CLI plus JSON schema is the contract). An agent that wants to own its own
projection — run the engine and read its grid directly — should copy
[phux-web](./web.md), the reference carry-your-own-engine consumer
(ADR-0030 §4).

The live wire does expose agent affordances: `GET_SCREEN`, `ROUTE_INPUT`,
`GET_TERMINAL_STATE`, `SUBSCRIBE_TERMINAL_EVENTS`, and an `AgentEvent` push
frame, documented in [`../spec/L1.md`](../spec/L1.md). Read those as
engine-convenience snapshots over the shared engine — a convenience for
consumers that have not adopted the carry-your-own-engine pattern — not a
normative structured contract and not a license to add new structured wire
surface (ADR-0030 §2).

## 1. What this is, what this isn't

This document is the agent-facing CLI surface, parallel to the
[TUI's product surface](./tui.md) and the [MCP adapter](./mcp.md). The TUI
projects the source-of-truth `Terminal` to VT bytes (it renders, like tmux);
agents project it to structured data — cells, OSC-133 marks, command results.

The agent surfaces nest:

- **This CLI** is the canonical, stable agent contract: the verbs and their
  `--json` shapes are what an agent depends on.
- [`mcp.md`](./mcp.md) is a thin adapter that wraps the same `phux-client`
  functions name-for-name over JSON-RPC stdio.
- [`sdk.md`](./sdk.md) documents `phux-client` itself — the library crate the
  CLI and MCP adapter are both built from. It exists today; it is L1-shaped and
  follows the same projection pattern.

All three are unprivileged consumers
([ADR-0017](../../ADR/0017-tui-not-protocol-privileged.md)); none holds a
protocol-level privilege. The wire underneath stays additive and versioned,
normative under [`../spec/`](../spec/).

The selector grammar is owned by [`tui.md`](./tui.md) §3; this file links there
rather than restating the table (the doc system's one-fact-one-home rule). The
decision rationale lives in
[ADR-0022](../../ADR/0022-tool-for-agents.md); client-side selector resolution
in [ADR-0021](../../ADR/0021-control-plane-commands.md).

**Side-effect-free against a live pane.** `snapshot`, `run`, `send-keys`, and
`wait` neither attach nor resize the target pane: the reads issue the
`GET_SCREEN` control command (the server walks its own grid), and input rides
`ROUTE_INPUT` (events route to a pane by id). An agent can drive — or just
watch — a pane a human is also attached to, without disturbing that human's
view.

## 2. The structured CLI surface (verb catalog)

phux is one binary; the verbs below are its agent-facing subcommands.
[`tui.md`](./tui.md) §1 has the full CLI table; this section zooms into the
agent verbs and their JSON. Exit codes are collected in §5.2.

- **`phux ls [--json] [--socket P]`** — list sessions. Does not auto-start a
  server (like `tmux ls`): with none running it reports as much and exits
  non-zero. `--json` emits `SessionListJson` (§4.1).
- **`phux snapshot [TARGET] [--json] [--scrollback[=N]] [--cells]
  [--socket P]`** — side-effect-free pane read via `GET_SCREEN`. `TARGET` is
  optional (defaults to the focused/last session). `--json` emits `ScreenState`
  (§4.2); without it, a boxed text view.
- **`phux send-keys TARGET KEYS... [--socket P]`** — route named keys or
  literal strings to one resolved pane by id (`ROUTE_INPUT`). `TARGET` is
  required. No JSON. `KEYS` are tmux-shaped: named keys (`Enter`, `Tab`,
  `Escape`, `Up`, `C-c`, `M-x`) or a literal string sent character by
  character.
- **`phux run TARGET CMD... [--timeout SECS] [--json] [--socket P]`** — run a
  command in a pane and capture its exit code, output, and duration via printed
  sentinels (assumes a POSIX shell: sh/bash/zsh). `TARGET` is required.
  `--json` emits `RunResult` (§4.3). The exit code mirrors the child (§5.2).
  Flags must precede `CMD`, or clap's `trailing_var_arg` swallows them into the
  command line.
- **`phux wait [TARGET] [--until TEXT] [--idle MS] [--timeout SECS] [--json]
  [--socket P]`** — poll the side-effect-free screen read until a condition
  holds. `--until` takes precedence over `--idle`; with neither, it settles on
  idle. `--json` emits the final `ScreenState`. Exit 0 when the condition is
  met, 124 on timeout. Two gotchas: flags must precede `TARGET`; and `--until`
  matches any visible row, including the shell's echo of the command you just
  typed — match on text that appears only in command output, never the command
  itself.
- **`phux watch [TARGET] [--json] [--socket P]`** — stream a pane's live events
  (the push half of the agent surface; see [`../spec/L1.md`](../spec/L1.md)).
  Subscribes to the server's event stream scoped to the resolved pane and
  prints one event per line until EOF (server gone) or Ctrl-C; the
  subscription neither attaches nor resizes the pane. With `--json`, each line
  is a JSON object `{ "event": <name>, "terminal"?: "@id", ... }` and stdout
  stays pure JSON (diagnostics on stderr); otherwise a compact tab-separated
  human line. Event names: `title_changed` (carries `title`), `bell`, `dirty`,
  `idle`, `pane_spawned`, `pane_closed` (carries `exit_status`), `asked`
  (carries `id`, `question`, `suggestions`, and nullable `elapsed_seconds`),
  plus the deferred `command_started` / `command_finished` (carries a nullable
  `exit_code` — see the gap note below). `watch` cuts `wait`'s poll-floor
  latency: a `watch` consumer wakes the instant an event fires rather than on
  the next poll tick. It is additive — `wait` still works without it, and a
  dropped event (full mailbox) falls back to polling.
  **Deferred:** `command_started` / `command_finished` are wire-allocated but
  not emitted by the current server (the OSC-133 command boundary is not
  cleanly observable without disturbing the per-consumer state-sync
  synthesizer); `command_finished.exit_code` is likewise always null until that
  shell-integration plumbing lands. The mechanism and the
  lifecycle/title/bell/dirty/idle events ship today.
- **`phux ask TARGET [--id ID] [--suggest TEXT...] [--elapsed-seconds SECS]
  [--json] [--socket P] QUESTION`** — report that an agent in a pane is blocked
  on a human-answerable question. This is the opt-in hook ingress from
  ADR-0036: configured plugin actions or first-party integrations call it
  instead of writing a `phux-ask` title sentinel themselves. It resolves
  `TARGET` client-side, does not attach or resize, and asks the server to emit
  the normal `asked` event on the existing watch stream. `--json` echoes the
  reported `{ event, terminal, id, question, suggestions, elapsed_seconds }`
  object after the server accepts the payload. Empty questions, empty
  suggestions, excessive suggestion counts, and unknown panes fail without
  emitting an event.
- **`phux agent <list|show|explain> [TARGET] [--json] [--socket P]`** —
  project public agent state from already-phux-shaped evidence: session/pane
  metadata, OSC/title hints, side-effect-free `snapshot --cells`, and enabled
  plugin `[[agents]]` declarations. `list` covers every pane; `show` returns
  the selected pane; `explain` keeps the same state but expands the evidence
  trail in the human view. `--json` emits `AgentStateJson` (§4.7). States are
  `unknown`, `idle`, `working`, `blocked`, or `done`; each state carries
  confidence and ordered provenance so consumers can show why phux believes it.
- **`phux new [-s NAME] [-c CWD] [-- COMMAND...] [--json] [--socket P]`** —
  create a new session. Without `--json` it creates and attaches: an explicit
  `-s NAME` that already exists is an error (like tmux's duplicate-session
  refusal); an omitted name is auto-assigned the smallest free numeric name
  (tmux-style); a server is auto-spawned if none is running. With `--json` it
  creates the session without attaching (no attach, no resize), then prints the
  seed pane id as JSON and exits. `--json` requires an explicit `-s NAME` and
  errors if that name is already in use (create-only, never create-or-attach).
  Shape in §4.4.
- **`phux plugin <list|link|unlink|enable|disable|validate> [--json]`** —
  manage declarative plugin manifest entries in the local config registry.
  This never contacts a running server and never executes plugin commands.
  `--json` emits the plugin registry document (§4.5); failure paths leave
  stdout empty and report diagnostics on stderr.
- **`phux config agents [--json]`** — project configured plugin
  `[[agents]]` declarations into a flat agent-state list. It never contacts a
  server. `--json` emits `ConfiguredAgentsJson` (§4.6).
- **`phux config run PLUGIN ACTION [--timeout SECS] [--cwd PATH] [--json]`** —
  execute one action declared by an enabled configured plugin manifest. The
  command runs as argv from the plugin root; there is no implicit shell
  expansion. `--json` emits `PluginActionOutput` (§4.8). Exit code mirrors the
  action's process status; timeout exits `125`.
- **`phux workspace inspect [PATH] [--json]`** — inspect the local git
  repository containing `PATH` and every checked-out worktree reported by git.
  This never contacts a running server and never creates, deletes, or checks out
  worktrees. Agents use the JSON shape (§4.9) to choose a checkout before
  creating a session (`phux new -c <worktree>`) or mapping existing sessions and
  panes back to repo paths.
- **`phux workspace save [--socket P] [--output PATH]`** — capture the running
  phux workspace as a typed JSON archive. With no `--output`, the archive is
  printed to stdout. This contacts the server but does not attach or resize.
- **`phux workspace restore ARCHIVE [--socket P]`** — recreate sessions missing
  from a saved archive. Restore starts new processes; it does not claim to
  resurrect the original PTYs.
- **`phux satellite <list|add|remove> [--json]`** — manage the hub-side
  federation satellite registry. This never contacts a running server and never
  opens a satellite transport; it only edits `[[satellites]]` in local config.
  `--json` emits the satellite registry document (§4.10); failure paths leave
  stdout empty and report diagnostics on stderr.

**Not implemented.** `split` and `detach` do not exist as subcommands today
(tracked as bead phux-99te). The shipped verbs are listed in
[`tui.md`](./tui.md) §1; the agent-relevant subset is the catalog above plus
`kill` and `attach`.

**How `new` decomposes on the wire.** Session create is no longer an L1
session verb. Per
[ADR-0030](../../ADR/0030-engine-delegated-wire-and-projection-consumers.md) §5,
the session lifecycle verbs were removed from L1 and decompose into substrate
primitives plus L3 metadata: `new` is `SPAWN_TERMINAL` plus an L3 metadata
write on the `phux.session.create/v1` key (the assigned identity is read back
via `phux.session.created/v1`), and rename is an L3 metadata SET on the
`phux.session.name/v1` key. Grouping conventions are owned by
[`../spec/L3.md`](../spec/L3.md). The user-facing UX of `new` is unchanged; the
divergence is on the wire, where the migration to this decomposition is tracked
against ADR-0030 (full `GroupId` removal is bead phux-0bmc).

**Socket precedence (once, for every verb).** The `--socket` argument wins,
then the `PHUX_SOCKET` environment variable, then the daemon default:
`$XDG_RUNTIME_DIR/phux/phux.sock`, falling back to `/tmp/phux-$UID/phux.sock`.

## 3. Targeting: the selector grammar

One grammar, every targeted command — `kill`, `snapshot`, `wait`, `send-keys`,
`run`, and `ask` all share `TARGET`. It is resolved client-side against a server
snapshot ([ADR-0021](../../ADR/0021-control-plane-commands.md)); the server
never parses a selector.

The full grammar table and CLI examples live in [`tui.md`](./tui.md) §3. In one
line, the forms are: `.` (current), `=` (last), `name` (session), `name:N` /
`name:tag` (window), `name:N.M` (pane), and `@N` (opaque id).

A selector that names several panes (a whole session or window) narrows to a
single pane: the focused pane when it is among the matches, else the first in
snapshot order (the `pick_target_pane` tiebreak the MCP tools share).
Optionality differs per verb: `snapshot` and `wait` default `TARGET` to the
last-focused session; `send-keys`, `run`, and `ask` require it.

## 4. JSON contracts (the per-verb machine shapes)

Each `--json` verb emits a versioned, plain-data struct from `phux-core` or
`phux-client`. These structs are the stable agent contract
([ADR-0022](../../ADR/0022-tool-for-agents.md)); they are a local projection
over the shared engine, and the wire underneath stays additive and versioned.
Each struct carries its own `schema_version`, tracked independently.

### 4.1 `SessionListJson` — `phux ls --json`

Defined in `crates/phux-core/src/session_list.rs` (`LS_SCHEMA_VERSION = 1`).
Shape, name-sorted:

```json
{
  "schema_version": 1,
  "sessions": [
    { "name": "work", "windows": 3, "attached": true }
  ]
}
```

`windows` is the window count; `attached` is a bool — whether any client is
attached. **Cross-surface gotcha:** the MCP `phux_ls` tool ([`mcp.md`](./mcp.md)
§3.1) surfaces the raw wire fields `window_count` / `attached_client_count`;
the CLI's `--json` projects them to `windows` / `attached`. The two surfaces do
not share identical keys — do not carry a parser across them.

### 4.2 `ScreenState` — `phux snapshot --json` (and `phux wait --json`)

Defined in `crates/phux-core/src/screen.rs` (`SCHEMA_VERSION = 3`). The same
struct the server returns from `GET_SCREEN`, not an agents-specific shape.
Fields:

| Field | Type | Meaning |
|---|---|---|
| `schema_version` | u32 | Contract version (currently `3`); the pin/branch signal. |
| `pane` | u32 | Wire-local id of the captured pane. |
| `cols`, `rows` | u16 | Grid dimensions. |
| `cursor` | `Option<{x,y,visible}>` | Viewport-relative, zero-based; `None` when the cursor is not viewport-resident (scrollback or hidden). |
| `lines` | `Vec<String>` | Viewport rows, top to bottom, right-trimmed. |
| `scrollback` | `Vec<String>` | History rows above the viewport, oldest first; empty unless requested. |
| `cells` | `Option<Vec<CellInfo>>` | Per-cell marks and styles; present only with `--cells`. |

**`scrollback` is tri-state** (mirrors [`mcp.md`](./mcp.md) §3.2): flag absent →
viewport only; `--scrollback` or `--scrollback=0` → all retained history;
`--scrollback N` → the most-recent `N` rows. On the wire this is `None` /
`Some(0)` (all) / `Some(n)`.

**`--cells`** populates `cells` with a sparse `Vec<CellInfo>` — only cells
carrying a non-default style or an OSC-133 mark, in row-major order, skipping
the right half of double-width glyphs. Each `CellInfo` is
`{ col, row, semantic?, style }`:

- `semantic` is `SemanticContent` — `Input` (typed input) or `Prompt` (shell
  prompt). `Output` is the default for every cell and is collapsed to absence,
  so `semantic` is `Some` only for marked input vs prompt.
- `style` is `CellStyle`: nine SGR booleans (`bold`, `faint`, `italic`,
  `underline`, `blink`, `inverse`, `invisible`, `strikethrough`, `overline`)
  plus `fg` / `bg`, each a `CellColor` tagged enum with `kind` of `default`,
  `palette` (`{ index }`), or `rgb` (`{ r, g, b }`). The tag distinguishes
  "terminal default" from "explicitly black".

**Back-compat.** `scrollback` and `cells` are `#[serde(default)]` (and `cells`
is `skip_serializing_if` `None`), so a `cells = None` snapshot serializes to
exactly the pre-cells shape, and an older consumer reading a newer payload
ignores extra keys. `schema_version` is the bump signal.

### 4.3 `RunResult` — `phux run --json` (on completion)

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

- `exit_code` (i32) is the child's `$?`, parsed out of a printed sentinel
  (`run` brackets the command with `BEGIN`/`RC` markers — it does not rely on
  shell integration).
- `output` is the rows between the `BEGIN` and `RC` markers.
- `duration_ms` (u64) is wall-clock from submit to sentinel-seen, including
  poll latency — an upper bound on the child's runtime, not a precise
  measurement.
- `truncated` is `true` when the `BEGIN` marker had scrolled out of the
  viewport, so `output` is best-effort visible context; a full capture needs
  scrollback.

**On timeout, `run --json` emits no JSON.** `RunOutcome::TimedOut` carries the
command, elapsed time, and last screen internally, but the CLI's `--json` path
serializes only the completed `RunResult`. The timeout signal is the exit code
(125 — see §5.2), printed alongside a stderr diagnostic. An agent must read the
exit code here and must not expect an `outcome: "timed_out"` body — that shape
exists in the MCP `phux_run` tool ([`mcp.md`](./mcp.md) §3.4), not in the CLI's
`--json` output.

### 4.4 `phux new --json`

`phux new --json -s NAME` emits a small fixed object naming the created session
and its seed pane's wire-local id, then exits `0` without attaching:

```json
{ "session": "NAME", "terminal_id": 2 }
```

It is create-only: `--json` requires an explicit `-s NAME` and errors (exit `1`)
if that name is already in use. Unlike the versioned `ScreenState` /
`RunResult` / `SessionListJson` shapes, this is a flat ad-hoc object with no
`schema_version`. The wire decomposition behind it is in §2.

### 4.5 Plugin registry — `phux plugin ... --json`

The plugin lifecycle surface is config-local. It edits or reads
`[[plugins]]` entries and validates referenced `phux-plugin.toml` manifests;
it does not load plugin code into phux and does not run plugin commands.

`phux plugin list --json` and `phux plugin validate --json` emit:

```json
{
  "schema_version": 1,
  "plugins": [
    {
      "id": "example.agent-tools",
      "name": "Agent Tools",
      "version": "0.1.0",
      "min_phux_version": "0.0.2",
      "description": null,
      "manifest": "./plugins/agent-tools/phux-plugin.toml",
      "manifest_path": "/abs/path/phux-plugin.toml",
      "plugin_root": "/abs/path",
      "enabled": true,
      "platforms": null,
      "build": [],
      "actions": [],
      "events": [],
      "panes": [],
      "links": []
    }
  ]
}
```

`validate --json` also carries `"valid": true`. `link`, `enable`, and
`disable` wrap the same plugin object under `"plugin"`; `unlink` wraps the
removed object under `"removed"`. The registry JSON enumerates declarative
actions, event hooks, pane providers, and link handlers from each manifest but
does not execute them. Invalid or missing manifests are hard failures: exit
nonzero, stdout empty, stderr diagnostic.

### 4.6 `ConfiguredAgentsJson` — `phux config agents --json`

`phux config agents --json` emits configured plugin agent declarations as a
consumer-ready list. It is a config projection, not a live runtime detector:

```json
{
  "schema_version": 1,
  "agents": [
    {
      "plugin_id": "example.agent-tools",
      "plugin_enabled": true,
      "id": "codex",
      "label": "Codex",
      "description": "Coding agent",
      "state": "working",
      "attention": "normal",
      "contexts": ["workspace", "pane"]
    }
  ]
}
```

`state` is one of `unknown`, `idle`, `working`, or `blocked`. `attention` is
one of `none`, `low`, `normal`, or `high`. Invalid manifests are hard failures
and leave stdout empty on `--json`, preserving the script contract.

### 4.7 `AgentStateJson` — `phux agent ... --json`

`phux agent list --json`, `phux agent show --json [TARGET]`, and
`phux agent explain --json [TARGET]` emit the same versioned shape. `explain`
differs only in the human output; JSON always includes the evidence trail:

```json
{
  "schema_version": 1,
  "agents": [
    {
      "terminal": "@3",
      "session": "work",
      "window": "window-0",
      "agent": { "id": "codex", "label": "Codex", "kind": "codex" },
      "state": "blocked",
      "confidence": 0.95,
      "attention": "high",
      "title": "phux-ask[deploy]:Approve deploy??s=Yes|No",
      "cwd": "/repo",
      "sources": [
        {
          "kind": "title_ask",
          "signal": "phux-ask title sentinel",
          "confidence": 0.95,
          "observed": "phux-ask[deploy]:Approve deploy??s=Yes|No"
        }
      ],
      "explanation": "waiting on a reported human-answerable ask"
    }
  ]
}
```

`agent.kind` is `codex`, `claude`, `plugin`, or `unknown`. `state` is
`unknown`, `idle`, `working`, `blocked`, or `done`; `attention` is `none`,
`low`, `normal`, or `high`. `sources` is sorted by descending confidence and is
the provenance contract: current sources include `title_ask`, `screen`,
`semantic_cells`, `identity`, and `plugin_report`. The detector is deliberately
explainable rather than magical: a plugin report is lower precedence than a
live `phux-ask` title sentinel or an explicit blocked/completed screen cue, and
unknown/missing signals stay `unknown` or low-confidence `idle`.

This is a public clean-room projection. It does not copy external agent
manifests or private tradecraft rules; Codex and Claude recognition comes from
public, user-visible pane text/title evidence plus optional local phux plugin
declarations.

### 4.8 `PluginActionOutput` — `phux config run --json`

Defined in `crates/phux-plugin/src/lib.rs` (`schema_version = 1`). Shape:

```json
{
  "schema_version": 1,
  "plugin_id": "example.agent-tools",
  "action_id": "summarize",
  "command": ["python3", "summarize.py"],
  "cwd": "/path/to/plugin",
  "outcome": "completed",
  "exit_code": 0,
  "stdout": "...",
  "stderr": "",
  "duration_ms": 42
}
```

`outcome` is `"completed"` or `"timed_out"`. `exit_code` is `null` when the OS
does not provide a process code or when phux kills the child on timeout. The
runtime executes the manifest's argv directly from the plugin root, captures
stdout/stderr lossily as UTF-8, inherits the phux process environment, and adds
`PHUX_PLUGIN_ID`, `PHUX_PLUGIN_ACTION_ID`, and `PHUX_PLUGIN_ROOT`.

### 4.9 Workspace commands — `phux workspace ...`

`phux workspace inspect --json` is repo-local. It shells out to git's porcelain worktree
listing and reports the current worktree plus siblings as a stable JSON
projection:

```json
{
  "schema_version": 1,
  "repo": {
    "path": "/abs/path/repo",
    "head": "012345...",
    "branch": "main",
    "detached": false
  },
  "worktrees": [
    {
      "path": "/abs/path/repo-feature",
      "head": "89abcd...",
      "branch": "feature",
      "detached": false,
      "current": false
    }
  ]
}
```

For detached worktrees, `branch` is `null` and `detached` is `true`. Missing
or non-git paths are hard failures: exit nonzero, stdout empty, stderr
diagnostic. The command is intentionally read-only; creation and deletion stay
in git/plugin/provider territory rather than the terminal substrate.

`phux workspace save` emits a separate archive shape:

```json
{
  "schema_version": 1,
  "sessions": [
    {
      "name": "agent-bench-codex",
      "active": true,
      "windows": [
        {
          "name": "0",
          "active": true,
          "panes": [
            {
              "active": true,
              "title": "codex",
              "cwd": "/repo",
              "command": null,
              "cols": 120,
              "rows": 40
            }
          ]
        }
      ]
    }
  ]
}
```

`command` is nullable because process argv is not always known. Plugin-authored
archives may fill it, and `workspace restore` uses it when present; otherwise it
starts the default shell in the saved cwd when available. Existing session names
are skipped, and restore prints a summary JSON document with `restored` and
`skipped_existing` arrays.

Restored sessions are fresh PTYs. The archive preserves window/pane metadata and
split-layout shape for inspection and future replay, but the current restore
command only recreates missing sessions and their seed process. Use `phux
upgrade` for live PTY handoff across a server re-exec; do not present workspace
restore as resurrecting already-running processes.

### 4.10 Satellite registry — `phux satellite ... --json`

The satellite lifecycle surface is config-local. It edits or reads
`[[satellites]]` entries and does not dial remote hosts.

`phux satellite list --json` emits:

```json
{
  "schema_version": 1,
  "satellites": [
    {
      "name": "devbox",
      "endpoint": "ssh://devbox",
      "enabled": true
    }
  ]
}
```

`add --json` wraps the same satellite object under `"satellite"`; `remove
--json` wraps the removed object under `"removed"`. Invalid names, invalid
endpoint URIs, duplicate configured names, and refused registry writes are hard
failures: exit nonzero, stdout empty, stderr diagnostic.

## 5. The read-act-wait loop and exit-code mirroring

### 5.1 The loop

The canonical agent pattern is read → act → wait → read: snapshot the pane,
send input or run a command, wait for the result to land, snapshot again. A
worked example in `sh`:

```sh
phux send-keys build "cargo test" Enter
phux wait build --until "test result:" --timeout 120
phux snapshot build --json --scrollback 200 > out.json
```

When you only want a command's exit code and output, the one-shot `phux run` is
the higher-level alternative — it brackets the command with sentinels and
mirrors `$?`:

```sh
phux run build "cargo test" --json
```

The contrast: `run` is "I want the exit code"; `send-keys` plus `wait` is "I am
driving an interactive or long-lived program." Because `run` mirrors the
child's code (§5.2), `phux run ... && next` composes like a shell
([ADR-0022](../../ADR/0022-tool-for-agents.md) §3).

### 5.2 Exit-code mirroring

Exit codes are not uniform across verbs:

| Verb | Exit codes |
|---|---|
| `ls` | `0` ok; `1` no server / unexpected result. |
| `snapshot` | `0` ok; `1` failure (no server, serialize error, resolve miss). |
| `send-keys` | `0` ok; `1` failure (no server / refused / miss). |
| `ask` | `0` accepted; `1` no server, unknown pane, or invalid ask payload. |
| `agent` | `0` ok; `1` no server, unknown pane, or JSON render failure. |
| `run` | the child's own code clamped to `0..=255` (negative or `>255` saturate to `255`); `125` when phux gave up waiting for the sentinel (`--timeout`); `1` for no server / refused target / other. |
| `wait` | `0` condition met; `124` on `--timeout`; `1` no server / parse / read error. |
| `new` | `0` ok; `1` duplicate `-s` name / failure. |
| `plugin` | `0` ok; `1` invalid/missing manifest, invalid config, refused registry write, or unknown plugin id. |
| `workspace` | `0` ok; `1` missing git repo, invalid git output, no server for save/restore, invalid archive, or JSON render failure. |
| `satellite` | `0` ok; `1` invalid name/endpoint, duplicate configured name, invalid config, refused registry write, or unknown satellite name. |
| `kill` | `0` ok; `1` selector miss / no server / parse; `2` server-side refusal. |

**Why `run` uses 125, not 124.** `run` mirrors the child's own code into
`0..=255`, and `124` is a code real commands produce — notably GNU `timeout`.
So `run` reserves `125` (the wrapper-failure convention, as used by `env` and
`timeout`) for "phux itself gave up," keeping it distinct from a child that
legitimately exited `124`. `wait`, which wraps nothing, uses `124` for its own
timeout. `kill` is a control-plane verb (not strictly an agent read) but shares
`TARGET`; its `0`/`1`/`2` triad is listed for completeness.

## 6. Relationship to the other agent surfaces

The CLI verbs here are the stable contract. The [MCP adapter](./mcp.md) exposes
them name-for-name (`phux_ls` ↔ `ls`, and `phux_snapshot` / `phux_send_keys` /
`phux_run` / `phux_wait` ↔ the matching subcommands) over the same
`phux-client` functions — same client-side resolution, same tiebreaks.
[`sdk.md`](./sdk.md) documents `phux-client`, the library crate those surfaces
are built from. All three are unprivileged consumers
([ADR-0017](../../ADR/0017-tui-not-protocol-privileged.md)); the wire
underneath stays additive and versioned under [`../spec/`](../spec/)
([ADR-0022](../../ADR/0022-tool-for-agents.md)).
