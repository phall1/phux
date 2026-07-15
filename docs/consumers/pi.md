---
audience: humans, agents, consumers, contributors
stability: evolving
last-reviewed: 2026-07-15
---

# Pi integration

**TL;DR.** `@phux/pi` lets Pi select and operate a pane in an external local
phux server while preserving the target in Pi's session history. It provides
six bounded terminal tools, three human commands, and best-effort Pi lifecycle
metadata. It does not embed a terminal, provide remote authentication, or own
the phux server.

---

## Requirements and installation

The package requires Node.js 20 or newer, Pi, and an external `phux` executable
on `PATH`. It does not bundle phux or start a separate implementation. The
minimum compatible binary is `phux 0.1.0`; check it before loading the package:

```sh
phux --version
```

From a checkout of this repository, install the package directory, not the git
root:

```sh
pi install ./integrations/pi
```

The repository root is not a Pi package, so a git source that resolves to that
root is not an installation method for this integration. `@phux/pi` is also not
published to npm today. Do not use `npm:@phux/pi` unless a future release is
actually present in the registry.

A packed artifact can be tested or moved to another machine without implying
registry publication:

```sh
cd integrations/pi
npm ci
npm pack
mkdir -p phux-pi-packed
tar -xzf phux-pi-0.1.0.tgz -C phux-pi-packed
pi install ./phux-pi-packed/package
```

`npm pack` runs the package build. Pi installs the extracted package directory;
it does not load the `.tgz` file as an extension. The artifact remains dependent
on a compatible external `phux` binary on the destination machine.

The extension inherits `PHUX_SOCKET`; set it before starting Pi when the server
uses a non-default local Unix socket. There is no package command for choosing
an alternate executable. Library consumers can construct `PhuxCli` with an
absolute `executable`, but the installed Pi extension expects `phux` on `PATH`.

## Surface

The extension registers exactly these six model tools:

| Tool | Operation |
|---|---|
| `phux_list` | List phux sessions. |
| `phux_create` | Create a named session without attaching and select its seed pane. |
| `phux_snapshot` | Read a pane's bounded screen projection. |
| `phux_send_keys` | Send named keys or literal key text to a pane. |
| `phux_run` | Run one shell command line and return its exit result. |
| `phux_wait` | Wait for visible text or idleness and return the bounded final screen. |

It registers exactly these three human commands:

| Command | Operation |
|---|---|
| `/phux` | Inventory panes and choose the default target. |
| `/phux-status` | Refresh and report the saved target and its availability. |
| `/phux-attach` | Print a human attach argv; it never executes the attach. |

The headless phux CLI owns argument syntax, selector rules, JSON, and exit
codes. Use the [agent CLI guide](./agents.md) for that canonical contract rather
than treating this adapter as a second CLI definition.

Tool output sent to the model is bounded to 200 lines and 12 KiB. The result
states when the adapter truncated output and preserves a separate truncation
flag reported by phux.

## Selecting and preserving a target

`/phux` inventories the public agent projection, groups panes by session, and
stores the chosen canonical pane selector plus its owning session and window.
`phux_create` stores the same ownership fields for the newly created seed pane.
The selection is appended as a versioned custom entry in Pi's session branch,
so resuming or moving through the branch reconstructs the latest selection on
that branch.

Restoration never silently falls back to phux's focused pane. Before an
implicit target is made available to tools or lifecycle reporting, the
extension confirms that the saved pane id still belongs to the saved session
and window. A missing pane or reused id is **stale**: the selection remains
visible for diagnosis, but an implicit targeted tool refuses it. An inventory
failure is **unavailable** and likewise preserves the saved selection. Pass an
explicit target to a tool only when intentionally overriding the selection.

## First shared-terminal walkthrough

1. Start or locate a local phux server outside Pi. For a first local session,
   `phux new work` creates and attaches interactively.
2. Start Pi with the package installed and run `/phux`.
3. Choose the pane under `work`. The Pi status line shows the saved target.
4. Ask Pi to inspect the pane or run a discrete command. Pi can use
   `phux_snapshot`, `phux_run`, and `phux_wait` without attaching or resizing
   the human view.
5. Run `/phux-status` before a handoff if the pane may have exited or moved.
6. Run `/phux-attach`. Pi prints an argv such as
   `["phux","attach","work"]` and identifies the pane to navigate to after
   attach. Copy the argv into a separate real terminal. The extension does not
   execute it and does not open a nested terminal inside Pi.

A human attach joins the same terminal state. It is not a read-only monitor:
agent input and human input can interleave at the PTY. The package does not
serialize independent writers, reserve a prompt, or provide a transaction
around `snapshot` followed by `send_keys`. Coordinate before typing into a pane
that the other participant is actively driving, and use explicit targets when
several panes are live.

## Lifecycle metadata

When a target is ownership-validated as available, the extension reports a
whole `phux.agent/v1` record with `name=pi`, `kind=pi`, and a Pi-session owner
in the `session` field. Pi's `agent_start` event maps to `working` with normal
attention; `agent_settled` maps to `idle` with low attention. Writes are
serialized, debounced, locally bounded, and best-effort, so a missing server
does not break Pi startup or shutdown.

A target switch clears the old declaration only after reading it back and
confirming that Pi still owns it. Normal shutdown applies the same ownership
check. Extension reload preserves the declaration for the replacement
instance, avoiding a clear/set flicker. This is status metadata, not an input
lock; it does not prevent the interleaved-input case above.

## Current boundaries and security

- There is no paste tool. `phux_send_keys` sends key items, not a clipboard or
  bracketed-paste operation. Do not present it as safe paste support; dedicated
  paste remains tracked work under bead `phux-foir`.
- The package is a Node/Pi integration around an external native process. It
  has no WASM build and does not render or nest a terminal inside Pi.
- Remote phux attach, pairing, and token transport are not supported. The
  adapter accepts a local Unix socket path, not `--quic`, `--ws`, bearer-token,
  or certificate arguments.
- Pairing tokens and certificate material are secrets. Never place them in a
  Pi prompt, tool argument, saved target, lifecycle record, attach handoff, or
  smoke-test output. Configure remote access outside this package and expose a
  suitable local phux endpoint only if its trust boundary is understood.
- `/phux-attach` deliberately emits only local attach argv plus session/pane
  navigation. It neither reads nor prints remote credentials.

Package-local development and validation commands live in
[`../../integrations/pi/README.md`](../../integrations/pi/README.md).
