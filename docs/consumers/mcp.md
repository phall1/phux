---
audience: consumers, contributors, agents
stability: evolving
last-reviewed: 2026-05-29
---

# The phux MCP adapter

**TL;DR.** The agent-facing consumer surface: `phux-mcp` exposes six
tools (`phux_ls`, `phux_snapshot`, `phux_send_keys`, `phux_run`,
`phux_wait`, `phux_new`) over JSON-RPC stdio, with their JSON input schemas, the
shared CLI selector grammar, the tri-state scrollback / per-cell
snapshot semantics, and the `tools/call` envelope. It is a thin adapter
over the same structured surface the CLI uses, with no protocol
privilege.

---

## 0. What this is, what this isn't

This document is the **agent-facing consumer's surface**, parallel to the
[TUI's product surface](./tui.md). Every consumer projects the one
source-of-truth libghostty `Terminal`
([ADR-0022](../../ADR/0022-tool-for-agents.md)); the TUI projects it to
VT bytes (it renders, like tmux), while agents project it to
**structured data** — cells, OSC-133 marks, command results.

`phux-mcp` is "MCP as a thin adapter": it has no separate core. Each tool
is a thin wrapper over the same structured agent surface the CLI's `phux
snapshot` / `send-keys` / `run` / `wait` subcommands use, plus a direct
`GET_STATE` control command. Like every consumer, it holds no
protocol-level privilege
([ADR-0017](../../ADR/0017-tui-not-protocol-privileged.md)).

The selector grammar and the CLI equivalents are owned by
[`tui.md`](./tui.md) §3 — this file links there rather than restating the
table (the doc system's "one fact, one home" rule). The normative wire
lives under [`../spec/`](../spec/); the decision rationale is in
[ADR-0022](../../ADR/0022-tool-for-agents.md).

---

## 1. Transport and lifecycle

`phux-mcp` speaks **JSON-RPC 2.0 over the MCP stdio transport**:
newline-delimited JSON, one message per line on stdin and stdout. The
JSON-RPC is hand-rolled over `serde_json` — no framework dependency.

The MCP protocol version is pinned to the `2024-11-05` revision. Newer MCP
revisions are additive; the pin will be bumped when the adapter adopts
one.

The methods:

| Method | Reply |
|---|---|
| `initialize` | `protocolVersion`, `capabilities` (`{ "tools": {} }`), and `serverInfo` (`name` = `"phux"`, `version` = the crate version) |
| `notifications/initialized` | none (it is a notification) |
| `tools/list` | the tool catalog (see §3) |
| `tools/call` | dispatch by tool name (see §3, §4) |
| `ping` | an empty result (keepalive) |

Robustness: a malformed line yields a JSON-RPC parse error with a null id;
an unknown method on a request yields a method-not-found error (an unknown
notification, having no id, is silently ignored). Neither stops the loop,
which runs until stdin EOF.

---

## 2. How a tool resolves a target

Every targeted tool (`phux_snapshot`, `phux_send_keys`, `phux_run`,
`phux_wait`) takes a `target` selector string in the **same grammar as the
CLI's `TARGET`**. The full grammar table and CLI examples live in
[`tui.md`](./tui.md) §3; the forms, in one line, are: `.` (current), `=`
(last), `name` (session), `name:N` / `name:tag` (window), `name:N.M`
(pane), and `@N` (opaque id).

Resolution is **client-side**, exactly as the CLI resolves it
([ADR-0021](../../ADR/0021-control-plane-commands.md)): the adapter fetches
a `GET_STATE` snapshot, expands the selector to candidate `TerminalId`s,
then narrows to a single pane — the focused pane if it is among the
candidates, else the first in snapshot order. This is the same
`pick_target_pane` tiebreak the CLI uses. The server never parses a
selector.

`target` optionality differs per tool:

- `phux_snapshot` and `phux_wait` make `target` **optional**; when absent
  it defaults to the focused/last session (`Selector::Last`).
- `phux_send_keys` and `phux_run` **require** `target`.

Every tool also takes an optional `socket` string naming the Unix-domain
socket to connect to. Precedence: an explicit `socket` argument, then the
`PHUX_SOCKET` environment variable, then the daemon default
(`$XDG_RUNTIME_DIR/phux/phux.sock`, falling back to
`/tmp/phux-$UID/phux.sock` — the segment is `$UID`, then `$USER`, then the
literal `default`).

---

## 3. The tool catalog

Six tools, returned verbatim by `tools/list`. Each `inputSchema` is a
JSON Schema `object`. Tools that take no required argument (e.g.
`phux_ls`) work with no `arguments` at all.

### 3.1 `phux_ls`

Lists phux sessions on the running server via `GET_STATE`. No target.

| Param | Type | Required | Meaning |
|---|---|---|---|
| `socket` | string | no | Override the UDS path (see §2). |

Result: `{ "sessions": [ { "name", "window_count", "attached_client_count" } ] }`,
sorted by name.

### 3.2 `phux_snapshot`

Captures a pane as structured screen data. Side-effect-free: it does not
attach or resize.

| Param | Type | Required | Meaning |
|---|---|---|---|
| `target` | string | no | Selector (see §2). Defaults to focused/last. |
| `scrollback` | number | no | Tri-state — see below. |
| `cells` | boolean | no | When true, include per-cell OSC-133 marks and styles. Default `false`. |
| `socket` | string | no | Override the UDS path (see §2). |

`scrollback` is **tri-state**, and the distinction is load-bearing:

- **absent** — the viewport only.
- **`0`** — all retained history.
- **`N`** — the most-recent `N` rows.

Result: a serialized `phux-core` `ScreenState` (the same struct `phux
snapshot` emits — not a bespoke MCP shape). Its fields are
`schema_version` (the contract version), `pane` (the captured pane's
wire-local id), `cols`, `rows`, `cursor` (optional `{ x, y, visible }`),
`lines` (the viewport rows), `scrollback` (history rows; empty unless
requested), and `cells` (an optional sparse array of per-cell
`{ col, row, semantic?, style }`, present only when `cells` is true).

### 3.3 `phux_send_keys`

Routes input to the resolved pane by id. No attach, no resize.

| Param | Type | Required | Meaning |
|---|---|---|---|
| `target` | string | yes | Selector (see §2). |
| `keys` | array of string | yes | Keys to send; must be non-empty. |
| `socket` | string | no | Override the UDS path (see §2). |

Each entry in `keys` is a named key (`Enter`, `Tab`, `C-c`, ...) or a
literal string, tmux-style.

Result: `{ "sent": true, "pane": "<pane>" }`. Note that `pane` is rendered
via the `TerminalId` `Debug` formatting, not a stable numeric id —
`TerminalId` has no `Serialize` impl yet (tracked under phux-93b), so do
not parse it as a number.

### 3.4 `phux_run`

Runs a command in the resolved pane and reports its result. Assumes a
POSIX shell.

| Param | Type | Required | Meaning |
|---|---|---|---|
| `target` | string | yes | Selector (see §2). |
| `command` | string | yes | The command line to run. |
| `timeout_secs` | number | no | Default `600`; `0` waits indefinitely. |
| `socket` | string | no | Override the UDS path (see §2). |

Result on completion: a serialized `phux-client` `RunResult`
(`{ command, exit_code, output, duration_ms, truncated }`). `truncated`
is `true` when the command's `BEGIN` marker had scrolled out of the
viewport, so `output` is best-effort visible context rather than a clean
capture; a full capture needs scrollback (phux-o1v).

Result on timeout: `{ "outcome": "timed_out", "command", "duration_ms" }`.

### 3.5 `phux_wait`

Polls the resolved pane until a condition holds.

| Param | Type | Required | Meaning |
|---|---|---|---|
| `target` | string | no | Selector (see §2). Defaults to focused/last. |
| `until` | string | no | Succeed once a visible line contains this substring. |
| `idle_ms` | number | no | Succeed once the screen holds still this long. |
| `timeout_secs` | number | no | Give up after this many seconds. Default: wait forever. |
| `socket` | string | no | Override the UDS path (see §2). |

Condition precedence: `until` wins when present (succeed on a substring
match); otherwise the tool settles on idle, using `idle_ms` or the default
dwell when `idle_ms` is absent.

Result: `{ "outcome": "met" | "timed_out", "polls": N }`.

### 3.6 `phux_new`

Creates a named session on the running server without attaching. The
server must already be running (this tool does not auto-spawn one).

| Param | Type | Required | Meaning |
|---|---|---|---|
| `name` | string | yes | Name for the new session. A name already in use is rejected. |
| `command` | array | no | Initial command (argv) for the seed pane. Omit or pass `[]` for the server's default shell. |
| `cwd` | string | no | Working directory for the seed pane. |
| `socket` | string | no | Override the UDS path (see §2). |

Result: the new session's name and seed pane id.

### 3.7 Event stream (`phux watch`) — not yet an MCP tool

The push half of the agent surface — a subscribed stream of tagged
lifecycle / activity events (`title_changed`, `bell`, `dirty`, `idle`,
`pane_spawned`, `pane_closed`; SPEC §7.5) — ships today as the CLI verb
[`phux watch`](./agents.md#1-the-structured-cli-surface-verb-catalog),
the latency-cutting accelerator of `phux_wait`'s poll floor. It is **not**
exposed as an MCP tool in this pass: MCP `tools/call` is request/response,
whereas the event stream is a long-lived push, so a streaming `phux_watch`
tool needs an MCP notification/streaming shape that is a separate ticket.
The `phux_wait` polling tool remains the MCP-native way to block on a pane
condition; the event stream is available to MCP hosts that shell out to
`phux watch --json` in the meantime.

---

## 4. A worked `tools/call` example

A `phux_run` against an explicit pane, target `work:1.0`:

Request (one line on stdin):

```json
{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"phux_run","arguments":{"target":"work:1.0","command":"cargo test"}}}
```

Success response:

```json
{"jsonrpc":"2.0","id":1,"result":{"content":[{"type":"text","text":"{\n  \"command\": \"cargo test\",\n  \"exit_code\": 0,\n  \"output\": \"...\",\n  \"duration_ms\": 8123,\n  \"truncated\": false\n}"}],"isError":false}}
```

The result is a **single text content block**. A structured result is
pretty-printed JSON (here, the serialized `RunResult`); a bare error
string is shown verbatim.

A **tool** failure — no such target, no running server, a malformed
argument — is a *successful* JSON-RPC response carrying `isError: true`,
never a JSON-RPC error and never a crash. Contrast this with
**protocol-level** errors (a parse error, an unknown method, missing
`tools/call` params), which *are* JSON-RPC `error` responses.

A second example, `phux_snapshot` of a pane by opaque id with scrollback
and per-cell data:

```json
{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"phux_snapshot","arguments":{"target":"@42","scrollback":200,"cells":true}}}
```

This returns the last 200 scrollback rows plus the viewport, with the
sparse per-cell `cells` array populated.

---

## 5. Relationship to the CLI

The MCP tools are name-for-name the CLI's agent subcommands: `phux_ls` ↔
`phux ls`, and `phux_snapshot` / `phux_send_keys` / `phux_run` /
`phux_wait` / `phux_new` ↔ `phux snapshot` / `send-keys` / `run` / `wait` /
`new` (see [`tui.md`](./tui.md) §1 and §3). Same surface, same client-side
resolution, same tiebreaks — because the adapter wraps the same
`phux-client` functions the CLI does.

Per [ADR-0022](../../ADR/0022-tool-for-agents.md), the CLI and its JSON
schema are the stable agent contract; the wire underneath stays additive
and versioned, and MCP is one thin adapter over it among several.
