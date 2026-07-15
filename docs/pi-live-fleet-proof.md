---
audience: consumers, contributors
stability: stable
last-reviewed: 2026-07-15
---
# Pi live-fleet proof

**TL;DR.** [`pi-live-fleet.cast`](./assets/pi-live-fleet.cast) records Pi using
only the `@phux/pi` tools to create and spatially arrange a real Claude Code +
OpenAI Codex fleet, drive both agents to independently return proof tokens,
swap their panes, inventory ownership, and capture the composited layout.

Play the checked-in asciinema v3 recording locally:

```sh
asciinema play docs/assets/pi-live-fleet.cast
```

The successful take records all model-facing tool calls and results. Its final
checks include:

```text
CLAUDE_LIVE_FLEET_OK
CODEX_LIVE_FLEET_OK
LIVE_PI_PHUX_FLEET_OK
```

The take is intentionally useful as failure-recovery evidence too: Claude's
first response omitted part of its requested token, so Pi inspected the pane,
corrected the prompt through `phux_send_keys`, and waited for the exact result
before declaring success. The recording redacts only the operator name, email,
and hostname exposed by agent startup UI; orchestration output is unchanged.
Its SHA-256 is
`d8efab7ea38857d908377a4dbfa57041e64ff2c50e93355e0e29be82a15f008b`.

## Reproduce it

Prerequisites are a current debug build, Pi with a configured model, locally
authenticated `claude` and `codex` CLIs, asciinema, Node.js, and `jq`. The demo
agent integrations must be enabled in the phux config.

```sh
cargo build -p phux
asciinema record --return --window-size 140x40 --idle-time-limit 2 \
  --title 'Pi orchestrates a real Claude + Codex fleet through phux' \
  --command examples/agents/record-pi-live-fleet \
  /tmp/pi-live-fleet.cast
```

`record-pi-live-fleet` starts a private Unix-socket server and gives Pi no
builtin tools. The Pi extension invokes the canonical external CLI without a
shell. The harness requires all three exact proof tokens, independently checks
the final agent inventory with the CLI, and then removes its isolated server
and temporary working directory. Override `PHUX`, `PHUX_PI_EXTENSION`, or
`PI_MODEL` to test another binary, extension checkout, or configured Pi model.
