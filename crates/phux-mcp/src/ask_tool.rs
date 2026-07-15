use std::path::Path;

use phux_client::ask::AskedPayload;
use phux_client::attach::connection::Connection;
use phux_client::selector::{self, Selector};
use phux_protocol::ids::TerminalId;
use phux_protocol::wire::frame::{
    Command as WireCommand, CommandResult, CommandValue, FrameKind, StateScope,
};
use phux_protocol::wire::info::SessionSnapshot;
use serde_json::{Value, json};

use crate::socket;
use crate::tools::ToolError;

pub(crate) async fn call(args: &Value) -> Result<Value, ToolError> {
    let socket = socket::resolve(str_arg(args, "socket"));
    let target = required_str(args, "target")?;
    let selector = selector::parse(target)
        .map_err(|err| ToolError::new(format!("invalid target '{target}': {err}")))?;
    let snapshot = get_state(&socket).await?;
    let pane = resolve_one(&selector, &snapshot)?;
    let payload = AskedPayload {
        id: required_str(args, "id")?.to_owned(),
        question: required_str(args, "question")?.to_owned(),
        suggestions: string_array_opt(args, "suggestions")?.unwrap_or_default(),
        elapsed_seconds: num_arg(args, "elapsed_seconds"),
    };

    phux_client::ask::report(&socket, pane.clone(), payload.clone()).await?;
    Ok(json!({
        "event": "asked",
        "terminal": format!("@{}", pane.local_id().unwrap_or(0)),
        "id": payload.id,
        "question": payload.question,
        "suggestions": payload.suggestions,
        "elapsed_seconds": payload.elapsed_seconds,
    }))
}

pub(crate) fn schema() -> Value {
    json!({
        "name": "phux_ask",
        "description": "Report that an agent in a pane is asking for human input, emitting the same asked event phux watch observes.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "target": { "type": "string", "description": "Target selector: session, session:window, session:window.pane, @paneid, or `.` for the focused session. `=` is unsupported because MCP has no attached-client focus history." },
                "id": { "type": "string", "description": "Stable question id for answer correlation." },
                "question": { "type": "string", "description": "Human-facing question text." },
                "suggestions": { "type": "array", "items": { "type": "string" }, "description": "Suggested answers in display order." },
                "elapsed_seconds": { "type": "number", "description": "Seconds the agent has already been waiting." },
                "socket": { "type": "string" }
            },
            "required": ["target", "id", "question"]
        }
    })
}

async fn get_state(socket: &Path) -> Result<SessionSnapshot, ToolError> {
    let mut conn = Connection::connect(socket).await?;
    conn.send(&FrameKind::Command {
        request_id: 1,
        command: WireCommand::GetState {
            scope: StateScope::Server,
        },
    })
    .await?;
    loop {
        if let FrameKind::CommandResult {
            request_id: 1,
            result,
        } = conn.recv().await?
        {
            return match result {
                CommandResult::OkWith(CommandValue::State(snap)) => Ok(snap),
                CommandResult::Error { message, .. } => Err(ToolError::new(message)),
                other => Err(ToolError::new(format!(
                    "unexpected GET_STATE result: {other:?}"
                ))),
            };
        }
    }
}

fn resolve_one(selector: &Selector, snapshot: &SessionSnapshot) -> Result<TerminalId, ToolError> {
    let candidates = selector::resolve(selector, snapshot);
    selector::pick_target_pane(&candidates, &snapshot.focused_pane)
        .ok_or_else(|| ToolError::new("no such target"))
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

fn string_array_opt(args: &Value, key: &str) -> Result<Option<Vec<String>>, ToolError> {
    let Some(value) = args.get(key) else {
        return Ok(None);
    };
    let arr = value
        .as_array()
        .ok_or_else(|| ToolError::new(format!("`{key}` must be an array of strings")))?;
    arr.iter()
        .map(|v| {
            v.as_str()
                .map(str::to_owned)
                .ok_or_else(|| ToolError::new(format!("`{key}` must contain only strings")))
        })
        .collect::<Result<Vec<String>, _>>()
        .map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_requires_target_id_and_question() {
        let schema = schema();
        assert_eq!(schema["name"], json!("phux_ask"));
        assert_eq!(
            schema["inputSchema"]["required"],
            json!(["target", "id", "question"])
        );
        assert_eq!(
            schema["inputSchema"]["properties"]["suggestions"]["items"]["type"],
            json!("string")
        );
    }

    #[test]
    fn suggestions_default_to_absent_and_reject_non_strings() {
        assert_eq!(string_array_opt(&json!({}), "suggestions").unwrap(), None);
        assert_eq!(
            string_array_opt(&json!({ "suggestions": ["yes", "no"] }), "suggestions").unwrap(),
            Some(vec!["yes".to_owned(), "no".to_owned()])
        );
        assert!(string_array_opt(&json!({ "suggestions": [1] }), "suggestions").is_err());
    }
}
