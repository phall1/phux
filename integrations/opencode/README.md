# @phux/opencode

Public OpenCode tools for controlling shared terminals through the external
`phux` CLI. Loading the plugin does not start `phux`, contact an LLM, import
OpenCode TUI internals, or pair to a remote service.

## Setup

The package default export is an OpenCode server plugin:

```json
{
  "$schema": "https://opencode.ai/config.json",
  "plugin": ["@phux/opencode"]
}
```

The optional plugin settings `executable`, `socket`, and
`lifecycleTimeoutMs` select an alternate CLI, socket, and metadata-command
local deadline. The defaults are `phux`, its default socket, and 1000 ms.

The package exposes six tools:

| Tool | Behavior |
| --- | --- |
| `phux_list` | List sessions without changing focus. |
| `phux_create` | Create a session without attaching and select its seed pane for this plugin instance. |
| `phux_snapshot` | Read a bounded terminal projection. |
| `phux_send_keys` | Send key items; this is not a paste operation. |
| `phux_run` | Run one shell command string through the documented phux sentinel. |
| `phux_wait` | Wait for visible text, idleness, or indefinitely when conditions and deadlines are omitted. |

Targeted tools resolve a target in this order: an explicit `target`, the pane
auto-selected by `phux_create` in this plugin instance, then `PHUX_TARGET`.
They fail if none is available and never silently use phux focus. `until` and
`idle_ms` are mutually exclusive. `phux_run.command` is one string, not argv.
Snapshot, run, and wait results are bounded to the newest 200 lines and 12 KiB
and carry an explicit truncation notice. OpenCode's tool abort signal is passed
to every subprocess. Short commands have a 10 second default local deadline;
run and wait have no implicit local deadline so their documented indefinite
forms remain indefinite.

## Lifecycle metadata

The plugin uses only documented public server hooks and events. Public
`session.status` busy/idle and `session.idle` events publish `working`/`idle`
agent records with owner label `opencode:<public session id>`. A successful
`phux_create` tool invocation is also an honest working signal for that public
session when no status event has arrived. Metadata commands are best effort and
use short local deadlines.

Documented `session.deleted` and plugin `dispose` teardown paths inspect the
current declaration with `phux agent show` and call `phux agent clear` only if
its `name`, `kind`, and owner session still match. Retry status and unrelated
events do not invent state transitions. OpenCode 1.18.1 exposes no documented
per-session reload distinction, so reload preservation is not claimed; process
crashes and forced termination cannot run best-effort cleanup.

## Public SDK boundary

Development is pinned exactly to `@opencode-ai/plugin` 1.18.1. Source uses only
its public root `Plugin`, hooks, tool helper, schemas, context, and result types.
The package does not import the optional TUI export or OpenCode internals. The
public helper and schemas are bundled into the standalone artifact, as is the
host-independent shared `PhuxCli` source from `integrations/pi`; the packed
runtime has no `@phux/pi` or `@opencode-ai/plugin` import. The external `phux`
executable is not embedded.

## Development

```sh
npm ci
npm run typecheck
npm test
npm run smoke:opencode # requires an opencode executable; no LLM call
```

`npm test` type-checks, builds, runs fake-runner tool and lifecycle contract
tests, and installs the generated tarball in a temporary consumer. The smoke
command loads the built plugin URL through `opencode debug config` under
isolated HOME/XDG directories. Set `OPENCODE_BIN` to select the executable.
