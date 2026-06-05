use serde::{Deserialize, Serialize};

use super::{LayoutDecodeError, LayoutNode, SplitDir, TerminalId, unknown_layout_variant, unknown_split_dir};

#[derive(Debug, Deserialize)]
pub struct VersionProbe {
    pub version: u8,
}

/// The legacy v1 single-window envelope.
#[derive(Debug, Serialize, Deserialize)]
pub struct CborEnvelope {
    pub version: u8,
    pub root: CborLayoutNode,
    pub focus: CborTerminalId,
}

/// The v2 multi-window envelope (docs/spec/L3.md §3.2).
#[derive(Debug, Serialize, Deserialize)]
pub struct CborWorkspaceEnvelope {
    pub version: u8,
    pub windows: Vec<CborWindow>,
    pub focused_window_index: u32,
}

/// One window inside [`CborWorkspaceEnvelope`].
#[derive(Debug, Serialize, Deserialize)]
pub struct CborWindow {
    pub name: String,
    pub root: CborLayoutNode,
    pub focused_terminal: CborTerminalId,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CborLayoutNode {
    Leaf {
        pane: CborTerminalId,
    },
    Split {
        dir: CborSplitDir,
        ratio: f32,
        left: Box<Self>,
        right: Box<Self>,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CborSplitDir {
    Horizontal,
    Vertical,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CborTerminalId {
    Local { id: u32 },
    Satellite { host: String, id: u32 },
}

impl From<SplitDir> for CborSplitDir {
    fn from(value: SplitDir) -> Self {
        match value {
            SplitDir::Horizontal => Self::Horizontal,
            SplitDir::Vertical => Self::Vertical,
            // `SplitDir` is `#[non_exhaustive]` (wire-crate concession);
            // a future variant would be wire-breaking and cannot have
            // been decoded into the in-memory tree this function sees.
            _ => unknown_split_dir(),
        }
    }
}

impl From<CborSplitDir> for SplitDir {
    fn from(value: CborSplitDir) -> Self {
        match value {
            CborSplitDir::Horizontal => Self::Horizontal,
            CborSplitDir::Vertical => Self::Vertical,
        }
    }
}

impl From<&TerminalId> for CborTerminalId {
    fn from(value: &TerminalId) -> Self {
        match value {
            TerminalId::Local { id } => Self::Local { id: *id },
            TerminalId::Satellite { host, id } => Self::Satellite {
                host: host.as_str().to_owned(),
                id: *id,
            },
        }
    }
}

impl From<CborTerminalId> for TerminalId {
    fn from(value: CborTerminalId) -> Self {
        match value {
            CborTerminalId::Local { id } => Self::local(id),
            CborTerminalId::Satellite { host, id } => Self::satellite(host.as_str(), id),
        }
    }
}

impl From<&LayoutNode> for CborLayoutNode {
    fn from(value: &LayoutNode) -> Self {
        match value {
            LayoutNode::Leaf(pane) => Self::Leaf { pane: pane.into() },
            LayoutNode::Split {
                dir,
                ratio,
                left,
                right,
            } => Self::Split {
                dir: (*dir).into(),
                ratio: *ratio,
                left: Box::new(Self::from(left.as_ref())),
                right: Box::new(Self::from(right.as_ref())),
            },
            _ => unknown_layout_variant(),
        }
    }
}

impl CborLayoutNode {
    pub fn into_layout_node(self) -> Result<LayoutNode, LayoutDecodeError> {
        Ok(match self {
            Self::Leaf { pane } => LayoutNode::Leaf(pane.into()),
            Self::Split {
                dir,
                ratio,
                left,
                right,
            } => {
                if !ratio.is_finite() || !(0.0..=1.0).contains(&ratio) {
                    return Err(LayoutDecodeError::MalformedRatio(ratio));
                }
                LayoutNode::Split {
                    dir: dir.into(),
                    ratio,
                    left: Box::new(left.into_layout_node()?),
                    right: Box::new(right.into_layout_node()?),
                }
            }
        })
    }
}

// -----------------------------------------------------------------------------
// Tests — unit + property
// -----------------------------------------------------------------------------

