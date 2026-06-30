# phux Agent Tools Demo Plugin

This fixture is a first-class local package for trying the agentic plugin
surface without touching your real `~/.config/phux/config.toml`.

Run it from the repository root:

```sh
export XDG_CONFIG_HOME="$PWD/examples/plugins/agent-tools/config"

cargo run -q -p phux -- config plugins
cargo run -q -p phux -- config plugins --json
cargo run -q -p phux -- config run com.phux.demo.agent-tools inspect
cargo run -q -p phux -- config run com.phux.demo.agent-tools inspect --json
cargo run -q -p phux -- config run com.phux.demo.agent-tools list-integrations
cargo run -q -p phux -- config run com.phux.demo.agent-tools validate-integrations
cargo run -q -p phux -- config run com.phux.demo.agent-tools detect-agents
```

Expected human output:

```text
com.phux.demo.agent-tools 0.1.0 (enabled)
```

The action prints the boundary the plugin system is meant to keep:

```text
phux plugin demo
core=stable terminal/session host
plugin=agentic workflow package
plugin_id=com.phux.demo.agent-tools
action_id=inspect
root=/path/to/phux/examples/plugins/agent-tools
```

`phux config run --json` wraps that stdout with the stable action result
schema, including argv, cwd, exit code, stderr, and duration.

The manifest also declares an `agent-bench` workspace profile. It composes
the local inspection/list/validation actions with an `agent-board` pane entry,
which gives future `phux save` / `phux restore` / herd-style runners a typed
profile to consume without loading plugin code into the server.

## Integration templates

The `integrations/*.toml` files are sample manifests for terminal-native
agents that phux can launch, supervise, and report on through plugin actions.
They are intentionally local, documented fixtures rather than hidden product
magic:

- `codex.toml`
- `claude-code.toml`
- `gemini-cli.toml`
- `generic-shell-agent.toml`

Each template declares a stable id, display name, capabilities, a launch
command, and an opt-in detection command. Detection never probes the user's
machine by default. To run it deliberately:

```sh
PHUX_AGENT_TOOLS_DETECT=1 \
  cargo run -q -p phux -- config run com.phux.demo.agent-tools detect-agents
```

Tests can avoid local installations by overriding the detection search path:

```sh
PHUX_AGENT_TOOLS_DETECT=1 \
PHUX_AGENT_TOOLS_PATH=/tmp/fake-agent-bin \
  cargo run -q -p phux -- config run com.phux.demo.agent-tools detect-agents
```

`list-integrations` and `validate-integrations` are pure fixture checks and
do not execute or inspect any local agent binaries.
