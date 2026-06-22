use std::path::PathBuf;
use std::time::Duration;

use phux_config::loader as config_loader;
use serde_json::{Value, json};

use crate::tools::ToolError;

pub(crate) async fn call(args: &Value) -> Result<Value, ToolError> {
    let plugin_id = required_str(args, "plugin_id")?.to_owned();
    let action_id = required_str(args, "action_id")?.to_owned();
    let timeout = num_arg(args, "timeout_secs").map(Duration::from_secs);
    let cwd = str_arg(args, "cwd").map(PathBuf::from);
    let config_path =
        str_arg(args, "config").map_or_else(config_loader::config_path, PathBuf::from);

    let request = phux_plugin::PluginActionRequest {
        plugin_id,
        action_id,
        timeout,
        cwd,
    };
    let result = phux_plugin::run_configured_action(&config_path, &request)
        .await
        .map_err(|err| ToolError::new(err.to_string()))?;
    serde_json::to_value(result)
        .map_err(|err| ToolError::new(format!("failed to serialize plugin action: {err}")))
}

pub(crate) fn schema() -> Value {
    json!({
        "name": "phux_plugin_action",
        "description": "Execute one action declared by a configured phux plugin manifest. Runs argv directly from the plugin root; no hidden shell expansion.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "plugin_id": { "type": "string", "description": "Configured plugin id." },
                "action_id": { "type": "string", "description": "Plugin-local action id." },
                "timeout_secs": { "type": "number", "description": "Give up after this many seconds. Omit to wait indefinitely." },
                "cwd": { "type": "string", "description": "Override cwd. Relative paths resolve under the plugin root." },
                "config": { "type": "string", "description": "Override config.toml path. Defaults to the normal phux config path." }
            },
            "required": ["plugin_id", "action_id"]
        }
    })
}

fn str_arg<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(Value::as_str)
}

fn required_str<'a>(args: &'a Value, key: &str) -> Result<&'a str, ToolError> {
    str_arg(args, key).ok_or_else(|| ToolError::new(format!("missing required string `{key}`")))
}

fn num_arg(args: &Value, key: &str) -> Option<u64> {
    args.get(key).and_then(Value::as_u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn fixture(tmp: &TempDir) -> std::path::PathBuf {
        let plugin_dir = tmp.path().join("plugin");
        std::fs::create_dir_all(&plugin_dir).expect("create plugin dir");
        std::fs::write(
            plugin_dir.join("phux-plugin.toml"),
            r#"
id = "example.actions"
name = "Actions"
version = "0.1.0"
min_phux_version = "0.0.2"

[[actions]]
id = "probe"
title = "Probe"
command = ["sh", "-c", "printf mcp"]
"#,
        )
        .expect("write manifest");
        let config_path = tmp.path().join("config.toml");
        std::fs::write(
            &config_path,
            format!(
                r#"
[[plugins]]
manifest = "{}"
enabled = true
"#,
                plugin_dir.join("phux-plugin.toml").display()
            ),
        )
        .expect("write config");
        config_path
    }

    #[tokio::test]
    async fn plugin_action_tool_executes_configured_action() {
        let tmp = TempDir::new().expect("tempdir");
        let config = fixture(&tmp);

        let result = call(&json!({
            "plugin_id": "example.actions",
            "action_id": "probe",
            "config": config,
        }))
        .await
        .expect("tool succeeds");

        assert_eq!(result["plugin_id"], "example.actions");
        assert_eq!(result["action_id"], "probe");
        assert_eq!(result["outcome"], "completed");
        assert_eq!(result["exit_code"], 0);
        assert_eq!(result["stdout"], "mcp");
    }
}
