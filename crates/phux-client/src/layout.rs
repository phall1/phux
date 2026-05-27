//! Client-side mirror of the binary split-tree pane layout.
//!
//! Per [ADR-0019] decision 3 the reference TUI keeps its own copy of the
//! layout tree. The shape is the wire-side [`LayoutNode`] (re-exported,
//! not redefined); the operations (`split_at`, `kill_pane`,
//! `focus_direction`, `pane_rects`) are re-implemented here as free
//! functions over the wire type so the client crate's edge to
//! `phux-core` stays as thin as today.
//!
//! Layout persistence (per [ADR-0019] decision 1) wraps a snapshot of
//! this state in a versioned CBOR envelope —
//! `{version: u8 = 1, root: LayoutNode, focus: TerminalId}` —
//! and stores it server-side under the L3 metadata key
//! `phux.tui.layout/v1`. The encoding logic lives in
//! [`LayoutState::encode_cbor`] / [`LayoutState::decode_cbor`].
//!
//! The wire crate exposes neither `serde::Serialize` for its types nor
//! a public encoder API; for the CBOR envelope we therefore round-trip
//! through small local shim types (`CborLayoutNode`, `CborSplitDir`,
//! `CborTerminalId`) that mirror the wire shape and convert via `From`.
//!
//! [ADR-0019]: ../../ADR/0019-tui-multi-pane-rendering.md

use std::collections::HashMap;
use std::io::Cursor;

use phux_protocol::TerminalId;
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub use phux_protocol::wire::info::{LayoutNode, SplitDir};

/// Current version of the layout CBOR envelope.
///
/// Stored as the `version` field of the envelope. Bumped when the
/// envelope shape changes incompatibly; readers MUST refuse unknown
/// versions (see [`LayoutDecodeError::UnsupportedVersion`]).
pub const LAYOUT_ENVELOPE_VERSION: u8 = 1;

// -----------------------------------------------------------------------------
// Direction / Rect — TUI-local geometry types
// -----------------------------------------------------------------------------

/// Cardinal direction for [`focus_direction`].
///
/// Mirrors `phux_core::window::Direction`. Lives here, not in the wire
/// crate, because focus movement is a TUI-private concern (no
/// `FOCUS_CHANGED` frame ever rides the substrate — see ADR-0017 +
/// ADR-0019 decision 6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// Move focus upward.
    Up,
    /// Move focus downward.
    Down,
    /// Move focus left.
    Left,
    /// Move focus right.
    Right,
}

/// An axis-aligned rectangle in cell coordinates.
///
/// Mirrors `phux_core::window::Rect`. Origin is the outer viewport's
/// top-left. Border-divider accounting (per ADR-0019 decision 4)
/// happens *outside* [`pane_rects`]: callers pass `(cols - h_dividers,
/// rows - v_dividers)`, and the divider cells are drawn in the gaps
/// the tree explicitly excluded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    /// Top-left x coordinate (column).
    pub x: u16,
    /// Top-left y coordinate (row).
    pub y: u16,
    /// Width in cells.
    pub w: u16,
    /// Height in cells.
    pub h: u16,
}

// -----------------------------------------------------------------------------
// Errors
// -----------------------------------------------------------------------------

/// Errors returned by the free-function layout operations.
///
/// Mirrors `phux_core::window::LayoutError`. Duplicated because the
/// client may not depend on `phux-core` for the algorithm types
/// (ADR-0019 decision 3); this is the same enum with the same
/// variants and the same string forms.
#[derive(Debug, Clone, PartialEq, Error)]
pub enum LayoutError {
    /// The target [`TerminalId`] is not present in the tree.
    #[error("pane not in layout: {0:?}")]
    PaneNotInLayout(TerminalId),
    /// The requested split ratio is outside `(0.0, 1.0)`, or is NaN.
    #[error("invalid split ratio: {0}")]
    InvalidRatio(f32),
    /// The tree has only one leaf — [`kill_pane`] would empty it.
    ///
    /// The function returns `Ok(None)` in this case (the empty tree).
    /// This variant exists so the proptest port can keep parity with
    /// the core surface but is not emitted by the implementation.
    #[error("cannot kill the last pane in the layout")]
    LastPane,
}

/// Errors returned by [`LayoutState::decode_cbor`].
#[derive(Debug, Error)]
pub enum LayoutDecodeError {
    /// The envelope's `version` byte is one this build doesn't recognise.
    #[error("unsupported layout envelope version: {0}")]
    UnsupportedVersion(u8),
    /// The envelope decodes as CBOR but `Split.ratio` is NaN, infinite,
    /// or outside `(0.0, 1.0)`.
    #[error("malformed layout ratio: {0}")]
    MalformedRatio(f32),
    /// The envelope's CBOR shape failed to decode.
    #[error("cbor decode failure: {0}")]
    Cbor(String),
}

/// Errors returned by [`LayoutState::encode_cbor`].
#[derive(Debug, Error)]
pub enum LayoutEncodeError {
    /// Encoding a layout with no tree (`tree.is_none()`) or no focus
    /// (`focus.is_none()`) is meaningless; the envelope schema
    /// requires both.
    #[error("cannot encode empty layout state")]
    Empty,
    /// The ciborium encoder failed (typically an OOM on the
    /// in-memory `Vec<u8>` buffer — vanishingly rare in practice).
    #[error("cbor encode failure: {0}")]
    Cbor(String),
}

// -----------------------------------------------------------------------------
// LayoutState — in-memory mirror
// -----------------------------------------------------------------------------

/// In-memory mirror of the TUI's binary split tree plus the attaching
/// client's focused leaf.
///
/// The reference TUI holds one of these per attached window. On attach
/// the client requests `phux.tui.layout/v1` from the server's L3
/// metadata; if present, it decodes via [`Self::decode_cbor`] and
/// re-renders multi-pane. If absent, it falls back to single-pane
/// (the [`Default`] shape — an empty tree, no focus). On `split_at` /
/// `kill_pane` the TUI mutates the in-memory tree and pushes the new
/// shape back to the server via [`Self::encode_cbor`] + `SET_METADATA`.
///
/// Focus is per-client and never travels over the wire (ADR-0019
/// decision 6); it lives here as a convenience for the renderer.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LayoutState {
    /// The binary split tree. `None` until the first pane is seeded.
    pub tree: Option<LayoutNode>,
    /// The client-local focused leaf. `None` until the first pane is
    /// seeded; reset to `None` if the tree becomes empty.
    pub focus: Option<TerminalId>,
}

impl LayoutState {
    /// Construct an empty state — no tree, no focus.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            tree: None,
            focus: None,
        }
    }

    /// Construct a state with a single leaf and matching focus.
    #[must_use]
    pub fn single(pane: TerminalId) -> Self {
        let focus = pane.clone();
        Self {
            tree: Some(LayoutNode::Leaf(pane)),
            focus: Some(focus),
        }
    }

    /// Encode the layout as the v1 CBOR envelope described in
    /// ADR-0019 decision 1.
    ///
    /// # Errors
    /// * [`LayoutEncodeError::Empty`] if `tree` or `focus` is `None`.
    /// * [`LayoutEncodeError::Cbor`] if ciborium fails to encode (rare;
    ///   in practice only on allocator OOM for the output buffer).
    pub fn encode_cbor(&self) -> Result<Vec<u8>, LayoutEncodeError> {
        let (Some(tree), Some(focus)) = (self.tree.as_ref(), self.focus.as_ref()) else {
            return Err(LayoutEncodeError::Empty);
        };
        let envelope = CborEnvelope {
            version: LAYOUT_ENVELOPE_VERSION,
            root: CborLayoutNode::from(tree),
            focus: CborTerminalId::from(focus),
        };
        let mut buf = Vec::with_capacity(64);
        ciborium::ser::into_writer(&envelope, &mut buf)
            .map_err(|e| LayoutEncodeError::Cbor(e.to_string()))?;
        Ok(buf)
    }

    /// Decode a v1 CBOR envelope into a [`LayoutState`].
    ///
    /// # Errors
    /// * [`LayoutDecodeError::UnsupportedVersion`] if the envelope's
    ///   version byte isn't one this build knows about.
    /// * [`LayoutDecodeError::MalformedRatio`] if any `Split.ratio` is
    ///   NaN, infinite, or outside `(0.0, 1.0)`.
    /// * [`LayoutDecodeError::Cbor`] for malformed CBOR.
    pub fn decode_cbor(bytes: &[u8]) -> Result<Self, LayoutDecodeError> {
        let envelope: CborEnvelope = ciborium::de::from_reader(Cursor::new(bytes))
            .map_err(|e| LayoutDecodeError::Cbor(e.to_string()))?;
        if envelope.version != LAYOUT_ENVELOPE_VERSION {
            return Err(LayoutDecodeError::UnsupportedVersion(envelope.version));
        }
        let tree = envelope.root.into_layout_node()?;
        let focus: TerminalId = envelope.focus.into();
        Ok(Self {
            tree: Some(tree),
            focus: Some(focus),
        })
    }
}

// -----------------------------------------------------------------------------
// Free-function algorithms — ports of phux-core::window
// -----------------------------------------------------------------------------

/// Split the leaf for `target` into two, with `new_pane` as the new
/// sibling along `dir` at `ratio`.
///
/// Mirrors `phux_core::Window::split` semantics: on success the tree
/// grows by one [`LayoutNode::Leaf`] and one [`LayoutNode::Split`];
/// `target` becomes the `left` child and `new_pane` the `right` child
/// of the new interior node.
///
/// `tree` is `None` only for a fresh window. The first leaf is seeded
/// via `tree = Some(LayoutNode::Leaf(pane))` directly — callers don't
/// need a separate `seed_layout` helper because there is no
/// `LayoutNode` invariant to protect (unlike `phux-core::Window`,
/// which also tracks `panes: Vec<TerminalId>`).
///
/// # Errors
/// * [`LayoutError::PaneNotInLayout`] if `target` is not present.
/// * [`LayoutError::InvalidRatio`] if `ratio` is NaN or outside `(0, 1)`.
pub fn split_at(
    tree: &LayoutNode,
    target: &TerminalId,
    new_pane: &TerminalId,
    dir: SplitDir,
    ratio: f32,
) -> Result<LayoutNode, LayoutError> {
    validate_ratio(ratio)?;
    if !contains(tree, target) {
        return Err(LayoutError::PaneNotInLayout(target.clone()));
    }
    Ok(split_inner(tree, target, new_pane, dir, ratio))
}

/// Centralised handler for the `#[non_exhaustive]` wildcard arms on
/// matches over [`LayoutNode`]. v0.1 only knows `Leaf` and `Split`;
/// any future variant would need a corresponding update here and a
/// wire-protocol bump (see SPEC.md). Reached only via a forward-
/// compatible decode from a newer server.
//
// `clippy::panic` is workspace-denied to keep production panics rare,
// but the alternative (returning `Result` through every internal
// helper to thread a "future variant" error up to the public surface)
// trades a real algorithmic complexity for a `#[non_exhaustive]`
// concession the wire validation already enforces. Localised allow.
#[cold]
#[inline(never)]
#[allow(clippy::panic)]
fn unknown_layout_variant() -> ! {
    panic!("unknown LayoutNode variant — wire-protocol newer than this client")
}

/// Same role as [`unknown_layout_variant`] for [`SplitDir`].
#[cold]
#[inline(never)]
#[allow(clippy::panic)]
fn unknown_split_dir() -> ! {
    panic!("unknown SplitDir variant — wire-protocol newer than this client")
}

fn split_inner(
    node: &LayoutNode,
    target: &TerminalId,
    new_pane: &TerminalId,
    dir: SplitDir,
    ratio: f32,
) -> LayoutNode {
    match node {
        LayoutNode::Leaf(p) if p == target => LayoutNode::Split {
            dir,
            ratio,
            left: Box::new(LayoutNode::Leaf(target.clone())),
            right: Box::new(LayoutNode::Leaf(new_pane.clone())),
        },
        LayoutNode::Leaf(p) => LayoutNode::Leaf(p.clone()),
        LayoutNode::Split {
            dir: sd,
            ratio: r,
            left,
            right,
        } => {
            if contains(left, target) {
                LayoutNode::Split {
                    dir: *sd,
                    ratio: *r,
                    left: Box::new(split_inner(left, target, new_pane, dir, ratio)),
                    right: right.clone(),
                }
            } else {
                LayoutNode::Split {
                    dir: *sd,
                    ratio: *r,
                    left: left.clone(),
                    right: Box::new(split_inner(right, target, new_pane, dir, ratio)),
                }
            }
        }
        // `LayoutNode` is `#[non_exhaustive]`; v0.1 only defines `Leaf`
        // and `Split`. Future variants would be wire-breaking and would
        // need to reach this module before being decoded into a tree.
        _ => unknown_layout_variant(),
    }
}

/// Remove the leaf for `target`, collapsing its parent [`LayoutNode::Split`]
/// so the sibling takes its grandparent's slot.
///
/// Returns `Ok(None)` iff `target` was the last leaf — the caller
/// (typically the TUI driver) is expected to drop the whole window in
/// that case. This packs the `LastPane` signal into the `Option`,
/// which is more ergonomic for callers than the `Result<_, LastPane>`
/// shape the core surface uses.
///
/// # Errors
/// [`LayoutError::PaneNotInLayout`] if `target` is not present.
pub fn kill_pane(
    tree: &LayoutNode,
    target: &TerminalId,
) -> Result<Option<LayoutNode>, LayoutError> {
    match tree {
        LayoutNode::Leaf(p) if p == target => Ok(None),
        LayoutNode::Leaf(_) => Err(LayoutError::PaneNotInLayout(target.clone())),
        LayoutNode::Split { .. } => {
            let (new_root, found) = collapse(tree, target);
            if found {
                Ok(Some(new_root))
            } else {
                Err(LayoutError::PaneNotInLayout(target.clone()))
            }
        }
        _ => unknown_layout_variant(),
    }
}

fn collapse(node: &LayoutNode, target: &TerminalId) -> (LayoutNode, bool) {
    match node {
        LayoutNode::Leaf(p) => (LayoutNode::Leaf(p.clone()), false),
        LayoutNode::Split {
            dir,
            ratio,
            left,
            right,
        } => {
            if let LayoutNode::Leaf(p) = left.as_ref()
                && p == target
            {
                return ((**right).clone(), true);
            }
            if let LayoutNode::Leaf(p) = right.as_ref()
                && p == target
            {
                return ((**left).clone(), true);
            }
            let (new_left, found_l) = collapse(left, target);
            if found_l {
                return (
                    LayoutNode::Split {
                        dir: *dir,
                        ratio: *ratio,
                        left: Box::new(new_left),
                        right: right.clone(),
                    },
                    true,
                );
            }
            let (new_right, found_r) = collapse(right, target);
            (
                LayoutNode::Split {
                    dir: *dir,
                    ratio: *ratio,
                    left: Box::new(new_left),
                    right: Box::new(new_right),
                },
                found_r,
            )
        }
        _ => unknown_layout_variant(),
    }
}

/// Return the neighbour of `current` in direction `dir`, if any.
///
/// Mirrors `phux_core::Window::focus_direction` — see that function's
/// docs for the algorithm. Returns `None` if `current` is not in the
/// tree or if no neighbour exists in that direction (border of the
/// outer viewport).
#[must_use]
pub fn focus_direction(
    tree: &LayoutNode,
    current: &TerminalId,
    dir: Direction,
) -> Option<TerminalId> {
    let mut path: Vec<(SplitDir, ChildSide)> = Vec::new();
    if !record_path(tree, current, &mut path) {
        return None;
    }
    for i in (0..path.len()).rev() {
        let (split_dir, came_from) = path[i];
        if matches_to_sibling(split_dir, dir, came_from) {
            let sibling = sibling_at_depth(tree, &path, i)?;
            return Some(descend_to_leaf(sibling, dir, &path[i + 1..]));
        }
    }
    None
}

/// Compute the bounding rectangle of every leaf given the outer
/// viewport dims.
///
/// Returns an empty map if the tree is empty (the caller passed
/// `LayoutState::tree.is_none()` and called this directly on a
/// freshly-constructed sentinel — defensive; the real call site
/// guards on `Option`).
///
/// Rectangles tile `(0, 0, dims.0, dims.1)` exactly: dimensions sum
/// to the parent's dim along the split axis at every interior node
/// (the `split_dim` rounding rule below guarantees no slop).
///
/// `dims` is the **content** rectangle — border-divider accounting
/// happens outside this function (see ADR-0019 decision 4).
#[must_use]
pub fn pane_rects(tree: &LayoutNode, dims: (u16, u16)) -> HashMap<TerminalId, Rect> {
    let mut out = HashMap::new();
    fill_rects(
        tree,
        Rect {
            x: 0,
            y: 0,
            w: dims.0,
            h: dims.1,
        },
        &mut out,
    );
    out
}

fn fill_rects(node: &LayoutNode, bounds: Rect, out: &mut HashMap<TerminalId, Rect>) {
    match node {
        LayoutNode::Leaf(p) => {
            out.insert(p.clone(), bounds);
        }
        LayoutNode::Split {
            dir,
            ratio,
            left,
            right,
        } => match dir {
            SplitDir::Horizontal => {
                let left_w = split_dim(bounds.w, *ratio);
                let right_w = bounds.w - left_w;
                fill_rects(
                    left,
                    Rect {
                        x: bounds.x,
                        y: bounds.y,
                        w: left_w,
                        h: bounds.h,
                    },
                    out,
                );
                fill_rects(
                    right,
                    Rect {
                        x: bounds.x + left_w,
                        y: bounds.y,
                        w: right_w,
                        h: bounds.h,
                    },
                    out,
                );
            }
            SplitDir::Vertical => {
                let top_h = split_dim(bounds.h, *ratio);
                let bot_h = bounds.h - top_h;
                fill_rects(
                    left,
                    Rect {
                        x: bounds.x,
                        y: bounds.y,
                        w: bounds.w,
                        h: top_h,
                    },
                    out,
                );
                fill_rects(
                    right,
                    Rect {
                        x: bounds.x,
                        y: bounds.y + top_h,
                        w: bounds.w,
                        h: bot_h,
                    },
                    out,
                );
            }
            _ => unknown_split_dir(),
        },
        _ => unknown_layout_variant(),
    }
}

// -----------------------------------------------------------------------------
// Internal helpers — same shapes as phux-core::window
// -----------------------------------------------------------------------------

fn contains(node: &LayoutNode, target: &TerminalId) -> bool {
    match node {
        LayoutNode::Leaf(p) => p == target,
        LayoutNode::Split { left, right, .. } => contains(left, target) || contains(right, target),
        _ => unknown_layout_variant(),
    }
}

/// Collect every leaf of `node` in left-to-right depth-first order.
///
/// Useful for the "default focus on attach" rule from ADR-0019
/// decision 6 (focus defaults to the first leaf in left-to-right
/// traversal order) and for invariant proptests.
#[must_use]
pub fn leaves(node: &LayoutNode) -> Vec<TerminalId> {
    let mut out = Vec::new();
    collect_leaves(node, &mut out);
    out
}

fn collect_leaves(node: &LayoutNode, out: &mut Vec<TerminalId>) {
    match node {
        LayoutNode::Leaf(p) => out.push(p.clone()),
        LayoutNode::Split { left, right, .. } => {
            collect_leaves(left, out);
            collect_leaves(right, out);
        }
        _ => unknown_layout_variant(),
    }
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]
fn split_dim(total: u16, ratio: f32) -> u16 {
    let raw = (f32::from(total) * ratio).round();
    if raw < 0.0 {
        0
    } else if raw > f32::from(total) {
        total
    } else {
        raw as u16
    }
}

fn validate_ratio(ratio: f32) -> Result<(), LayoutError> {
    if ratio.is_nan() || ratio <= 0.0 || ratio >= 1.0 {
        Err(LayoutError::InvalidRatio(ratio))
    } else {
        Ok(())
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ChildSide {
    Left,
    Right,
}

fn record_path(
    node: &LayoutNode,
    target: &TerminalId,
    out: &mut Vec<(SplitDir, ChildSide)>,
) -> bool {
    match node {
        LayoutNode::Leaf(p) => p == target,
        LayoutNode::Split {
            dir: sd,
            left,
            right,
            ..
        } => {
            out.push((*sd, ChildSide::Left));
            if record_path(left, target, out) {
                return true;
            }
            out.pop();
            out.push((*sd, ChildSide::Right));
            if record_path(right, target, out) {
                return true;
            }
            out.pop();
            false
        }
        _ => unknown_layout_variant(),
    }
}

const fn matches_to_sibling(split: SplitDir, dir: Direction, came_from: ChildSide) -> bool {
    matches!(
        (split, dir, came_from),
        (SplitDir::Horizontal, Direction::Right, ChildSide::Left)
            | (SplitDir::Horizontal, Direction::Left, ChildSide::Right)
            | (SplitDir::Vertical, Direction::Down, ChildSide::Left)
            | (SplitDir::Vertical, Direction::Up, ChildSide::Right)
    )
}

fn sibling_at_depth<'a>(
    root: &'a LayoutNode,
    path: &[(SplitDir, ChildSide)],
    depth: usize,
) -> Option<&'a LayoutNode> {
    let mut cur = root;
    for (_, side) in &path[..depth] {
        let LayoutNode::Split { left, right, .. } = cur else {
            return None;
        };
        cur = match side {
            ChildSide::Left => left,
            ChildSide::Right => right,
        };
    }
    let LayoutNode::Split { left, right, .. } = cur else {
        return None;
    };
    let (_, came_from) = path[depth];
    Some(match came_from {
        ChildSide::Left => right,
        ChildSide::Right => left,
    })
}

fn descend_to_leaf(
    node: &LayoutNode,
    dir: Direction,
    suffix: &[(SplitDir, ChildSide)],
) -> TerminalId {
    let perp = perpendicular_axis(dir);
    let hints: Vec<ChildSide> = suffix
        .iter()
        .filter_map(|(sd, side)| if *sd == perp { Some(*side) } else { None })
        .collect();
    let mut hint_idx = 0;
    let mut cur = node;
    loop {
        match cur {
            LayoutNode::Leaf(p) => return p.clone(),
            LayoutNode::Split {
                dir: sd,
                left,
                right,
                ..
            } => {
                if axis_parallel(*sd, dir) {
                    cur = match dir {
                        Direction::Right | Direction::Down => left,
                        Direction::Left | Direction::Up => right,
                    };
                } else {
                    let side = hints.get(hint_idx).copied().unwrap_or(ChildSide::Left);
                    hint_idx += 1;
                    cur = match side {
                        ChildSide::Left => left,
                        ChildSide::Right => right,
                    };
                }
            }
            _ => unknown_layout_variant(),
        }
    }
}

const fn axis_parallel(split: SplitDir, dir: Direction) -> bool {
    matches!(
        (split, dir),
        (SplitDir::Horizontal, Direction::Left | Direction::Right)
            | (SplitDir::Vertical, Direction::Up | Direction::Down)
    )
}

const fn perpendicular_axis(dir: Direction) -> SplitDir {
    match dir {
        Direction::Left | Direction::Right => SplitDir::Vertical,
        Direction::Up | Direction::Down => SplitDir::Horizontal,
    }
}

// -----------------------------------------------------------------------------
// CBOR envelope — local shim types
// -----------------------------------------------------------------------------
//
// The wire-side `LayoutNode`, `SplitDir`, and `TerminalId` don't derive
// `serde::Serialize`/`Deserialize` and we can't modify the wire crate
// from this ticket (sibling-agent rule). The CBOR envelope therefore
// round-trips through small local types that mirror the wire shapes 1:1.
// Conversions are pure (no allocation beyond the recursive tree clone)
// and unit-tested below.

#[derive(Debug, Serialize, Deserialize)]
struct CborEnvelope {
    version: u8,
    root: CborLayoutNode,
    focus: CborTerminalId,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum CborLayoutNode {
    Leaf {
        pane: CborTerminalId,
    },
    Split {
        dir: CborSplitDir,
        ratio: f32,
        left: Box<CborLayoutNode>,
        right: Box<CborLayoutNode>,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum CborSplitDir {
    Horizontal,
    Vertical,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum CborTerminalId {
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
    fn into_layout_node(self) -> Result<LayoutNode, LayoutDecodeError> {
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

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::float_cmp)]
mod tests {
    use std::collections::HashSet;

    use proptest::prelude::*;

    use super::*;

    fn t(id: u32) -> TerminalId {
        TerminalId::local(id)
    }

    fn leaf(id: u32) -> LayoutNode {
        LayoutNode::Leaf(t(id))
    }

    // -------------------------------------------------------------------------
    // split_at
    // -------------------------------------------------------------------------

    #[test]
    fn split_at_replaces_leaf_with_split() {
        let tree = leaf(1);
        let out = split_at(&tree, &t(1), &t(2), SplitDir::Horizontal, 0.5).unwrap();
        let LayoutNode::Split {
            dir,
            ratio,
            left,
            right,
        } = out
        else {
            panic!("expected Split");
        };
        assert_eq!(dir, SplitDir::Horizontal);
        assert_eq!(ratio, 0.5);
        assert!(matches!(*left, LayoutNode::Leaf(ref p) if *p == t(1)));
        assert!(matches!(*right, LayoutNode::Leaf(ref p) if *p == t(2)));
    }

    #[test]
    fn split_at_rejects_missing_target() {
        let tree = leaf(1);
        let err = split_at(&tree, &t(99), &t(2), SplitDir::Horizontal, 0.5).unwrap_err();
        assert!(matches!(err, LayoutError::PaneNotInLayout(_)));
    }

    #[test]
    fn split_at_rejects_bad_ratio() {
        let tree = leaf(1);
        for bad in [0.0_f32, 1.0, -0.1, 1.1, f32::NAN] {
            let err = split_at(&tree, &t(1), &t(2), SplitDir::Horizontal, bad).unwrap_err();
            assert!(matches!(err, LayoutError::InvalidRatio(_)));
        }
    }

    #[test]
    fn split_at_deep() {
        // Build (1|2) then split 2 vertically with 3.
        let t1 = split_at(&leaf(1), &t(1), &t(2), SplitDir::Horizontal, 0.5).unwrap();
        let t2 = split_at(&t1, &t(2), &t(3), SplitDir::Vertical, 0.3).unwrap();
        let leaves_v = leaves(&t2);
        assert_eq!(leaves_v, vec![t(1), t(2), t(3)]);
    }

    // -------------------------------------------------------------------------
    // kill_pane
    // -------------------------------------------------------------------------

    #[test]
    fn kill_pane_last_leaf_returns_none() {
        let out = kill_pane(&leaf(1), &t(1)).unwrap();
        assert!(out.is_none());
    }

    #[test]
    fn kill_pane_missing_returns_err() {
        let err = kill_pane(&leaf(1), &t(99)).unwrap_err();
        assert!(matches!(err, LayoutError::PaneNotInLayout(_)));
    }

    #[test]
    fn kill_pane_collapses_split() {
        let tree = split_at(&leaf(1), &t(1), &t(2), SplitDir::Horizontal, 0.5).unwrap();
        let out = kill_pane(&tree, &t(2)).unwrap().expect("non-empty");
        assert!(matches!(out, LayoutNode::Leaf(ref p) if *p == t(1)));
    }

    #[test]
    fn kill_pane_collapses_deep() {
        // ((1|2)|3) — kill 1 should leave (2|3) at the root.
        let t1 = split_at(&leaf(1), &t(1), &t(2), SplitDir::Horizontal, 0.5).unwrap();
        let t2 = split_at(&t1, &t(2), &t(3), SplitDir::Vertical, 0.5).unwrap();
        let out = kill_pane(&t2, &t(1)).unwrap().expect("non-empty");
        // After killing 1 from ((1|(2/3))), the left subtree collapses to
        // (2/3). Tree shape: Split[h, (2/3), ?]... wait — let's just
        // check leaves.
        let mut got: Vec<_> = leaves(&out);
        got.sort_by_key(|id| id.local_id().unwrap_or_default());
        assert_eq!(got, vec![t(2), t(3)]);
    }

    // -------------------------------------------------------------------------
    // pane_rects
    // -------------------------------------------------------------------------

    #[test]
    fn pane_rects_single_leaf() {
        let rects = pane_rects(&leaf(1), (80, 24));
        let r = rects.get(&t(1)).unwrap();
        assert_eq!(
            *r,
            Rect {
                x: 0,
                y: 0,
                w: 80,
                h: 24
            }
        );
    }

    #[test]
    fn pane_rects_horizontal_split_tiles() {
        let tree = split_at(&leaf(1), &t(1), &t(2), SplitDir::Horizontal, 0.5).unwrap();
        let rects = pane_rects(&tree, (80, 24));
        let r1 = rects.get(&t(1)).unwrap();
        let r2 = rects.get(&t(2)).unwrap();
        assert_eq!(r1.w + r2.w, 80);
        assert_eq!(r1.h, 24);
        assert_eq!(r2.h, 24);
        assert_eq!(r1.x, 0);
        assert_eq!(r2.x, r1.w);
    }

    // -------------------------------------------------------------------------
    // focus_direction
    // -------------------------------------------------------------------------

    #[test]
    fn focus_direction_right_across_split() {
        let tree = split_at(&leaf(1), &t(1), &t(2), SplitDir::Horizontal, 0.5).unwrap();
        assert_eq!(focus_direction(&tree, &t(1), Direction::Right), Some(t(2)));
        assert_eq!(focus_direction(&tree, &t(2), Direction::Left), Some(t(1)));
        assert_eq!(focus_direction(&tree, &t(1), Direction::Up), None);
    }

    #[test]
    fn focus_direction_returns_none_for_missing() {
        let tree = leaf(1);
        assert_eq!(focus_direction(&tree, &t(99), Direction::Right), None);
    }

    // -------------------------------------------------------------------------
    // CBOR round-trip
    // -------------------------------------------------------------------------

    #[test]
    fn cbor_round_trip_single_pane() {
        let state = LayoutState::single(t(7));
        let bytes = state.encode_cbor().unwrap();
        let decoded = LayoutState::decode_cbor(&bytes).unwrap();
        assert_eq!(decoded, state);
    }

    #[test]
    fn cbor_round_trip_multi_split() {
        // ((1 | 2) / 3)
        let t1 = split_at(&leaf(1), &t(1), &t(2), SplitDir::Horizontal, 0.4).unwrap();
        let t2 = split_at(&t1, &t(2), &t(3), SplitDir::Vertical, 0.6).unwrap();
        let state = LayoutState {
            tree: Some(t2),
            focus: Some(t(2)),
        };
        let bytes = state.encode_cbor().unwrap();
        let decoded = LayoutState::decode_cbor(&bytes).unwrap();
        assert_eq!(decoded, state);
    }

    #[test]
    fn cbor_round_trip_satellite_focus() {
        let focus = TerminalId::satellite("peer.example", 42);
        let state = LayoutState {
            tree: Some(LayoutNode::Leaf(focus.clone())),
            focus: Some(focus),
        };
        let bytes = state.encode_cbor().unwrap();
        let decoded = LayoutState::decode_cbor(&bytes).unwrap();
        assert_eq!(decoded, state);
    }

    #[test]
    fn cbor_rejects_empty_state() {
        let state = LayoutState::default();
        let err = state.encode_cbor().unwrap_err();
        assert!(matches!(err, LayoutEncodeError::Empty));
    }

    #[test]
    fn cbor_rejects_unsupported_version() {
        // Hand-build an envelope with version=99.
        #[derive(Serialize)]
        struct Forged {
            version: u8,
            root: CborLayoutNode,
            focus: CborTerminalId,
        }
        let forged = Forged {
            version: 99,
            root: CborLayoutNode::Leaf {
                pane: CborTerminalId::Local { id: 1 },
            },
            focus: CborTerminalId::Local { id: 1 },
        };
        let mut buf = Vec::new();
        ciborium::ser::into_writer(&forged, &mut buf).unwrap();
        let err = LayoutState::decode_cbor(&buf).unwrap_err();
        assert!(matches!(err, LayoutDecodeError::UnsupportedVersion(99)));
    }

    #[test]
    fn cbor_rejects_malformed_ratio() {
        #[derive(Serialize)]
        struct Forged {
            version: u8,
            root: CborLayoutNode,
            focus: CborTerminalId,
        }
        let forged = Forged {
            version: 1,
            root: CborLayoutNode::Split {
                dir: CborSplitDir::Horizontal,
                ratio: 2.0, // out of range
                left: Box::new(CborLayoutNode::Leaf {
                    pane: CborTerminalId::Local { id: 1 },
                }),
                right: Box::new(CborLayoutNode::Leaf {
                    pane: CborTerminalId::Local { id: 2 },
                }),
            },
            focus: CborTerminalId::Local { id: 1 },
        };
        let mut buf = Vec::new();
        ciborium::ser::into_writer(&forged, &mut buf).unwrap();
        let err = LayoutState::decode_cbor(&buf).unwrap_err();
        assert!(matches!(err, LayoutDecodeError::MalformedRatio(_)));
    }

    // -------------------------------------------------------------------------
    // Proptest invariants — ported from phux-core::window proptests.
    // -------------------------------------------------------------------------

    #[derive(Debug, Clone, Copy)]
    enum Op {
        AddPane,
        KillPaneAt(usize),
    }

    fn arb_op() -> impl Strategy<Value = Op> {
        prop_oneof![
            4 => Just(Op::AddPane),
            1 => (0_usize..16).prop_map(Op::KillPaneAt),
        ]
    }

    /// Apply `ops` against a fresh single-leaf tree, returning the final
    /// tree (or `None` if killed empty) plus the ordered list of leaves
    /// that should currently live in the tree.
    #[allow(clippy::needless_pass_by_value)]
    fn apply_ops(ops: Vec<Op>) -> (Option<LayoutNode>, Vec<TerminalId>) {
        let mut next_id: u32 = 1;
        let first = TerminalId::local(next_id);
        next_id += 1;
        let mut tree: Option<LayoutNode> = Some(LayoutNode::Leaf(first.clone()));
        let mut alive: Vec<TerminalId> = vec![first];

        for op in ops {
            match op {
                Op::AddPane => {
                    let new_pane = TerminalId::local(next_id);
                    next_id += 1;
                    let Some(target) = alive.last().cloned() else {
                        // Tree was empty — reseed.
                        tree = Some(LayoutNode::Leaf(new_pane.clone()));
                        alive.push(new_pane);
                        continue;
                    };
                    let Some(cur) = tree else {
                        tree = Some(LayoutNode::Leaf(new_pane.clone()));
                        alive.push(new_pane);
                        continue;
                    };
                    let dir = if next_id.is_multiple_of(2) {
                        SplitDir::Horizontal
                    } else {
                        SplitDir::Vertical
                    };
                    match split_at(&cur, &target, &new_pane, dir, 0.5) {
                        Ok(t) => {
                            tree = Some(t);
                            alive.push(new_pane);
                        }
                        Err(_) => {
                            // Restore tree and skip.
                            tree = Some(cur);
                        }
                    }
                }
                Op::KillPaneAt(idx) => {
                    if alive.is_empty() {
                        continue;
                    }
                    let target = alive[idx % alive.len()].clone();
                    let Some(cur) = tree else { continue };
                    match kill_pane(&cur, &target) {
                        Ok(new_tree) => {
                            tree = new_tree;
                            alive.retain(|p| *p != target);
                        }
                        Err(_) => {
                            tree = Some(cur);
                        }
                    }
                }
            }
        }
        (tree, alive)
    }

    proptest! {
        #![proptest_config(ProptestConfig { cases: 64, ..ProptestConfig::default() })]

        /// Invariant 1: every TerminalId appears as exactly one leaf.
        #[test]
        fn proptest_leaves_match_alive(ops in prop::collection::vec(arb_op(), 1..20)) {
            let (tree, alive) = apply_ops(ops);
            let tree_leaves = tree.as_ref().map_or_else(Vec::new, leaves);
            let alive_set: HashSet<_> = alive.into_iter().collect();
            let leaf_set: HashSet<_> = tree_leaves.iter().cloned().collect();
            // Set equality.
            prop_assert_eq!(&alive_set, &leaf_set);
            // Exactly one leaf per id (no duplicates).
            prop_assert_eq!(tree_leaves.len(), leaf_set.len());
        }

        /// Invariant 2: `pane_rects` tiles the bounding rectangle exactly.
        #[test]
        fn proptest_pane_rects_tile(ops in prop::collection::vec(arb_op(), 1..20)) {
            let (tree, _) = apply_ops(ops);
            let Some(tree) = tree else { return Ok(()) };
            let rects = pane_rects(&tree, (80, 24));
            let total: u32 = rects.values()
                .map(|r| u32::from(r.w) * u32::from(r.h))
                .sum();
            prop_assert_eq!(total, 80 * 24);

            let mut covered: HashSet<(u16, u16)> = HashSet::new();
            for r in rects.values() {
                for y in r.y..r.y.saturating_add(r.h) {
                    for x in r.x..r.x.saturating_add(r.w) {
                        prop_assert!(covered.insert((x, y)));
                    }
                }
            }
            prop_assert_eq!(covered.len(), 80 * 24);
        }

        /// Invariant 3: `focus_direction` is partial, deterministic, and
        /// only returns ids that are leaves of the tree.
        #[test]
        fn proptest_focus_direction_partial_deterministic(
            ops in prop::collection::vec(arb_op(), 1..20),
            dir_pick in 0_u8..4,
        ) {
            let (tree, alive) = apply_ops(ops);
            let Some(tree) = tree else { return Ok(()) };
            if alive.is_empty() { return Ok(()) }
            let leaf_set: HashSet<_> = leaves(&tree).into_iter().collect();
            let dir = match dir_pick {
                0 => Direction::Up,
                1 => Direction::Down,
                2 => Direction::Left,
                _ => Direction::Right,
            };
            for src in &alive {
                let a = focus_direction(&tree, src, dir);
                let b = focus_direction(&tree, src, dir);
                // Deterministic.
                prop_assert_eq!(&a, &b);
                if let Some(neighbour) = a {
                    // Neighbour is a leaf of the tree.
                    prop_assert!(leaf_set.contains(&neighbour));
                    // Different from source.
                    prop_assert_ne!(&neighbour, src);
                }
            }
        }

        /// Invariant 4: CBOR round-trips for any state derived from
        /// random ops (with focus on the last surviving leaf).
        #[test]
        fn proptest_cbor_round_trip(ops in prop::collection::vec(arb_op(), 1..15)) {
            let (tree, alive) = apply_ops(ops);
            let Some(tree) = tree else { return Ok(()) };
            let Some(focus) = alive.last().cloned() else { return Ok(()) };
            let state = LayoutState { tree: Some(tree), focus: Some(focus) };
            let bytes = state.encode_cbor().expect("encode");
            let decoded = LayoutState::decode_cbor(&bytes).expect("decode");
            prop_assert_eq!(decoded, state);
        }
    }
}
