---
audience: consumers, contributors, agents
stability: evolving
last-reviewed: 2026-06-06
---

# The phux MCP adapter

**TL;DR.** This doc covers what is MCP-specific in `phux-mcp`: the eleven
JSON-RPC stdio tools (`phux_ls`, `phux_snapshot`, `phux_send_keys`,
`phux_run`, `phux_wait`, `phux_new`, `phux_kill`, `phux_watch`,
`phux_ask`, `phux_plugin_action`, `phux_plugin_workspace`), the stdio
transport and lifecycle, how a tool resolves a target client-side, and the
`tools/call` envelope.
The structured shapes the tools return and the selector grammar are the
shared agent surface and live in their owning docs; this file links them.

---

## 0. What this is, what this isn't

This is the MCP adapter only. `phux-mcp` has no separate core: each tool
is a thin wrapper over the same `phux-client` functions the agent CLI
verbs use ([`agents.md`](./agents.md)). Like every consumer it holds no
protocol-level privilege
([ADR-0017](../../ADR/0017-tui-not-protocol-privileged.md)); the
structured agent surface is a local projection over the shared engine,
exposed through the CLI and its versioned JSON schema, not a wire service
([ADR-0030](../../ADR/0030-engine-delegated-wire-and-projection-consumers.md)).

Two things this file does not restate, by the "one fact, one home" rule:

- The structured return shapes — `ScreenState`, `RunResult`,
  `SessionListJson` — are owned by [`agents.md`](./agents.md) §3. Each
  tool below names the shape and links there.
- The selector grammar is owned by [`tui.md`](./tui.md) §3. §2 below
  links it.

---

## 1. Transport and lifecycle

`phux-mcp` speaks **JSON-RPC 2.0 over the MCP stdio transport**:
newline-delimited JSON, one message per line on stdin and stdout. The
JSON-RPC is hand-rolled over `serde_json` — no framework dependency.

The MCP protocol version is pinned to the `2024-11-05` revision. Newer
MCP revisions are additive; the pin is bumped when the adapter adopts
one.

The methods:

| Method | Reply |
|---|---|
| `initialize` | `protocolVersion`, `capabilities` (`{ "tools": {} }`), and `serverInfo` (`name` = `"phux"`, `version` = the crate version) |
| `notifications/initialized` | none (it is a notification) |
| `tools/list` | the tool catalog (see §3) |
| `tools/call` | dispatch by tool name (see §3, §4) |
| `ping` | an empty result (keepalive) |

Robustness: a malformed line yields a JSON-RPC parse error with a null
id; an unknown method on a request yields a method-not-found error (an
unknown notification, having no id, is silently ignored). Neither stops
the loop, which runs until stdin EOF.

---

## 2. How a tool resolves a target

Every targeted tool (`phux_snapshot`, `phux_send_keys`, `phux_run`,
`phux_wait`, `phux_kill`, `phux_watch`, `phux_ask`) takes a `target` selector string in the **same
grammar as the CLI's `TARGET`**, whose table and examples live in
[`tui.md`](./tui.md) §3. In one line, the forms are: `.` (current), `name`
(session), `name:N` / `name:tag` (window), `name:N.M`
(pane), and `@N` (opaque id).

Resolution is **client-side**, exactly as the CLI resolves it
([ADR-0021](../../ADR/0021-control-plane-commands.md)): the adapter
fetches a state snapshot, expands the selector to candidate
`TerminalId`s, then narrows to a single pane — the focused pane if it is
among the candidates, else the first in snapshot order. This is the same
`pick_target_pane` tiebreak the CLI uses. The server never parses a
selector. `=` is explicitly unsupported here because an MCP request has no
attached-client focus history; callers must use `.` or an explicit target.

`target` optionality differs per tool:

- `phux_snapshot` and `phux_wait` make `target` **optional**; when absent
  it defaults to the focused session (`Selector::Current`).
- `phux_send_keys` and `phux_run` **require** `target`.

Every tool also takes an optional `socket` string naming the
Unix-domain socket to connect to. Precedence: an explicit `socket`
argument, then the `PHUX_SOCKET` environment variable, then the daemon
default (`$XDG_RUNTIME_DIR/phux/phux.sock`, falling back to
`/tmp/phux-$UID/phux.sock` — the segment is `$UID`, then `$USER`, then
the literal `default`).

---

## 3. The tool catalog

Eleven tools, returned verbatim by `tools/list`. Each `inputSchema` is a
JSON Schema `object`. Tools that take no required argument (e.g.
`phux_ls`) work with no `arguments` at all. The return shapes are the
shared agent shapes owned by [`agents.md`](./agents.md) §3; each tool
names its shape and links there.

### 3.1 `phux_ls`

Lists phux sessions on the running server. No target.

| Param | Type | Required | Meaning |
|---|---|---|---|
| `socket` | string | no | Override the UDS path (see §2). |

Result: `{ "sessions": [ { "name", "window_count", "attached_client_count" } ] }`,
sorted by name. The MCP tool surfaces the **raw wire fields**
(`window_count` / `attached_client_count`); the CLI's `ls --json`
projects them to `windows` / `attached`. The two surfaces do not share
identical keys ([`agents.md`](./agents.md) §3.1) — do not carry a parser
across them.

### 3.2 `phux_snapshot`

Captures a pane as structured screen data. Side-effect-free: it does not
attach or resize.

| Param | Type | Required | Meaning |
|---|---|---|---|
| `target` | string | no | Selector (see §2). Defaults to focused. |
| `scrollback` | number | no | Tri-state — see below. |
| `cells` | boolean | no | When true, include per-cell OSC-133 marks and styles. Default `false`. |
| `socket` | string | no | Override the UDS path (see §2). |

`scrollback` is **tri-state**: **absent** captures the viewport only;
**`0`** captures all retained history; **`N`** captures the most-recent
`N` rows.

Result: a serialized `ScreenState` — the same struct `phux snapshot`
emits, with `cells` populated only when `cells` is true. The field
catalog (schema version, `cursor`, `lines`, `scrollback`, the sparse
`cells` array) is owned by [`agents.md`](./agents.md) §3.2.

### 3.3 `phux_send_keys`

Routes input to the resolved pane by id. No attach, no resize.

| Param | Type | Required | Meaning |
|---|---|---|---|
| `target` | string | yes | Selector (see §2). |
| `keys` | array of string | yes | Keys to send; must be non-empty. |
| `socket` | string | no | Override the UDS path (see §2). |

Each entry in `keys` is a named key (`Enter`, `Tab`, `C-c`, ...) or a
literal string, tmux-style.

Result: `{ "sent": true, "pane": "<pane>" }`. `pane` is rendered via the
`TerminalId` `Debug` formatting, not a stable numeric id — `TerminalId`
has no `Serialize` impl yet (tracked under phux-93b), so do not parse it
as a number.

### 3.4 `phux_run`

Runs a command in the resolved pane and reports its result. Assumes a
POSIX shell.

| Param | Type | Required | Meaning |
|---|---|---|---|
| `target` | string | yes | Selector (see §2). |
| `command` | string | yes | The command line to run. |
| `timeout_secs` | number | no | Default `600`; `0` waits indefinitely. |
| `socket` | string | no | Override the UDS path (see §2). |

Result on completion: a serialized `RunResult`
(`{ command, exit_code, output, duration_ms, truncated }`), shape owned
by [`agents.md`](./agents.md) §3.3.

**MCP-vs-CLI divergence — the timeout shape.** On timeout the MCP tool
returns a JSON body: `{ "outcome": "timed_out", "command", "duration_ms" }`.
The CLI's `run --json` does **not**: it emits no JSON on timeout and
signals the timeout through exit code `125`
([`agents.md`](./agents.md) §3.3). An agent driving MCP reads the
`outcome` body; an agent driving the CLI reads the exit code. This is the
one genuine shape difference between the two surfaces; everything else is
name-for-name.

### 3.5 `phux_wait`

Polls the resolved pane until a condition holds.

| Param | Type | Required | Meaning |
|---|---|---|---|
| `target` | string | no | Selector (see §2). Defaults to focused. |
| `until` | string | no | Succeed once a visible line contains this substring. |
| `idle_ms` | number | no | Succeed once the screen holds still this long. |
| `timeout_secs` | number | no | Give up after this many seconds. Default: wait forever. |
| `socket` | string | no | Override the UDS path (see §2). |

Condition precedence: `until` wins when present (succeed on a substring
match); otherwise the tool settles on idle, using `idle_ms` or the
default dwell when `idle_ms` is absent.

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

### 3.7 `phux_kill`

Tears down the Terminal(s) a selector resolves to — a whole session, a
window, a pane, or `@id` — in one atomic `KILL_TERMINALS`.

| Param | Type | Required | Meaning |
|---|---|---|---|
| `target` | string | yes | Selector (§2). Resolves to its full id set. |
| `socket` | string | no | Override the UDS path (see §2). |

Result: `{ killed: N }`, the number of Terminals torn down. A clean server
disconnect after the kill (the server self-exits once its last session is
reaped) is reported as success.

### 3.8 `phux_watch`

The push half of the agent surface — tagged lifecycle/activity events
(`command_started`/`finished`, `title_changed`, `bell`,
`pane_spawned`/`closed`, `dirty`, `idle`, `asked`) — exposed as a **bounded
one-shot** tool. MCP `tools/call` is request/response while the underlying
stream is long-lived, so the tool collects events until a bound is reached,
then returns the batch; an MCP host that wants a truly live stream still
shells out to `phux watch --json`.

| Param | Type | Required | Meaning |
|---|---|---|---|
| `target` | string | no | Pane selector to watch. Omit for server-wide events. |
| `max_events` | number | no | Return after collecting this many events. |
| `timeout_secs` | number | no | Return after this many seconds. **Recommended** — without it (and without `max_events`) the call blocks until the server exits. |
| `socket` | string | no | Override the UDS path (see §2). |

Result: `{ events: [ { event, terminal?, ...payload } ], count: N }`, the
same per-event JSON shape as `phux watch --json`, including `asked` payloads
with `id`, `question`, `suggestions`, and nullable `elapsed_seconds`. It is an
accelerator of `phux_wait`'s poll floor, not a replacement: `phux_wait` is
still the way to block on a specific screen condition.

### 3.9 `phux_plugin_action`

Executes one action declared by an enabled configured plugin manifest.

| Param | Type | Required | Meaning |
|---|---|---|---|
| `plugin_id` | string | yes | Configured plugin id. |
| `action_id` | string | yes | Plugin-local action id. |
| `timeout_secs` | number | no | Give up after this many seconds. Omit to wait indefinitely. |
| `cwd` | string | no | Override cwd. Relative paths resolve under the plugin root. |
| `config` | string | no | Override `config.toml` path; defaults to the normal phux config path. |

Result: the same `schema_version = 1` action result as
`phux config run --json`: `plugin_id`, `action_id`, `command`, `cwd`,
`outcome`, `exit_code`, `stdout`, `stderr`, and `duration_ms`. The runtime
executes argv directly from the plugin root; there is no hidden shell
expansion.

### 3.10 `phux_ask`

Reports that an agent in a pane is asking for human input. This is the
MCP twin of `phux ask`: it emits the same `asked` event that
`phux_watch` / `phux watch --json` observe, without writing a sentinel to
the target PTY.

| Param | Type | Required | Meaning |
|---|---|---|---|
| `target` | string | yes | Selector (see §2). |
| `id` | string | yes | Stable question id for answer correlation. |
| `question` | string | yes | Human-facing question text. |
| `suggestions` | array of string | no | Suggested answers in display order. |
| `elapsed_seconds` | number | no | Seconds the agent has already been waiting. |
| `socket` | string | no | Override the UDS path (see §2). |

Result: `{ event: "asked", terminal: "@N", id, question, suggestions,
elapsed_seconds }`, matching the CLI's `phux ask --json` projection.

### 3.11 `phux_plugin_workspace`

Lists configured plugin workspace profiles. This is the workspace
composition/read half of the plugin surface: it returns the manifest-level
agents, actions, events, and pane roles that describe an agent bench. It
does not create panes by itself; agents compose the returned profile with
the existing `phux_new`, `phux_send_keys`, `phux_run`, `phux_wait`, and
`phux_plugin_action` tools.

| Param | Type | Required | Meaning |
|---|---|---|---|
| `plugin_id` | string | no | Optional configured plugin id filter. |
| `workspace_id` | string | no | Optional plugin-local workspace id filter. |
| `config` | string | no | Override `config.toml` path; defaults to the normal phux config path. |

Result: `{ workspaces, count }`, where each item contains
`plugin_id`, `plugin_name`, `enabled`, and the serialized plugin
`workspace` profile. A filtered miss is an MCP tool error.

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

The MCP tools are name-for-name adapters over the CLI's agent surfaces:
`phux_ls` ↔ `phux ls`; `phux_snapshot` / `phux_send_keys` / `phux_run` /
`phux_wait` / `phux_new` ↔ `phux snapshot` / `send-keys` / `run` / `wait` /
`new`; `phux_ask` ↔ `phux ask`; `phux_plugin_action` ↔ `phux config run`;
and `phux_plugin_workspace` ↔ the plugin manifest workspace profile
([`agents.md`](./agents.md) §1). Targeted pane tools use the same client-side
resolution and tiebreaks because the adapter wraps the same `phux-client`
functions the CLI does. Plugin actions instead share the `phux-plugin`
runtime with the CLI. The one shape difference is the `phux_run` timeout
body (§3.4).

Per [ADR-0022](../../ADR/0022-tool-for-agents.md), the CLI and its JSON
schema are the stable agent contract; the wire underneath stays additive
and versioned, and MCP is one thin adapter over it among several.
