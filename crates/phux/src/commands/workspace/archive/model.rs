use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

pub(super) const ARCHIVE_SCHEMA_VERSION: u8 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(super) struct WorkspaceArchive {
    pub(super) schema_version: u8,
    pub(super) sessions: Vec<WorkspaceSession>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(super) struct WorkspaceSession {
    pub(super) name: String,
    #[serde(default)]
    pub(super) active: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) command: Option<Vec<String>>,
    #[serde(default)]
    pub(super) windows: Vec<WorkspaceWindow>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(super) struct WorkspaceWindow {
    pub(super) name: String,
    #[serde(default)]
    pub(super) active: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) layout: Option<WorkspaceLayoutNode>,
    #[serde(default)]
    pub(super) panes: Vec<WorkspacePane>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct WorkspacePane {
    #[serde(default)]
    pub(super) active: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) command: Option<Vec<String>>,
    #[serde(default)]
    pub(super) cols: u16,
    #[serde(default)]
    pub(super) rows: u16,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(super) enum WorkspaceLayoutNode {
    Pane {
        pane: usize,
    },
    Split {
        dir: WorkspaceSplitDir,
        ratio: f32,
        left: Box<WorkspaceLayoutNode>,
        right: Box<WorkspaceLayoutNode>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum WorkspaceSplitDir {
    Horizontal,
    Vertical,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct RestoreSummary {
    pub(super) schema_version: u8,
    pub(super) restored: Vec<String>,
    pub(super) skipped_existing: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RestorePlan {
    pub(super) creates: Vec<CreateRequest>,
    pub(super) skipped_existing: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct CreateRequest {
    pub(super) name: String,
    pub(super) cwd: Option<String>,
    pub(super) command: Option<Vec<String>>,
}

pub(super) fn parse_archive(input: &str) -> Result<WorkspaceArchive, String> {
    let archive: WorkspaceArchive = serde_json::from_str(input)
        .map_err(|err| format!("invalid workspace archive JSON: {err}"))?;
    if archive.schema_version != ARCHIVE_SCHEMA_VERSION {
        return Err(format!(
            "unsupported workspace archive schema {}; expected {ARCHIVE_SCHEMA_VERSION}",
            archive.schema_version
        ));
    }
    validate_archive(&archive)?;
    Ok(archive)
}

pub(super) fn restore_plan(
    archive: &WorkspaceArchive,
    existing_sessions: &[String],
) -> Result<RestorePlan, String> {
    validate_archive(archive)?;
    let existing: BTreeSet<&str> = existing_sessions.iter().map(String::as_str).collect();
    let mut creates = Vec::new();
    let mut skipped_existing = Vec::new();
    for session in &archive.sessions {
        if existing.contains(session.name.as_str()) {
            skipped_existing.push(session.name.clone());
            continue;
        }
        creates.push(CreateRequest {
            name: session.name.clone(),
            cwd: session
                .cwd
                .clone()
                .or_else(|| first_pane(session).and_then(|pane| pane.cwd.clone())),
            command: session
                .command
                .clone()
                .or_else(|| first_pane(session).and_then(|pane| pane.command.clone())),
        });
    }
    Ok(RestorePlan {
        creates,
        skipped_existing,
    })
}

fn validate_archive(archive: &WorkspaceArchive) -> Result<(), String> {
    let mut names = BTreeSet::new();
    for session in &archive.sessions {
        if session.name.trim().is_empty() {
            return Err("workspace archive contains a session with an empty name".to_owned());
        }
        if !names.insert(&session.name) {
            return Err(format!(
                "workspace archive contains duplicate session '{}'",
                session.name
            ));
        }
    }
    Ok(())
}

fn first_pane(session: &WorkspaceSession) -> Option<&WorkspacePane> {
    session
        .windows
        .iter()
        .find_map(|window| window.panes.first())
}

#[cfg(test)]
#[allow(clippy::expect_used, reason = "tests")]
mod tests {
    use super::*;

    #[test]
    fn parses_restore_archive_with_missing_command_and_cwd() {
        let json = r#"{
            "schema_version": 1,
            "sessions": [
                {
                    "name": "bench",
                    "windows": [
                        {
                            "name": "main",
                            "panes": [
                                { "title": "agent", "cols": 80, "rows": 24 }
                            ]
                        }
                    ]
                }
            ]
        }"#;

        let archive = parse_archive(json).expect("archive parses");
        let plan = restore_plan(&archive, &[]).expect("restore plan");

        assert_eq!(plan.creates.len(), 1);
        assert_eq!(plan.creates[0].name, "bench");
        assert_eq!(plan.creates[0].cwd, None);
        assert_eq!(plan.creates[0].command, None);
    }
}
