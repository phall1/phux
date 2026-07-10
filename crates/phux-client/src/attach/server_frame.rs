//! Server-to-client frame handling: dispatches `FrameKind` variants to
//! the right state mutations and rendering.
//!
//! Returns a `FrameOutcome` describing the follow-up the async driver
//! should take (e.g. exit on `DETACHED`, send `GET_METADATA` after
//! `ATTACHED`, repaint after a layout-replacing frame).

use std::collections::HashMap;

use phux_protocol::ids::{ClientId, SessionId, TerminalId};
use phux_protocol::wire::frame::{AgentEvent, FrameKind, Scope, SpawnError, SpawnResult};
use phux_protocol::wire::info::SessionInfo;

use super::actions::{self, PendingSplit, PendingWindow, apply_spawned_ok, apply_terminal_closed};
use super::driver::{AttachError, DEFAULT_GROUP_ID, PaneSlot};
use super::paint::{SidebarReservation, content_rect, paint_bar_after_pane, paint_focused_pane};
use crate::agent_meta::{AgentRecord, TERMINAL_AGENT_KEY, parse_agent_record};
use crate::layout::{self, LayoutState, Workspace};
use crate::predict::{Overlay, PredictionState, reconcile_terminal_output_per_cell};
use crate::render::chrome::status_bar::StatusBarPainter;

/// ADR-0040 (`phux-3ert`): the driver-held index of `phux.agent/v1` records.
///
/// `records` is what the window chrome reads (structured agent labels for
/// the sidebar/tab strip); `pending` correlates in-flight `GET_METADATA`
/// request ids to the Terminal they asked about; `subscribed` tracks which
/// Terminals already have a live `SUBSCRIBE_METADATA` so the driver's
/// subscription sweep is idempotent.
#[derive(Debug, Default)]
pub(super) struct AgentMetaIndex {
    /// Terminal → its decoded agent record (absent = no declared agent).
    pub(super) records: HashMap<TerminalId, AgentRecord>,
    /// In-flight `GET_METADATA` request id → the Terminal it targets.
    pub(super) pending: HashMap<u32, TerminalId>,
    /// Terminals with a live `SUBSCRIBE_METADATA` on the agent key.
    pub(super) subscribed: std::collections::HashSet<TerminalId>,
}

impl AgentMetaIndex {
    /// Apply a metadata value for `terminal` (a `GET` reply or a
    /// `METADATA_CHANGED` broadcast; `None` bytes = tombstone). Returns
    /// `true` when the stored record actually changed, so the driver only
    /// repaints chrome for real transitions.
    fn apply(&mut self, terminal: &TerminalId, bytes: Option<&[u8]>) -> bool {
        match bytes.and_then(parse_agent_record) {
            Some(record) => self.records.insert(terminal.clone(), record.clone()) != Some(record),
            None => self.records.remove(terminal).is_some(),
        }
    }
}

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
    /// `true` ⇒ the loop should exit cleanly: either the server sent
    /// `DETACHED`, or a `TERMINAL_CLOSED` folded the last pane out of the
    /// layout and the consumer-owned detach policy (phux-4r1) decided to
    /// leave (nothing left to render or route input to).
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
    /// phux-3uv: `Some((terminal_id, seq))` ⇒ the client applied a
    /// `TERMINAL_OUTPUT` frame and the driver must send a cumulative
    /// `FRAME_ACK { terminal_id, seq }` back to the server. This closes
    /// the ADR-0018 lazy-state-sync loop: the server's per-consumer
    /// `SnapshotSynthesizer` calls `mark_synced` on receipt, clearing the
    /// dirty bits that produced the acked frame so the next tick re-diffs
    /// against the acked reference (rather than re-emitting an unbounded
    /// unacked delta forever). Set ONLY by the `TerminalOutput` arm.
    pub(super) ack: Option<(TerminalId, u64)>,
    /// phux-4li.20: `Some((sessions, focused))` ⇒ ATTACHED just landed
    /// and carried the server's full session graph. The driver caches
    /// it so the `<leader> a` session picker can list the other
    /// sessions without a follow-up request/response frame — the
    /// `ATTACHED` snapshot is already authoritative at attach time (SPEC
    /// §13). Set ONLY by the `Attached` arm.
    pub(super) sessions: Option<(Vec<SessionInfo>, SessionId)>,
    /// ADR-0033: `Some(id)` ⇒ ATTACHED carried this client's own server-assigned
    /// `ClientId`. The driver caches it to tell "you have the wheel" from
    /// another client holding it when rendering the supervisory badge. Set ONLY
    /// by the `Attached` arm.
    pub(super) own_client_id: Option<ClientId>,
    /// ADR-0033 / phux-foz.1: `true` ⇒ an agent event updated a pane's
    /// lifecycle, input-lease holder (`TerminalControl`), or asked-attention
    /// flag (ADR-0035 `Asked`), so the driver must repaint the chrome
    /// (supervisory badge, attention hint, window-tab markers) even though no
    /// grid content changed. Set ONLY by the `Event` arms.
    pub(super) chrome_dirty: bool,
    /// ADR-0040: `true` ⇒ a `phux.agent/v1` record changed for some pane
    /// (a `GET_METADATA` reply or a `METADATA_CHANGED` broadcast). Window
    /// labels derive from it, so the driver refreshes the window chrome
    /// (tab strip + sidebar) and repaints. Set ONLY by the
    /// `MetadataValue` / `MetadataChanged` arms.
    pub(super) agent_meta_changed: bool,
    /// phux-p4vp: per-pane working directories carried by the `ATTACHED`
    /// snapshot (`TerminalInfo::cwd`). The driver folds these into its
    /// pane-cwd index, from which the sidebar's branch line is derived
    /// client-side (see `crate::vcs`). Set ONLY by the `Attached` arm;
    /// empty otherwise.
    pub(super) pane_cwds: Vec<(TerminalId, String)>,
}

/// Payload-free label for the inbound `FrameKind` — the `kind` field on
/// the per-frame dispatch span. Keeps the trace line small and free of
/// content bytes / session names; the heavy content frames additionally
/// record `terminal_id` / `seq` / `bytes`. `FrameKind` is large and
/// `#[non_exhaustive]`, so this covers the S->C arms this handler acts on
/// and folds the rest into `"other"`.
const fn frame_kind_label(frame: &FrameKind) -> &'static str {
    match frame {
        FrameKind::Attached { .. } => "attached",
        FrameKind::TerminalSnapshot { .. } => "terminal_snapshot",
        FrameKind::TerminalOutput { .. } => "terminal_output",
        FrameKind::Detached => "detached",
        FrameKind::Bell { .. } => "bell",
        FrameKind::MetadataValue { .. } => "metadata_value",
        FrameKind::MetadataChanged { .. } => "metadata_changed",
        FrameKind::TerminalSpawned { .. } => "terminal_spawned",
        FrameKind::TerminalClosed { .. } => "terminal_closed",
        _ => "other",
    }
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
    // phux-x2hm: the driver's pane-zoom state. RENDER/REFLOW geometry reads go
    // through `Workspace::render_window(zoomed)` so a zoomed pane paints to the
    // full window and non-zoomed panes (absent from the synthetic single-leaf
    // layout) correctly do not paint. A `TerminalSpawned`-ok split clears this
    // (`*zoomed = None`) so a new pane un-zooms, matching tmux. Mutation/input
    // reads (focus reconcile) keep using the REAL `active_window`.
    zoomed: &mut Option<TerminalId>,
    session_name: &mut String,
    status_bar: Option<&mut StatusBarPainter>,
    // phux-4h5a: the window-sidebar reservation, threaded identically to
    // `status_bar` so every layout site in this dispatcher tiles panes into
    // the SAME inset content rect the driver paints + reflows against. `None`
    // (sidebar disabled, the default) makes `content_rect` the full pane
    // viewport, so the whole dispatcher stays byte-identical to the
    // pre-sidebar path.
    sidebar: Option<SidebarReservation>,
    viewport_dims: (u16, u16),
    predict: &mut PredictionState,
    overlay: &Overlay,
    pending_layout_request: Option<u32>,
    pending_splits: &mut HashMap<u32, PendingSplit>,
    pending_windows: &mut HashMap<u32, PendingWindow>,
    // ADR-0040: the driver-held `phux.agent/v1` index. The MetadataValue /
    // MetadataChanged arms decode agent records into it; the driver reads
    // it when composing window labels.
    agent_meta: &mut AgentMetaIndex,
    // phux-5ke.4: when `true` an overlay is on top; pane libghostty
    // mirrors keep ingesting `vt_write` (per ADR-0013) but stdout
    // flushes (render_at, bar paint, predict-overlay paint) are
    // suppressed so the modal doesn't get scribbled over. The driver
    // triggers a full repaint on overlay dismiss.
    overlay_active: bool,
    // phux-jhv8: when `true` this frame is an earlier member of a coalesced
    // burst — a later frame in the same drain targets this pane, so its
    // libghostty mirror still ingests `vt_write` (state stays correct) but the
    // stdout paint (render_at, bar, predict-overlay, reconcile) is suppressed.
    // The driver passes `defer_paint = false` for each pane's LAST frame in the
    // burst, so every touched pane settles exactly once instead of repainting
    // on every intermediate redraw. Same vt_write-but-no-paint contract as
    // `overlay_active`, minus the modal semantics.
    defer_paint: bool,
) -> Result<FrameOutcome, AttachError> {
    // Per-inbound-frame dispatch span (debug; off under the default
    // `phux=info` filter and free when disabled). For the content frames
    // this function also paints (TERMINAL_SNAPSHOT / TERMINAL_OUTPUT) the
    // span's CLOSE duration is the client-side apply+paint cost — the
    // client lag signal the flywheel reads. `kind` is a payload-free label;
    // `terminal_id` / `seq` / `bytes` are recorded inside the content arms
    // below. Declared `Empty` so they exist for later `record`.
    let frame_span = tracing::debug_span!(
        "handle_server_frame",
        kind = frame_kind_label(&frame),
        terminal_id = tracing::field::Empty,
        seq = tracing::field::Empty,
        bytes = tracing::field::Empty,
    )
    .entered();
    match frame {
        FrameKind::Attached {
            snapshot,
            initial_client_id,
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
            // Seed client-side mirrors at their server-advertised sizes
            // before any TERMINAL_OUTPUT can race ahead of the per-pane
            // TERMINAL_SNAPSHOT. VT interpretation is geometry-sensitive;
            // starting at 80x24 and resizing later corrupts wraps, clips,
            // and absolute cursor movement for wider/taller viewports.
            for pane in &snapshot.panes {
                if let std::collections::hash_map::Entry::Vacant(v) = panes.entry(pane.id.clone()) {
                    let slot = v.insert(PaneSlot::new_with_size(pane.cols, pane.rows)?);
                    // phux-foz.4: seed the pane's cwd from the snapshot (the
                    // spawn cwd); `cwd_changed` events refine it live.
                    slot.cwd.clone_from(&pane.cwd);
                }
            }
            // phux-p4vp: hand the per-pane cwds up to the driver so the
            // sidebar can derive each window's VCS branch client-side.
            let pane_cwds: Vec<(TerminalId, String)> = snapshot
                .panes
                .iter()
                .filter_map(|p| p.cwd.clone().map(|cwd| (p.id.clone(), cwd)))
                .collect();
            // Ensure the focused pane has a slot even if an older server's
            // ATTACHED graph omitted it. Fall back to the current pane
            // viewport (the same dimensions used for rendering) rather
            // than the historical 80x24 placeholder.
            if let std::collections::hash_map::Entry::Vacant(v) = panes.entry(bootstrap) {
                let content = content_rect(viewport_dims, status_bar.is_some(), sidebar);
                v.insert(PaneSlot::new_with_size(content.w, content.h)?);
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
            // phux-4li.20: hand the driver the full session graph so the
            // `<leader> a` session picker can list peer sessions. The
            // snapshot is the authoritative session list at attach time;
            // a dedicated request/response frame would be redundant.
            let session_cache = (snapshot.sessions.clone(), snapshot.focused_session);
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
                sessions: Some(session_cache),
                // ADR-0033: cache our own ClientId so the supervisory badge can
                // distinguish "you hold the wheel" from another client.
                own_client_id: Some(initial_client_id),
                pane_cwds,
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
            // Correlate this apply with the pane + payload size; the span's
            // CLOSE duration is the client-side snapshot-apply (vt_write +
            // render) cost.
            frame_span.record("terminal_id", tracing::field::debug(&terminal_id));
            frame_span.record("bytes", vt_replay_bytes.len());
            // phux-4li.4: route per-pane snapshots into per-pane slots.
            // Allocate a fresh slot on first sight so output frames for
            // pre-split panes don't drop on the floor.
            let is_focused = Some(&terminal_id) == focused_pane.as_ref();
            // Resolve the pane's outer-viewport Rect BEFORE the
            // `panes.entry(terminal_id)` move. Multi-pane: ask the
            // layout. Single-pane / no layout: anchor at (0,0).
            let has_bar = status_bar.is_some();
            let content = content_rect(viewport_dims, has_bar, sidebar);
            // The pane's outer-viewport Rect: origin positions the paint,
            // (w, h) clips it. Multi-pane: ask the layout. Single-pane / no
            // layout: anchor at the content rect spanning the full pane area.
            let rect = workspace
                .render_window(zoomed.as_ref())
                .filter(|ls| ls.tree.is_some())
                .and_then(|ls| {
                    super::multi_pane::compute_layout_in(ls.as_ref(), content, viewport_dims)
                        .rects
                        .get(&terminal_id)
                        .copied()
                })
                .unwrap_or(content);
            let origin = (rect.x, rect.y);
            let slot = match panes.entry(terminal_id.clone()) {
                std::collections::hash_map::Entry::Occupied(o) => o.into_mut(),
                std::collections::hash_map::Entry::Vacant(v) => {
                    v.insert(PaneSlot::new_with_size(cols, rows)?)
                }
            };
            let prior_sync_output = slot.sync_output_since;
            // The mirror grid size is server-authoritative: the TERMINAL_SNAPSHOT
            // carries the server's `(cols, rows)` and this is the ONE place that
            // resizes the pane's libghostty Terminal to them (alongside a future
            // server resize-ack). The client layout rect clips and positions
            // rendering but NEVER calls `resize()` on the mirror — resizing the
            // alt-screen mirror to a transient client-rect size during a resize
            // handshake strands previous-screen content (the ghost cells).
            super::paint::safe_resize(&mut slot.terminal, cols, rows)?;
            // phux-flywheel: time the snapshot VT-apply (scrollback +
            // visible replay into the libghostty mirror) under its own
            // child span, distinct from the render trigger below — same
            // apply-vs-paint split as the `TerminalOutput` hot path. The
            // `bytes` field counts the replay payload (the dominant term;
            // scrollback is rarely present in the live attach path).
            let sync_output_active = {
                let _apply =
                    tracing::debug_span!("vt_apply", bytes = vt_replay_bytes.len()).entered();
                // Apply scrollback first (if any), then the visible-state
                // replay — order per SPEC §8.4 / §13.
                if let Some(sb) = scrollback_bytes {
                    slot.terminal.vt_write(&sb);
                }
                slot.terminal.vt_write(&vt_replay_bytes);
                let active = slot.update_sync_output(tokio::time::Instant::now());
                if let Some(since) = prior_sync_output {
                    // A resync snapshot may reset DEC modes in its preamble,
                    // but it must not tear down a raw transaction that began
                    // before the snapshot. The matching live `?2026l` remains
                    // the publication barrier.
                    slot.sync_output_since = Some(since);
                    slot.sync_output_dirty = true;
                    true
                } else {
                    active
                }
            };
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
                // The live coalescing path never defers a snapshot (the driver
                // excludes `TerminalSnapshot` from the defer mask), so here
                // `defer_paint` is set only by the headless ingest path, which
                // suppresses all VT emission and composes once at the end.
                if overlay_active || defer_paint || sync_output_active {
                    let _ = overlay;
                } else {
                    // phux-flywheel: the snapshot paint trigger, timed
                    // separately from the `vt_apply` above.
                    let _paint_trigger =
                        tracing::debug_span!("paint_trigger", rows = viewport_dims.1).entered();
                    // A snapshot is authoritative full state, so force a full
                    // redraw rather than trusting libghostty's per-row dirty
                    // bits: a `safe_resize` + replay can leave rows the client
                    // still needs marked clean (resize-grow, alt-screen
                    // transitions), and a plain `render_at` would skip them,
                    // leaving stale/garbage cells — the attach/reattach/resize
                    // "mangled screen" bug.
                    let _ =
                        slot.renderer
                            .render_at_full(&slot.terminal, out, origin, (rect.w, rect.h));
                    // Re-anchor the predict layer in PANE-LOCAL coordinates
                    // (predictions are pane-local; the overlay re-adds the
                    // origin). Feeding the outer-absolute `last_cursor` here
                    // clamps a lower pane's cursor up into the wrong region —
                    // the mid-screen ghost echo after a split (phux-7ry0).
                    if let Some((row, col)) = slot.renderer.last_cursor_local() {
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
                        // The pane render stays above the bar row; let the
                        // painter's cache decide whether a re-emit is needed.
                        false,
                    );
                }
            } else if !overlay_active && !defer_paint && !sync_output_active {
                // phux-paer: a NON-focused pane's snapshot must paint into its
                // rect — the symmetric counterpart to the `TerminalOutput`
                // non-focused branch (phux-2x9). Without it, re-attaching to a
                // split leaves every non-focused pane blank: its libghostty
                // mirror is warm (the `vt_write` above) but never rendered,
                // while input still routes — exactly the "screens wiped but
                // still typable" report. A pane absent from the active window's
                // composition is off-screen and must NOT paint (off-screen
                // invariant), so we render only when it has a rect.
                let rects = workspace
                    .render_window(zoomed.as_ref())
                    .map(|ls| {
                        super::multi_pane::compute_layout_in(ls.as_ref(), content, viewport_dims)
                            .rects
                    })
                    .unwrap_or_default();
                if let Some(rect) = rects.get(&terminal_id).copied() {
                    if let Some(slot) = panes.get_mut(&terminal_id) {
                        // Authoritative snapshot → force a full redraw of the
                        // pane rect (see the focused branch above).
                        let _ = slot.renderer.render_at_full(
                            &slot.terminal,
                            out,
                            (rect.x, rect.y),
                            (rect.w, rect.h),
                        );
                    }
                    // The render above left the host cursor inside the
                    // non-focused pane; restore the focused pane's cursor so it
                    // stays where the user is typing.
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
                            false,
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
        FrameKind::TerminalOutput {
            terminal_id,
            seq,
            bytes,
        } => {
            // phux-3uv / ADR-0018: ack every applied frame. The bytes are
            // written into the pane mirror below (vt_write) before any
            // render branch, so by the time we return the cumulative
            // application invariant holds regardless of focus/overlay
            // state — emit the `FRAME_ACK` unconditionally on the outcome.
            // `seq == 0` is the server's "empty initial frame" sentinel
            // (see `LastAckedCursorMode`); never acking it keeps
            // `last_acked_seq` at its `0` initial value, which is correct.
            let ack = (seq != 0).then(|| (terminal_id.clone(), seq));
            // Correlate this apply: which pane, which seq, how many bytes.
            // The span's CLOSE duration is the per-frame client paint cost
            // (vt_write + render_at for the focused pane) — the headline
            // client lag signal a trace reader greps `handle_server_frame`
            // with `kind=terminal_output` for.
            frame_span.record("terminal_id", tracing::field::debug(&terminal_id));
            frame_span.record("seq", seq);
            frame_span.record("bytes", bytes.len());
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
            //
            // phux-flywheel: this VT-apply (feeding bytes into the
            // libghostty mirror) is timed by its OWN child span, distinct
            // from the paint trigger below. The parent
            // `handle_server_frame` close-duration is apply+paint fused;
            // splitting them lets a trace attribute client lag to the
            // libghostty parse (`vt_apply`) versus the render
            // (`paint_full_frame`, opened inside `paint_focused_pane`)
            // separately. Debug-level + a lazy `bytes` field ⇒ free at the
            // default `phux=info` filter.
            let sync_output_active = {
                let _apply = tracing::debug_span!("vt_apply", bytes = bytes.len()).entered();
                let has_bar = status_bar.is_some();
                let content = content_rect(viewport_dims, has_bar, sidebar);
                // Best-known dims for sizing a freshly-allocated slot only. An
                // existing slot's libghostty grid is server-authoritative and
                // must NOT be resized here: the server authored these bytes for
                // its own grid size, and resizing the alt-screen mirror to a
                // transient client-rect size during a resize handshake strands
                // previous-screen content in the dropped columns (the ghost
                // cells — the alt screen does not reflow). The mirror is resized
                // only at the TERMINAL_SNAPSHOT resync (and a future resize-ack);
                // here we just feed bytes in.
                let initial_dims = workspace
                    .render_window(zoomed.as_ref())
                    .and_then(|ls| {
                        super::multi_pane::compute_layout_in(ls.as_ref(), content, viewport_dims)
                            .rects
                            .get(&terminal_id)
                            .map(|r| (r.w, r.h))
                    })
                    .unwrap_or((content.w, content.h));
                let slot = match panes.entry(terminal_id.clone()) {
                    std::collections::hash_map::Entry::Occupied(o) => o.into_mut(),
                    std::collections::hash_map::Entry::Vacant(v) => {
                        v.insert(PaneSlot::new_with_size(initial_dims.0, initial_dims.1)?)
                    }
                };
                slot.terminal.vt_write(&bytes);
                slot.update_sync_output(tokio::time::Instant::now())
            };
            // The libghostty mirror is now warm even for panes in a
            // non-active window (off-screen invariant). Rendering only
            // applies to the active window's composition; if there's no
            // active window there's nothing on-screen to repaint.
            // phux-x2hm: render against the zoom-honoring view so a zoomed
            // pane paints to the whole window and the others (absent from the
            // synthetic single-leaf layout) get no rect and so do not paint.
            let Some(active_ls) = workspace.render_window(zoomed.as_ref()) else {
                return Ok(FrameOutcome {
                    ack,
                    ..FrameOutcome::default()
                });
            };
            let active_ls = active_ls.as_ref();
            if is_focused
                && !overlay_active
                && !defer_paint
                && !sync_output_active
                && let Some(fid) = focused_pane.as_ref()
            {
                // phux-flywheel: the paint trigger — render the focused
                // pane (this enters `paint_full_frame`'s span inside
                // `paint_focused_pane`), reconcile predictions, repaint the
                // bar. Its OWN child span isolates paint cost from the
                // `vt_apply` above so a trace shows apply-ms vs paint-ms
                // separately. Debug-level + lazy `rows` field ⇒ free at the
                // default filter.
                let _paint_trigger =
                    tracing::debug_span!("paint_trigger", rows = viewport_dims.1).entered();
                let has_bar = status_bar.is_some();
                let _ = paint_focused_pane(
                    out,
                    active_ls,
                    panes,
                    fid,
                    viewport_dims,
                    has_bar,
                    sidebar,
                    false,
                );
                // The reconcile + overlay work entirely in PANE-LOCAL
                // coordinates (predictions are pane-local; the cell reader
                // indexes the pane's own grid). `focused_cursor` (outer) is
                // kept only for the host-cursor restore in the bar paint.
                let (focused_cursor, focused_cursor_local, pane_origin) =
                    panes.get(fid).map_or((None, None, (0, 0)), |s| {
                        (
                            s.renderer.last_cursor(),
                            s.renderer.last_cursor_local(),
                            s.renderer.last_origin(),
                        )
                    });
                // Per-cell match reconcile (phux-9gw.1.1): walk pending
                // predictions against the freshly painted cell grid;
                // confirmed predictions drop, contradictions drop their
                // suffix, predictions still ahead of confirmed state
                // stay alive. See [`crate::predict`] for the truth table.
                if let Some((row, col)) = focused_cursor_local {
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
                // of a partial confirmation), shifted by the focused pane's
                // outer origin. On a fully-drained queue this is a no-op.
                let _ = overlay.render(predict, pane_origin, out);
                // phux-9xn: compute the focused pane's Rect origin so
                // the bar paint can park the cursor there if
                // `last_cursor` is None. Without this fallback the
                // bar's final write leaves the host terminal cursor
                // at bottom-right.
                let content = content_rect(viewport_dims, has_bar, sidebar);
                let fallback_origin =
                    super::multi_pane::compute_layout_in(active_ls, content, viewport_dims)
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
                    // Hot path: pane render stays above the bar row, so the
                    // painter's cache makes an unchanged bar a zero-byte
                    // no-op (incremental-paint win).
                    false,
                );
            } else if !overlay_active && !defer_paint && !sync_output_active {
                // phux-2x9: repaint a NON-focused pane on its own output
                // so it isn't visually frozen — output (and the
                // post-split/resize resync snapshot) must show without
                // the user focusing the pane. render_at is dirty-tracked,
                // so steady-state output only repaints changed rows. After
                // painting into this pane's rect we restore the focused
                // pane's cursor so the host cursor stays where the user is
                // typing.
                let has_bar = status_bar.is_some();
                let content = content_rect(viewport_dims, has_bar, sidebar);
                let rects =
                    super::multi_pane::compute_layout_in(active_ls, content, viewport_dims).rects;
                if let Some(rect) = rects.get(&terminal_id).copied() {
                    if let Some(slot) = panes.get_mut(&terminal_id) {
                        let _ = slot.renderer.render_at(
                            &slot.terminal,
                            out,
                            (rect.x, rect.y),
                            (rect.w, rect.h),
                        );
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
                            // Non-focused pane render stays above the bar
                            // row; cache decides whether to re-emit.
                            false,
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
            Ok(FrameOutcome {
                ack,
                ..FrameOutcome::default()
            })
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
            // ADR-0040: a pending per-Terminal `phux.agent/v1` GET reply.
            // `value: None` (key absent) clears any stale record.
            if let Some(terminal) = agent_meta.pending.remove(&request_id) {
                let changed = agent_meta.apply(&terminal, value.as_deref());
                return Ok(FrameOutcome {
                    agent_meta_changed: changed,
                    ..FrameOutcome::default()
                });
            }
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
            // ADR-0040: a `phux.agent/v1` broadcast for a subscribed pane.
            // A tombstone (`value: None`, the DELETE_METADATA path) clears
            // the record and the label falls back to the OSC title.
            if key == TERMINAL_AGENT_KEY {
                if let Scope::Terminal(terminal) = &scope {
                    let changed = agent_meta.apply(terminal, value.as_deref());
                    return Ok(FrameOutcome {
                        agent_meta_changed: changed,
                        ..FrameOutcome::default()
                    });
                }
                return Ok(FrameOutcome::default());
            }
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
                            // phux-x2hm: a split un-zooms (tmux parity). The
                            // new pane needs its tile, and the reflow_panes
                            // diff below is taken against the now-cleared
                            // (real, tiled) view.
                            *zoomed = None;
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
                            // Re-anchor predictive echo to the freshly
                            // focused pane (phux-7ry0). The split leaves the
                            // predict layer holding the previous pane's
                            // viewport + cursor; a keystroke before the new
                            // pane's first snapshot would otherwise echo at
                            // the old pane's coordinates (mid-screen ghost).
                            if let Some(fid) = focused_pane.as_ref() {
                                super::driver::reanchor_predict_to_pane(predict, panes, fid);
                            }
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
                SpawnResult::Err(SpawnError::GroupNotFound) => {
                    // v0.1 clients only ever target DEFAULT_GROUP_ID,
                    // which the server always exposes; this branch
                    // means a server-side L2 invariant changed under
                    // us. Log loudly + bell.
                    tracing::warn!(
                        request_id,
                        "TerminalSpawned: server reports GroupNotFound for DEFAULT group",
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
                    // phux-4r1: consumer-owned detach policy (ADR-0015 L1).
                    // The server reports the fact (TERMINAL_CLOSED) and stops
                    // there; deciding whether *this* client detaches is the
                    // TUI's call. When the last pane closed there is nothing
                    // left to render or to route input to, so detach. For
                    // v0.1 single-pane this is behaviorally identical to the
                    // old server-baked "EOF ⇒ DETACHED" (the seed pane closes
                    // ⇒ client exits), but now multi-Terminal-ready: closing
                    // one of several panes folds it out and keeps the attach
                    // alive.
                    if workspace.windows.is_empty() {
                        tracing::info!("TerminalClosed folded the last pane; detaching");
                        return Ok(FrameOutcome {
                            exit: true,
                            ..FrameOutcome::default()
                        });
                    }
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
        // ADR-0033: a pushed agent event. We subscribed to the stream at
        // attach (SUBSCRIBE_EVENTS) for the supervisory `TerminalControl`
        // broadcast; fold its lifecycle + lease-holder into the pane's slot so
        // the next paint renders the "FROZEN" / "wheel" badge. The ADR-0035
        // `Asked` event is folded into the same per-pane state below. Other
        // event kinds (dirty/idle/bell/...) are not consumed by the
        // interactive TUI.
        FrameKind::Event {
            terminal: Some(terminal),
            event:
                AgentEvent::TerminalControl {
                    lifecycle,
                    input_holder,
                    ..
                },
        } => {
            if let Some(slot) = panes.get_mut(&terminal) {
                slot.lifecycle = lifecycle;
                slot.input_holder = input_holder;
                Ok(FrameOutcome {
                    chrome_dirty: true,
                    ..FrameOutcome::default()
                })
            } else {
                // A control event for a pane we have no slot for yet (it can
                // precede the first snapshot). Harmless to drop — the lease is
                // server-authoritative and the next event re-states it.
                Ok(FrameOutcome::default())
            }
        }
        // phux-foz.1 / ADR-0035: an agent in `terminal` is waiting on a human
        // answer. Mirror the `TerminalControl` fold above: raise the pane's
        // attention flag so the next chrome paint renders the window-tab `!`
        // marker and the status-bar `[ ASK ]` hint. The flag clears when the
        // user sends key/paste input to the pane (see
        // `driver::clear_attention_on_input`); a repeated `Asked` while
        // already flagged changes nothing, so no repaint is requested for it.
        FrameKind::Event {
            terminal: Some(terminal),
            event: AgentEvent::Asked { .. },
        } => {
            if let Some(slot) = panes.get_mut(&terminal) {
                if slot.attention {
                    Ok(FrameOutcome::default())
                } else {
                    slot.attention = true;
                    Ok(FrameOutcome {
                        chrome_dirty: true,
                        ..FrameOutcome::default()
                    })
                }
            } else {
                // An Asked for a pane we have no slot for yet (it can precede
                // the first snapshot). Dropped like an early TerminalControl;
                // the ADR-0036 detector coalesces repeated markers, so the
                // next re-ask re-raises it once the slot exists.
                Ok(FrameOutcome::default())
            }
        }
        // phux-foz.4: the pane's shell changed directory (kernel-observed,
        // announced at prompt boundaries / output settle). Fold it into the
        // slot so the status-bar `cwd` widget tracks the focused pane;
        // `chrome_dirty` only when the value actually moved, and the chrome
        // refresh itself no-ops for an unfocused pane's change.
        FrameKind::Event {
            terminal: Some(terminal),
            event: AgentEvent::CwdChanged { cwd },
        } => {
            match panes.get_mut(&terminal) {
                Some(slot) if slot.cwd.as_deref() != Some(cwd.as_str()) => {
                    slot.cwd = Some(cwd);
                    Ok(FrameOutcome {
                        chrome_dirty: true,
                        ..FrameOutcome::default()
                    })
                }
                // Unchanged value, or a pane we have no slot for yet — the
                // next cwd_changed (or the ATTACHED seed) covers it.
                _ => Ok(FrameOutcome::default()),
            }
        }
        // phux-foz.4: a command finished in the pane; record its OSC-133
        // exit code for the status-bar `exit` widget. `None` is recorded
        // too — "the last command reported no code" honestly blanks the
        // widget rather than pinning a stale code.
        FrameKind::Event {
            terminal: Some(terminal),
            event: AgentEvent::CommandFinished { exit_code },
        } => match panes.get_mut(&terminal) {
            Some(slot) if slot.last_exit != exit_code => {
                slot.last_exit = exit_code;
                Ok(FrameOutcome {
                    chrome_dirty: true,
                    ..FrameOutcome::default()
                })
            }
            _ => Ok(FrameOutcome::default()),
        },
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

/// Decide whether `(scope, key)` matches a layout-coordination key ADR-0019
/// reserves (`phux.tui.layout/v1[/<session>]`, scoped to the default Group).
///
/// Per-session keying (phux-jy4t) means the key carries a session suffix; a
/// client only ever receives broadcasts for the key it subscribed to (its own
/// session), so matching the family is sufficient.
fn is_layout_key(scope: &Scope, key: &str) -> bool {
    matches!(scope, Scope::Group(id) if *id == DEFAULT_GROUP_ID)
        && super::driver::is_layout_key_string(key)
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
/// Reconcile a freshly decoded [`Workspace`], discarding it for a clean single
/// pane only when it belongs to a *different* session, and otherwise fixing each
/// window's focus to point at a leaf of its own tree.
///
/// The foreign-session discard (phux-jy4t) is evaluated at **workspace scope**,
/// not per window. Layout metadata is group-scoped and shared across every
/// session (one `DEFAULT_GROUP_ID`), so a freshly created session reads a
/// sibling session's entire persisted workspace. The signal that the loaded
/// workspace is foreign is that this session's real ATTACHED pane
/// (`bootstrap_focus`) is a leaf of **none** of its windows — every window
/// references terminals this session will never own. In that case discard the
/// whole thing.
///
/// Doing the discard per window instead aliased every *non-active* window onto
/// the focused pane: a non-active window legitimately never contains the focused
/// pane, so the per-window guard rewrote it to `LayoutState::single(focus)` —
/// leaving two windows referencing one `TerminalId`, so opening (say) vim in one
/// window showed it in the other. Workspace scope is the fix.
fn reconcile_loaded_workspace(
    mut workspace: Workspace,
    bootstrap_focus: Option<&TerminalId>,
    panes: &HashMap<TerminalId, PaneSlot>,
) -> Workspace {
    if let Some(focus) = bootstrap_focus {
        let mut any_leaves = false;
        let mut owns_focus = false;
        for w in &workspace.windows {
            let leaves = w
                .state
                .tree
                .as_ref()
                .map(crate::layout::leaves)
                .unwrap_or_default();
            any_leaves |= !leaves.is_empty();
            owns_focus |= leaves.contains(focus);
        }
        // Some window has real leaves, but none of them is our pane ⇒ the whole
        // workspace is a foreign session's. Start from a clean single pane.
        if any_leaves && !owns_focus {
            return Workspace::single(focus.clone());
        }
    }
    for w in &mut workspace.windows {
        let reconciled = reconcile_loaded_layout(w.state.clone(), bootstrap_focus, panes);
        w.state = reconciled;
    }
    workspace
}

/// Fix a single window's focus so it points at a leaf of *its own* tree.
///
/// No longer discards a tree that omits `bootstrap_focus` — that workspace-scope
/// decision moved to [`reconcile_loaded_workspace`]. Here `bootstrap_focus` is
/// only a *preference* for repairing an invalid focus; a non-active window whose
/// tree doesn't contain it simply falls back to its first leaf, never to the
/// global focus (which would alias it onto the active window's pane).
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
    use super::{AgentMetaIndex, FrameOutcome, handle_server_frame};
    use std::collections::HashMap;
    use std::sync::Mutex;

    use phux_protocol::ids::{ClientId, SessionId, TerminalId, WindowId};
    use phux_protocol::wire::frame::FrameKind;
    use phux_protocol::wire::info::{LayoutNode, SessionSnapshot, SplitDir, TerminalInfo};

    use crate::attach::driver::PaneSlot;
    use crate::layout::{LayoutState, Workspace};
    use crate::predict::{Overlay, PredictionState, PredictiveConfig};

    static TRACE_TEST_LOCK: Mutex<()> = Mutex::new(());

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

    fn split2(a: u32, b: u32, focus: u32) -> LayoutState {
        LayoutState {
            tree: Some(LayoutNode::Split {
                dir: SplitDir::Horizontal,
                ratio: 0.5,
                left: Box::new(LayoutNode::Leaf(tid(a))),
                right: Box::new(LayoutNode::Leaf(tid(b))),
            }),
            focus: Some(tid(focus)),
        }
    }

    /// A single-window workspace wrapping `state`, for the reconcile tests.
    fn ws1(state: LayoutState) -> Workspace {
        Workspace {
            windows: vec![crate::layout::WindowState {
                name: "1".to_owned(),
                state,
            }],
            active: 0,
        }
    }

    /// Leaves of a workspace's window at `idx`.
    fn window_leaves(ws: &Workspace, idx: usize) -> Vec<TerminalId> {
        ws.windows[idx]
            .state
            .tree
            .as_ref()
            .map(crate::layout::leaves)
            .unwrap_or_default()
    }

    /// phux-jy4t: a freshly created session reads the group-shared layout
    /// metadata, which holds a DIFFERENT session's tree. When this session's
    /// real ATTACHED pane is not a leaf of ANY window, the whole loaded
    /// workspace is foreign and must be discarded for a clean single pane — not
    /// rendered as the old layout with dead/empty panes.
    #[test]
    fn reconcile_discards_a_foreign_session_layout() {
        let foreign = ws1(split2(1, 2, 1)); // leaves {1, 2}, from another session
        let out = super::reconcile_loaded_workspace(foreign, Some(&tid(9)), &HashMap::new());
        assert_eq!(out.windows.len(), 1);
        assert_eq!(
            window_leaves(&out, 0),
            vec![tid(9)],
            "foreign layout discarded → clean single pane of the real terminal"
        );
        assert_eq!(out.windows[0].state.focus, Some(tid(9)));
    }

    #[test]
    fn reconcile_keeps_a_layout_that_contains_the_session_pane() {
        // Legitimate re-attach: the session's focused pane IS a leaf, so the
        // multi-pane tree is preserved (not discarded).
        let own = ws1(split2(1, 2, 1));
        let out = super::reconcile_loaded_workspace(own, Some(&tid(1)), &HashMap::new());
        let leaves = window_leaves(&out, 0);
        assert!(
            leaves.contains(&tid(1)) && leaves.contains(&tid(2)),
            "the session's own layout must be kept: {leaves:?}"
        );
    }

    #[test]
    fn reconcile_without_bootstrap_focus_keeps_the_tree() {
        // No ATTACHED focus to validate against ⇒ don't discard.
        let tree = ws1(split2(1, 2, 1));
        let out = super::reconcile_loaded_workspace(tree, None, &HashMap::new());
        assert_eq!(
            window_leaves(&out, 0).len(),
            2,
            "no focus to validate ⇒ tree preserved"
        );
    }

    /// Regression: a multi-window workspace must NOT alias its non-active
    /// windows onto the focused pane. The focused pane is a leaf of window 0
    /// only; window 1 references a different terminal and must keep it (the
    /// "open vim in one window, it shows in the other" bug, where the
    /// per-window foreign-discard rewrote every non-active window to
    /// `single(focus)`).
    #[test]
    fn reconcile_multi_window_does_not_alias_non_active_windows() {
        let ws = Workspace {
            windows: vec![
                crate::layout::WindowState {
                    name: "1".to_owned(),
                    state: LayoutState::single(tid(1)),
                },
                crate::layout::WindowState {
                    name: "2".to_owned(),
                    state: LayoutState::single(tid(2)),
                },
            ],
            active: 0,
        };
        // Focus is on window 0's pane (tid 1); window 1 (tid 2) is non-active.
        let out = super::reconcile_loaded_workspace(ws, Some(&tid(1)), &HashMap::new());
        assert_eq!(out.windows.len(), 2, "both windows survive");
        assert_eq!(window_leaves(&out, 0), vec![tid(1)]);
        assert_eq!(
            window_leaves(&out, 1),
            vec![tid(2)],
            "non-active window keeps its own terminal, not aliased onto the focus"
        );
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
        let _ = drive_output_seq(out, layout, focused, panes, terminal_id, bytes, 1);
    }

    /// Like [`drive_output`] but stamps an explicit `seq` and returns the
    /// [`FrameOutcome`] so ack-emission tests can inspect `outcome.ack`.
    fn drive_output_seq(
        out: &mut Vec<u8>,
        layout: &mut Workspace,
        focused: &mut Option<TerminalId>,
        panes: &mut HashMap<TerminalId, PaneSlot>,
        terminal_id: &TerminalId,
        bytes: &[u8],
        seq: u64,
    ) -> FrameOutcome {
        drive_output_seq_with_viewport(
            out,
            layout,
            focused,
            panes,
            terminal_id,
            bytes,
            seq,
            (80, 24),
        )
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "test driver mirrors frame inputs"
    )]
    fn drive_output_seq_with_viewport(
        out: &mut Vec<u8>,
        layout: &mut Workspace,
        focused: &mut Option<TerminalId>,
        panes: &mut HashMap<TerminalId, PaneSlot>,
        terminal_id: &TerminalId,
        bytes: &[u8],
        seq: u64,
        viewport_dims: (u16, u16),
    ) -> FrameOutcome {
        let mut session_name = String::new();
        let mut zoomed: Option<TerminalId> = None;
        let mut predict = PredictionState::new(
            PredictiveConfig::disabled(),
            viewport_dims.0,
            viewport_dims.1,
        );
        let overlay = Overlay;
        let mut pending_splits = HashMap::new();
        let mut pending_windows = HashMap::new();
        handle_server_frame(
            out,
            FrameKind::TerminalOutput {
                terminal_id: terminal_id.clone(),
                seq,
                bytes: bytes::Bytes::copy_from_slice(bytes),
            },
            panes,
            layout,
            focused,
            &mut zoomed,
            &mut session_name,
            None,
            None,
            viewport_dims,
            &mut predict,
            &overlay,
            None,
            &mut pending_splits,
            &mut pending_windows,
            &mut AgentMetaIndex::default(),
            false,
            false,
        )
        .expect("handle_server_frame")
    }

    /// phux-ih39: a `TERMINAL_OUTPUT` that races ahead of
    /// `TERMINAL_SNAPSHOT` must allocate the pane mirror at the current viewport width before
    /// `vt_write`, not at the historical 80x24 placeholder. Absolute cursor
    /// movement past column 80 is a compact regression oracle: if the slot
    /// starts 80-wide, the `X` cannot land in column 100.
    #[test]
    fn output_before_snapshot_uses_current_viewport_width() {
        let pane = tid(1);
        let mut layout = Workspace::single(pane.clone());
        let mut focused = Some(pane.clone());
        let mut panes = HashMap::new();
        let mut out: Vec<u8> = Vec::new();

        drive_output_seq_with_viewport(
            &mut out,
            &mut layout,
            &mut focused,
            &mut panes,
            &pane,
            b"\x1b[1;100HX",
            1,
            (120, 30),
        );

        let slot = panes.get_mut(&pane).expect("slot allocated");
        assert_eq!(slot.terminal.cols().expect("cols"), 120);
        assert_eq!(slot.terminal.rows().expect("rows"), 30);
        let cell = slot
            .renderer
            .read_grapheme_at(&slot.terminal, 0, 99)
            .expect("read cell");
        assert_eq!(cell, Some('X'));
    }

    #[test]
    fn synchronized_output_paints_only_after_end_across_frames() {
        let pane = tid(1);
        let mut layout = Workspace::single(pane.clone());
        let mut focused = Some(pane.clone());
        let mut panes = panes_for(&[&pane]);
        let mut out = Vec::new();

        drive_output(
            &mut out,
            &mut layout,
            &mut focused,
            &mut panes,
            &pane,
            b"\x1b[?2026hhalf-drawn",
        );
        assert!(out.is_empty(), "begin/body must update only the mirror");
        assert!(panes[&pane].sync_output_since.is_some());

        drive_output(
            &mut out,
            &mut layout,
            &mut focused,
            &mut panes,
            &pane,
            b" frame\x1b[?2026l",
        );
        assert!(!out.is_empty(), "end must publish the completed frame");
        assert!(panes[&pane].sync_output_since.is_none());
        let printable = strip_csi(&String::from_utf8_lossy(&out));
        assert!(printable.contains("half-drawn frame"));
    }

    #[test]
    fn snapshot_during_synchronized_output_waits_for_live_end() {
        let pane = tid(1);
        let mut layout = Workspace::single(pane.clone());
        let mut focused = Some(pane.clone());
        let mut panes = panes_for(&[&pane]);
        let mut out = Vec::new();

        drive_output(
            &mut out,
            &mut layout,
            &mut focused,
            &mut panes,
            &pane,
            b"\x1b[?2026hpartial",
        );
        drive_snapshot(
            &mut out,
            &mut layout,
            &mut focused,
            &mut panes,
            &pane,
            80,
            24,
            b"\x1b[!p\x1b[2J\x1b[Hstable snapshot",
            (80, 24),
        );
        assert!(out.is_empty(), "snapshot must not break the transaction");
        assert!(panes[&pane].sync_output_since.is_some());

        drive_output(
            &mut out,
            &mut layout,
            &mut focused,
            &mut panes,
            &pane,
            b"\x1b[?2026l",
        );
        assert!(!out.is_empty());
        assert!(panes[&pane].sync_output_since.is_none());
    }

    /// phux-ih39: the ATTACHED graph already carries per-pane dimensions.
    /// Seed slots from that graph so output between ATTACHED and
    /// `TERMINAL_SNAPSHOT` doesn't get interpreted at 80x24.
    #[test]
    fn attached_seeds_pane_slots_from_snapshot_dimensions() {
        let pane = tid(1);
        let window = WindowId::new(1);
        let session = SessionId::new(1);
        let snapshot = SessionSnapshot::new(session, window, pane.clone())
            .with_panes(vec![TerminalInfo::new(pane.clone(), window, 132, 43)]);
        let mut panes = HashMap::new();
        let mut workspace = Workspace::default();
        let mut focused = None;
        let mut zoomed: Option<TerminalId> = None;
        let mut session_name = String::new();
        let mut predict = PredictionState::new(PredictiveConfig::disabled(), 132, 43);
        let overlay = Overlay;
        let mut pending_splits = HashMap::new();
        let mut pending_windows = HashMap::new();
        let mut out: Vec<u8> = Vec::new();

        handle_server_frame(
            &mut out,
            FrameKind::Attached {
                snapshot,
                initial_client_id: ClientId::new(1),
            },
            &mut panes,
            &mut workspace,
            &mut focused,
            &mut zoomed,
            &mut session_name,
            None,
            None,
            (132, 43),
            &mut predict,
            &overlay,
            None,
            &mut pending_splits,
            &mut pending_windows,
            &mut AgentMetaIndex::default(),
            false,
            false,
        )
        .expect("attached");

        let slot = panes.get_mut(&pane).expect("slot seeded");
        assert_eq!(slot.terminal.cols().expect("cols"), 132);
        assert_eq!(slot.terminal.rows().expect("rows"), 43);
    }

    /// phux-3uv: a `TERMINAL_OUTPUT` with a non-zero `seq` yields an
    /// `ack` outcome carrying that frame's `(terminal_id, seq)`, so the
    /// driver sends a cumulative `FRAME_ACK`. The ack is set regardless of
    /// focus — the bytes are applied to the pane mirror before any render
    /// branch — so we drive a NON-focused pane and still expect the ack.
    #[test]
    fn terminal_output_yields_frame_ack_outcome() {
        let left = tid(1);
        let right = tid(2);
        let mut layout = two_pane_workspace(&left, &right, &left);
        let mut focused = Some(left.clone());
        let mut panes = panes_for(&[&left, &right]);

        let mut out: Vec<u8> = Vec::new();
        let outcome = drive_output_seq(
            &mut out,
            &mut layout,
            &mut focused,
            &mut panes,
            &right,
            b"hi",
            7,
        );
        assert_eq!(
            outcome.ack,
            Some((right.clone(), 7)),
            "non-zero seq must ack the delivering terminal's seq",
        );
    }

    /// phux-3uv: `seq == 0` is the server's "empty initial frame"
    /// sentinel; the client must NOT ack it (acking 0 is meaningless and
    /// would be a no-op against the server's `last_acked_seq == 0`).
    #[test]
    fn terminal_output_seq_zero_is_not_acked() {
        let left = tid(1);
        let right = tid(2);
        let mut layout = two_pane_workspace(&left, &right, &left);
        let mut focused = Some(left.clone());
        let mut panes = panes_for(&[&left, &right]);

        let mut out: Vec<u8> = Vec::new();
        let outcome = drive_output_seq(
            &mut out,
            &mut layout,
            &mut focused,
            &mut panes,
            &right,
            b"hi",
            0,
        );
        assert_eq!(outcome.ack, None, "seq=0 sentinel must not be acked");
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

    #[allow(
        clippy::too_many_arguments,
        reason = "test driver mirrors frame inputs"
    )]
    fn drive_snapshot(
        out: &mut Vec<u8>,
        layout: &mut Workspace,
        focused: &mut Option<TerminalId>,
        panes: &mut HashMap<TerminalId, PaneSlot>,
        terminal_id: &TerminalId,
        cols: u16,
        rows: u16,
        vt_replay_bytes: &[u8],
        viewport_dims: (u16, u16),
    ) -> FrameOutcome {
        let mut session_name = String::new();
        let mut zoomed: Option<TerminalId> = None;
        let mut predict = PredictionState::new(
            PredictiveConfig::disabled(),
            viewport_dims.0,
            viewport_dims.1,
        );
        let overlay = Overlay;
        let mut pending_splits = HashMap::new();
        let mut pending_windows = HashMap::new();
        handle_server_frame(
            out,
            FrameKind::TerminalSnapshot {
                terminal_id: terminal_id.clone(),
                cols,
                rows,
                vt_replay_bytes: vt_replay_bytes.to_vec(),
                scrollback_bytes: None,
            },
            panes,
            layout,
            focused,
            &mut zoomed,
            &mut session_name,
            None,
            None,
            viewport_dims,
            &mut predict,
            &overlay,
            None,
            &mut pending_splits,
            &mut pending_windows,
            &mut AgentMetaIndex::default(),
            false,
            false,
        )
        .expect("handle_server_frame")
    }

    /// phux-paer: on re-attach the server sends a `TERMINAL_SNAPSHOT` per
    /// pane; a NON-focused pane's snapshot must paint into its rect, or the
    /// pane renders blank while input still routes — the "screens wiped but
    /// still typable" report. The symmetric counterpart to
    /// [`non_focused_pane_repaints_on_output`].
    #[test]
    fn non_focused_pane_repaints_on_snapshot() {
        let left = tid(1);
        let right = tid(2);
        let mut layout = two_pane_workspace(&left, &right, &left);
        let mut focused = Some(left.clone());
        let mut panes = panes_for(&[&left, &right]);

        let mut out: Vec<u8> = Vec::new();
        drive_snapshot(
            &mut out,
            &mut layout,
            &mut focused,
            &mut panes,
            &right,
            39,
            24,
            b"hello",
            (80, 24),
        );

        let s = String::from_utf8_lossy(&out);
        // Same geometry as the output test: 80-col / 0.5 split ⇒ right pane
        // origin at 0-based col 41 ⇒ 1-based CUP `;42H`.
        assert!(
            s.contains(";42H"),
            "expected CUP into right pane origin (col 42); out = {s:?}"
        );
        let visible = strip_csi(&s);
        assert!(
            visible.contains("hello"),
            "non-focused pane snapshot should render its glyphs; visible = {visible:?}, raw = {s:?}"
        );
    }

    /// The focused pane's snapshot still renders into its own rect — guards
    /// against the phux-paer non-focused branch regressing the focused path.
    #[test]
    fn focused_pane_repaints_on_snapshot() {
        let left = tid(1);
        let right = tid(2);
        let mut layout = two_pane_workspace(&left, &right, &left);
        let mut focused = Some(left.clone());
        let mut panes = panes_for(&[&left, &right]);

        let mut out: Vec<u8> = Vec::new();
        drive_snapshot(
            &mut out,
            &mut layout,
            &mut focused,
            &mut panes,
            &left,
            39,
            24,
            b"world",
            (80, 24),
        );

        let s = String::from_utf8_lossy(&out);
        assert!(
            s.contains("\x1b[1;1H"),
            "expected CUP into left pane origin (col 1); out = {s:?}"
        );
        let visible = strip_csi(&s);
        assert!(
            visible.contains("world"),
            "focused pane snapshot should render its glyphs; visible = {visible:?}, raw = {s:?}"
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

    /// phux-flywheel: the apply-vs-paint split is observable. Driving a
    /// `TERMINAL_OUTPUT` for the focused pane under a debug-level capturing
    /// subscriber must close BOTH child spans — `vt_apply` (libghostty
    /// parse) and `paint_trigger` (render) — so a trace can attribute
    /// client lag to apply-ms vs paint-ms separately. We assert on
    /// span-close events (the parse + render each report their own busy
    /// time) rather than the fused parent `handle_server_frame` close.
    #[test]
    fn output_emits_separate_apply_and_paint_spans() {
        use std::sync::Arc;
        use tracing_subscriber::fmt::MakeWriter;
        use tracing_subscriber::layer::SubscriberExt as _;
        use tracing_subscriber::{Registry, fmt};

        #[derive(Clone, Default)]
        struct Buf(Arc<Mutex<Vec<u8>>>);
        impl std::io::Write for Buf {
            fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
                self.0.lock().expect("lock").extend_from_slice(b);
                Ok(b.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        impl<'a> MakeWriter<'a> for Buf {
            type Writer = Self;
            fn make_writer(&'a self) -> Self::Writer {
                self.clone()
            }
        }

        let _guard = TRACE_TEST_LOCK.lock().expect("trace test lock");

        let buf = Buf::default();
        let layer = fmt::layer()
            .with_ansi(false)
            .with_writer(buf.clone())
            .with_span_events(fmt::format::FmtSpan::CLOSE);
        let subscriber = Registry::default().with(layer);

        {
            tracing::subscriber::set_global_default(subscriber)
                .expect("install test tracing subscriber");
            tracing_core::callsite::rebuild_interest_cache();
            let left = tid(1);
            let right = tid(2);
            let mut layout = two_pane_workspace(&left, &right, &left);
            let mut focused = Some(left.clone());
            let mut panes = panes_for(&[&left, &right]);
            let mut out: Vec<u8> = Vec::new();
            // Drive the focused pane so the paint trigger fires.
            drive_output(
                &mut out,
                &mut layout,
                &mut focused,
                &mut panes,
                &left,
                b"hi",
            );
        }

        let log = String::from_utf8(buf.0.lock().expect("lock").clone()).expect("utf8");
        // Both child spans must have closed (FmtSpan::CLOSE prints a
        // `close` line carrying `time.busy` per span name).
        assert!(
            log.contains("vt_apply"),
            "vt_apply span never closed; log:\n{log}"
        );
        assert!(
            log.contains("paint_trigger"),
            "paint_trigger span never closed; log:\n{log}"
        );
        // And the parent fused span is still present (apply+paint).
        assert!(
            log.contains("handle_server_frame"),
            "parent span missing; log:\n{log}"
        );
    }

    /// A `Bell` frame routes a BEL byte through the injected sink, so a
    /// headless capture (and a future agent surface) can observe it.
    #[test]
    fn bell_frame_writes_bel_to_sink() {
        let mut layout = Workspace::single(tid(1));
        let mut focused = Some(tid(1));
        let mut zoomed: Option<TerminalId> = None;
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
            &mut zoomed,
            &mut session_name,
            None,
            None,
            (80, 24),
            &mut predict,
            &overlay,
            None,
            &mut pending_splits,
            &mut pending_windows,
            &mut AgentMetaIndex::default(),
            false,
            false,
        )
        .expect("handle_server_frame");

        assert_eq!(&out, b"\x07", "bell must emit a single BEL byte");
    }

    /// Drive a `TERMINAL_CLOSED { terminal_id, exit_status }` through
    /// [`handle_server_frame`] and return the resulting [`FrameOutcome`]
    /// so the consumer-side detach policy (phux-4r1) can be asserted.
    fn drive_closed(
        layout: &mut Workspace,
        focused: &mut Option<TerminalId>,
        panes: &mut HashMap<TerminalId, PaneSlot>,
        terminal_id: &TerminalId,
        exit_status: Option<i32>,
    ) -> FrameOutcome {
        let mut out: Vec<u8> = Vec::new();
        let mut session_name = String::new();
        let mut zoomed: Option<TerminalId> = None;
        let mut predict = PredictionState::new(PredictiveConfig::disabled(), 80, 24);
        let overlay = Overlay;
        let mut pending_splits = HashMap::new();
        let mut pending_windows = HashMap::new();
        handle_server_frame(
            &mut out,
            FrameKind::TerminalClosed {
                terminal_id: terminal_id.clone(),
                exit_status,
            },
            panes,
            layout,
            focused,
            &mut zoomed,
            &mut session_name,
            None,
            None,
            (80, 24),
            &mut predict,
            &overlay,
            None,
            &mut pending_splits,
            &mut pending_windows,
            &mut AgentMetaIndex::default(),
            false,
            false,
        )
        .expect("handle_server_frame")
    }

    /// phux-4r1: the detach policy is consumer-owned. When the LAST pane
    /// closes there is nothing left to render or route input to, so the
    /// TUI detaches itself — the `TerminalClosed` arm returns
    /// `FrameOutcome { exit: true }`. This is the consumer-side half of
    /// the EOF reshape: the server emits `TERMINAL_CLOSED` (an L1
    /// lifecycle fact) and the client decides to leave.
    #[test]
    fn last_pane_closed_detaches_the_client() {
        let pane = tid(1);
        let mut workspace = Workspace::single(pane.clone());
        let mut focused = Some(pane.clone());
        let mut panes = panes_for(&[&pane]);

        let outcome = drive_closed(&mut workspace, &mut focused, &mut panes, &pane, Some(0));

        assert!(
            outcome.exit,
            "closing the only pane must make the consumer detach (exit: true)",
        );
        assert!(
            workspace.windows.is_empty(),
            "the workspace must have no windows left after the last pane closes",
        );
        assert!(
            !panes.contains_key(&pane),
            "the closed pane's slot must be dropped",
        );
    }

    /// Drive an `EVENT { terminal, Asked }` through [`handle_server_frame`]
    /// and return the outcome (phux-foz.1 / ADR-0035).
    fn drive_asked(
        layout: &mut Workspace,
        focused: &mut Option<TerminalId>,
        panes: &mut HashMap<TerminalId, PaneSlot>,
        terminal_id: &TerminalId,
    ) -> FrameOutcome {
        use phux_protocol::wire::frame::AgentEvent;
        let mut out: Vec<u8> = Vec::new();
        let mut session_name = String::new();
        let mut zoomed: Option<TerminalId> = None;
        let mut predict = PredictionState::new(PredictiveConfig::disabled(), 80, 24);
        let overlay = Overlay;
        let mut pending_splits = HashMap::new();
        let mut pending_windows = HashMap::new();
        let mut agent_meta = AgentMetaIndex::default();
        handle_server_frame(
            &mut out,
            FrameKind::Event {
                terminal: Some(terminal_id.clone()),
                event: AgentEvent::Asked {
                    id: "q1".to_owned(),
                    question: "deploy to prod?".to_owned(),
                    suggestions: vec!["yes".to_owned(), "no".to_owned()],
                    elapsed_seconds: None,
                },
            },
            panes,
            layout,
            focused,
            &mut zoomed,
            &mut session_name,
            None,
            None,
            (80, 24),
            &mut predict,
            &overlay,
            None,
            &mut pending_splits,
            &mut pending_windows,
            &mut agent_meta,
            false,
            false,
        )
        .expect("handle_server_frame")
    }

    /// phux-foz.1: an ADR-0035 `Asked` event raises the pane's attention
    /// flag and asks the driver to repaint the chrome — including for a
    /// NON-focused pane (the whole point is surfacing a question the user
    /// is not looking at).
    #[test]
    fn asked_event_sets_attention_and_dirties_chrome() {
        let left = tid(1);
        let right = tid(2);
        let mut layout = two_pane_workspace(&left, &right, &left);
        let mut focused = Some(left.clone());
        let mut panes = panes_for(&[&left, &right]);

        let outcome = drive_asked(&mut layout, &mut focused, &mut panes, &right);

        assert!(
            panes.get(&right).expect("slot").attention,
            "the asking pane's attention flag must raise"
        );
        assert!(
            !panes.get(&left).expect("slot").attention,
            "the other pane stays quiet"
        );
        assert!(outcome.chrome_dirty, "the chrome must repaint");
    }

    /// phux-foz.1: a repeated `Asked` while the flag is already up changes
    /// no visible state, so it must not request another repaint.
    #[test]
    fn repeated_asked_event_does_not_redirty_chrome() {
        let pane = tid(1);
        let mut layout = Workspace::single(pane.clone());
        let mut focused = Some(pane.clone());
        let mut panes = panes_for(&[&pane]);

        let first = drive_asked(&mut layout, &mut focused, &mut panes, &pane);
        assert!(first.chrome_dirty);
        let second = drive_asked(&mut layout, &mut focused, &mut panes, &pane);
        assert!(
            !second.chrome_dirty,
            "an already-flagged pane must not force a repaint"
        );
        assert!(panes.get(&pane).expect("slot").attention, "flag stays up");
    }

    /// phux-foz.1: an `Asked` for a pane with no slot yet (it can precede
    /// the first snapshot) is dropped without a repaint, mirroring the
    /// early-`TerminalControl` policy.
    #[test]
    fn asked_event_for_unknown_pane_is_dropped() {
        let known = tid(1);
        let unknown = tid(9);
        let mut layout = Workspace::single(known.clone());
        let mut focused = Some(known.clone());
        let mut panes = panes_for(&[&known]);

        let outcome = drive_asked(&mut layout, &mut focused, &mut panes, &unknown);

        assert!(!outcome.chrome_dirty, "no slot, nothing to repaint");
        assert!(
            !panes.contains_key(&unknown),
            "no slot is allocated for an event-only pane"
        );
    }

    /// phux-foz.4: drive one agent event through [`handle_server_frame`]
    /// with minimal single-pane scaffolding; returns the outcome.
    fn drive_event(
        panes: &mut HashMap<TerminalId, PaneSlot>,
        terminal_id: &TerminalId,
        event: phux_protocol::wire::frame::AgentEvent,
    ) -> FrameOutcome {
        let mut layout = Workspace::single(terminal_id.clone());
        let mut focused = Some(terminal_id.clone());
        let mut out: Vec<u8> = Vec::new();
        let mut session_name = String::new();
        let mut zoomed: Option<TerminalId> = None;
        let mut predict = PredictionState::new(PredictiveConfig::disabled(), 80, 24);
        let overlay = Overlay;
        let mut pending_splits = HashMap::new();
        let mut pending_windows = HashMap::new();
        let mut agent_meta = AgentMetaIndex::default();
        handle_server_frame(
            &mut out,
            FrameKind::Event {
                terminal: Some(terminal_id.clone()),
                event,
            },
            panes,
            &mut layout,
            &mut focused,
            &mut zoomed,
            &mut session_name,
            None,
            None,
            (80, 24),
            &mut predict,
            &overlay,
            None,
            &mut pending_splits,
            &mut pending_windows,
            &mut agent_meta,
            false,
            false,
        )
        .expect("handle_server_frame")
    }

    /// phux-foz.4: a `cwd_changed` event lands in the pane's slot and
    /// dirties the chrome; repeating the same directory is a no-op.
    #[test]
    fn cwd_changed_event_updates_slot_and_coalesces() {
        use phux_protocol::wire::frame::AgentEvent;
        let pane = tid(1);
        let mut panes = panes_for(&[&pane]);

        let first = drive_event(
            &mut panes,
            &pane,
            AgentEvent::CwdChanged {
                cwd: "/tmp/work".to_owned(),
            },
        );
        assert!(first.chrome_dirty, "a new cwd must repaint the chrome");
        assert_eq!(
            panes.get(&pane).expect("slot").cwd.as_deref(),
            Some("/tmp/work")
        );

        let repeat = drive_event(
            &mut panes,
            &pane,
            AgentEvent::CwdChanged {
                cwd: "/tmp/work".to_owned(),
            },
        );
        assert!(!repeat.chrome_dirty, "unchanged cwd must not repaint");
    }

    /// phux-foz.4: a `command_finished` event records the exit code (and a
    /// later code replaces it); an unchanged value is a no-op.
    #[test]
    fn command_finished_event_records_last_exit() {
        use phux_protocol::wire::frame::AgentEvent;
        let pane = tid(1);
        let mut panes = panes_for(&[&pane]);
        assert_eq!(panes.get(&pane).expect("slot").last_exit, None);

        let first = drive_event(
            &mut panes,
            &pane,
            AgentEvent::CommandFinished { exit_code: Some(0) },
        );
        assert!(first.chrome_dirty);
        assert_eq!(panes.get(&pane).expect("slot").last_exit, Some(0));

        let repeat = drive_event(
            &mut panes,
            &pane,
            AgentEvent::CommandFinished { exit_code: Some(0) },
        );
        assert!(!repeat.chrome_dirty, "same code must not repaint");

        let failed = drive_event(
            &mut panes,
            &pane,
            AgentEvent::CommandFinished {
                exit_code: Some(127),
            },
        );
        assert!(failed.chrome_dirty);
        assert_eq!(panes.get(&pane).expect("slot").last_exit, Some(127));
    }

    /// phux-foz.4: cwd/exit events for a pane with no slot yet are dropped
    /// without a repaint, mirroring the early-`TerminalControl` policy.
    #[test]
    fn cwd_and_exit_events_for_unknown_pane_are_dropped() {
        use phux_protocol::wire::frame::AgentEvent;
        let known = tid(1);
        let unknown = tid(9);
        let mut panes = panes_for(&[&known]);

        let cwd = drive_event(
            &mut panes,
            &unknown,
            AgentEvent::CwdChanged {
                cwd: "/x".to_owned(),
            },
        );
        let exit = drive_event(
            &mut panes,
            &unknown,
            AgentEvent::CommandFinished { exit_code: Some(1) },
        );
        assert!(!cwd.chrome_dirty && !exit.chrome_dirty);
        assert!(!panes.contains_key(&unknown));
    }

    /// phux-4r1: closing one of several panes is NOT a detach. The
    /// survivor stays attached — the `TerminalClosed` arm folds the
    /// closed leaf out, re-anchors focus, and asks for a repaint +
    /// reflow + broadcast, with `exit: false`.
    #[test]
    fn closing_one_of_several_panes_keeps_the_client_attached() {
        let left = tid(1);
        let right = tid(2);
        let mut workspace = two_pane_workspace(&left, &right, &left);
        let mut focused = Some(left.clone());
        let mut panes = panes_for(&[&left, &right]);

        let outcome = drive_closed(&mut workspace, &mut focused, &mut panes, &left, Some(0));

        assert!(
            !outcome.exit,
            "a surviving pane means the client stays attached (exit: false)",
        );
        assert_eq!(
            workspace.windows.len(),
            1,
            "the window survives with the remaining pane",
        );
        assert_eq!(
            focused,
            Some(right),
            "focus re-anchors onto the surviving leaf",
        );
        assert!(
            outcome.layout_replaced && outcome.emit_set_metadata && outcome.reflow_panes,
            "the fold triggers repaint + sibling broadcast + survivor reflow",
        );
    }

    /// ADR-0040: drive one frame through [`handle_server_frame`] with a
    /// caller-owned [`AgentMetaIndex`], for the agent-metadata arms.
    fn drive_meta_frame(frame: FrameKind, agent_meta: &mut AgentMetaIndex) -> FrameOutcome {
        let pane = tid(1);
        let mut layout = Workspace::single(pane.clone());
        let mut focused = Some(pane);
        let mut panes: HashMap<TerminalId, PaneSlot> = HashMap::new();
        let mut out: Vec<u8> = Vec::new();
        let mut session_name = String::new();
        let mut zoomed: Option<TerminalId> = None;
        let mut predict = PredictionState::new(PredictiveConfig::disabled(), 80, 24);
        let overlay = Overlay;
        let mut pending_splits = HashMap::new();
        let mut pending_windows = HashMap::new();
        handle_server_frame(
            &mut out,
            frame,
            &mut panes,
            &mut layout,
            &mut focused,
            &mut zoomed,
            &mut session_name,
            None,
            None,
            (80, 24),
            &mut predict,
            &overlay,
            None,
            &mut pending_splits,
            &mut pending_windows,
            agent_meta,
            false,
            false,
        )
        .expect("handle_server_frame")
    }

    /// ADR-0040: a subscribed `phux.agent/v1` broadcast decodes into the
    /// index and flags the chrome refresh; the tombstone (DELETE) clears
    /// the record so labels fall back to the OSC-title path.
    #[test]
    fn agent_metadata_broadcast_updates_index_and_tombstone_clears_it() {
        use phux_protocol::wire::frame::{Scope, TERMINAL_AGENT_KEY};
        let pane = tid(1);
        let mut agent_meta = AgentMetaIndex::default();

        let outcome = drive_meta_frame(
            FrameKind::MetadataChanged {
                scope: Scope::Terminal(pane.clone()),
                key: TERMINAL_AGENT_KEY.to_owned(),
                value: Some(br#"{"name":"reviewer","state":"blocked"}"#.to_vec()),
            },
            &mut agent_meta,
        );
        assert!(outcome.agent_meta_changed, "a new record must flag chrome");
        let record = agent_meta.records.get(&pane).expect("record stored");
        assert_eq!(record.name, "reviewer");
        assert_eq!(record.state, crate::agent_meta::AgentMetaState::Blocked);

        // Re-asserting the identical record is a no-op (no repaint churn).
        let outcome = drive_meta_frame(
            FrameKind::MetadataChanged {
                scope: Scope::Terminal(pane.clone()),
                key: TERMINAL_AGENT_KEY.to_owned(),
                value: Some(br#"{"name":"reviewer","state":"blocked"}"#.to_vec()),
            },
            &mut agent_meta,
        );
        assert!(
            !outcome.agent_meta_changed,
            "identical record must not flag"
        );

        // Tombstone (DELETE_METADATA) clears the record.
        let outcome = drive_meta_frame(
            FrameKind::MetadataChanged {
                scope: Scope::Terminal(pane.clone()),
                key: TERMINAL_AGENT_KEY.to_owned(),
                value: None,
            },
            &mut agent_meta,
        );
        assert!(outcome.agent_meta_changed, "a cleared record must flag");
        assert!(!agent_meta.records.contains_key(&pane));
    }

    /// ADR-0040: a `GET_METADATA` reply correlated through
    /// `AgentMetaIndex::pending` seeds the record for a pane whose agent
    /// declared itself before we attached; an absent key (`value: None`)
    /// resolves the pending entry without inventing a record.
    #[test]
    fn agent_metadata_get_reply_is_correlated_by_request_id() {
        let pane = tid(1);
        let mut agent_meta = AgentMetaIndex::default();
        agent_meta.pending.insert(77, pane.clone());

        let outcome = drive_meta_frame(
            FrameKind::MetadataValue {
                request_id: 77,
                value: Some(br#"{"name":"codex","kind":"codex","state":"working"}"#.to_vec()),
            },
            &mut agent_meta,
        );
        assert!(outcome.agent_meta_changed);
        assert!(agent_meta.pending.is_empty(), "pending entry consumed");
        assert_eq!(agent_meta.records.get(&pane).expect("record").name, "codex");

        agent_meta.pending.insert(78, pane);
        let outcome = drive_meta_frame(
            FrameKind::MetadataValue {
                request_id: 78,
                value: None,
            },
            &mut agent_meta,
        );
        assert!(outcome.agent_meta_changed, "absent key clears the record");
        assert!(agent_meta.records.is_empty());
    }

    /// ADR-0040: malformed record bytes (bad JSON, empty name) must read
    /// as "no declared agent" — never a stored record, never a crash.
    #[test]
    fn agent_metadata_rejects_malformed_records() {
        use phux_protocol::wire::frame::{Scope, TERMINAL_AGENT_KEY};
        let pane = tid(1);
        let mut agent_meta = AgentMetaIndex::default();
        let outcome = drive_meta_frame(
            FrameKind::MetadataChanged {
                scope: Scope::Terminal(pane),
                key: TERMINAL_AGENT_KEY.to_owned(),
                value: Some(b"not json at all".to_vec()),
            },
            &mut agent_meta,
        );
        assert!(!outcome.agent_meta_changed);
        assert!(agent_meta.records.is_empty());
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
