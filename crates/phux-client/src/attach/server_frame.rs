//! Server-to-client frame handling: dispatches `FrameKind` variants to
//! the right state mutations and rendering.
//!
//! Returns a `FrameOutcome` describing the follow-up the async driver
//! should take (e.g. exit on `DETACHED`, send `GET_METADATA` after
//! `ATTACHED`, repaint after a layout-replacing frame).

use std::collections::HashMap;
use std::io::{self, Write};

use phux_protocol::ids::TerminalId;
use phux_protocol::wire::frame::{FrameKind, Scope, SpawnError, SpawnResult};

use super::actions::{self, PendingSplit, apply_spawned_ok, apply_terminal_closed};
use super::driver::{AttachError, DEFAULT_COLLECTION_ID, LAYOUT_KEY, PaneSlot};
use super::paint::{paint_bar_after_pane, paint_focused_pane, pane_viewport};
use super::status_bar::StatusBarPainter;
use crate::layout::{self, LayoutState};
use crate::predict::{Overlay, PredictionState, reconcile_terminal_output_per_cell};

/// Outcome of processing a single server-to-client frame.
///
/// The driver translates these into async actions (send a frame, exit
/// the loop, repaint). Keeping the side-effect-free decision inside
/// [`handle_server_frame`] lets the function stay synchronous.
#[allow(
    clippy::struct_excessive_bools,
    reason = "four parallel server-frame outcome flags; refactor into bitset would obscure callers"
)]
#[derive(Debug, Clone, Default)]
pub(super) struct FrameOutcome {
    /// `true` ⇒ the loop should exit cleanly (server sent `DETACHED`).
    pub(super) exit: bool,
    /// `true` ⇒ ATTACHED just landed; the driver should emit
    /// `GET_METADATA` + `SUBSCRIBE_METADATA` for the layout key so
    /// other clients' mutations broadcast back to us (ADR-0019).
    pub(super) subscribe_layout: bool,
    /// `true` ⇒ `layout_state` was replaced by a server-side layout
    /// envelope (`MetadataValue` reply or `MetadataChanged` broadcast).
    /// The driver triggers a full repaint of the multi-pane composition.
    pub(super) layout_replaced: bool,
    /// phux-4li.12: `true` ⇒ the server-side frame mutated layout in
    /// a way the *local* client originated (split landed, kill folded);
    /// the driver should broadcast the new envelope via
    /// `SET_METADATA` so sibling clients reconcile.
    pub(super) emit_set_metadata: bool,
}

/// Process one server-to-client frame. Returns a [`FrameOutcome`]
/// describing any follow-up the async driver needs to perform.
///
/// `status_bar` is `Option<&mut StatusBarPainter>` so an attach with no
/// configured widgets pays nothing for the chrome path. `viewport_dims`
/// is `(cols, rows)` of the outer terminal — used by the painter to
/// pick the bottom row.
#[allow(clippy::too_many_arguments)] // arg list bundles status-bar + predict state; follow-up to refactor into a context struct
#[allow(
    clippy::too_many_lines,
    reason = "phux-4li.5 added L3 reconcile branches; refactor with the status-bar arg-list cleanup"
)]
#[allow(
    clippy::cognitive_complexity,
    reason = "phux-4li.12 adds TerminalSpawned/TerminalClosed branches with full SpawnError matching; per-frame dispatcher is intentionally flat"
)]
pub(super) fn handle_server_frame(
    frame: FrameKind,
    panes: &mut HashMap<TerminalId, PaneSlot>,
    layout_state: &mut LayoutState,
    focused_pane: &mut Option<TerminalId>,
    session_name: &mut String,
    status_bar: Option<&mut StatusBarPainter>,
    viewport_dims: (u16, u16),
    predict: &mut PredictionState,
    overlay: &Overlay,
    pending_layout_request: Option<u32>,
    pending_splits: &mut HashMap<u32, PendingSplit>,
) -> Result<FrameOutcome, AttachError> {
    match frame {
        FrameKind::Attached {
            snapshot,
            initial_client_id: _,
        } => {
            // Capture the initial focused pane so subsequent INPUT_* frames
            // know where to route.
            let bootstrap = snapshot.focused_pane;
            *focused_pane = Some(bootstrap.clone());
            // phux-4li.4: seed the layout mirror with a single leaf so
            // the existing single-pane render path keeps working. The
            // L3 metadata-fetch path (.2/.3) replaces this with the
            // server-stored tree when present; until that ticket lands
            // every attach is single-pane.
            *layout_state = LayoutState::single(bootstrap.clone());
            // Ensure the focused pane has a slot ready for output
            // frames; output may race ahead of the snapshot. If
            // libghostty refuses to allocate a Terminal we surface
            // the failure rather than silently dropping the bootstrap.
            if let std::collections::hash_map::Entry::Vacant(v) = panes.entry(bootstrap) {
                v.insert(PaneSlot::new()?);
            }
            // phux-nz4.5: stash the session name for the status-bar
            // `WidgetContext`. The Snapshot type names the session via
            // its window graph; for v0 the session-name widget reads
            // from a string slot we maintain here, defaulting to the
            // empty string until a session-graph carrier lands.
            *session_name = String::new();
            // `ATTACHED` per SPEC §13 carries the session/window/pane
            // graph; the per-pane initial cells arrive separately via
            // TERMINAL_SNAPSHOT.
            //
            // phux-4li.5: signal the driver to emit GET_METADATA and
            // SUBSCRIBE_METADATA for the layout key so we (a) reconcile
            // against a persisted layout from a previous session and
            // (b) receive METADATA_CHANGED broadcasts from sibling
            // clients (ADR-0019 decision 2).
            Ok(FrameOutcome {
                subscribe_layout: true,
                ..FrameOutcome::default()
            })
        }
        FrameKind::TerminalSnapshot {
            terminal_id,
            cols,
            rows,
            vt_replay_bytes,
            scrollback_bytes,
        } => {
            // phux-4li.4: route per-pane snapshots into per-pane slots.
            // Allocate a fresh slot on first sight so output frames for
            // pre-split panes don't drop on the floor.
            let is_focused = Some(&terminal_id) == focused_pane.as_ref();
            // Resolve the pane's outer-viewport Rect BEFORE the
            // `panes.entry(terminal_id)` move. Multi-pane: ask the
            // layout. Single-pane / no layout: anchor at (0,0).
            let has_bar = status_bar.is_some();
            let pane_dims = pane_viewport(viewport_dims, has_bar);
            let origin = if layout_state.tree.is_some() {
                super::multi_pane::compute_layout(layout_state, pane_dims)
                    .rects
                    .get(&terminal_id)
                    .map_or((0, 0), |r| (r.x, r.y))
            } else {
                (0, 0)
            };
            let slot = match panes.entry(terminal_id) {
                std::collections::hash_map::Entry::Occupied(o) => o.into_mut(),
                std::collections::hash_map::Entry::Vacant(v) => v.insert(PaneSlot::new()?),
            };
            slot.terminal.resize(cols, rows, 0, 0)?;
            // Apply scrollback first (if any), then the visible-state
            // replay — order per SPEC §8.4 / §13.
            if let Some(sb) = scrollback_bytes {
                slot.terminal.vt_write(&sb);
            }
            slot.terminal.vt_write(&vt_replay_bytes);
            if is_focused {
                // A fresh snapshot replaces the world — drop any
                // outstanding predictions and resize the predict layer.
                predict.set_viewport(cols, rows);
                let mut stdout = io::stdout().lock();
                let _ = slot.renderer.render_at(&slot.terminal, &mut stdout, origin);
                if let Some((row, col)) = slot.renderer.last_cursor() {
                    predict.set_cursor(row, col);
                }
                // Snapshot is authoritative — overlay only repaints if
                // new keystrokes arrived after the snapshot was issued
                // and before reconcile cleared the queue. In v0 we
                // simply leave the queue empty.
                let _ = overlay;
                // phux-nz4.5: the pane renderer just wrote to the
                // bottom row of its own grid; force a status-bar
                // repaint over it.
                let focused_cursor = slot.renderer.last_cursor();
                paint_bar_after_pane(
                    status_bar,
                    &mut stdout,
                    viewport_dims,
                    session_name,
                    focused_cursor,
                );
            }
            Ok(FrameOutcome::default())
        }
        FrameKind::TerminalOutput {
            terminal_id,
            seq: _,
            bytes,
        } => {
            // phux-4li.4: ingest output into the matching pane's
            // libghostty Terminal even when it's not focused, so the
            // mirror stays warm for when the user focuses it. Render +
            // predict-reconcile only fire for the focused pane.
            let is_focused = Some(&terminal_id) == focused_pane.as_ref();
            // Drop bytes into the slot's libghostty Terminal. Scoped so
            // the mut borrow on `panes` releases before the focused-pane
            // render path re-borrows it via `paint_focused_pane`.
            {
                let slot = match panes.entry(terminal_id) {
                    std::collections::hash_map::Entry::Occupied(o) => o.into_mut(),
                    std::collections::hash_map::Entry::Vacant(v) => v.insert(PaneSlot::new()?),
                };
                slot.terminal.vt_write(&bytes);
            }
            if is_focused && let Some(fid) = focused_pane.as_ref() {
                let mut stdout = io::stdout().lock();
                let has_bar = status_bar.is_some();
                let focused_cursor = paint_focused_pane(
                    &mut stdout,
                    layout_state,
                    panes,
                    fid,
                    viewport_dims,
                    has_bar,
                );
                // Per-cell match reconcile (phux-9gw.1.1): walk pending
                // predictions against the freshly painted cell grid;
                // confirmed predictions drop, contradictions drop their
                // suffix, predictions still ahead of confirmed state
                // stay alive. See [`crate::predict`] for the truth table.
                if let Some((row, col)) = focused_cursor {
                    let _stats = reconcile_terminal_output_per_cell(predict, row, col, |r, c| {
                        panes.get_mut(fid).and_then(|s| {
                            s.renderer
                                .read_grapheme_at(&s.terminal, r, c)
                                .ok()
                                .flatten()
                        })
                    });
                } else {
                    // Cursor hidden — we can't anchor reliably; fall
                    // back to the wholesale drain. Rare path (programs
                    // that hide the cursor before a redraw).
                    predict.clear();
                }
                // Overlay paints any predictions still alive (the tail
                // of a partial confirmation). On a fully-drained queue
                // this is a no-op.
                let _ = overlay.render(predict, &mut stdout);
                paint_bar_after_pane(
                    status_bar,
                    &mut stdout,
                    viewport_dims,
                    session_name,
                    focused_cursor,
                );
            }
            Ok(FrameOutcome::default())
        }
        FrameKind::Detached => Ok(FrameOutcome {
            exit: true,
            ..FrameOutcome::default()
        }),
        FrameKind::Bell { .. } => {
            // Forward bell to the outer terminal. The user's terminal
            // emulator decides whether to render visually, audibly, or
            // not at all.
            let mut stdout = io::stdout().lock();
            let _ = stdout.write_all(b"\x07");
            let _ = stdout.flush();
            Ok(FrameOutcome::default())
        }
        // phux-4li.5: reconcile-on-attach reply path. The driver sends
        // `GET_METADATA { request_id }` immediately after ATTACHED;
        // the server replies with `MetadataValue { request_id, value }`.
        // Match by id, decode the v1 CBOR envelope, and replace
        // `layout_state` in place. `value: None` means "no persisted
        // layout" — keep the single-pane bootstrap untouched.
        FrameKind::MetadataValue { request_id, value } => {
            if Some(request_id) != pending_layout_request {
                tracing::debug!(
                    request_id,
                    "dropping MetadataValue with no matching pending request"
                );
                return Ok(FrameOutcome::default());
            }
            let Some(bytes) = value else {
                return Ok(FrameOutcome::default());
            };
            match LayoutState::decode_cbor(&bytes) {
                Ok(new_state) => {
                    *layout_state =
                        reconcile_loaded_layout(new_state, focused_pane.as_ref(), panes);
                    Ok(FrameOutcome {
                        layout_replaced: true,
                        ..FrameOutcome::default()
                    })
                }
                Err(err) => {
                    tracing::warn!(error = %err, "failed to decode persisted layout; keeping bootstrap");
                    Ok(FrameOutcome::default())
                }
            }
        }
        // phux-4li.5: broadcast reconcile. Another attached client
        // mutated `phux.tui.layout/v1`; decode + replace + repaint.
        // Tombstones (`value: None`) are treated as "layout reset" —
        // fall back to the single-pane bootstrap so the next render
        // doesn't try to draw against a stale tree.
        FrameKind::MetadataChanged { scope, key, value } => {
            if !is_layout_key(&scope, &key) {
                return Ok(FrameOutcome::default());
            }
            if let Some(bytes) = value {
                match LayoutState::decode_cbor(&bytes) {
                    Ok(new_state) => {
                        *layout_state =
                            reconcile_loaded_layout(new_state, focused_pane.as_ref(), panes);
                        Ok(FrameOutcome {
                            layout_replaced: true,
                            ..FrameOutcome::default()
                        })
                    }
                    Err(err) => {
                        tracing::warn!(error = %err, "broadcast layout decode failed; ignoring");
                        Ok(FrameOutcome::default())
                    }
                }
            } else {
                // Tombstone: layout reset. Fall back to single-pane
                // bootstrap (or empty if there's no focus to anchor on).
                *layout_state = focused_pane
                    .clone()
                    .map_or_else(LayoutState::default, LayoutState::single);
                Ok(FrameOutcome {
                    layout_replaced: true,
                    ..FrameOutcome::default()
                })
            }
        }
        // phux-4li.12: split-pane reply path. Look up the parked
        // PendingSplit by request id; on Ok apply the split + seed the
        // new PaneSlot + broadcast the envelope. On Err log + bell.
        FrameKind::TerminalSpawned { request_id, result } => {
            let Some(pending) = pending_splits.remove(&request_id) else {
                tracing::debug!(
                    request_id,
                    "stray TerminalSpawned with no matching pending split; ignoring",
                );
                return Ok(FrameOutcome::default());
            };
            match result {
                SpawnResult::Ok(new_id) => {
                    match apply_spawned_ok(layout_state, new_id.clone(), &pending) {
                        Ok(new_state) => {
                            *layout_state = new_state;
                            // Seed a PaneSlot for the new Terminal so the
                            // first TERMINAL_SNAPSHOT lands on a warm
                            // mirror. Vacant-or-occupied — never overwrite
                            // an existing slot (a TERMINAL_OUTPUT could
                            // legally race ahead of TERMINAL_SPAWNED if
                            // the server batched the spawn-then-output).
                            if let std::collections::hash_map::Entry::Vacant(v) =
                                panes.entry(new_id)
                            {
                                v.insert(PaneSlot::new()?);
                            }
                            // Move focus to the freshly spawned pane —
                            // tmux-compatible (apply_split already sets
                            // focus inside the returned state).
                            focused_pane.clone_from(&layout_state.focus);
                            Ok(FrameOutcome {
                                layout_replaced: true,
                                emit_set_metadata: true,
                                ..FrameOutcome::default()
                            })
                        }
                        Err(err) => {
                            tracing::warn!(
                                error = %err,
                                terminal = ?new_id,
                                "apply_spawned_ok failed; dropping spawned terminal",
                            );
                            bell_to_stdout();
                            Ok(FrameOutcome::default())
                        }
                    }
                }
                SpawnResult::Err(SpawnError::CollectionNotFound) => {
                    // v0.1 clients only ever target DEFAULT_COLLECTION_ID,
                    // which the server always exposes; this branch
                    // means a server-side L2 invariant changed under
                    // us. Log loudly + bell.
                    tracing::warn!(
                        request_id,
                        "TerminalSpawned: server reports CollectionNotFound for DEFAULT collection",
                    );
                    bell_to_stdout();
                    Ok(FrameOutcome::default())
                }
                SpawnResult::Err(SpawnError::SpawnFailed(reason)) => {
                    tracing::warn!(
                        request_id,
                        reason = %reason,
                        "TerminalSpawned: server-side spawn failed",
                    );
                    bell_to_stdout();
                    Ok(FrameOutcome::default())
                }
                // SpawnError is #[non_exhaustive] — catch future
                // variants so newer servers don't take the client down.
                SpawnResult::Err(other) => {
                    tracing::warn!(
                        request_id,
                        error = ?other,
                        "TerminalSpawned: unknown spawn error variant",
                    );
                    bell_to_stdout();
                    Ok(FrameOutcome::default())
                }
                // SpawnResult is also #[non_exhaustive].
                _ => {
                    tracing::warn!(request_id, "TerminalSpawned: unknown SpawnResult variant");
                    Ok(FrameOutcome::default())
                }
            }
        }
        // phux-4li.12: a Terminal closed. Fold it out of the layout if
        // it's a known leaf, drop its PaneSlot regardless. If we
        // initiated the kill (or it died on us spontaneously), the
        // server still broadcasts this so every attached client folds
        // in lockstep.
        FrameKind::TerminalClosed {
            terminal_id,
            exit_status,
        } => {
            tracing::info!(
                terminal = ?terminal_id,
                exit_status = ?exit_status,
                "TerminalClosed",
            );
            let tree_leaves: Vec<TerminalId> = layout_state
                .tree
                .as_ref()
                .map(layout::leaves)
                .unwrap_or_default();
            let known_leaf = tree_leaves.contains(&terminal_id);
            // Always drop the slot — even for unknown leaves (could be
            // a spawn-failure cleanup race or a stale id from before
            // an attach).
            panes.remove(&terminal_id);
            if !known_leaf {
                return Ok(FrameOutcome::default());
            }
            match apply_terminal_closed(layout_state, &terminal_id) {
                Ok(new_state) => {
                    *layout_state = new_state;
                    // Re-anchor `focused_pane`. `apply_terminal_closed`
                    // (via `apply_kill`) sets the new focus to the
                    // first DFS leaf, or `None` if the tree is empty.
                    focused_pane.clone_from(&layout_state.focus);
                    Ok(FrameOutcome {
                        layout_replaced: true,
                        emit_set_metadata: true,
                        ..FrameOutcome::default()
                    })
                }
                Err(err) => {
                    // Closed terminal wasn't a leaf in the tree (race
                    // we already covered with `known_leaf`, or the
                    // layout was empty). Drop quietly — slot is gone.
                    tracing::debug!(
                        error = %err,
                        terminal = ?terminal_id,
                        "apply_terminal_closed: layout fold failed",
                    );
                    Ok(FrameOutcome::default())
                }
            }
        }
        other => {
            // Anything else — `HELLO_OK`, `PONG`, future spec frames — is
            // accepted-but-ignored. The protocol decoder rejects unknown
            // discriminants; this branch handles known-but-not-yet-wired
            // frames.
            tracing::debug!(kind = ?other, "ignoring server frame");
            Ok(FrameOutcome::default())
        }
    }
}

/// phux-4li.12: write a BEL to stdout. Used by `handle_server_frame`'s
/// error branches (spawn failed, layout fold rejected) where we need
/// to signal the user without surfacing structured error chrome.
fn bell_to_stdout() {
    let mut stdout = io::stdout().lock();
    let _ = actions::write_bell(&mut stdout);
}

/// Decide whether `(scope, key)` matches the layout-coordination key
/// ADR-0019 reserves (`phux.tui.layout/v1`, scoped to the default
/// Collection).
fn is_layout_key(scope: &Scope, key: &str) -> bool {
    matches!(scope, Scope::Collection(id) if *id == DEFAULT_COLLECTION_ID) && key == LAYOUT_KEY
}

/// Sanity-check a freshly decoded layout against the panes the driver
/// has slots for, and fall back to a safe focus if the persisted focus
/// no longer exists (e.g. the leaf was killed in a previous session).
///
/// We accept the persisted tree as-is — panes that don't yet have a
/// `PaneSlot` will get one lazily when their first `TERMINAL_OUTPUT`
/// arrives, so an arbitrary tree shape is fine. Focus is the one
/// invariant we can't recover from: if the persisted focused leaf
/// isn't a member of the tree the renderer would have no focused
/// pane to draw input chrome on.
fn reconcile_loaded_layout(
    mut state: LayoutState,
    bootstrap_focus: Option<&TerminalId>,
    _panes: &HashMap<TerminalId, PaneSlot>,
) -> LayoutState {
    let tree_leaves = state
        .tree
        .as_ref()
        .map(crate::layout::leaves)
        .unwrap_or_default();
    let focus_ok = state
        .focus
        .as_ref()
        .is_some_and(|f| tree_leaves.contains(f));
    if !focus_ok {
        // Prefer the bootstrap focus if it's actually in the tree;
        // otherwise pick the first leaf (ADR-0019 decision 6 default);
        // otherwise clear focus entirely.
        state.focus = bootstrap_focus
            .filter(|f| tree_leaves.contains(f))
            .cloned()
            .or_else(|| tree_leaves.into_iter().next());
    }
    state
}
