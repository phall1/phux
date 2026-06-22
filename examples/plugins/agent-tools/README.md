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
