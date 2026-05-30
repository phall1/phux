//! Server-to-client frame handling: dispatches `FrameKind` variants to
//! the right state mutations and rendering.
//!
//! Returns a `FrameOutcome` describing the follow-up the async driver
//! should take (e.g. exit on `DETACHED`, send `GET_METADATA` after
//! `ATTACHED`, repaint after a layout-replacing frame).

use std::collections::HashMap;

use phux_protocol::ids::TerminalId;
use phux_protocol::wire::frame::{FrameKind, Scope, SpawnError, SpawnResult};

use super::actions::{self, PendingSplit, PendingWindow, apply_spawned_ok, apply_terminal_closed};
use super::driver::{AttachError, DEFAULT_COLLECTION_ID, LAYOUT_KEY, PaneSlot};
use super::paint::{paint_bar_after_pane, paint_focused_pane, pane_viewport};
use crate::layout::{self, LayoutState, Workspace};
use crate::predict::{Overlay, PredictionState, reconcile_terminal_output_per_cell};
use crate::render::chrome::status_bar::StatusBarPainter;

/// Outcome of processing a single server-to-client frame.
///
/// The driver translates these into async actions (send a frame, exit
/// the loop, repaint). Keeping the side-effect-free decision inside
/// [`handle_server_frame`] lets the function stay synchronous.
#[allow(
    clippy::struct_excessive_bools,
    reason = "parallel server-frame outcome flags; refactor into bitset would obscure callers"
)]
#[derive(Debug, Clone, Default)]
pub(super) struct FrameOutcome {
    /// `true` ⇒ the loop should exit cleanly (server sent `DETACHED`).
    pub(super) exit: bool,
    /// `true` ⇒ ATTACHED just landed; the driver should emit
    /// `GET_METADATA` + `SUBSCRIBE_METADATA` for the layout key so
    /// other clients' mutations broadcast back to us (ADR-0019).
    pub(super) subscribe_layout: bool,
    /// `true` ⇒ the workspace was replaced by a server-side layout
    /// envelope (`MetadataValue` reply or `MetadataChanged` broadcast).
    /// The driver triggers a full repaint of the multi-pane composition.
    pub(super) layout_replaced: bool,
    /// phux-4li.12: `true` ⇒ the server-side frame mutated layout in
    /// a way the *local* client originated (split landed, kill folded);
    /// the driver should broadcast the new envelope via
    /// `SET_METADATA` so sibling clients reconcile.
    pub(super) emit_set_metadata: bool,
    /// phux-tnh: `true` ⇒ a pane lifecycle event (close/spawn) changed
    /// surviving panes' dimensions. The driver must diff the new layout
    /// against the pre-frame rects and emit a `TERMINAL_RESIZE` per
    /// changed leaf so the server reflows each PTY (TIOCSWINSZ) — without
    /// this the survivor of a close keeps its old small winsize and the
    /// shell never redraws to fill the freed space. Set ONLY by the
    /// `TerminalClosed`/`TerminalSpawned` arms, not by the broader
    /// `layout_replaced` reconcile/broadcast paths (which already sized
    /// their panes and would otherwise thrash on attach).
    pub(super) reflow_panes: bool,
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
pub(super) fn handle_server_frame<W: super::RenderSink>(
    out: &mut W,
    frame: FrameKind,
    panes: &mut HashMap<TerminalId, PaneSlot>,
    workspace: &mut Workspace,
    focused_pane: &mut Option<TerminalId>,
    session_name: &mut String,
    status_bar: Option<&mut StatusBarPainter>,
    viewport_dims: (u16, u16),
    predict: &mut PredictionState,
    overlay: &Overlay,
    pending_layout_request: Option<u32>,
    pending_splits: &mut HashMap<u32, PendingSplit>,
    pending_windows: &mut HashMap<u32, PendingWindow>,
    // phux-5ke.4: when `true` an overlay is on top; pane libghostty
    // mirrors keep ingesting `vt_write` (per ADR-0013) but stdout
    // flushes (render_at, bar paint, predict-overlay paint) are
    // suppressed so the modal doesn't get scribbled over. The driver
    // triggers a full repaint on overlay dismiss.
    overlay_active: bool,
) -> Result<FrameOutcome, AttachError> {
    match frame {
        FrameKind::Attached {
            snapshot,
            initial_client_id: _,
        } => {
            // Capture the initial focused pane so subsequent INPUT_* frames
            // know where to route.
            let bootstrap = snapshot.focused_pane.clone();
            tracing::debug!(
                terminal_id = ?bootstrap,
                "ATTACHED: seeding focused_pane from snapshot"
            );
            *focused_pane = Some(bootstrap.clone());
            // phux-4li.4: seed the workspace with a single window holding
            // one leaf so the existing single-pane render path keeps
            // working. The L3 metadata-fetch path replaces this with the
            // server-stored layout (possibly multi-window) when present.
            *workspace = Workspace::single(bootstrap.clone());
            // Ensure the focused pane has a slot ready for output
            // frames; output may race ahead of the snapshot. If
            // libghostty refuses to allocate a Terminal we surface
            // the failure rather than silently dropping the bootstrap.
            if let std::collections::hash_map::Entry::Vacant(v) = panes.entry(bootstrap) {
                v.insert(PaneSlot::new()?);
            }
            // phux-17u: stash the session name for the status-bar
            // `WidgetContext`. The snapshot carries `sessions:
            // Vec<SessionInfo>` plus `focused_session`; the name is the
            // `SessionInfo` whose `id` matches the focused session. The
            // server populates this from `Session::name` in
            // `build_session_snapshot`. Falls back to empty if the
            // focused session somehow isn't in the list (shouldn't
            // happen — the focused session is always one of them).
            *session_name = focused_session_name(&snapshot);
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
            let origin = workspace
                .active_window()
                .filter(|ls| ls.tree.is_some())
                .and_then(|ls| {
                    super::multi_pane::compute_layout(ls, pane_dims)
                        .rects
                        .get(&terminal_id)
                        .map(|r| (r.x, r.y))
                })
                .unwrap_or((0, 0));
            let slot = match panes.entry(terminal_id) {
                std::collections::hash_map::Entry::Occupied(o) => o.into_mut(),
                std::collections::hash_map::Entry::Vacant(v) => v.insert(PaneSlot::new()?),
            };
            super::paint::safe_resize(&mut slot.terminal, cols, rows)?;
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
                // phux-5ke.4: when an overlay is up, suppress the
                // stdout flush. The libghostty mirror was already
                // updated above via `vt_write`; this branch is just
                // the outbound emit, which would scribble over the
                // modal. On dismiss the driver triggers a full
                // repaint and the user sees the latest content.
                if overlay_active {
                    let _ = overlay;
                } else {
                    let _ = slot.renderer.render_at(&slot.terminal, out, origin);
                    if let Some((row, col)) = slot.renderer.last_cursor() {
                        predict.set_cursor(row, col);
                    }
                    // Snapshot is authoritative — predict-overlay only
                    // repaints if new keystrokes arrived after the
                    // snapshot was issued and before reconcile cleared
                    // the queue. In v0 we simply leave the queue empty.
                    let _ = overlay;
                    // phux-nz4.5: the pane renderer just wrote to the
                    // bottom row of its own grid; force a status-bar
                    // repaint over it.
                    let focused_cursor = slot.renderer.last_cursor();
                    // phux-9xn: when libghostty's snapshot can't tell
                    // us a cursor position (fresh attach with no PTY
                    // output, alt-screen transitions, hidden cursor),
                    // fall back to the focused pane's origin so the
                    // host terminal cursor doesn't strand at the end
                    // of the bar row (bottom-right).
                    paint_bar_after_pane(
                        status_bar,
                        out,
                        viewport_dims,
                        session_name,
                        focused_cursor,
                        Some(origin),
                    );
                }
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
            // render path re-borrows it via `paint_focused_pane`. Clone
            // the id into the entry so `terminal_id` survives for the
            // non-focused repaint below.
            {
                let slot = match panes.entry(terminal_id.clone()) {
                    std::collections::hash_map::Entry::Occupied(o) => o.into_mut(),
                    std::collections::hash_map::Entry::Vacant(v) => v.insert(PaneSlot::new()?),
                };
                slot.terminal.vt_write(&bytes);
            }
            // The libghostty mirror is now warm even for panes in a
            // non-active window (off-screen invariant). Rendering only
            // applies to the active window's composition; if there's no
            // active window there's nothing on-screen to repaint.
            let Some(active_ls) = workspace.active_window() else {
                return Ok(FrameOutcome::default());
            };
            if is_focused
                && !overlay_active
                && let Some(fid) = focused_pane.as_ref()
            {
                let has_bar = status_bar.is_some();
                let focused_cursor =
                    paint_focused_pane(out, active_ls, panes, fid, viewport_dims, has_bar, false);
                // Per-cell match reconcile (phux-9gw.1.1): walk pending
                // predictions against the freshly painted cell grid;
                // confirmed predictions drop, contradictions drop their
                // suffix, predictions still ahead of confirmed state
                // stay alive. See [`crate::predict`] for the truth table.
                if let Some((row, col)) = focused_cursor {
                    let _stats = reconcile_terminal_output_per_cell(predict, row, col, |r, c| {
                        panes.get_mut(fid).and_then(|s| {
                            // Read the full grapheme cluster, not just the
                            // base scalar, so multi-codepoint Insert
                            // predictions (flag emoji, ZWJ sequences, base
                            // plus combining marks) reconcile against the
                            // whole painted cluster (phux-9gw.1.6).
                            s.renderer
                                .read_grapheme_string_at(&s.terminal, r, c)
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
                let _ = overlay.render(predict, out);
                // phux-9xn: compute the focused pane's Rect origin so
                // the bar paint can park the cursor there if
                // `last_cursor` is None. Without this fallback the
                // bar's final write leaves the host terminal cursor
                // at bottom-right.
                let pane_dims = pane_viewport(viewport_dims, has_bar);
                let fallback_origin = super::multi_pane::compute_layout(active_ls, pane_dims)
                    .rects
                    .get(fid)
                    .map(|r| (r.x, r.y))
                    .or(Some((0, 0)));
                paint_bar_after_pane(
                    status_bar,
                    out,
                    viewport_dims,
                    session_name,
                    focused_cursor,
                    fallback_origin,
                );
            } else if !overlay_active {
                // phux-2x9: repaint a NON-focused pane on its own output
                // so it isn't visually frozen — output (and the
                // post-split/resize resync snapshot) must show without
                // the user focusing the pane. render_at is dirty-tracked,
                // so steady-state output only repaints changed rows. After
                // painting into this pane's rect we restore the focused
                // pane's cursor so the host cursor stays where the user is
                // typing.
                let has_bar = status_bar.is_some();
                let pane_dims = pane_viewport(viewport_dims, has_bar);
                let rects = super::multi_pane::compute_layout(active_ls, pane_dims).rects;
                if let Some(rect) = rects.get(&terminal_id).copied() {
                    if let Some(slot) = panes.get_mut(&terminal_id) {
                        let _ = slot
                            .renderer
                            .render_at(&slot.terminal, out, (rect.x, rect.y));
                    }
                    // Restore the focused pane's cursor: the render above
                    // left the host cursor inside the non-focused pane.
                    let focused_cursor = focused_pane
                        .as_ref()
                        .and_then(|fid| panes.get(fid))
                        .and_then(|s| s.renderer.last_cursor());
                    if status_bar.is_some() {
                        let fallback = focused_pane
                            .as_ref()
                            .and_then(|fid| rects.get(fid))
                            .map(|r| (r.x, r.y));
                        paint_bar_after_pane(
                            status_bar,
                            out,
                            viewport_dims,
                            session_name,
                            focused_cursor,
                            fallback,
                        );
                    } else if let Some((row, col)) = focused_cursor {
                        let _ = write!(
                            out,
                            "\x1b[{};{}H\x1b[?25h",
                            row.saturating_add(1),
                            col.saturating_add(1)
                        );
                        let _ = out.flush();
                    } else {
                        let _ = out.flush();
                    }
                }
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
            // not at all. Routed through the injected sink so a headless
            // capture sees the BEL too (an agent can observe `\x07`).
            let _ = actions::write_bell(out);
            Ok(FrameOutcome::default())
        }
        // phux-4li.5: reconcile-on-attach reply path. The driver sends
        // `GET_METADATA { request_id }` immediately after ATTACHED;
        // the server replies with `MetadataValue { request_id, value }`.
        // Match by id, decode the v1 CBOR envelope, and replace
        // the workspace in place. `value: None` means "no persisted
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
            match Workspace::decode_cbor(&bytes) {
                Ok(new_ws) => {
                    *workspace = reconcile_loaded_workspace(new_ws, focused_pane.as_ref(), panes);
                    // Re-anchor the driver's focused-pane mirror onto the
                    // active window's reconciled focus.
                    *focused_pane = workspace.active_window().and_then(|ls| ls.focus.clone());
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
                match Workspace::decode_cbor(&bytes) {
                    Ok(new_ws) => {
                        *workspace =
                            reconcile_loaded_workspace(new_ws, focused_pane.as_ref(), panes);
                        *focused_pane = workspace.active_window().and_then(|ls| ls.focus.clone());
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
                *workspace = focused_pane
                    .clone()
                    .map_or_else(Workspace::default, Workspace::single);
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
            // phux-4li.15: a parked new-window takes priority — its reply
            // opens a window on the spawned pane instead of splitting the
            // active one. Request ids are unique across both maps.
            if let Some(pending) = pending_windows.remove(&request_id) {
                return handle_window_spawned(
                    out,
                    workspace,
                    focused_pane,
                    panes,
                    &pending,
                    result,
                );
            }
            let Some(pending) = pending_splits.remove(&request_id) else {
                tracing::debug!(
                    request_id,
                    "stray TerminalSpawned with no matching pending split or window; ignoring",
                );
                return Ok(FrameOutcome::default());
            };
            match result {
                SpawnResult::Ok(new_id) => {
                    let Some(active_ls) = workspace.active_window_mut() else {
                        tracing::warn!("TerminalSpawned: no active window to apply split into");
                        let _ = actions::write_bell(out);
                        return Ok(FrameOutcome::default());
                    };
                    match apply_spawned_ok(active_ls, new_id.clone(), &pending) {
                        Ok(new_state) => {
                            *active_ls = new_state;
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
                            focused_pane.clone_from(
                                &workspace.active_window().and_then(|ls| ls.focus.clone()),
                            );
                            Ok(FrameOutcome {
                                layout_replaced: true,
                                emit_set_metadata: true,
                                // phux-tnh: the split shrank the sibling
                                // and added a leaf; emit per-leaf resizes
                                // so the server learns the real split dims
                                // instead of leaving panes at spawn size.
                                reflow_panes: true,
                                ..FrameOutcome::default()
                            })
                        }
                        Err(err) => {
                            tracing::warn!(
                                error = %err,
                                terminal = ?new_id,
                                "apply_spawned_ok failed; dropping spawned terminal",
                            );
                            let _ = actions::write_bell(out);
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
                    let _ = actions::write_bell(out);
                    Ok(FrameOutcome::default())
                }
                SpawnResult::Err(SpawnError::SpawnFailed(reason)) => {
                    tracing::warn!(
                        request_id,
                        reason = %reason,
                        "TerminalSpawned: server-side spawn failed",
                    );
                    let _ = actions::write_bell(out);
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
                    let _ = actions::write_bell(out);
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
            // Always drop the slot — even for unknown leaves (could be
            // a spawn-failure cleanup race or a stale id from before
            // an attach).
            panes.remove(&terminal_id);
            // Find the window holding this leaf (panes can live in any
            // window, not just the active one) and fold it out there.
            let owner = workspace.windows.iter().position(|w| {
                w.state
                    .tree
                    .as_ref()
                    .map(layout::leaves)
                    .unwrap_or_default()
                    .contains(&terminal_id)
            });
            let Some(idx) = owner else {
                return Ok(FrameOutcome::default());
            };
            match apply_terminal_closed(&workspace.windows[idx].state, &terminal_id) {
                Ok(new_state) => {
                    workspace.windows[idx].state = new_state;
                    // The fold may have emptied the window; drop any such
                    // windows and keep `active` valid.
                    workspace.prune_empty_windows();
                    // Re-anchor `focused_pane` onto the (possibly new)
                    // active window's focus. `apply_terminal_closed` sets
                    // a surviving window's focus to the first DFS leaf;
                    // a pruned active window hands focus to its successor.
                    *focused_pane = workspace.active_window().and_then(|ls| ls.focus.clone());
                    Ok(FrameOutcome {
                        layout_replaced: true,
                        emit_set_metadata: true,
                        // phux-tnh: the survivor's Rect grew; tell the
                        // server so its PTY winsize grows too.
                        reflow_panes: true,
                        ..FrameOutcome::default()
                    })
                }
                Err(err) => {
                    // The leaf vanished from the tree between the lookup
                    // and the fold (a race), or the window emptied. Drop
                    // quietly — the slot is already gone.
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

/// phux-4li.15: apply a `TERMINAL_SPAWNED` reply for a parked
/// `new-window` action. On success it appends a window seeded on the
/// freshly spawned pane (making it active), seeds the pane's slot, and
/// re-anchors `focused_pane`. The follow-up flags mirror the split path:
/// `layout_replaced` triggers a full repaint, `emit_set_metadata`
/// broadcasts the new workspace to siblings, and `reflow_panes` sizes the
/// new full-window pane.
fn handle_window_spawned<W: super::RenderSink>(
    out: &mut W,
    workspace: &mut Workspace,
    focused_pane: &mut Option<TerminalId>,
    panes: &mut HashMap<TerminalId, PaneSlot>,
    pending: &PendingWindow,
    result: SpawnResult,
) -> Result<FrameOutcome, AttachError> {
    match result {
        SpawnResult::Ok(new_id) => {
            workspace.add_window(pending.name.clone(), new_id.clone());
            if let std::collections::hash_map::Entry::Vacant(v) = panes.entry(new_id) {
                v.insert(PaneSlot::new()?);
            }
            *focused_pane = workspace.active_window().and_then(|ls| ls.focus.clone());
            Ok(FrameOutcome {
                layout_replaced: true,
                emit_set_metadata: true,
                reflow_panes: true,
                ..FrameOutcome::default()
            })
        }
        SpawnResult::Err(err) => {
            tracing::warn!(error = ?err, "new-window: server-side spawn failed");
            let _ = actions::write_bell(out);
            Ok(FrameOutcome::default())
        }
        // SpawnResult is #[non_exhaustive] — tolerate future variants.
        _ => {
            tracing::warn!("new-window: unknown SpawnResult variant");
            Ok(FrameOutcome::default())
        }
    }
}

/// phux-17u: resolve the focused session's display name from an
/// `ATTACHED` snapshot for the status-bar `session-name` widget.
///
/// The snapshot carries `sessions: Vec<SessionInfo>` plus a
/// `focused_session` id; the name is the `SessionInfo` whose `id`
/// matches. Returns the empty string when the focused session isn't in
/// the list — which shouldn't happen (the focused session is always one
/// of the snapshot's own sessions), but an empty widget is a safer
/// degradation than a panic.
fn focused_session_name(snapshot: &phux_protocol::wire::info::SessionSnapshot) -> String {
    snapshot
        .sessions
        .iter()
        .find(|s| s.id == snapshot.focused_session)
        .map(|s| s.name.clone())
        .unwrap_or_default()
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
/// Reconcile every window of a freshly decoded [`Workspace`] via
/// [`reconcile_loaded_layout`], fixing any per-window focus that points
/// at a leaf no longer in that window's tree.
fn reconcile_loaded_workspace(
    mut workspace: Workspace,
    bootstrap_focus: Option<&TerminalId>,
    panes: &HashMap<TerminalId, PaneSlot>,
) -> Workspace {
    for w in &mut workspace.windows {
        let reconciled = reconcile_loaded_layout(w.state.clone(), bootstrap_focus, panes);
        w.state = reconciled;
    }
    workspace
}

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

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, reason = "tests")]
mod tests {
    use super::handle_server_frame;
    use std::collections::HashMap;

    use phux_protocol::ids::TerminalId;
    use phux_protocol::wire::frame::FrameKind;
    use phux_protocol::wire::info::{LayoutNode, SplitDir};

    use crate::attach::driver::PaneSlot;
    use crate::layout::{LayoutState, Workspace};
    use crate::predict::{Overlay, PredictionState, PredictiveConfig};

    /// Strip CSI escape sequences (`ESC [ ... final`) from a captured
    /// render stream, leaving only the printable glyphs, so a content
    /// assertion can't be satisfied by control bytes that happen to share
    /// a letter (e.g. the `h`/`l` in `\x1b[?25h` / `\x1b[?25l`).
    fn strip_csi(s: &str) -> String {
        let mut out = String::new();
        let mut chars = s.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '\x1b' && chars.peek() == Some(&'[') {
                chars.next(); // consume '['
                // Consume params/intermediates up to the final byte (@..~).
                for n in chars.by_ref() {
                    if ('@'..='~').contains(&n) {
                        break;
                    }
                }
            } else if c != '\x1b' {
                out.push(c);
            }
        }
        out
    }

    fn tid(id: u32) -> TerminalId {
        TerminalId::local(id)
    }

    /// Build a `panes` map with a warm [`PaneSlot`] per supplied id.
    fn panes_for(ids: &[&TerminalId]) -> HashMap<TerminalId, PaneSlot> {
        let mut panes = HashMap::new();
        for id in ids {
            panes.insert((*id).clone(), PaneSlot::new().expect("pane slot"));
        }
        panes
    }

    /// A single-window workspace whose window is two leaves split
    /// side-by-side (vertical divider), with `focus` on the supplied
    /// leaf. Exercises the multi-pane render paths without a real tty.
    fn two_pane_workspace(left: &TerminalId, right: &TerminalId, focus: &TerminalId) -> Workspace {
        let state = LayoutState {
            tree: Some(LayoutNode::Split {
                dir: SplitDir::Horizontal,
                ratio: 0.5,
                left: Box::new(LayoutNode::Leaf(left.clone())),
                right: Box::new(LayoutNode::Leaf(right.clone())),
            }),
            focus: Some(focus.clone()),
        };
        Workspace {
            windows: vec![crate::layout::WindowState {
                name: "1".to_owned(),
                state,
            }],
            active: 0,
        }
    }

    fn drive_output(
        out: &mut Vec<u8>,
        layout: &mut Workspace,
        focused: &mut Option<TerminalId>,
        panes: &mut HashMap<TerminalId, PaneSlot>,
        terminal_id: &TerminalId,
        bytes: &[u8],
    ) {
        let mut session_name = String::new();
        let mut predict = PredictionState::new(PredictiveConfig::disabled(), 80, 24);
        let overlay = Overlay;
        let mut pending_splits = HashMap::new();
        let mut pending_windows = HashMap::new();
        handle_server_frame(
            out,
            FrameKind::TerminalOutput {
                terminal_id: terminal_id.clone(),
                seq: 1,
                bytes: bytes.to_vec(),
            },
            panes,
            layout,
            focused,
            &mut session_name,
            None,
            (80, 24),
            &mut predict,
            &overlay,
            None,
            &mut pending_splits,
            &mut pending_windows,
            false,
        )
        .expect("handle_server_frame");
    }

    /// phux-2x9 via the injectable sink: a NON-focused pane must repaint
    /// on its own `TERMINAL_OUTPUT` so it isn't visually frozen. We feed
    /// output for the right (non-focused) pane and assert the captured VT
    /// carries a CUP into the right pane's rect origin plus the emitted
    /// graphemes — proving the regression without a live terminal.
    #[test]
    fn non_focused_pane_repaints_on_output() {
        let left = tid(1);
        let right = tid(2);
        let mut layout = two_pane_workspace(&left, &right, &left);
        let mut focused = Some(left.clone());
        let mut panes = panes_for(&[&left, &right]);

        let mut out: Vec<u8> = Vec::new();
        drive_output(
            &mut out,
            &mut layout,
            &mut focused,
            &mut panes,
            &right,
            b"hello",
        );

        let s = String::from_utf8_lossy(&out);
        // The right pane occupies the columns after the divider in an
        // 80-col / 0.5 split: left pane cols 0..39, divider at col 40,
        // right pane from col 41 (0-based) ⇒ 1-based CUP `;42H`.
        assert!(
            s.contains(";42H"),
            "expected CUP into right pane origin (col 42); out = {s:?}"
        );
        // The renderer emits one cell at a time with an SGR delta between
        // cells, so the graphemes are interleaved with escape sequences.
        // Strip CSI sequences before the glyph check, otherwise `h`/`l`
        // would be satisfied by the cursor mode-set bytes (`\x1b[?25h` /
        // `\x1b[?25l`) rather than the pane content itself.
        let visible = strip_csi(&s);
        assert!(
            visible.contains("hello"),
            "non-focused pane should render its glyphs; visible = {visible:?}, raw = {s:?}"
        );
    }

    /// The focused pane's output renders into its own rect (column 1 for
    /// the left pane) and the captured stream is non-empty.
    #[test]
    fn focused_pane_repaints_on_output() {
        let left = tid(1);
        let right = tid(2);
        let mut layout = two_pane_workspace(&left, &right, &left);
        let mut focused = Some(left.clone());
        let mut panes = panes_for(&[&left, &right]);

        let mut out: Vec<u8> = Vec::new();
        drive_output(
            &mut out,
            &mut layout,
            &mut focused,
            &mut panes,
            &left,
            b"world",
        );

        let s = String::from_utf8_lossy(&out);
        // Focused pane renders at column 1 (left pane origin). Glyphs are
        // interleaved with SGR resets, so assert on ordered chars.
        assert!(
            s.contains("\x1b[1;1H"),
            "expected CUP into left pane origin (col 1); out = {s:?}"
        );
        for ch in ['w', 'o', 'r', 'l', 'd'] {
            assert!(
                s.contains(ch),
                "focused pane glyph {ch:?} missing; out = {s:?}"
            );
        }
    }

    /// Off-screen invariant: a `TERMINAL_OUTPUT` for a pane that lives in
    /// a NON-active window must warm that pane's libghostty mirror but
    /// paint nothing (it isn't on screen). The pane has no rect in the
    /// active window's composition, so the renderer emits no CUP.
    #[test]
    fn output_for_inactive_window_pane_warms_mirror_but_does_not_paint() {
        let active_pane = tid(1);
        let other_pane = tid(2);
        // Two windows: active window holds pane 1; window 2 holds pane 2.
        let mut workspace = Workspace::single(active_pane.clone());
        workspace.add_window("2".to_owned(), other_pane.clone());
        // Re-select window 0 as active (add_window activated the new one).
        workspace.select(0);
        let mut focused = Some(active_pane.clone());
        let mut panes = panes_for(&[&active_pane, &other_pane]);

        let mut out: Vec<u8> = Vec::new();
        drive_output(
            &mut out,
            &mut workspace,
            &mut focused,
            &mut panes,
            &other_pane,
            b"offscreen",
        );

        // Nothing painted: the off-screen pane has no rect in the active
        // window, so the renderer wrote no bytes at all.
        assert!(
            out.is_empty(),
            "off-screen pane must not paint; out = {:?}",
            String::from_utf8_lossy(&out),
        );
        // The mirror is warm: reading the grapheme grid back shows the
        // bytes landed in pane 2's libghostty Terminal.
        let slot = panes.get_mut(&other_pane).expect("pane 2 slot");
        let cell = slot
            .renderer
            .read_grapheme_at(&slot.terminal, 0, 0)
            .expect("read cell");
        assert_eq!(cell, Some('o'), "pane 2 mirror should hold the output");
    }

    /// phux-4li.15: a `TERMINAL_SPAWNED` reply for a parked new-window
    /// opens a new window seeded on the spawned pane, makes it active,
    /// re-anchors focus, and asks for a broadcast + reflow.
    #[test]
    fn window_spawned_opens_active_window_focused_on_new_pane() {
        use super::handle_window_spawned;
        use crate::attach::actions::PendingWindow;
        use phux_protocol::wire::frame::SpawnResult;

        let mut workspace = Workspace::single(tid(1)); // window "1", pane 1
        let mut focused = Some(tid(1));
        let mut panes = panes_for(&[&tid(1)]);
        let mut out: Vec<u8> = Vec::new();

        let outcome = handle_window_spawned(
            &mut out,
            &mut workspace,
            &mut focused,
            &mut panes,
            &PendingWindow {
                name: "2".to_owned(),
            },
            SpawnResult::Ok(tid(2)),
        )
        .expect("handle_window_spawned");

        assert_eq!(workspace.windows.len(), 2);
        assert_eq!(workspace.active, 1, "new window is active");
        assert_eq!(workspace.windows[1].name, "2");
        assert_eq!(focused, Some(tid(2)), "focus follows the new pane");
        assert!(panes.contains_key(&tid(2)), "new pane got a slot");
        assert!(outcome.layout_replaced && outcome.emit_set_metadata && outcome.reflow_panes);
    }

    /// A `Bell` frame routes a BEL byte through the injected sink, so a
    /// headless capture (and a future agent surface) can observe it.
    #[test]
    fn bell_frame_writes_bel_to_sink() {
        let mut layout = Workspace::single(tid(1));
        let mut focused = Some(tid(1));
        let mut panes: HashMap<TerminalId, PaneSlot> = HashMap::new();
        let mut session_name = String::new();
        let mut predict = PredictionState::new(PredictiveConfig::disabled(), 80, 24);
        let overlay = Overlay;
        let mut pending_splits = HashMap::new();
        let mut pending_windows = HashMap::new();

        let mut out: Vec<u8> = Vec::new();
        handle_server_frame(
            &mut out,
            FrameKind::Bell {
                terminal_id: tid(1),
            },
            &mut panes,
            &mut layout,
            &mut focused,
            &mut session_name,
            None,
            (80, 24),
            &mut predict,
            &overlay,
            None,
            &mut pending_splits,
            &mut pending_windows,
            false,
        )
        .expect("handle_server_frame");

        assert_eq!(&out, b"\x07", "bell must emit a single BEL byte");
    }
}

#[cfg(test)]
mod session_name_tests {
    use super::focused_session_name;
    use phux_protocol::ids::{SessionId, TerminalId, WindowId};
    use phux_protocol::wire::info::{SessionInfo, SessionSnapshot};

    fn snapshot_with(sessions: Vec<SessionInfo>, focused: SessionId) -> SessionSnapshot {
        SessionSnapshot::new(focused, WindowId::new(0), TerminalId::local(0))
            .with_sessions(sessions)
    }

    #[test]
    fn focused_session_name_resolves_the_matching_session() {
        // phux-17u: the widget reads the name of the focused session,
        // not the first session in the list.
        let snapshot = snapshot_with(
            vec![
                SessionInfo::new(SessionId::new(1), "work"),
                SessionInfo::new(SessionId::new(2), "play"),
            ],
            SessionId::new(2),
        );
        assert_eq!(focused_session_name(&snapshot), "play");
    }

    #[test]
    fn focused_session_name_is_empty_when_focus_is_absent() {
        // Degrade to an empty widget rather than panic if the focused
        // session somehow isn't in the list.
        let snapshot = snapshot_with(
            vec![SessionInfo::new(SessionId::new(1), "work")],
            SessionId::new(99),
        );
        assert_eq!(focused_session_name(&snapshot), "");
    }
}
