//! Input dispatcher: translates parser-emitted events into wire frames
//! or layout-action effects.
//!
//! Owns the resolver-intercept path (prefix chord → `ResolvedAction` →
//! mutate the active window of the `Workspace`), the predict overlay's
//! keystroke feed, and the parked-spawn bookkeeping (`PendingSplit` /
//! `PendingWindow`) that bridges a local `split-pane` / `new-window`
//! chord to its remote `SPAWN_TERMINAL` reply.

use std::collections::HashMap;

use libghostty_vt::terminal::{Mode, ScrollViewport};
use phux_protocol::TerminalId;
use phux_protocol::input::InputEvent;
use phux_protocol::input::mouse::{MouseAction, MouseButton, MouseEvent};
use phux_protocol::wire::frame::{
    Command, FrameKind, InputMode, SESSION_NAME_KEY, Scope, TerminalSignal,
};

use super::actions::{self, ActionError, PendingSplit, PendingWindow};
use super::connection::Connection;
use super::driver::{AttachError, DEFAULT_GROUP_ID, PaneSlot, layout_key};
use super::paint::{SidebarReservation, content_rect};
use super::plugin_actions::{PluginActionEntry, PluginRunResult};
use crate::layout::{Direction, SplitDir, Workspace};
use crate::predict::{Overlay, PredictionState};
use crate::render::Theme;
use crate::render::overlay::{
    HelpOverlay, OverlayOutcome, OverlayState, PromptOverlay, SelectItem, SelectList,
};

/// Mutable context the input-dispatch path needs to update on a chord
/// that resolves to a layout action (phux-4li.5). Bundles the items
/// that would otherwise inflate `dispatch_input_events`'s argument
/// list past clippy's threshold.
pub(super) struct DispatchCtx<'a> {
    /// Keybind resolver state. `None` when the on-disk config failed
    /// to parse; the dispatcher then forwards every key to the focused
    /// pane unchanged.
    pub resolver: Option<&'a mut phux_config::keybind::Resolver>,
    /// Client-side multi-window mirror. Pane actions operate on the
    /// active window ([`Workspace::active_window_mut`]); the whole
    /// workspace is what gets serialized to L3 on a `SET_METADATA`.
    pub workspace: &'a mut Workspace,
    /// Outer-viewport `(cols, rows)`. Used by `apply_resize` to convert
    /// `amount` (cells) to a ratio delta.
    pub viewport: (u16, u16),
    /// Monotonic source of new request ids. We don't currently issue
    /// per-action correlated requests (the only side-channel today is
    /// the layout `SET_METADATA`, which doesn't need a reply), but we
    /// reserve the counter for future `SPAWN`/kill wiring.
    pub next_request_id: &'a mut u32,
    /// phux-4li.12: parked split actions awaiting their
    /// `TERMINAL_SPAWNED` reply. `run_action` inserts;
    /// `handle_server_frame` removes.
    pub pending_splits: &'a mut HashMap<u32, PendingSplit>,
    /// phux-4li.15: parked `new-window` actions awaiting their
    /// `TERMINAL_SPAWNED` reply. Same lifecycle as `pending_splits`,
    /// keyed in the same request-id space.
    pub pending_windows: &'a mut HashMap<u32, PendingWindow>,
    /// phux-5ke.4: overlay stack. When non-empty the dispatcher routes
    /// key events to the active overlay (no resolver, no predict, no
    /// pane forwarding) and the `show-help` action pushes onto it.
    pub overlays: &'a mut OverlayState,
    /// phux-5ke.4: snapshot of the on-disk keybindings, captured at
    /// driver start. The help overlay reads this to render the modal
    /// body. `None` when config load failed (overlay still pushes but
    /// shows "no bindings configured").
    pub keybindings: Option<&'a phux_config::KeybindingsCfg>,
    /// phux-ahv.4: chrome + overlay color theme, resolved from
    /// `[theme]` config at driver start. Overlays snapshot it at
    /// construction (`show-help`, `rename-window`) so their painted
    /// colors flow from a single source of truth.
    pub theme: &'a Theme,
    /// phux-4li.20: the server's session graph, cached from the latest
    /// `ATTACHED` snapshot. The `session-picker` action builds its rows
    /// from this list. Empty until the first snapshot lands (picker then
    /// bells).
    pub sessions: &'a [phux_protocol::wire::info::SessionInfo],
    /// phux-4li.20: id of the session this client is attached to. The
    /// picker marks this row and excludes it from selection (switching
    /// to the current session is a no-op). `None` before the first
    /// snapshot.
    pub focused_session: Option<phux_protocol::ids::SessionId>,
    /// phux-eb0: the name of the session this client is attached to,
    /// resolved from the latest ATTACHED snapshot. A `switch-session`
    /// targeting this name is a no-op (guarded in
    /// [`apply_action_effects`]) even though the picker already excludes
    /// the current row. Empty before the first snapshot.
    ///
    /// Mutable so the `rename-session` action can optimistically update it
    /// the moment the user commits a rename: the client sends the
    /// `RENAME_SESSION` command and reflects the new name in its own status
    /// bar immediately, rather than waiting a round-trip. The server is
    /// authoritative — the next `ATTACHED` snapshot overwrites this with the
    /// server's value (and is how other attached clients learn the rename).
    pub session_name: &'a mut String,
    /// phux-eb0: out-channel for a committed `switch-session { name }`.
    /// `apply_action_effects` sets this to `Some(target)` when the user
    /// picks a peer session; the driver's `main_loop` reads it after the
    /// dispatch batch and returns `LoopExit::SwitchTo(target)` so the
    /// outer loop re-attaches. Cleared by the driver each iteration.
    pub switch_request: &'a mut Option<ReattachTarget>,
    /// phux-x2hm: the driver's pane-zoom state — `Some(id)` when pane `id`
    /// is zoomed to fill the window. `apply_action_effects` flips this for a
    /// `toggle-zoom` action; the driver reads it (via `Workspace::render_window`)
    /// to render/reflow the zoomed pane.
    pub zoomed: &'a mut Option<TerminalId>,
    /// phux-4h5a: the active sidebar reservation, or `None` when the sidebar is
    /// disabled. The `resize-pane` min-cell gate tiles into the inset content
    /// rect so the underflow check matches the width panes actually paint into
    /// when a sidebar is docked.
    pub sidebar: Option<SidebarReservation>,
    /// phux-4h5a: the driver's sidebar on/off state. `toggle-sidebar` flips
    /// this (via `ActionEffects::toggle_sidebar`); the driver re-folds it into
    /// the per-frame `sidebar` reservation after dispatch so the toggle repaint
    /// reflects the new state. Owned by the driver like `zoomed`.
    pub sidebar_enabled: &'a mut bool,
    /// Whether the status bar reserves its row this frame (`status_bar.is_some()`
    /// in the driver). Mouse routing folds this into the same
    /// `content_rect(viewport, has_bar, sidebar)` the paint path uses so a
    /// click hit-tests against the rects actually on screen.
    pub has_bar: bool,
    /// ADR-0035: the in-flight divider drag, or `None` when no divider is
    /// grabbed. A press on a divider cell records the grabbed split here;
    /// subsequent button-motion events re-tune that split's ratio from the
    /// pointer position; a release clears it. Owned by `main_loop` (it
    /// must survive across dispatch batches) and threaded in by reference.
    pub drag: &'a mut Option<DragGrab>,
    /// phux-r82.5: enabled plugins' manifest `[[actions]]`, snapshotted at
    /// driver start (same lifecycle as `keybindings`). The command palette
    /// appends one namespaced row per entry under a "Plugin" header.
    pub plugin_actions: &'a [PluginActionEntry],
    /// phux-r82.5: sender half of the driver's plugin-events channel. A
    /// dispatched `plugin-action` spawns the child-process run off the
    /// input loop and reports completion here; the driver's `select!`
    /// surfaces failures as a toast. `None` in unit tests (no runtime).
    pub plugin_tx: Option<&'a tokio::sync::mpsc::UnboundedSender<PluginRunResult>>,
}

/// An active divider drag (ADR-0035).
///
/// Press on a divider cell records the controlling split (`node_path`) and
/// its `axis`; while held, each button-motion event sets that split's
/// ratio so the divider tracks the pointer; release drops it. The grab is
/// keyed by split identity, not by cursor cell, so a fast drag that
/// outruns the divider still re-tunes the right split.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct DragGrab {
    /// Path to the grabbed [`crate::layout::LayoutNode::Split`].
    pub node_path: crate::layout::NodePath,
    /// The grabbed split's axis (drives x vs y of the pointer).
    pub axis: SplitDir,
}

/// Translate a batch of parser events into wire frames and ship them.
///
/// Detach actions short-circuit into a single `FrameKind::Detach` and
/// flip `detach_pending`. Pre-attach events (no `focused_pane` yet) are
/// dropped with a debug log — the wire spec has no "pre-attach buffer"
/// notion.
///
/// phux-4li.5: when a `KeyEvent` matches a configured keybind, the
/// chord is consumed by the dispatcher and the corresponding layout
/// action runs (focus move / resize / etc.). The key is NOT forwarded
/// to the focused pane in that case — same convention as tmux's
/// `prefix` table.
// arg list bundles transport + render + predict context; follow-up to
// refactor into a context struct.
#[allow(clippy::too_many_arguments, reason = "see comment above")]
#[allow(
    clippy::future_not_send,
    reason = "client-side libghostty Terminal is !Send; ADR-0003 binds us to current-thread"
)]
#[allow(
    clippy::too_many_lines,
    reason = "phux-4li.6 added the mouse-routing branch alongside resolver + predict + key forwarding; splitting would require carrying the connection + many mut locals through helpers"
)]
#[allow(
    clippy::cognitive_complexity,
    reason = "branch density rises with each input-event kind we route; same shape as the action-dispatch arm"
)]
pub(super) async fn dispatch_input_events<W: super::RenderSink>(
    out: &mut W,
    conn: &mut Connection,
    events: Vec<InputEvent>,
    focused_pane: &mut Option<TerminalId>,
    detach_pending: &mut bool,
    predict: &mut PredictionState,
    overlay: &Overlay,
    panes: &mut HashMap<TerminalId, PaneSlot>,
    ctx: &mut DispatchCtx<'_>,
) -> Result<bool, AttachError> {
    let mut predicted_any = false;
    let mut layout_changed = false;
    for ev in events {
        // phux-foz.2: the which-key popup is transparent to input. It is
        // dismissed by — and never consumes — the next event: a key press
        // pops it and then executes exactly as if the popup were absent
        // (the resolver still holds the pending prefix, so the chord
        // completes normally), except Esc, which pops it AND cancels the
        // pending prefix without reaching the pane. Mouse input pops it
        // and cancels the prefix too (a click is not a chord
        // continuation), then routes normally. Non-press key events and
        // paste/focus bypass the popup entirely (it stays up; they flow
        // to the pane) — the popup must never eat or delay real input.
        if ctx.overlays.top_is_passthrough() {
            use phux_protocol::input::key::{KeyAction, PhysicalKey};
            match &ev {
                InputEvent::Key(key_event) if matches!(key_event.action, KeyAction::Press) => {
                    ctx.overlays.dismiss();
                    layout_changed = true;
                    if key_event.key == PhysicalKey::Escape {
                        if let Some(resolver) = ctx.resolver.as_deref_mut() {
                            resolver.reset();
                        }
                        tracing::debug!("which-key: Esc cancelled the pending prefix");
                        continue;
                    }
                    // Fall through: the key executes as if no popup existed.
                }
                InputEvent::Mouse(_) => {
                    ctx.overlays.dismiss();
                    layout_changed = true;
                    if let Some(resolver) = ctx.resolver.as_deref_mut() {
                        resolver.reset();
                    }
                    // Fall through to normal mouse routing.
                }
                _ => {}
            }
        }
        // phux-5ke.4: while any overlay is active the stack captures all
        // input. Key events flow to `OverlayState::handle_key`, which
        // routes them to the *top* overlay (which may dismiss, popping
        // back to whatever is beneath it); mouse / paste / focus events
        // are dropped so they don't reach the pane underneath.
        //
        // The keybind resolver is bypassed entirely while an overlay is
        // up: the overlay owns every keystroke, exactly as tmux's command
        // prompt and menus consume the prefix key as literal input rather
        // than firing prefix bindings. This keeps a prefix chord (e.g. the
        // leader `C-a`) from being swallowed by the resolver before it can
        // reach the overlay — a name typed into the rename prompt that
        // starts with the leader key must land verbatim. Detach while a
        // modal is open is reachable by dismissing first (Esc), then
        // chording. The resolver is reset on entry so a partial chord begun
        // before the overlay opened cannot leak into post-dismiss input.
        //
        // phux-foz.2: a passthrough popup (which-key) is excluded — the
        // block above already dismissed it for presses/mouse, and events
        // it deliberately ignores (key release/repeat, paste, focus) must
        // flow to the pane, not be captured (and must NOT reset the
        // resolver, which is holding the pending prefix the popup shows).
        if ctx.overlays.is_active() && !ctx.overlays.top_is_passthrough() {
            if let InputEvent::Key(ref key_event) = ev {
                if let Some(resolver) = ctx.resolver.as_deref_mut() {
                    resolver.reset();
                }
                let was_active = ctx.overlays.is_active();
                // phux-ahv.1: an overlay may commit an action (e.g. the
                // rename prompt returning `rename-window { name }`); run
                // it through the same path as a keybinding.
                match ctx.overlays.handle_key(key_event) {
                    OverlayOutcome::RunAction(resolved) => {
                        let effects = run_action(&resolved, ctx, focused_pane.as_ref());
                        if apply_action_effects(
                            effects,
                            out,
                            conn,
                            ctx,
                            focused_pane,
                            detach_pending,
                            predict,
                            panes,
                        )
                        .await?
                        {
                            layout_changed = true;
                        }
                    }
                    OverlayOutcome::Copy(req) => {
                        // Copy-mode commit: resolve the selection against the
                        // focused pane's own engine and write it to the host
                        // clipboard via OSC 52. Client-local per ADR-0030 —
                        // no wire traffic.
                        if let Some(fid) = focused_pane.as_ref()
                            && let Some(slot) = panes.get(fid)
                        {
                            super::copy::copy_to_host_clipboard(out, &slot.terminal, req)?;
                        }
                    }
                    OverlayOutcome::ScrollViewport(delta) => {
                        if scroll_focused_pane_viewport(panes, focused_pane.as_ref(), delta) {
                            layout_changed = true;
                        }
                    }
                    OverlayOutcome::None => {
                        // Overlay consumed the key but nothing else to do.
                    }
                }
                // On dismiss, repaint everything: the overlay scribbled
                // over pane cells and we need a coherent base for the
                // next TERMINAL_OUTPUT.
                if was_active && !ctx.overlays.is_active() {
                    layout_changed = true;
                }
            } else if let InputEvent::Mouse(ref mouse) = ev {
                match ctx.overlays.handle_mouse(mouse) {
                    OverlayOutcome::Copy(req) => {
                        if let Some(fid) = focused_pane.as_ref()
                            && let Some(slot) = panes.get(fid)
                        {
                            super::copy::copy_to_host_clipboard(out, &slot.terminal, req)?;
                        }
                        layout_changed = true;
                    }
                    OverlayOutcome::ScrollViewport(delta) => {
                        if scroll_focused_pane_viewport(panes, focused_pane.as_ref(), delta) {
                            layout_changed = true;
                        }
                    }
                    OverlayOutcome::RunAction(resolved) => {
                        let effects = run_action(&resolved, ctx, focused_pane.as_ref());
                        if apply_action_effects(
                            effects,
                            out,
                            conn,
                            ctx,
                            focused_pane,
                            detach_pending,
                            predict,
                            panes,
                        )
                        .await?
                        {
                            layout_changed = true;
                        }
                    }
                    OverlayOutcome::None => {}
                }
            }
            continue;
        }
        // phux-4li.5: resolver intercept. Run BEFORE the predict layer
        // so a chord that resolves to e.g. `focus-direction` doesn't
        // leave a stale ghost overlay on the previous focused pane.
        if let InputEvent::Key(ref key_event) = ev
            && let Some(outcome) = consume_chord(ctx, key_event)
        {
            match outcome {
                ChordOutcome::Partial => {
                    // Still waiting on the next chord in a multi-chord
                    // sequence; absorb the byte and move on.
                    continue;
                }
                ChordOutcome::Resolved(resolved) => {
                    let effects = run_action(&resolved, ctx, focused_pane.as_ref());
                    if apply_action_effects(
                        effects,
                        out,
                        conn,
                        ctx,
                        focused_pane,
                        detach_pending,
                        predict,
                        panes,
                    )
                    .await?
                    {
                        layout_changed = true;
                    }
                    continue;
                }
            }
        }
        // phux-4li.6 / ADR-0035: INPUT_MOUSE routing + click-to-focus +
        // divider drag-to-resize. The parser emits mouse coordinates in
        // outer-viewport cells (treated as 1-px-per-cell f64 per SPEC
        // §9.2.1); we hit-test against the multi-pane composition's
        // `Rect`s. A press on a divider cell *grabs* the split that
        // divider controls; button-motion while grabbed re-tunes the
        // split's ratio so the divider tracks the cursor; release drops
        // the grab. A click in a pane forwards the event (with pane-local
        // coords) to that pane — so an inner TUI that turned mouse
        // tracking on still receives every pointer event over its own
        // cells (the divider cells are the only ones whose meaning the
        // client claims).
        if let InputEvent::Mouse(ref mouse) = ev {
            use super::multi_pane::{RouteDecision, route_mouse_event};
            // ADR-0035: a release ALWAYS ends any in-flight drag first,
            // regardless of where it lands — the cursor may have left the
            // divider cell mid-drag. The commit broadcasts the final
            // layout via SET_METADATA, the same persistence path the
            // keyboard resize uses, so other attached clients converge. A
            // release with no active drag falls through to normal routing
            // (an inner app may want it).
            if matches!(mouse.action, MouseAction::Release) && ctx.drag.is_some() {
                *ctx.drag = None;
                if let Some(session) = ctx.focused_session
                    && let Some(bytes) = encode_layout_or_log(ctx.workspace)
                {
                    let request_id = *ctx.next_request_id;
                    *ctx.next_request_id = ctx.next_request_id.wrapping_add(1);
                    conn.send(&FrameKind::SetMetadata {
                        request_id,
                        scope: Scope::Group(DEFAULT_GROUP_ID),
                        key: layout_key(session),
                        value: bytes,
                    })
                    .await?;
                }
                tracing::debug!("divider drag: released, broadcast layout");
                continue;
            }
            // While a divider is grabbed, motion re-tunes that split and
            // nothing reaches a pane. Press/other actions fall through.
            if let Some(grab) = ctx.drag.clone()
                && matches!(mouse.action, MouseAction::Motion)
            {
                if drag_resize(ctx, mouse, &grab) {
                    layout_changed = true;
                }
                continue;
            }
            // phux-fce4: the sidebar strip claims every pointer event over
            // its own cells BEFORE pane routing — its rows are hit targets,
            // not pane content. A left press resolves against the strip's
            // row model (`sidebar::hit_test`) and dispatches the mapped
            // action through the same `run_action` path a keybinding or
            // palette row uses: a window block commits `select-window`, the
            // `+ new` affordance `new-window`, and `= menu` the command
            // palette (the session/plugin menu). Everything else over the
            // strip (motion, non-left presses, blank rows, the separator
            // column) is consumed and dropped so it can never leak into a
            // pane whose rect does not contain it anyway.
            if let Some(res) = ctx.sidebar {
                let strip = super::paint::sidebar_rect(ctx.viewport, ctx.has_bar, res);
                let (cell_x, cell_y) = (quantize_cell(mouse.x), quantize_cell(mouse.y));
                if strip_contains(strip, cell_x, cell_y) {
                    if matches!(mouse.action, MouseAction::Press)
                        && mouse.button == MouseButton::Left
                        && let Some(resolved) =
                            sidebar_click_action(strip, ctx.workspace.windows.len(), cell_x, cell_y)
                    {
                        tracing::debug!(action = %resolved.action, "sidebar: click dispatched");
                        let effects = run_action(&resolved, ctx, focused_pane.as_ref());
                        if apply_action_effects(
                            effects,
                            out,
                            conn,
                            ctx,
                            focused_pane,
                            detach_pending,
                            predict,
                            panes,
                        )
                        .await?
                        {
                            layout_changed = true;
                        }
                    }
                    continue;
                }
            }
            // Hit-test against the SAME inset content rect the renderer tiles
            // into — status-bar row and sidebar columns folded off the outer
            // viewport. Routing against the full viewport instead disagrees with
            // what is painted: a click near a divider lands one row off (the
            // status bar) and, with a sidebar docked, one strip-width off in x,
            // so it focuses/forwards to the wrong pane. Clicks in the reserved
            // chrome miss every pane rect and become a Miss (dropped).
            let content = content_rect(ctx.viewport, ctx.has_bar, ctx.sidebar);
            // phux-jow6: hit-test against the RENDER layout, not the real
            // tiled tree. When a pane is zoomed (phux-x2hm) the render layout
            // is a single full-content leaf, so any click lands on the
            // visible zoomed pane instead of whichever hidden tiled pane sits
            // under the cursor. Compute the decision in a scope that drops the
            // borrowing `Cow` before the click-to-focus `active_window_mut()`
            // below needs the workspace mutably.
            let decision = {
                let Some(render_ls) = ctx.workspace.render_window(ctx.zoomed.as_ref()) else {
                    tracing::debug!("dropping mouse event: no active window");
                    continue;
                };
                route_mouse_event(&render_ls, content, ctx.viewport, mouse)
            };
            match decision {
                RouteDecision::Pane {
                    target,
                    pane_x,
                    pane_y,
                    focus_changed,
                } => {
                    if focus_changed {
                        if let Some(ls) = ctx.workspace.active_window_mut() {
                            ls.focus = Some(target.clone());
                        }
                        *focused_pane = Some(target.clone());
                        // Re-anchor predict to the clicked pane: drop the
                        // old pane's queue AND reset the cursor + viewport
                        // to the new pane, so a keystroke before the next
                        // reconcile echoes at the right place rather than
                        // the old pane's (mid-screen) coordinates (phux-7ry0).
                        super::driver::reanchor_predict_to_pane(predict, panes, &target);
                        // Heavy-edge chrome moves with focus; repaint
                        // dividers + all leaves so the focused pane's
                        // surrounding edges render heavy.
                        layout_changed = true;
                    }
                    let mut routed = *mouse;
                    routed.x = pane_x;
                    routed.y = pane_y;
                    if let Some(delta) = wheel_scroll_delta(&routed)
                        && let Some(slot) = panes.get_mut(&target)
                        && !terminal_wants_mouse_tracking(slot)
                    {
                        slot.terminal.scroll_viewport(ScrollViewport::Delta(delta));
                        layout_changed = true;
                        continue;
                    }
                    conn.send(&FrameKind::InputMouse {
                        terminal_id: target,
                        event: routed,
                    })
                    .await?;
                    continue;
                }
                RouteDecision::Divider { node_path, axis } => {
                    // ADR-0035: a LEFT-button press on a divider starts a drag
                    // and immediately snaps the split to the press position (so
                    // a click-without-motion still nudges, matching the
                    // intuitive "grab here"). Scroll-wheel and right/middle
                    // presses encode as Press too, but landing on a 1-cell
                    // divider must not snap the split — those, and stray
                    // grab-less motions, are dropped (the divider gap has no
                    // pane to forward to).
                    if matches!(mouse.action, MouseAction::Press)
                        && mouse.button == MouseButton::Left
                    {
                        let grab = DragGrab { node_path, axis };
                        if drag_resize(ctx, mouse, &grab) {
                            layout_changed = true;
                        }
                        *ctx.drag = Some(grab);
                        tracing::debug!("divider drag: grabbed");
                    } else {
                        tracing::trace!(x = mouse.x, y = mouse.y, "dropping mouse on divider");
                    }
                    continue;
                }
                RouteDecision::Miss => {
                    tracing::trace!(x = mouse.x, y = mouse.y, "dropping mouse: no target");
                    continue;
                }
                RouteDecision::NoFocus => {
                    tracing::debug!("dropping mouse event before ATTACHED");
                    continue;
                }
            }
        }

        // Predictive echo only fires for key events; mouse / paste / focus
        // intentionally bypass the prediction layer (they target the
        // server's input model, not the visual grid). The branch is
        // skipped entirely when the config flag is off — `predict_key`
        // returns `Disabled` and no overlay paint is scheduled.
        //
        // Arrows over a known cell on the current line (phux-9gw.1.3)
        // need a grid peek to know the width of the grapheme they step
        // over; we hand `read_grapheme_at` to the predict layer so it
        // can refuse the prediction when the cell is blank.
        //
        // phux-4li.6: peek the focused pane's grid via the active
        // window's focus. The driver also mirrors that id into its
        // `focused_pane` local (server-frame handlers rely on it);
        // either reads the same TerminalId here.
        if let InputEvent::Key(ref key_event) = ev
            && predict.is_enabled()
            && let Some(fid) = ctx.workspace.active_window().and_then(|w| w.focus.as_ref())
            && let Some(slot) = panes.get_mut(fid)
        {
            use crate::predict::PredictionOutcome;
            let outcome = predict.predict_key_with_grid(key_event, |r, c| {
                slot.renderer
                    .read_grapheme_at(&slot.terminal, r, c)
                    .ok()
                    .flatten()
            });
            if matches!(outcome, PredictionOutcome::Predicted) {
                predicted_any = true;
            }
        }
        // phux-4li.6: INPUT_KEY / INPUT_FOCUS / INPUT_PASTE all target
        // the client's focused pane (per ADR-0019 decision 6). Focus
        // is canonically the active window's focus; the driver-side
        // `focused_pane` mirror stays in sync for the render path.
        // When focus is unset (pre-ATTACHED), drop the event with a
        // debug log instead of panicking — wave-A's "always Some
        // post-ATTACHED" invariant is enforced by the seed in
        // `handle_server_frame`, but a stray input race during
        // bootstrap shouldn't take the loop down.
        let Some(pane) = ctx.workspace.active_window().and_then(|w| w.focus.as_ref()) else {
            tracing::debug!("dropping input received before ATTACHED");
            continue;
        };
        // phux-foz.1: forwarding key/paste input to a pane answers (or at
        // least engages) its pending agent question, so clear its asked
        // attention flag. Focus/mouse events don't clear — merely looking
        // at a pane is not answering it. A real transition schedules the
        // chrome repaint via `layout_changed`.
        if matches!(ev, InputEvent::Key(_) | InputEvent::Paste(_))
            && super::driver::clear_attention_on_input(panes, pane)
        {
            layout_changed = true;
        }
        let frame = ev.into_frame(pane.clone());
        conn.send(&frame).await?;
    }
    // Paint the prediction overlay once per dispatch batch so a burst of
    // keystrokes produces a single positioned write run, not one per
    // event. The overlay is a no-op on an empty queue. Predictions are
    // pane-local; shift them by the focused pane's render origin so a
    // non-top-left pane echoes over its own cells (phux-7ry0).
    if predicted_any {
        let origin = ctx
            .workspace
            .active_window()
            .and_then(|w| w.focus.as_ref())
            .and_then(|fid| panes.get(fid))
            .map_or((0, 0), |s| s.renderer.last_origin());
        let _ = overlay.render(predict, origin, out);
    }
    // Hand the layout-mutation signal back to `main_loop`, which holds
    // the status-bar painter and session name needed for a proper full
    // frame. We never paint from here.
    Ok(layout_changed)
}

fn wheel_scroll_delta(mouse: &MouseEvent) -> Option<isize> {
    if mouse.action != MouseAction::Press {
        return None;
    }
    match mouse.button {
        MouseButton::Four => Some(-3),
        MouseButton::Five => Some(3),
        _ => None,
    }
}

fn terminal_wants_mouse_tracking(slot: &PaneSlot) -> bool {
    [
        Mode::X10_MOUSE,
        Mode::NORMAL_MOUSE,
        Mode::BUTTON_MOUSE,
        Mode::ANY_MOUSE,
    ]
    .into_iter()
    .any(|mode| slot.terminal.mode(mode).unwrap_or(false))
}

fn scroll_focused_pane_viewport(
    panes: &mut HashMap<TerminalId, PaneSlot>,
    focused_pane: Option<&TerminalId>,
    delta: isize,
) -> bool {
    if delta == 0 {
        return false;
    }
    let Some(fid) = focused_pane else {
        return false;
    };
    let Some(slot) = panes.get_mut(fid) else {
        return false;
    };
    slot.terminal.scroll_viewport(ScrollViewport::Delta(delta));
    true
}

fn focused_pane_rect(
    ctx: &DispatchCtx<'_>,
    focused_pane: Option<&TerminalId>,
) -> crate::layout::Rect {
    focused_pane_rect_for(
        ctx.workspace,
        ctx.zoomed.as_ref(),
        focused_pane,
        ctx.viewport,
        ctx.has_bar,
        ctx.sidebar,
    )
}

fn focused_pane_rect_for(
    workspace: &Workspace,
    zoomed: Option<&TerminalId>,
    focused_pane: Option<&TerminalId>,
    viewport: (u16, u16),
    has_bar: bool,
    sidebar: Option<SidebarReservation>,
) -> crate::layout::Rect {
    let content = content_rect(viewport, has_bar, sidebar);
    let Some(fid) = focused_pane else {
        return content;
    };
    workspace
        .render_window(zoomed)
        .and_then(|layout| {
            crate::multi_pane::compute_layout_in(&layout, content, viewport)
                .rects
                .get(fid)
                .copied()
        })
        .unwrap_or(content)
}

/// Apply one drag step: re-tune the grabbed split so its divider tracks
/// `mouse`, returning `true` iff the layout changed (the caller repaints).
///
/// A pure mutation of the active window — no wire I/O (the `SET_METADATA`
/// broadcast happens once on release). Reuses [`actions::apply_divider_resize`]
/// so the drag, the keybind resize, and the persisted layout all run the
/// same `MIN_PANE_CELL` floor + `clamp_ratio` math. The pointer is
/// quantised to an outer-viewport cell exactly as the hit-test does.
/// `Ok(None)` from the resize (min-cell floor hit, or a stale grab whose
/// split the layout no longer has) leaves the layout untouched: the drag
/// stalls at the floor rather than collapsing a pane.
fn drag_resize(ctx: &mut DispatchCtx<'_>, mouse: &MouseEvent, grab: &DragGrab) -> bool {
    // Snapshot the geometry that feeds the resize before borrowing the
    // workspace mutably for the active window.
    let viewport = ctx.viewport;
    let has_bar = ctx.has_bar;
    let sidebar = ctx.sidebar;
    let Some(ls) = ctx.workspace.active_window_mut() else {
        return false;
    };
    let pointer = (quantize_cell(mouse.x), quantize_cell(mouse.y));
    match actions::apply_divider_resize(
        ls,
        &grab.node_path,
        grab.axis,
        pointer,
        viewport,
        has_bar,
        sidebar,
    ) {
        Ok(Some(new_state)) => {
            *ls = new_state;
            true
        }
        // Min-cell floor or stale grab — keep the divider where it is.
        Ok(None) | Err(_) => false,
    }
}

/// phux-fce4: whether an outer-viewport cell lies within the sidebar
/// strip's rect (separator column included — the strip consumes it even
/// though it is not a hit target).
const fn strip_contains(rect: crate::layout::Rect, x: u16, y: u16) -> bool {
    x >= rect.x
        && x < rect.x.saturating_add(rect.w)
        && y >= rect.y
        && y < rect.y.saturating_add(rect.h)
}

/// phux-fce4: map a left press on the sidebar strip to the action it
/// commits, or `None` when it lands on a blank row or the separator.
///
/// The mapping goes through [`ResolvedAction`] so a sidebar click runs
/// exactly what a keybinding, palette row, or overlay commit would — one
/// dispatch path, no bespoke click semantics:
///
/// * a window block (name or branch row) commits `select-window { index }`;
/// * `+ new` commits `new-window` (the strip lists windows, so its create
///   affordance creates one);
/// * `= menu` commits `command-palette` — the menu covering window,
///   session (`new-session` included), and plugin actions via the action
///   registry.
fn sidebar_click_action(
    strip: crate::layout::Rect,
    window_count: usize,
    x: u16,
    y: u16,
) -> Option<phux_config::keybind::ResolvedAction> {
    use crate::render::chrome::sidebar::{SidebarHit, hit_test};
    let (action, args) = match hit_test(strip, window_count, x, y)? {
        SidebarHit::Window(i) => {
            let mut args = std::collections::BTreeMap::new();
            args.insert(
                "index".to_owned(),
                toml::Value::Integer(i64::try_from(i).ok()?),
            );
            ("select-window", args)
        }
        SidebarHit::NewWindow => ("new-window", std::collections::BTreeMap::new()),
        SidebarHit::Menu => ("command-palette", std::collections::BTreeMap::new()),
    };
    Some(phux_config::keybind::ResolvedAction {
        action: action.to_owned(),
        args,
    })
}

/// Quantise an f64 pointer position (1-px-per-cell per SPEC §9.2.1) to an
/// outer-viewport cell, saturating into `u16` like the mouse hit-test.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "cell-quantised SGR/X10 input; saturate to keep malformed peers from breaking routing"
)]
fn quantize_cell(p: f64) -> u16 {
    if p.is_nan() || p < 0.0 {
        0
    } else if p >= f64::from(u16::MAX) {
        u16::MAX
    } else {
        p as u16
    }
}

/// Apply the side-effects of a resolved action: layout-mutation repaint
/// signal, focus move, prediction reset, `SET_METADATA` broadcast, bell,
/// detach, parked spawn (split / new-window), and kill-frame sequences.
///
/// Shared by the keybinding path and the overlay-commit path (phux-ahv.1)
/// so a rename committed from the prompt broadcasts and repaints exactly
/// like a keybinding would. Returns `true` if the layout changed (the
/// caller repaints).
#[allow(
    clippy::future_not_send,
    reason = "client-side libghostty Terminal is !Send; ADR-0003 binds us to current-thread"
)]
#[allow(
    clippy::too_many_arguments,
    reason = "shares the dispatch loop's transport + render + predict context; phux-7ry0 added the focused-pane map for the predict re-anchor"
)]
#[allow(
    clippy::cognitive_complexity,
    reason = "flat per-effect dispatch — one guarded block per ActionEffects field (zoom, focus, metadata, bell, detach, spawn, kill); splitting would thread the same transport + render context through helpers"
)]
#[allow(
    clippy::too_many_lines,
    reason = "same flat per-effect dispatch; phux-r82.5 added the plugin-run spawn block"
)]
async fn apply_action_effects<W: super::RenderSink>(
    effects: ActionEffects,
    out: &mut W,
    conn: &mut Connection,
    ctx: &mut DispatchCtx<'_>,
    focused_pane: &mut Option<TerminalId>,
    detach_pending: &mut bool,
    predict: &mut PredictionState,
    panes: &HashMap<TerminalId, PaneSlot>,
) -> Result<bool, AttachError> {
    let layout_changed = effects.layout_mutated;
    if effects.toggle_zoom {
        // phux-x2hm: flip pane-zoom. Un-zoom if zoomed; otherwise zoom the
        // focused pane. `run_action` already gated single-pane windows.
        *ctx.zoomed = if ctx.zoomed.is_some() {
            None
        } else {
            focused_pane.clone()
        };
    }
    if effects.toggle_sidebar {
        // phux-4h5a: flip the window-sidebar on/off state. The driver re-folds
        // `sidebar_enabled` into the per-frame reservation after dispatch, so
        // the `layout_mutated` repaint tiles into the new content rect.
        *ctx.sidebar_enabled = !*ctx.sidebar_enabled;
    }
    if let Some(target) = effects.set_focus {
        *focused_pane = Some(target);
        // Focus moved (keybinding pane navigation) — re-anchor predict to
        // the new pane: reset its cursor + viewport and drop the old pane's
        // queue, so a keystroke before the next reconcile echoes at the
        // right place rather than the old pane's (mid-screen) coordinates
        // (phux-7ry0). Subsumes the plain `clear_predict` drop below.
        if let Some(fid) = focused_pane.as_ref() {
            super::driver::reanchor_predict_to_pane(predict, panes, fid);
        }
    } else if effects.clear_predict {
        predict.clear();
    }
    if effects.set_metadata
        && let Some(session) = ctx.focused_session
    {
        // Encoding can fail only on an empty workspace (we just produced
        // it — shouldn't happen), but propagate cleanly if it ever does.
        // phux-jy4t: keyed per session so a split here persists to THIS
        // session's layout, not a key every session shares.
        if let Some(bytes) = encode_layout_or_log(ctx.workspace) {
            let request_id = *ctx.next_request_id;
            *ctx.next_request_id = ctx.next_request_id.wrapping_add(1);
            conn.send(&FrameKind::SetMetadata {
                request_id,
                scope: Scope::Group(DEFAULT_GROUP_ID),
                key: layout_key(session),
                value: bytes,
            })
            .await?;
        }
    }
    if effects.bell {
        let _ = actions::write_bell(out);
    }
    if effects.detach && !*detach_pending {
        conn.send(&FrameKind::Detach).await?;
        *detach_pending = true;
    }
    // Parked split — send the SPAWN_TERMINAL and remember the intent.
    if let Some((request_id, pending, frame)) = effects.spawn_terminal {
        ctx.pending_splits.insert(request_id, pending);
        conn.send(&frame).await?;
    }
    // Parked new-window — same SPAWN flow; the reply opens a window.
    if let Some((request_id, pending, frame)) = effects.spawn_window {
        ctx.pending_windows.insert(request_id, pending);
        conn.send(&frame).await?;
    }
    // kill-pane / kill-window keystroke sequences; the TERMINAL_CLOSED
    // fold-out happens when each shell exits.
    for frame in effects.kill_frames {
        conn.send(&frame).await?;
    }
    // ADR-0033: supervisory COMMAND frames (take/give the wheel, signal the
    // pane). Fire-and-forward — the server's COMMAND_RESULT + TerminalControl
    // broadcast drive the chrome; the input loop does not block on the reply.
    for frame in effects.command_frames {
        conn.send(&frame).await?;
    }
    // phux-r82.5: a plugin action runs as a spawned child-process task —
    // fire-and-forget from the input loop's perspective. The driver's
    // `select!` picks up the completion report and toasts failures. All
    // client-local (config + exec); nothing goes on the wire (ADR-0017).
    if let Some((plugin_id, action_id)) = effects.run_plugin {
        if let Some(tx) = ctx.plugin_tx {
            super::plugin_actions::spawn_plugin_action(tx.clone(), plugin_id, action_id);
        } else {
            tracing::warn!(
                plugin = %plugin_id,
                action = %action_id,
                "plugin-action dispatched with no plugin runtime channel; dropping",
            );
        }
    }
    // phux-eb0 / new-session: an in-process re-attach request. Hand the
    // target up to the driver via `ctx.switch_request`; `main_loop` reads
    // it after this dispatch batch and returns a `SwitchTo` exit so the
    // outer loop tears down the current session and re-attaches.
    //
    // Switching to the CURRENT session is a no-op (the picker excludes it,
    // but guard here too in case `switch-session { name }` is reached via
    // the command palette or a config-bound chord naming it); it bells so
    // the user gets feedback. `new-session` is never a no-op — naming an
    // existing session just attaches to it.
    if let Some(target) = effects.reattach {
        match target {
            ReattachTarget::Existing(name) if &name == ctx.session_name => {
                tracing::debug!(target_session = %name, "switch-session to current session; no-op");
                let _ = actions::write_bell(out);
            }
            ReattachTarget::Existing(name) => {
                tracing::info!(target_session = %name, "switch-session requested");
                *ctx.switch_request = Some(ReattachTarget::Existing(name));
            }
            ReattachTarget::Create(name) => {
                tracing::info!(session = %name, "new-session requested");
                *ctx.switch_request = Some(ReattachTarget::Create(name));
            }
        }
    }
    // rename-session: since the v0.3.0 "Option B" re-tier (ADR-0019 /
    // ADR-0027) removed the RENAME_SESSION verb, a rename is a SET_METADATA
    // write of the conventional SESSION_NAME_KEY (Scope::Global, value
    // `current\0new`); the server intercepts it and applies the registry
    // rename. We optimistically reflect the new name locally — the server is
    // authoritative, and the next ATTACHED snapshot overwrites `session_name`
    // (also how other attached clients learn the rename; a live
    // SESSION_RENAMED push is out of scope for this pass). A no-op rename
    // (new == current) is dropped: nothing to send, nothing to repaint.
    let renamed = if let Some(new_name) = effects.rename_session.filter(|n| n != &*ctx.session_name)
    {
        let request_id = *ctx.next_request_id;
        *ctx.next_request_id = ctx.next_request_id.wrapping_add(1);
        let mut value = ctx.session_name.as_bytes().to_vec();
        value.push(0);
        value.extend_from_slice(new_name.as_bytes());
        conn.send(&FrameKind::SetMetadata {
            request_id,
            scope: Scope::Global,
            key: SESSION_NAME_KEY.to_owned(),
            value,
        })
        .await?;
        tracing::info!(new_name = %new_name, "rename-session sent; optimistically updating local name");
        *ctx.session_name = new_name;
        true
    } else {
        false
    };
    // A rename repaints the status bar (it carries the session name) the
    // same way a layout mutation does; fold it into the caller's repaint
    // signal so the new name shows immediately.
    Ok(layout_changed || renamed)
}

/// Result of feeding a key event through the resolver.
enum ChordOutcome {
    /// Chord extended a partial sequence; absorb and wait.
    Partial,
    /// Chord completed a binding; effects follow.
    Resolved(phux_config::keybind::ResolvedAction),
}

/// Convert a `KeyEvent` into a `KeyChord` and feed the resolver. Returns
/// `None` when the resolver is disabled (no config) or the chord
/// doesn't match any binding — caller forwards normally in that case.
///
/// Release / repeat events are NOT fed to the resolver — chord matching
/// is press-only, matching the convention of `phux-config::keybind`'s
/// tests and tmux's prefix table. Repeats of held keys (e.g. arrow keys
/// scrolling) would otherwise re-fire actions per-tick.
fn consume_chord(
    ctx: &mut DispatchCtx<'_>,
    key_event: &phux_protocol::input::key::KeyEvent,
) -> Option<ChordOutcome> {
    use phux_protocol::input::key::KeyAction;
    let resolver = ctx.resolver.as_deref_mut()?;
    if !matches!(key_event.action, KeyAction::Press) {
        return None;
    }
    let chord = phux_config::keybind::KeyChord {
        modifiers: key_event.mods,
        key: key_event.key,
    };
    match resolver.feed(chord) {
        phux_config::keybind::Feed::NoMatch => None,
        phux_config::keybind::Feed::Partial => {
            // Mid-chord: the user is partway through a multi-chord binding.
            // Debug (not info) — chord progress is finer-grained than the
            // resolved-action lifecycle event in `run_action`.
            tracing::debug!("chord: partial match, awaiting next chord");
            Some(ChordOutcome::Partial)
        }
        phux_config::keybind::Feed::Resolved(r) => {
            tracing::debug!(action = %r.action, "chord: resolved to action");
            Some(ChordOutcome::Resolved(r))
        }
    }
}

/// Side-effects a resolved action wants from the driver.
#[derive(Debug, Default)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "action dispatcher returns independent side-effect flags to keep async I/O outside run_action"
)]
struct ActionEffects {
    /// `true` ⇒ the active window was mutated in-place; driver repaints.
    layout_mutated: bool,
    /// phux-x2hm: `true` ⇒ flip the driver's pane-zoom state (zoom the
    /// focused pane to fill the window, or un-zoom). `apply_action_effects`
    /// owns the actual toggle since the `zoomed` state lives in the driver.
    toggle_zoom: bool,
    /// phux-4h5a: `true` ⇒ flip the driver's window-sidebar on/off state.
    /// `apply_action_effects` owns the toggle since `sidebar_enabled` lives in
    /// the driver; it also sets `layout_mutated` so the panes reflow into (or
    /// out of) the sidebar's reserved columns on the same-iteration repaint.
    toggle_sidebar: bool,
    /// `Some(new_focus)` ⇒ swap the driver's `focused_pane` (input
    /// routing follows). The action helper already updated the active
    /// window's focus; this carries the new id so the driver
    /// doesn't have to re-read it.
    set_focus: Option<TerminalId>,
    /// `true` ⇒ emit `SET_METADATA` carrying the new layout envelope.
    set_metadata: bool,
    /// `true` ⇒ emit a terminal bell (BEL `\x07`).
    bell: bool,
    /// phux-4li.16: `true` ⇒ the active window changed; the driver must
    /// drop the prediction queue (anchored to the old window's focused
    /// pane) so a stale ghost echo doesn't paint into the new window
    /// before the next `TERMINAL_OUTPUT` reconciles.
    clear_predict: bool,
    /// `true` ⇒ emit `DETACH` and wait for `DETACHED`.
    detach: bool,
    /// phux-4li.12: a `split-pane` action emitted a `SPAWN_TERMINAL`
    /// and parked a [`PendingSplit`] keyed by `request_id`. The async
    /// caller sends the frame, then inserts the parked entry into the
    /// driver-wide `pending_splits` map.
    spawn_terminal: Option<(u32, PendingSplit, FrameKind)>,
    /// phux-4li.15: a `new-window` action emitted a `SPAWN_TERMINAL` and
    /// parked a [`PendingWindow`] keyed by `request_id`. The async caller
    /// sends the frame and inserts the parked entry into the driver-wide
    /// `pending_windows` map; the reply opens a new window on the
    /// spawned pane.
    spawn_window: Option<(u32, PendingWindow, FrameKind)>,
    /// phux-4li.12: a `kill-pane` action ships a sequence of frames to
    /// the focused Terminal (the "soft-kill via shell-exit" — see
    /// `run_action`). The async caller sends them in order; the
    /// resulting `TERMINAL_CLOSED` from the server folds the pane out
    /// of the layout in [`crate::attach::server_frame::handle_server_frame`].
    kill_frames: Vec<FrameKind>,
    /// ADR-0033: supervisory commands (`ACQUIRE_INPUT` / `RELEASE_INPUT` /
    /// `SIGNAL_TERMINAL`) the `take-input` / `give-input` / `signal-terminal`
    /// actions built for the focused pane. The async caller sends each as a
    /// `COMMAND` frame in order; the server's `TerminalControl` broadcast (which
    /// we subscribed to at attach) drives the chrome update on the way back.
    command_frames: Vec<FrameKind>,
    /// phux-4li.20 / phux-eb0 / new-session: an in-process re-attach the
    /// driver should perform after this batch — either switch to an
    /// existing session or create a new one. [`apply_action_effects`]
    /// hands it up via `DispatchCtx::switch_request`; the driver's
    /// `main_loop` returns a `SwitchTo` exit and the outer loop detaches
    /// and re-attaches on the same connection. An `Existing` request
    /// matching the current session is a no-op (bells).
    reattach: Option<ReattachTarget>,
    /// rename-session: a committed rename. Carries the new name. The async
    /// caller ([`apply_action_effects`]) sends a `RENAME_SESSION` command
    /// for the *current* session over the existing connection and
    /// optimistically updates the client's own cached `session_name` +
    /// repaints its status bar. The server is authoritative: the next
    /// `ATTACHED` snapshot reconciles the name (and is how other attached
    /// clients learn of it — a live `SESSION_RENAMED` push is out of scope
    /// for this pass). A refusal (unknown session / name taken) arrives as a
    /// `COMMAND_RESULT { Error }`; this pass logs it and lets the next
    /// snapshot correct the optimistic name rather than blocking the input
    /// loop on the reply.
    rename_session: Option<String>,
    /// phux-r82.5: a `plugin-action` dispatch carrying
    /// `(plugin_id, action_id)`. The async caller
    /// ([`apply_action_effects`]) spawns the child-process run via
    /// [`super::plugin_actions::spawn_plugin_action`] so the input loop
    /// never blocks on the plugin; completion lands on the driver's
    /// plugin-events channel (failure output toasts there).
    run_plugin: Option<(String, String)>,
}

/// An in-process re-attach request raised by a dispatched action.
///
/// Produced by `switch-session` / `new-session` (phux-eb0) and carried up
/// to the driver via `DispatchCtx::switch_request`; `main_loop` returns a
/// `SwitchTo` exit and the outer loop detaches and re-attaches on the same
/// connection without dropping the transport or leaving raw mode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReattachTarget {
    /// Switch to an existing session by name (`switch-session`).
    Existing(String),
    /// Create — or attach to, if it already exists — a session by name
    /// (`new-session`).
    Create(String),
}

/// Canonical names of every action `run_action` handles.
///
/// This is the single source of truth for the dispatcher's action set.
/// The command-palette registry
/// ([`super::action_registry::REGISTRY`]) is checked against this list by
/// a unit test so the two cannot drift: adding a `run_action` arm without
/// adding it here (and to the registry) fails CI. Keep this list in sync
/// with the `match resolved.action.as_str()` arms below — they are the
/// same set by construction, and the test enforces it.
pub const ACTION_NAMES: &[&str] = &[
    "split-pane",
    "kill-pane",
    "new-window",
    "kill-window",
    "next-window",
    "previous-window",
    "select-window",
    "rename-window",
    "rename-session",
    "focus-direction",
    "resize-pane",
    "show-help",
    "copy-mode",
    "detach",
    "next-pane",
    "previous-pane",
    "toggle-zoom",
    "toggle-sidebar",
    "command-palette",
    "window-picker",
    "session-picker",
    "switch-session",
    "new-session",
    "take-input",
    "give-input",
    "signal-terminal",
    "plugin-action",
];

/// Dispatch a resolved action against the driver's context.
///
/// Returns the [`ActionEffects`] the caller needs to apply. The function
/// is sync: it never touches the connection — frame I/O happens in the
/// caller (`dispatch_input_events`) so a hypothetical async wire-send
/// failure doesn't leave layout state half-mutated.
#[allow(
    clippy::too_many_lines,
    clippy::cognitive_complexity,
    reason = "per-action arms accrete one-by-one; splitting into per-action helpers would obscure the central dispatch table"
)]
fn run_action(
    resolved: &phux_config::keybind::ResolvedAction,
    ctx: &mut DispatchCtx<'_>,
    focused: Option<&TerminalId>,
) -> ActionEffects {
    // One event per resolved action the user triggered. Info level: a
    // keybinding firing is a user-lifecycle event a trace reader wants under
    // the default filter, and it is human-paced (not per-frame), so it costs
    // nothing meaningful on the hot path. The action name is the key field;
    // any render-triggering effect is captured by the resulting repaint /
    // frame spans downstream.
    tracing::info!(action = %resolved.action, "input: running resolved action");
    let mut effects = ActionEffects::default();
    match resolved.action.as_str() {
        "split-pane" => {
            // phux-4li.12: SPAWN_TERMINAL → server allocates the new
            // Terminal under DEFAULT_GROUP_ID and replies with
            // TERMINAL_SPAWNED { request_id, result: Ok(new_id) }. The
            // layout mutation happens in the reply handler — see
            // `handle_server_frame`'s TerminalSpawned arm and
            // `apply_spawned_ok`. We park a `PendingSplit` keyed by
            // request id so the reply knows which leaf to split.
            let Some(dir) = split_dir_arg(resolved) else {
                tracing::warn!(
                    args = ?resolved.args,
                    "split-pane missing/bad `direction` arg (expected horizontal|vertical)",
                );
                effects.bell = true;
                return effects;
            };
            let Some(focused_id) = focused.cloned() else {
                tracing::warn!("split-pane: no focused pane to split against; dropping action");
                effects.bell = true;
                return effects;
            };
            let request_id = *ctx.next_request_id;
            *ctx.next_request_id = ctx.next_request_id.wrapping_add(1);
            // CWD inheritance is phux-4li.1; until then we let the
            // server pick (typically $HOME). `command = None` invokes
            // the server's default shell; `env = None` inherits the
            // server's environment as-is.
            let frame = FrameKind::SpawnTerminal {
                request_id,
                group: DEFAULT_GROUP_ID,
                command: None,
                cwd: None,
                env: None,
                term: None,
            };
            effects.spawn_terminal = Some((
                request_id,
                PendingSplit {
                    focused_at_request: focused_id,
                    dir,
                },
                frame,
            ));
        }
        "kill-pane" => {
            // phux-4li.12: soft-kill — write `exit\n` as a sequence of
            // INPUT_KEY events to the focused Terminal. When the shell
            // processes those keystrokes it exits, the PTY closes, and
            // the server broadcasts TERMINAL_CLOSED which we then fold
            // out of the layout in `handle_server_frame`.
            //
            // Caveat: this is softer than tmux's `kill-pane`, which
            // sends SIGKILL to the entire process group. If the
            // focused pane has an unresponsive foreground process
            // (e.g. a stuck `cat` blocked on a non-existent FIFO) the
            // keystrokes go nowhere. A future ticket may add an
            // explicit KILL_TERMINAL wire frame; for v0.1 this gets
            // the daily-drive flow working end-to-end.
            let Some(focused_id) = focused.cloned() else {
                tracing::warn!("kill-pane: no focused pane to kill; dropping action");
                effects.bell = true;
                return effects;
            };
            effects.kill_frames = soft_kill_input_frames(&focused_id);
        }
        "take-input" => {
            // ADR-0033: seize the focused pane's input lease so only this
            // client's keystrokes reach the PTY. `Seize` preempts any holder;
            // the server broadcasts `TerminalControl` so the badge updates.
            let Some(focused_id) = focused.cloned() else {
                tracing::warn!("take-input: no focused pane; dropping action");
                effects.bell = true;
                return effects;
            };
            let request_id = *ctx.next_request_id;
            *ctx.next_request_id = ctx.next_request_id.wrapping_add(1);
            effects.command_frames.push(FrameKind::Command {
                request_id,
                command: Command::AcquireInput {
                    terminal_id: focused_id,
                    mode: InputMode::Seize,
                    ttl_ms: 0,
                },
            });
        }
        "give-input" => {
            // ADR-0033: release the focused pane's input lease back to open
            // input. A no-op server-side if we do not hold it.
            let Some(focused_id) = focused.cloned() else {
                tracing::warn!("give-input: no focused pane; dropping action");
                effects.bell = true;
                return effects;
            };
            let request_id = *ctx.next_request_id;
            *ctx.next_request_id = ctx.next_request_id.wrapping_add(1);
            effects.command_frames.push(FrameKind::Command {
                request_id,
                command: Command::ReleaseInput {
                    terminal_id: focused_id,
                },
            });
        }
        "signal-terminal" => {
            // ADR-0033: deliver a POSIX signal to the focused pane's process
            // group. `freeze`/`resume` is the reversible brake; distinct from
            // `kill-pane`, which removes the pane.
            let Some(signal) = signal_arg(resolved) else {
                tracing::warn!(
                    args = ?resolved.args,
                    "signal-terminal missing/bad `signal` arg (interrupt|freeze|resume|terminate|kill)",
                );
                effects.bell = true;
                return effects;
            };
            let Some(focused_id) = focused.cloned() else {
                tracing::warn!("signal-terminal: no focused pane; dropping action");
                effects.bell = true;
                return effects;
            };
            let request_id = *ctx.next_request_id;
            *ctx.next_request_id = ctx.next_request_id.wrapping_add(1);
            effects.command_frames.push(FrameKind::Command {
                request_id,
                command: Command::SignalTerminal {
                    terminal_id: focused_id,
                    signal,
                },
            });
        }
        "new-window" => {
            // phux-4li.15: open a new window. Spawn a fresh Terminal
            // (same SPAWN as a split) and park a `PendingWindow`; the
            // reply (`handle_server_frame`'s TerminalSpawned arm) adds a
            // window seeded on the spawned pane and makes it active. The
            // new pane is a bare leaf — the server files it under the
            // default Group; the TUI groups it into a window itself
            // (windows are a client convention, ADR-0017).
            let request_id = *ctx.next_request_id;
            *ctx.next_request_id = ctx.next_request_id.wrapping_add(1);
            let name = ctx.workspace.default_window_name();
            let frame = FrameKind::SpawnTerminal {
                request_id,
                group: DEFAULT_GROUP_ID,
                command: None,
                cwd: None,
                env: None,
                term: None,
            };
            effects.spawn_window = Some((request_id, PendingWindow { name }, frame));
        }
        "kill-window" => {
            // phux-4li.15: soft-kill every pane in the active window, the
            // same `exit\n` mechanism as `kill-pane`. As each
            // TERMINAL_CLOSED lands, `handle_server_frame` folds the pane
            // out; when the window's tree empties it is pruned and the
            // new layout broadcast. No synchronous window removal here.
            let leaves = ctx
                .workspace
                .active_window()
                .and_then(|ls| ls.tree.as_ref().map(crate::layout::leaves))
                .unwrap_or_default();
            if leaves.is_empty() {
                tracing::warn!("kill-window: no active window to kill; dropping action");
                effects.bell = true;
                return effects;
            }
            effects.kill_frames = leaves.iter().flat_map(soft_kill_input_frames).collect();
        }
        "next-window" => {
            switch_window(ctx, &mut effects, Workspace::next);
        }
        "previous-window" => {
            switch_window(ctx, &mut effects, Workspace::prev);
        }
        "select-window" => {
            let Some(index) = index_arg(resolved) else {
                tracing::warn!(args = ?resolved.args, "select-window missing/bad `index` arg");
                effects.bell = true;
                return effects;
            };
            switch_window(ctx, &mut effects, |w| {
                w.select(index);
            });
        }
        "rename-window" => {
            if ctx.workspace.active_window().is_none() {
                tracing::warn!("rename-window: no active window; dropping action");
                effects.bell = true;
                return effects;
            }
            if let Some(name) = name_arg(resolved) {
                // Explicit `name` renames immediately. A rename is shared
                // window state, so (unlike focus/switch) it broadcasts.
                ctx.workspace.rename_active(name);
                effects.layout_mutated = true;
                effects.set_metadata = true;
            } else {
                // No name ⇒ open the interactive prompt pre-filled with
                // the active window's current name. On commit it re-runs
                // `rename-window` with the typed name (phux-ahv.1).
                let current = ctx
                    .workspace
                    .windows
                    .get(ctx.workspace.active)
                    .map(|w| w.name.clone())
                    .unwrap_or_default();
                ctx.overlays
                    .push(Box::new(PromptOverlay::rename_window(&current, ctx.theme)));
                effects.layout_mutated = true;
            }
        }
        "rename-session" => {
            // Rename the session this client is attached to. With an explicit
            // `name` it renames directly; with no name it opens a prompt
            // pre-filled with the current session name, which commits
            // `rename-session { name }` back through this same path (the
            // rename-window precedent). The actual `RENAME_SESSION` send +
            // optimistic local-name update happen in `apply_action_effects`
            // (the connection is async, run_action is sync — the `detach`
            // model).
            if let Some(name) = name_arg(resolved) {
                effects.rename_session = Some(name);
            } else {
                ctx.overlays.push(Box::new(PromptOverlay::rename_session(
                    ctx.session_name,
                    ctx.theme,
                )));
                effects.layout_mutated = true;
            }
        }
        "focus-direction" => {
            if let Some(dir) = direction_arg(resolved) {
                if let Some(ls) = ctx.workspace.active_window_mut()
                    && let Some(new_state) = actions::apply_focus(ls, dir)
                {
                    let new_focus = new_state.focus.clone();
                    *ls = new_state;
                    effects.layout_mutated = true;
                    effects.set_focus = new_focus;
                }
                // No-neighbour case: silently drop (tmux convention —
                // bumping into the layout edge isn't a bell).
            } else {
                tracing::warn!(args = ?resolved.args, "focus-direction missing/bad `direction` arg");
                effects.bell = true;
            }
        }
        "resize-pane" => {
            if let (Some(dir), Some(amount)) = (direction_arg(resolved), amount_arg(resolved)) {
                let Some(ls) = ctx.workspace.active_window_mut() else {
                    effects.bell = true;
                    return effects;
                };
                match actions::apply_resize(ls, dir, amount, ctx.viewport, ctx.sidebar) {
                    Ok(Some(new_state)) => {
                        *ls = new_state;
                        effects.layout_mutated = true;
                        effects.set_metadata = true;
                    }
                    Ok(None) | Err(ActionError::NoResizableBoundary) => {
                        // Underflow guard tripped or no matching axis —
                        // bell-no-op (ADR-0019 decision 5).
                        effects.bell = true;
                    }
                    Err(err) => {
                        tracing::warn!(error = %err, "resize-pane failed");
                        effects.bell = true;
                    }
                }
            } else {
                tracing::warn!(args = ?resolved.args, "resize-pane missing args");
                effects.bell = true;
            }
        }
        "show-help" => {
            // phux-5ke.4: push the help overlay. The chord is consumed —
            // the bound key never reaches the pane (and it wouldn't make
            // sense to: F1 / `?` are typically reserved for the active
            // application, but the resolver matched, so the user's
            // intent was "open phux help"). Idempotent: pushing while
            // already active just replaces (debug-logged).
            let overlay = ctx.keybindings.map_or_else(
                || HelpOverlay::from_config(&phux_config::KeybindingsCfg::default(), ctx.theme),
                |kb| HelpOverlay::from_config(kb, ctx.theme),
            );
            ctx.overlays.push(Box::new(overlay));
        }
        "copy-mode" => {
            // phux-wave-a-copy-mode: enter selection/copy mode. Arrow keys move
            // the cursor without extending the selection unless Shift is held;
            // mouse drag can select and copy in one gesture.
            let pane_rect = focused_pane_rect(ctx, focused);
            let overlay = Box::new(crate::render::overlay::CopyModeOverlay::new(
                0,
                0,
                pane_rect.w,
                pane_rect.h,
            ));
            ctx.overlays.push(overlay);
        }
        "command-palette" => {
            // phux-ahv.8: push the command palette. It lists every action
            // the registry knows about, annotated with its currently-bound
            // chord from the live keybindings snapshot. Choosing a row
            // commits that action's `ResolvedAction`, which flows back
            // through this same `run_action` — keybinds and the palette
            // share one dispatch path (the architectural invariant).
            let items = super::action_registry::palette_items(ctx.keybindings, ctx.plugin_actions);
            ctx.overlays.push(Box::new(SelectList::new(
                "command palette",
                items,
                ctx.theme,
            )));
        }
        "window-picker" => {
            // phux-4li.19 / nav: push the `<leader> w` grouped window
            // picker. Sessions are section headers; under the current
            // session each window (`index:name`, pane count) commits
            // `select-window { index }` (the same per-client switch the
            // numeric prefix bindings use). Other sessions render a single
            // "switch to session" row committing `switch-session { name }`
            // — see the builder's doc-comment for the cross-session-window
            // gap. With no rows at all it bells.
            let items = window_picker_items(ctx.workspace, ctx.sessions, ctx.focused_session);
            if items.iter().all(SelectItem::is_header) {
                effects.bell = true;
                return effects;
            }
            ctx.overlays
                .push(Box::new(SelectList::new("windows", items, ctx.theme)));
        }
        "session-picker" => {
            // phux-4li.20: push the `<leader> a` session picker. Each row
            // is a peer session (name, window/client count as the
            // secondary) that commits `switch-session { name }` — the
            // same single dispatch path. The session this client is
            // attached to is excluded (switching to it is a no-op). With
            // no peer sessions it bells.
            // Peer sessions (current excluded) plus a trailing
            // "+ New session" row, so a session can always be created from
            // here. Each session row commits `switch-session { name }`; the
            // new-session row opens the name prompt. Always opens — even
            // with no peers you can still create one.
            let mut items = session_picker_items(ctx.sessions, ctx.focused_session);
            items.push(new_session_item());
            ctx.overlays
                .push(Box::new(SelectList::new("sessions", items, ctx.theme)));
        }
        "switch-session" => {
            // phux-4li.20 / phux-eb0: re-target this client to another
            // session. The effect carries the target up to
            // `apply_action_effects`, which routes it to the driver's
            // outer re-attach loop (in-process re-attach on the same
            // connection). A bad/absent `name` arg bells.
            if let Some(name) = name_arg(resolved) {
                effects.reattach = Some(ReattachTarget::Existing(name));
            } else {
                tracing::warn!(
                    args = ?resolved.args,
                    "switch-session missing/bad `name` arg",
                );
                effects.bell = true;
            }
        }
        "new-session" => {
            // Create a fresh session (or attach to one already named) and
            // switch this client to it in-process. An explicit `name`
            // creates it directly; with no name we open a prompt to type
            // one, which commits `new-session { name }` back through this
            // same path. Either way the re-attach uses CreateIfMissing.
            match name_arg(resolved) {
                Some(name) => effects.reattach = Some(ReattachTarget::Create(name)),
                None => ctx
                    .overlays
                    .push(Box::new(PromptOverlay::new_session(ctx.theme))),
            }
        }
        "detach" => {
            effects.detach = true;
        }
        "plugin-action" => {
            // phux-r82.5: run a plugin manifest action through the same
            // child-process runtime `phux config run PLUGIN ACTION` uses.
            // Sync dispatch only records the intent; the async caller
            // (`apply_action_effects`) spawns the run off the input loop so
            // a slow plugin never freezes the TUI. Completion arrives on
            // the driver's plugin-events channel; failures toast.
            let (Some(plugin), Some(action)) =
                (str_arg(resolved, "plugin"), str_arg(resolved, "action"))
            else {
                tracing::warn!(
                    args = ?resolved.args,
                    "plugin-action missing/bad `plugin`/`action` args",
                );
                effects.bell = true;
                return effects;
            };
            effects.run_plugin = Some((plugin, action));
        }
        "next-pane" => {
            if let Some(ls) = ctx.workspace.active_window_mut()
                && let Some(new_state) = actions::apply_next_pane(ls)
            {
                let new_focus = new_state.focus.clone();
                *ls = new_state;
                effects.layout_mutated = true;
                effects.set_focus = new_focus;
            }
        }
        "previous-pane" => {
            if let Some(ls) = ctx.workspace.active_window_mut()
                && let Some(new_state) = actions::apply_previous_pane(ls)
            {
                let new_focus = new_state.focus.clone();
                *ls = new_state;
                effects.layout_mutated = true;
                effects.set_focus = new_focus;
            }
        }
        "toggle-zoom" => {
            // phux-x2hm: zoom needs more than one pane (a single-pane window
            // bells, like tmux). When already zoomed the REAL tree still has
            // >1 leaf, so this same check permits un-zooming. The driver owns
            // the `zoomed` state; we just signal intent + request a repaint.
            let multi = ctx
                .workspace
                .active_window()
                .and_then(|ls| ls.tree.as_ref())
                .is_some_and(|t| crate::layout::leaves(t).len() > 1);
            if multi {
                effects.toggle_zoom = true;
                effects.layout_mutated = true;
            } else {
                effects.bell = true;
            }
        }
        "toggle-sidebar" => {
            // phux-4h5a: show/hide the window sidebar. Unconditional (unlike
            // zoom, which needs >1 pane) — the strip lists windows and is
            // meaningful even single-pane. The driver owns `sidebar_enabled`;
            // we signal intent + a repaint so the panes reflow into/out of the
            // reserved columns.
            // phux-4h5a P4 follow-up: a `focus-window`-by-index action (the
            // keyboard companion to clicking a strip row) is deferred; the
            // existing `select-window` jumps by tab position, but a strip-row
            // index action that pairs with mouse click-to-focus is not yet
            // wired.
            effects.toggle_sidebar = true;
            effects.layout_mutated = true;
        }
        other => {
            tracing::debug!(action = other, "unhandled resolved action");
        }
    }
    effects
}

/// Pull a `Direction` out of a [`phux_config::keybind::ResolvedAction`]'s `direction = "..."`
/// arg.
fn direction_arg(resolved: &phux_config::keybind::ResolvedAction) -> Option<Direction> {
    let s = resolved.args.get("direction")?.as_str()?;
    match s {
        "up" => Some(Direction::Up),
        "down" => Some(Direction::Down),
        "left" => Some(Direction::Left),
        "right" => Some(Direction::Right),
        // `split-pane direction=horizontal|vertical` uses a different
        // axis vocabulary; this helper is only for focus/resize.
        _ => None,
    }
}

/// Pull an `amount = N` arg out of a [`phux_config::keybind::ResolvedAction`]. TOML integers
/// decode as `i64`; we clamp to `i16` (the [`actions::apply_resize`]
/// signature). Out-of-range values are silently clamped — a `resize-pane
/// amount = 99999` user binding gets a 32767-cell amount, which the
/// underflow guard inside `apply_resize` then rejects.
#[allow(clippy::cast_possible_truncation)]
fn amount_arg(resolved: &phux_config::keybind::ResolvedAction) -> Option<i16> {
    let v = resolved.args.get("amount")?.as_integer()?;
    Some(v.clamp(i64::from(i16::MIN), i64::from(i16::MAX)) as i16)
}

/// Pull a window index out of a [`phux_config::keybind::ResolvedAction`]'s `index = N` arg.
/// Negative or non-integer values yield `None` (the caller bells).
fn index_arg(resolved: &phux_config::keybind::ResolvedAction) -> Option<usize> {
    let v = resolved.args.get("index")?.as_integer()?;
    usize::try_from(v).ok()
}

/// Pull a window name out of a [`phux_config::keybind::ResolvedAction`]'s `name = "..."` arg.
fn name_arg(resolved: &phux_config::keybind::ResolvedAction) -> Option<String> {
    resolved.args.get("name")?.as_str().map(ToOwned::to_owned)
}

/// Pull an arbitrary string arg out of a
/// [`phux_config::keybind::ResolvedAction`] (phux-r82.5: `plugin` /
/// `action` on `plugin-action`).
fn str_arg(resolved: &phux_config::keybind::ResolvedAction, key: &str) -> Option<String> {
    resolved.args.get(key)?.as_str().map(ToOwned::to_owned)
}

/// Build the `<leader> w` grouped window picker's rows (phux-4li.19 / nav).
///
/// The picker is hierarchical: one [`SelectItem::header`] per session, with
/// that session's windows nested (indented) beneath it. Sessions are
/// ordered with the **current** session first (so the windows you can act
/// on directly lead), then the rest by name for a stable layout.
///
/// - Under the **current** session, each window row is `index:name` with
///   the pane count as the dimmed secondary; it commits
///   `select-window { index }` — the same per-client window switch the
///   numeric prefix bindings use, routed through the single dispatch path.
/// - Under **other** sessions, the client cannot enumerate individual
///   windows: the cached session graph (`ctx.sessions`,
///   [`phux_protocol::wire::info::SessionInfo`]) carries only a
///   `window_count`, not per-window names, and the live `WindowInfo` list
///   from the ATTACHED snapshot is not retained client-side. Each such
///   session therefore renders a single "switch to session" row committing
///   `switch-session { name }`, which re-attaches this client to that
///   session (its own window picker then lists its windows). Selecting a
///   window directly inside a foreign session in one step would need both
///   the cached foreign `WindowInfo` list and a combined
///   switch-session-and-select-window flow; see the PR's risk notes.
///
/// Headers are non-selectable; a session with no rows beneath it (the
/// current session with zero windows) still contributes its header, and
/// the caller bells when *only* headers result.
fn window_picker_items(
    workspace: &Workspace,
    sessions: &[phux_protocol::wire::info::SessionInfo],
    focused: Option<phux_protocol::ids::SessionId>,
) -> Vec<SelectItem> {
    // Order sessions: current first, then the rest alphabetically by name
    // for a deterministic layout.
    let mut ordered: Vec<&phux_protocol::wire::info::SessionInfo> = sessions.iter().collect();
    ordered.sort_by(|a, b| {
        let a_cur = Some(a.id) == focused;
        let b_cur = Some(b.id) == focused;
        b_cur.cmp(&a_cur).then_with(|| a.name.cmp(&b.name))
    });

    let mut items = Vec::new();
    for session in ordered {
        let is_current = Some(session.id) == focused;
        let header = if is_current {
            format!("{} (current)", session.name)
        } else {
            session.name.clone()
        };
        items.push(SelectItem::header(header));

        if is_current {
            items.extend(current_session_window_rows(workspace));
        } else {
            // No per-window detail for a foreign session; offer a switch.
            let windows = if session.window_count == 1 {
                "1 window".to_owned()
            } else {
                format!("{} windows", session.window_count)
            };
            let mut args = std::collections::BTreeMap::new();
            args.insert("name".to_owned(), toml::Value::String(session.name.clone()));
            items.push(
                SelectItem::new(
                    "switch to this session",
                    phux_config::keybind::ResolvedAction {
                        action: "switch-session".to_owned(),
                        args,
                    },
                )
                .secondary(windows)
                .indented(),
            );
        }
    }

    // No sessions cached yet (pre-snapshot): fall back to a flat list of
    // the current workspace's windows so the picker is still useful.
    if items.is_empty() {
        items.extend(current_session_window_rows(workspace));
    }
    items
}

/// The indented, selectable window rows for the locally-attached session,
/// drawn from the client's [`Workspace`]. Each commits
/// `select-window { index }`.
fn current_session_window_rows(workspace: &Workspace) -> Vec<SelectItem> {
    workspace
        .windows
        .iter()
        .enumerate()
        .map(|(index, window)| {
            let panes = window
                .state
                .tree
                .as_ref()
                .map_or(0, |tree| crate::layout::leaves(tree).len());
            let label = format!("{index}:{}", window.name);
            let secondary = if panes == 1 {
                "1 pane".to_owned()
            } else {
                format!("{panes} panes")
            };
            let mut args = std::collections::BTreeMap::new();
            // Window counts never approach i64::MAX; the lossless path is
            // the only one that can fire in practice.
            let idx_i64 = i64::try_from(index).unwrap_or(i64::MAX);
            args.insert("index".to_owned(), toml::Value::Integer(idx_i64));
            SelectItem::new(
                label,
                phux_config::keybind::ResolvedAction {
                    action: "select-window".to_owned(),
                    args,
                },
            )
            .secondary(secondary)
            .indented()
        })
        .collect()
}

/// Build the `<leader> a` session picker's rows from the client's cached
/// session graph (phux-4li.20).
///
/// One row per session **other than** `focused` (the session this client
/// is attached to) — switching to the current session is a no-op, so it
/// is excluded rather than disabled. Each row's label is the session
/// name with a window/attached-client summary as the dimmed secondary;
/// choosing it commits `switch-session { name }`, which `run_action`
/// routes through the single dispatch path.
fn session_picker_items(
    sessions: &[phux_protocol::wire::info::SessionInfo],
    focused: Option<phux_protocol::ids::SessionId>,
) -> Vec<SelectItem> {
    sessions
        .iter()
        .filter(|s| Some(s.id) != focused)
        .map(|s| {
            let windows = if s.window_count == 1 {
                "1 window".to_owned()
            } else {
                format!("{} windows", s.window_count)
            };
            let secondary = if s.attached_client_count == 0 {
                windows
            } else {
                format!("{windows}, {} attached", s.attached_client_count)
            };
            let mut args = std::collections::BTreeMap::new();
            args.insert("name".to_owned(), toml::Value::String(s.name.clone()));
            SelectItem::new(
                s.name.clone(),
                phux_config::keybind::ResolvedAction {
                    action: "switch-session".to_owned(),
                    args,
                },
            )
            .secondary(secondary)
        })
        .collect()
}

/// The trailing "+ New session" row for the session picker. Committing it
/// runs the bare `new-session` action, which opens the name prompt — so a
/// new session is always reachable from `<leader> a`, even when this is
/// the only session.
fn new_session_item() -> SelectItem {
    SelectItem::new(
        "+ New session…".to_owned(),
        phux_config::keybind::ResolvedAction {
            action: "new-session".to_owned(),
            args: std::collections::BTreeMap::new(),
        },
    )
    .secondary("create".to_owned())
}

/// Apply a window-switch `mutate` to the workspace and, **only if the
/// active window actually changed**, record the follow-up: repaint the
/// new composition, drop the prediction queue, and move focus to the new
/// active window's focused leaf. A no-op switch (single window, wrap to
/// self, or an out-of-range `select`) leaves `effects` untouched.
///
/// Window selection is per-client like focus (ADR-0019 decision 6), so
/// this emits no `SET_METADATA` — siblings keep their own active window.
fn switch_window(
    ctx: &mut DispatchCtx<'_>,
    effects: &mut ActionEffects,
    mutate: impl FnOnce(&mut Workspace),
) {
    let before = ctx.workspace.active;
    mutate(ctx.workspace);
    if ctx.workspace.active == before {
        return;
    }
    effects.layout_mutated = true;
    effects.clear_predict = true;
    effects.set_focus = ctx.workspace.active_window().and_then(|w| w.focus.clone());
}

/// Encode the workspace for `SET_METADATA`, logging encode failures.
/// Returns `None` on failure — caller should not emit a frame in that case.
pub(super) fn encode_layout_or_log(workspace: &Workspace) -> Option<Vec<u8>> {
    match workspace.encode_cbor() {
        Ok(bytes) => Some(bytes),
        Err(err) => {
            tracing::warn!(error = %err, "layout CBOR encode failed; SET_METADATA skipped");
            None
        }
    }
}

/// Allow `SplitDir` to be parsed from a `direction = "horizontal|vertical"`
/// arg on a `split-pane` action. Lives here (not in `actions.rs`) so the
/// pure helper module stays free of `ResolvedAction` parsing.
///
/// The `direction` string names the DIVIDER orientation (the tmux mental
/// model the default config documents): `vertical` = a vertical divider,
/// i.e. side-by-side panes, which geometrically is a `SplitDir::Horizontal`
/// (split along the width — see `layout::fill_rects`). `horizontal` = a
/// horizontal divider, i.e. stacked panes = `SplitDir::Vertical`. The
/// names are deliberately crossed here: the user-facing word describes the
/// divider; the internal enum describes the split axis.
fn split_dir_arg(resolved: &phux_config::keybind::ResolvedAction) -> Option<SplitDir> {
    let s = resolved.args.get("direction")?.as_str()?;
    match s {
        "horizontal" => Some(SplitDir::Vertical),
        "vertical" => Some(SplitDir::Horizontal),
        _ => None,
    }
}

/// ADR-0033: parse the `signal` arg of a `signal-terminal` action into a
/// [`TerminalSignal`]. Recognises `interrupt` / `freeze` / `resume` /
/// `terminate` / `kill`; returns `None` for a missing or unknown value (the
/// arm bells and drops the action).
fn signal_arg(resolved: &phux_config::keybind::ResolvedAction) -> Option<TerminalSignal> {
    match resolved.args.get("signal")?.as_str()? {
        "interrupt" => Some(TerminalSignal::Interrupt),
        "freeze" => Some(TerminalSignal::Freeze),
        "resume" => Some(TerminalSignal::Resume),
        "terminate" => Some(TerminalSignal::Terminate),
        "kill" => Some(TerminalSignal::Kill),
        _ => None,
    }
}

/// phux-4li.12: build the `INPUT_KEY` frame sequence that types `exit\n`
/// into the targeted Terminal. The shell processes those bytes, exits,
/// the PTY closes, and the server emits `TERMINAL_CLOSED` which the
/// driver folds out of the layout. See the `kill-pane` arm of
/// [`run_action`] for the soft-kill caveat.
fn soft_kill_input_frames(target: &TerminalId) -> Vec<FrameKind> {
    use phux_protocol::input::key::{KeyAction, KeyEvent, ModSet, PhysicalKey};

    fn ascii_letter(ch: char, key: PhysicalKey) -> KeyEvent {
        KeyEvent {
            action: KeyAction::Press,
            key,
            mods: ModSet::empty(),
            consumed_mods: ModSet::empty(),
            composing: false,
            text: Some(ch.to_string()),
            unshifted_codepoint: Some(u32::from(ch)),
        }
    }
    const fn named(key: PhysicalKey) -> KeyEvent {
        KeyEvent {
            action: KeyAction::Press,
            key,
            mods: ModSet::empty(),
            consumed_mods: ModSet::empty(),
            composing: false,
            text: None,
            unshifted_codepoint: None,
        }
    }

    let events = [
        ascii_letter('e', PhysicalKey::E),
        ascii_letter('x', PhysicalKey::X),
        ascii_letter('i', PhysicalKey::I),
        ascii_letter('t', PhysicalKey::T),
        named(PhysicalKey::Enter),
    ];
    events
        .into_iter()
        .map(|event| FrameKind::InputKey {
            terminal_id: target.clone(),
            event,
        })
        .collect()
}

#[cfg(test)]
#[allow(clippy::expect_used, reason = "tests")]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn tid(id: u32) -> TerminalId {
        TerminalId::local(id)
    }

    #[test]
    fn soft_kill_input_frames_emits_exit_newline_sequence() {
        let frames = soft_kill_input_frames(&tid(7));
        assert_eq!(frames.len(), 5, "expected e/x/i/t/Enter");
        // Each frame is INPUT_KEY targeting tid(7).
        for f in &frames {
            match f {
                FrameKind::InputKey { terminal_id, .. } => {
                    assert_eq!(terminal_id, &tid(7));
                }
                other => panic!("expected InputKey, got {other:?}"),
            }
        }
        // First four are printable letters with text="e".."t".
        let expected_text = ["e", "x", "i", "t"];
        for (i, want) in expected_text.iter().enumerate() {
            match &frames[i] {
                FrameKind::InputKey { event, .. } => {
                    assert_eq!(
                        event.text.as_deref(),
                        Some(*want),
                        "frame {i}: text mismatch",
                    );
                }
                _ => unreachable!(),
            }
        }
        // Last frame is Enter (no text).
        match &frames[4] {
            FrameKind::InputKey { event, .. } => {
                assert_eq!(event.key, phux_protocol::input::key::PhysicalKey::Enter);
                assert_eq!(event.text, None);
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn split_dir_arg_parses_horizontal_and_vertical() {
        use phux_config::keybind::ResolvedAction;
        // `direction` names the divider orientation, not the split axis:
        // "horizontal" divider ⇒ stacked panes ⇒ SplitDir::Vertical;
        // "vertical" divider ⇒ side-by-side panes ⇒ SplitDir::Horizontal.
        let mut h = ResolvedAction {
            action: "split-pane".to_owned(),
            args: std::collections::BTreeMap::new(),
        };
        h.args.insert(
            "direction".to_owned(),
            toml::Value::String("horizontal".into()),
        );
        assert_eq!(split_dir_arg(&h), Some(SplitDir::Vertical));

        let mut v = ResolvedAction {
            action: "split-pane".to_owned(),
            args: std::collections::BTreeMap::new(),
        };
        v.args.insert(
            "direction".to_owned(),
            toml::Value::String("vertical".into()),
        );
        assert_eq!(split_dir_arg(&v), Some(SplitDir::Horizontal));

        let mut bogus = ResolvedAction {
            action: "split-pane".to_owned(),
            args: std::collections::BTreeMap::new(),
        };
        bogus.args.insert(
            "direction".to_owned(),
            toml::Value::String("diagonal".into()),
        );
        assert_eq!(split_dir_arg(&bogus), None);
    }

    #[test]
    fn focused_pane_rect_tracks_rendered_pane_bounds() {
        use crate::layout::{LayoutNode, LayoutState, Rect, WindowState, split_at};

        let tree = split_at(
            &LayoutNode::Leaf(tid(1)),
            &tid(1),
            &tid(2),
            SplitDir::Horizontal,
            0.5,
        )
        .unwrap();
        let workspace = Workspace {
            windows: vec![WindowState {
                name: "1".to_owned(),
                state: LayoutState {
                    tree: Some(tree),
                    focus: Some(tid(2)),
                },
            }],
            active: 0,
        };

        let split_rect =
            focused_pane_rect_for(&workspace, None, Some(&tid(2)), (80, 24), true, None);
        assert_eq!(split_rect.y, 0);
        assert_eq!(split_rect.h, 23, "status bar row is not copy-mode content");
        assert_eq!(split_rect.x + split_rect.w, 80);
        assert!(
            split_rect.w < 80,
            "split pane must not inherit the outer viewport width"
        );
        assert_ne!(
            split_rect,
            Rect {
                x: 0,
                y: 0,
                w: 80,
                h: 23
            }
        );

        let zoomed = tid(2);
        let zoomed_rect = focused_pane_rect_for(
            &workspace,
            Some(&zoomed),
            Some(&tid(2)),
            (80, 24),
            true,
            None,
        );
        assert_eq!(
            zoomed_rect,
            Rect {
                x: 0,
                y: 0,
                w: 80,
                h: 23
            }
        );
    }

    /// Build a [`ResolvedAction`] with no args.
    fn bare_action(name: &str) -> phux_config::keybind::ResolvedAction {
        phux_config::keybind::ResolvedAction {
            action: name.to_owned(),
            args: BTreeMap::new(),
        }
    }

    /// Run `action` against `workspace`, returning the resulting effects.
    fn run(
        action: &phux_config::keybind::ResolvedAction,
        workspace: &mut Workspace,
    ) -> ActionEffects {
        let mut next_request_id = 100;
        let mut pending_splits = HashMap::new();
        let mut pending_windows = HashMap::new();
        let mut overlays = OverlayState::new();
        let theme = Theme::default();
        let mut switch_request = None;
        let mut session_name = String::new();
        let mut zoomed = None;
        let mut sidebar_enabled = false;
        let mut drag: Option<DragGrab> = None;
        let mut ctx = DispatchCtx {
            resolver: None,
            workspace,
            viewport: (80, 24),
            next_request_id: &mut next_request_id,
            pending_splits: &mut pending_splits,
            pending_windows: &mut pending_windows,
            overlays: &mut overlays,
            keybindings: None,
            theme: &theme,
            sessions: &[],
            focused_session: None,
            session_name: &mut session_name,
            switch_request: &mut switch_request,
            zoomed: &mut zoomed,
            sidebar: None,
            sidebar_enabled: &mut sidebar_enabled,
            has_bar: false,
            drag: &mut drag,
            plugin_actions: &[],
            plugin_tx: None,
        };
        let focused = ctx.workspace.active_window().and_then(|w| w.focus.clone());
        run_action(action, &mut ctx, focused.as_ref())
    }

    #[test]
    fn new_window_parks_pending_and_emits_spawn() {
        let mut workspace = Workspace::single(tid(1)); // window "1"
        let effects = run(&bare_action("new-window"), &mut workspace);
        let (_req, pending, frame) = effects
            .spawn_window
            .expect("new-window should park a PendingWindow + SPAWN");
        // Default name skips the in-use "1".
        assert_eq!(pending.name, "2");
        assert!(matches!(frame, FrameKind::SpawnTerminal { .. }));
        // No synchronous workspace mutation — the window opens on reply.
        assert_eq!(workspace.windows.len(), 1);
    }

    #[test]
    fn kill_window_emits_one_soft_kill_sequence_per_leaf() {
        use crate::layout::{LayoutNode, LayoutState, SplitDir, WindowState, split_at};
        // Active window with three leaves: ((1|2)/3).
        let tree = split_at(
            &LayoutNode::Leaf(tid(1)),
            &tid(1),
            &tid(2),
            SplitDir::Horizontal,
            0.5,
        )
        .unwrap();
        let tree = split_at(&tree, &tid(2), &tid(3), SplitDir::Vertical, 0.5).unwrap();
        let mut workspace = Workspace {
            windows: vec![WindowState {
                name: "1".to_owned(),
                state: LayoutState {
                    tree: Some(tree),
                    focus: Some(tid(1)),
                },
            }],
            active: 0,
        };
        let effects = run(&bare_action("kill-window"), &mut workspace);
        // 3 leaves x 5 frames (e/x/i/t/Enter) each.
        assert_eq!(effects.kill_frames.len(), 15);
        // No synchronous removal — TerminalClosed folds + prunes.
        assert_eq!(workspace.windows.len(), 1);
    }

    #[test]
    fn next_window_switches_active_clears_predict_no_metadata() {
        let mut workspace = Workspace::single(tid(1));
        workspace.add_window("2".to_owned(), tid(2));
        workspace.select(0);
        let effects = run(&bare_action("next-window"), &mut workspace);
        assert_eq!(workspace.active, 1);
        assert!(effects.layout_mutated);
        assert!(effects.clear_predict);
        assert!(!effects.set_metadata, "window switch is per-client");
        assert_eq!(effects.set_focus, Some(tid(2)));
    }

    #[test]
    fn next_window_single_window_is_noop() {
        let mut workspace = Workspace::single(tid(1));
        let effects = run(&bare_action("next-window"), &mut workspace);
        assert_eq!(workspace.active, 0);
        assert!(!effects.layout_mutated);
        assert!(!effects.clear_predict);
    }

    #[test]
    fn select_window_jumps_to_index() {
        let mut workspace = Workspace::single(tid(1));
        workspace.add_window("2".to_owned(), tid(2));
        workspace.add_window("3".to_owned(), tid(3)); // active = 2
        let mut action = bare_action("select-window");
        action
            .args
            .insert("index".to_owned(), toml::Value::Integer(0));
        let effects = run(&action, &mut workspace);
        assert_eq!(workspace.active, 0);
        assert!(effects.layout_mutated);
        assert_eq!(effects.set_focus, Some(tid(1)));
    }

    #[test]
    fn select_window_out_of_range_is_noop() {
        let mut workspace = Workspace::single(tid(1)); // only index 0
        let mut action = bare_action("select-window");
        action
            .args
            .insert("index".to_owned(), toml::Value::Integer(5));
        let effects = run(&action, &mut workspace);
        assert_eq!(workspace.active, 0);
        assert!(!effects.layout_mutated);
    }

    #[test]
    fn select_window_missing_index_bells() {
        let mut workspace = Workspace::single(tid(1));
        let effects = run(&bare_action("select-window"), &mut workspace);
        assert!(effects.bell);
        assert!(!effects.layout_mutated);
    }

    /// phux-x2hm: a multi-pane window can zoom — `toggle-zoom` requests the
    /// driver-side flip (`toggle_zoom`) plus a repaint (`layout_mutated`),
    /// without mutating the real tree or bell-ing.
    #[test]
    fn toggle_zoom_on_multi_pane_window_requests_toggle() {
        use crate::layout::{LayoutState, WindowState, split_at};
        let tree = split_at(
            &crate::layout::LayoutNode::Leaf(tid(1)),
            &tid(1),
            &tid(2),
            crate::layout::SplitDir::Horizontal,
            0.5,
        )
        .unwrap();
        let mut workspace = Workspace {
            windows: vec![WindowState {
                name: "1".to_owned(),
                state: LayoutState {
                    tree: Some(tree),
                    focus: Some(tid(1)),
                },
            }],
            active: 0,
        };
        let effects = run(&bare_action("toggle-zoom"), &mut workspace);
        assert!(effects.toggle_zoom, "multi-pane window may zoom");
        assert!(effects.layout_mutated, "zoom toggles drive a repaint");
        assert!(!effects.bell);
    }

    /// phux-x2hm: a single-pane window has nothing to zoom — `toggle-zoom`
    /// bells (tmux parity) and does NOT request a toggle or repaint.
    #[test]
    fn toggle_zoom_on_single_pane_window_bells() {
        let mut workspace = Workspace::single(tid(1));
        let effects = run(&bare_action("toggle-zoom"), &mut workspace);
        assert!(effects.bell, "single-pane window cannot zoom");
        assert!(!effects.toggle_zoom);
        assert!(!effects.layout_mutated);
    }

    /// phux-4h5a: `toggle-sidebar` requests the driver-side flip
    /// (`toggle_sidebar`) plus a repaint (`layout_mutated`), unconditionally —
    /// even single-pane, since the strip lists windows. It never bells and
    /// mutates no tree.
    #[test]
    fn toggle_sidebar_requests_flip_and_repaint() {
        let mut workspace = Workspace::single(tid(1));
        let effects = run(&bare_action("toggle-sidebar"), &mut workspace);
        assert!(effects.toggle_sidebar, "toggle-sidebar requests the flip");
        assert!(
            effects.layout_mutated,
            "sidebar toggle drives a reflow repaint"
        );
        assert!(!effects.bell);
        assert!(!effects.toggle_zoom);
    }

    /// phux-4h5a: `apply_action_effects` flips the driver-owned
    /// `sidebar_enabled` when `toggle_sidebar` is set — off→on and back on a
    /// second toggle.
    #[tokio::test]
    async fn apply_effects_flips_sidebar_enabled_state() {
        let mut workspace = Workspace::single(tid(1));
        let mut next_request_id = 1;
        let mut pending_splits = HashMap::new();
        let mut pending_windows = HashMap::new();
        let mut overlays = OverlayState::new();
        let theme = Theme::default();
        let mut switch_request = None;
        let mut session_name = String::new();
        let mut zoomed = None;
        let mut sidebar_enabled = false;
        let mut drag: Option<DragGrab> = None;
        let mut ctx = DispatchCtx {
            resolver: None,
            workspace: &mut workspace,
            viewport: (80, 24),
            next_request_id: &mut next_request_id,
            pending_splits: &mut pending_splits,
            pending_windows: &mut pending_windows,
            overlays: &mut overlays,
            keybindings: None,
            theme: &theme,
            sessions: &[],
            focused_session: None,
            session_name: &mut session_name,
            switch_request: &mut switch_request,
            zoomed: &mut zoomed,
            sidebar: None,
            sidebar_enabled: &mut sidebar_enabled,
            has_bar: false,
            drag: &mut drag,
            plugin_actions: &[],
            plugin_tx: None,
        };
        let effects = run_action(&bare_action("toggle-sidebar"), &mut ctx, None);
        let mut out: Vec<u8> = Vec::new();
        let (a, _b) = tokio::net::UnixStream::pair().expect("uds pair");
        let mut conn = Connection::from_stream(a);
        let mut focused_pane = None;
        let mut detach_pending = false;
        let mut predict =
            PredictionState::new(crate::predict::PredictiveConfig::disabled(), 80, 24);
        let panes: HashMap<TerminalId, PaneSlot> = HashMap::new();
        apply_action_effects(
            effects,
            &mut out,
            &mut conn,
            &mut ctx,
            &mut focused_pane,
            &mut detach_pending,
            &mut predict,
            &panes,
        )
        .await
        .expect("apply effects");
        assert!(sidebar_enabled, "first toggle enables the sidebar");

        // A second toggle disables it again.
        let mut ctx = DispatchCtx {
            resolver: None,
            workspace: &mut workspace,
            viewport: (80, 24),
            next_request_id: &mut next_request_id,
            pending_splits: &mut pending_splits,
            pending_windows: &mut pending_windows,
            overlays: &mut overlays,
            keybindings: None,
            theme: &theme,
            sessions: &[],
            focused_session: None,
            session_name: &mut session_name,
            switch_request: &mut switch_request,
            zoomed: &mut zoomed,
            sidebar: None,
            sidebar_enabled: &mut sidebar_enabled,
            has_bar: false,
            drag: &mut drag,
            plugin_actions: &[],
            plugin_tx: None,
        };
        let effects = run_action(&bare_action("toggle-sidebar"), &mut ctx, None);
        apply_action_effects(
            effects,
            &mut out,
            &mut conn,
            &mut ctx,
            &mut focused_pane,
            &mut detach_pending,
            &mut predict,
            &panes,
        )
        .await
        .expect("apply effects");
        assert!(!sidebar_enabled, "second toggle disables the sidebar");
    }

    #[test]
    fn rename_window_with_name_arg_renames_and_broadcasts() {
        let mut workspace = Workspace::single(tid(1)); // window "1"
        let mut action = bare_action("rename-window");
        action
            .args
            .insert("name".to_owned(), toml::Value::String("build".into()));
        let effects = run(&action, &mut workspace);
        assert_eq!(workspace.windows[0].name, "build");
        assert!(effects.layout_mutated);
        assert!(effects.set_metadata, "rename is shared window state");
    }

    /// Like [`run`], but returns the `OverlayState` so a test can assert
    /// an action pushed an overlay.
    fn run_capturing(
        action: &phux_config::keybind::ResolvedAction,
        workspace: &mut Workspace,
    ) -> (ActionEffects, OverlayState) {
        run_capturing_with_sessions(action, workspace, &[], None)
    }

    /// Like [`run_capturing`], but seeds the dispatcher's cached session
    /// graph so `session-picker` tests can drive the picker.
    fn run_capturing_with_sessions(
        action: &phux_config::keybind::ResolvedAction,
        workspace: &mut Workspace,
        sessions: &[phux_protocol::wire::info::SessionInfo],
        focused_session: Option<phux_protocol::ids::SessionId>,
    ) -> (ActionEffects, OverlayState) {
        let mut next_request_id = 100;
        let mut pending_splits = HashMap::new();
        let mut pending_windows = HashMap::new();
        let mut overlays = OverlayState::new();
        let theme = Theme::default();
        let mut switch_request = None;
        let mut session_name = String::new();
        let mut zoomed = None;
        let mut sidebar_enabled = false;
        let mut drag: Option<DragGrab> = None;
        let effects = {
            let mut ctx = DispatchCtx {
                resolver: None,
                workspace,
                viewport: (80, 24),
                next_request_id: &mut next_request_id,
                pending_splits: &mut pending_splits,
                pending_windows: &mut pending_windows,
                overlays: &mut overlays,
                keybindings: None,
                theme: &theme,
                sessions,
                focused_session,
                session_name: &mut session_name,
                switch_request: &mut switch_request,
                zoomed: &mut zoomed,
                sidebar: None,
                sidebar_enabled: &mut sidebar_enabled,
                has_bar: false,
                drag: &mut drag,
                plugin_actions: &[],
                plugin_tx: None,
            };
            let focused = ctx.workspace.active_window().and_then(|w| w.focus.clone());
            run_action(action, &mut ctx, focused.as_ref())
        };
        (effects, overlays)
    }

    #[test]
    fn rename_window_no_arg_opens_prompt() {
        let mut workspace = Workspace::single(tid(1)); // window "1"
        let (effects, overlays) = run_capturing(&bare_action("rename-window"), &mut workspace);
        assert!(
            overlays.is_active(),
            "no-arg rename should open the prompt overlay"
        );
        assert!(effects.layout_mutated);
        // Not renamed yet — that happens when the prompt commits.
        assert_eq!(workspace.windows[0].name, "1");
        assert!(!effects.set_metadata, "no broadcast until commit");
    }

    #[test]
    fn kill_window_on_empty_workspace_bells() {
        let mut workspace = Workspace::default();
        let effects = run(&bare_action("kill-window"), &mut workspace);
        assert!(effects.bell);
        assert!(effects.kill_frames.is_empty());
    }

    #[test]
    fn palette_committed_action_routes_through_run_action() {
        // A palette row's ResolvedAction, fed back through run_action,
        // produces the same effect a keybind would. Use `detach` — a row
        // whose effect is unambiguous.
        let cfg = phux_config::parse_str(
            phux_config::DEFAULT_CONFIG_TOML,
            std::path::Path::new("default.toml"),
        )
        .expect("default config parses");
        let items = crate::attach::action_registry::palette_items(Some(&cfg.keybindings), &[]);
        let detach = items
            .iter()
            .find(|i| i.action.action == "detach")
            .expect("detach in palette");
        let mut workspace = Workspace::default();
        let effects = run(&detach.action, &mut workspace);
        assert!(effects.detach, "committing the detach palette row detaches");
    }

    #[test]
    fn plugin_action_records_run_intent_for_the_async_caller() {
        // phux-r82.5: the sync dispatcher never execs the plugin itself —
        // it records (plugin, action) and the async caller spawns the
        // child-process run so the input loop can't freeze on a plugin.
        let mut args = BTreeMap::new();
        args.insert(
            "plugin".to_owned(),
            toml::Value::String("com.example.tools".to_owned()),
        );
        args.insert(
            "action".to_owned(),
            toml::Value::String("summarize".to_owned()),
        );
        let action = phux_config::keybind::ResolvedAction {
            action: "plugin-action".to_owned(),
            args,
        };
        let mut workspace = Workspace::single(tid(1));
        let effects = run(&action, &mut workspace);
        assert_eq!(
            effects.run_plugin,
            Some(("com.example.tools".to_owned(), "summarize".to_owned()))
        );
        assert!(!effects.bell);
        assert!(!effects.layout_mutated, "no repaint for a spawned run");
    }

    #[test]
    fn plugin_action_missing_args_bells() {
        let mut workspace = Workspace::single(tid(1));
        let effects = run(&bare_action("plugin-action"), &mut workspace);
        assert!(effects.bell, "missing plugin/action args must bell");
        assert!(effects.run_plugin.is_none());
    }

    #[test]
    fn command_palette_action_pushes_overlay() {
        let mut workspace = Workspace::single(tid(1));
        let (effects, overlays) = run_capturing(&bare_action("command-palette"), &mut workspace);
        assert!(
            overlays.is_active(),
            "command-palette should push the palette overlay"
        );
        assert_eq!(overlays.depth(), 1);
        // No layout mutation — opening the palette doesn't touch windows.
        assert!(!effects.layout_mutated);
        assert!(!effects.bell);
    }

    #[test]
    fn window_picker_action_pushes_overlay_with_windows() {
        let mut workspace = Workspace::single(tid(1));
        workspace.add_window("2".to_owned(), tid(2));
        let (effects, overlays) = run_capturing(&bare_action("window-picker"), &mut workspace);
        assert!(overlays.is_active(), "window-picker should push an overlay");
        assert!(!effects.bell);
    }

    #[test]
    fn window_picker_on_empty_workspace_bells() {
        let mut workspace = Workspace::default();
        let (effects, overlays) = run_capturing(&bare_action("window-picker"), &mut workspace);
        assert!(!overlays.is_active(), "no windows ⇒ no overlay");
        assert!(effects.bell);
    }

    #[test]
    fn current_session_window_rows_label_index_name_and_pane_count() {
        let mut workspace = Workspace::single(tid(1)); // window "1", 1 pane
        workspace.add_window("editor".to_owned(), tid(2));
        let items = current_session_window_rows(&workspace);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].label, "0:1");
        assert_eq!(items[0].secondary.as_deref(), Some("1 pane"));
        assert!(items[0].indented, "window rows nest under their session");
        assert_eq!(items[1].label, "1:editor");
        // Each row commits select-window with its index.
        assert_eq!(items[1].action.action, "select-window");
        assert_eq!(
            items[1].action.args.get("index"),
            Some(&toml::Value::Integer(1))
        );
    }

    #[test]
    fn window_picker_groups_windows_under_their_session() {
        let mut workspace = Workspace::single(tid(1));
        workspace.add_window("editor".to_owned(), tid(2));
        let sessions = [sinfo(1, "work"), sinfo(2, "scratch")];
        let items = window_picker_items(
            &workspace,
            &sessions,
            Some(phux_protocol::ids::SessionId::new(1)),
        );
        // Current session ("work") leads, as a header marked "(current)".
        assert!(items[0].is_header());
        assert_eq!(items[0].label, "work (current)");
        // Its windows nest directly beneath, selectable + indented.
        assert!(!items[1].is_header() && items[1].indented);
        assert_eq!(items[1].action.action, "select-window");
        assert_eq!(items[2].action.action, "select-window");
        // The foreign session is a header followed by a switch-session row.
        let scratch = items
            .iter()
            .position(|i| i.is_header() && i.label == "scratch")
            .expect("scratch header present");
        assert_eq!(items[scratch + 1].action.action, "switch-session");
        assert_eq!(
            items[scratch + 1].action.args.get("name"),
            Some(&toml::Value::String("scratch".to_owned())),
        );
    }

    #[test]
    fn window_picker_commit_routes_select_window_through_run_action() {
        // The architectural invariant: a picker selection commits a
        // select-window ResolvedAction that, when fed back through
        // run_action, performs the same per-client switch a numeric prefix
        // binding does.
        let mut workspace = Workspace::single(tid(1));
        workspace.add_window("2".to_owned(), tid(2));
        workspace.select(0); // active = 0
        let items = current_session_window_rows(&workspace);
        // Commit the picker row for window index 1.
        let effects = run(&items[1].action, &mut workspace);
        assert_eq!(
            workspace.active, 1,
            "select-window switched the active window"
        );
        assert!(effects.layout_mutated);
        assert_eq!(effects.set_focus, Some(tid(2)));
        assert!(!effects.set_metadata, "window switch is per-client");
    }

    fn sinfo(id: u32, name: &str) -> phux_protocol::wire::info::SessionInfo {
        phux_protocol::wire::info::SessionInfo::new(phux_protocol::ids::SessionId::new(id), name)
            .with_window_count(1)
    }

    #[test]
    fn session_picker_items_exclude_focused_and_commit_switch_session() {
        let sessions = [sinfo(1, "work"), sinfo(2, "scratch"), sinfo(3, "logs")];
        let items = session_picker_items(&sessions, Some(phux_protocol::ids::SessionId::new(1)));
        // Focused session ("work") is excluded; the two peers remain.
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].label, "scratch");
        assert_eq!(items[1].label, "logs");
        // Each row commits switch-session with the session name.
        assert_eq!(items[0].action.action, "switch-session");
        assert_eq!(
            items[0].action.args.get("name"),
            Some(&toml::Value::String("scratch".to_owned()))
        );
        assert_eq!(items[0].secondary.as_deref(), Some("1 window"));
    }

    #[test]
    fn session_picker_action_pushes_overlay_with_peer_sessions() {
        let mut workspace = Workspace::single(tid(1));
        let sessions = [sinfo(1, "work"), sinfo(2, "scratch")];
        let (effects, overlays) = run_capturing_with_sessions(
            &bare_action("session-picker"),
            &mut workspace,
            &sessions,
            Some(phux_protocol::ids::SessionId::new(1)),
        );
        assert!(
            overlays.is_active(),
            "session-picker should push an overlay"
        );
        assert!(!effects.bell);
    }

    #[test]
    fn session_picker_with_only_current_session_still_opens_for_new() {
        // Even when the client's own session is the only one, the picker
        // opens so the user can create a new session via the "+ New
        // session" row — it no longer bells into a dead end.
        let mut workspace = Workspace::single(tid(1));
        let sessions = [sinfo(1, "work")];
        let (effects, overlays) = run_capturing_with_sessions(
            &bare_action("session-picker"),
            &mut workspace,
            &sessions,
            Some(phux_protocol::ids::SessionId::new(1)),
        );
        assert!(overlays.is_active(), "picker opens to offer + New session");
        assert!(!effects.bell);
    }

    #[test]
    fn session_picker_with_no_sessions_still_opens_for_new() {
        // Before the first ATTACHED snapshot lands the cache is empty; the
        // picker still opens with the "+ New session" row.
        let mut workspace = Workspace::single(tid(1));
        let (effects, overlays) =
            run_capturing_with_sessions(&bare_action("session-picker"), &mut workspace, &[], None);
        assert!(overlays.is_active());
        assert!(!effects.bell);
    }

    #[test]
    fn session_picker_commit_routes_switch_session_through_run_action() {
        // The architectural invariant: a picker row commits a
        // switch-session ResolvedAction that, fed back through run_action,
        // yields the reattach effect keyed by the chosen name.
        let mut workspace = Workspace::single(tid(1));
        let sessions = [sinfo(1, "work"), sinfo(2, "scratch")];
        let items = session_picker_items(&sessions, Some(phux_protocol::ids::SessionId::new(1)));
        let effects = run(&items[0].action, &mut workspace);
        assert_eq!(
            effects.reattach,
            Some(ReattachTarget::Existing("scratch".to_owned())),
            "committing the picker row requests a switch to that session"
        );
    }

    #[test]
    fn switch_session_missing_name_bells() {
        let mut workspace = Workspace::single(tid(1));
        let effects = run(&bare_action("switch-session"), &mut workspace);
        assert!(effects.reattach.is_none());
        assert!(effects.bell, "a switch-session with no name arg bells");
    }

    #[test]
    fn new_session_with_name_requests_create_reattach() {
        let mut workspace = Workspace::single(tid(1));
        let mut args = BTreeMap::new();
        args.insert("name".to_owned(), toml::Value::String("scratch".to_owned()));
        let action = phux_config::keybind::ResolvedAction {
            action: "new-session".to_owned(),
            args,
        };
        let effects = run(&action, &mut workspace);
        assert_eq!(
            effects.reattach,
            Some(ReattachTarget::Create("scratch".to_owned())),
            "new-session with a name requests a create-and-switch"
        );
    }

    #[test]
    fn new_session_without_name_opens_prompt() {
        let mut workspace = Workspace::single(tid(1));
        let (effects, overlays) = run_capturing(&bare_action("new-session"), &mut workspace);
        assert!(
            overlays.is_active(),
            "new-session with no name opens the name prompt"
        );
        assert!(
            effects.reattach.is_none(),
            "the prompt commit drives the re-attach later"
        );
    }

    #[test]
    fn detach_action_requests_detach_effect() {
        let mut workspace = Workspace::default();
        let mut next_request_id = 1;
        let mut pending_splits = HashMap::new();
        let mut pending_windows = HashMap::new();
        let mut overlays = OverlayState::new();
        let theme = Theme::default();
        let mut switch_request = None;
        let mut session_name = String::new();
        let mut zoomed = None;
        let mut sidebar_enabled = false;
        let mut drag: Option<DragGrab> = None;
        let mut ctx = DispatchCtx {
            resolver: None,
            workspace: &mut workspace,
            viewport: (80, 24),
            next_request_id: &mut next_request_id,
            pending_splits: &mut pending_splits,
            pending_windows: &mut pending_windows,
            overlays: &mut overlays,
            keybindings: None,
            theme: &theme,
            sessions: &[],
            focused_session: None,
            session_name: &mut session_name,
            switch_request: &mut switch_request,
            zoomed: &mut zoomed,
            sidebar: None,
            sidebar_enabled: &mut sidebar_enabled,
            has_bar: false,
            drag: &mut drag,
            plugin_actions: &[],
            plugin_tx: None,
        };
        let action = phux_config::keybind::ResolvedAction {
            action: "detach".to_owned(),
            args: BTreeMap::new(),
        };

        let effects = run_action(&action, &mut ctx, None);

        assert!(effects.detach);
        assert!(!effects.layout_mutated);
    }

    #[test]
    fn rename_session_with_name_arg_requests_rename_effect() {
        // An explicit `name` produces the rename-session effect carrying the
        // new name; no prompt is opened. The send + local-name update happen
        // in `apply_action_effects` (async), so run_action only sets the
        // effect.
        let mut workspace = Workspace::single(tid(1));
        let mut args = BTreeMap::new();
        args.insert("name".to_owned(), toml::Value::String("notes".to_owned()));
        let action = phux_config::keybind::ResolvedAction {
            action: "rename-session".to_owned(),
            args,
        };
        let effects = run(&action, &mut workspace);
        assert_eq!(
            effects.rename_session.as_deref(),
            Some("notes"),
            "rename-session with a name requests the rename effect",
        );
    }

    #[test]
    fn rename_session_without_name_opens_prompt_prefilled() {
        // No `name` arg opens the prompt pre-filled with the current session
        // name; the rename itself is deferred to the prompt commit.
        let mut workspace = Workspace::single(tid(1));
        let mut next_request_id = 100;
        let mut pending_splits = HashMap::new();
        let mut pending_windows = HashMap::new();
        let mut overlays = OverlayState::new();
        let theme = Theme::default();
        let mut switch_request = None;
        let mut session_name = "work".to_owned();
        let mut zoomed = None;
        let mut sidebar_enabled = false;
        let mut drag: Option<DragGrab> = None;
        let effects = {
            let mut ctx = DispatchCtx {
                resolver: None,
                workspace: &mut workspace,
                viewport: (80, 24),
                next_request_id: &mut next_request_id,
                pending_splits: &mut pending_splits,
                pending_windows: &mut pending_windows,
                overlays: &mut overlays,
                keybindings: None,
                theme: &theme,
                sessions: &[],
                focused_session: None,
                session_name: &mut session_name,
                switch_request: &mut switch_request,
                zoomed: &mut zoomed,
                sidebar: None,
                sidebar_enabled: &mut sidebar_enabled,
                has_bar: false,
                drag: &mut drag,
                plugin_actions: &[],
                plugin_tx: None,
            };
            run_action(&bare_action("rename-session"), &mut ctx, None)
        };
        assert!(
            overlays.is_active(),
            "no-arg rename-session opens the name prompt",
        );
        assert!(
            effects.rename_session.is_none(),
            "the prompt commit drives the rename later",
        );
    }

    #[test]
    fn rename_session_prompt_commits_rename_session_action() {
        // The prompt the bare action opens must commit a
        // `rename-session { name }` ResolvedAction, so feeding it back
        // through run_action yields the rename effect (the same single
        // dispatch path rename-window uses).
        use crate::render::overlay::{OverlayCommand, RenderOverlay};
        use phux_protocol::input::key::{KeyAction, KeyEvent, ModSet, PhysicalKey};

        let mut prompt = PromptOverlay::rename_session("work", &Theme::default());
        let press = |key: PhysicalKey, text: Option<&str>| KeyEvent {
            action: KeyAction::Press,
            key,
            mods: ModSet::empty(),
            consumed_mods: ModSet::empty(),
            composing: false,
            text: text.map(ToOwned::to_owned),
            unshifted_codepoint: None,
        };
        // Clear the prefilled "work" and type "notes".
        for _ in 0..4 {
            let _ = prompt.handle_key(&press(PhysicalKey::Backspace, None));
        }
        for ch in ['n', 'o', 't', 'e', 's'] {
            let _ = prompt.handle_key(&press(PhysicalKey::A, Some(&ch.to_string())));
        }
        let OverlayCommand::Commit(resolved) = prompt.handle_key(&press(PhysicalKey::Enter, None))
        else {
            panic!("Enter on a non-empty prompt should commit");
        };
        assert_eq!(resolved.action, "rename-session");

        let mut workspace = Workspace::single(tid(1));
        let effects = run(&resolved, &mut workspace);
        assert_eq!(
            effects.rename_session.as_deref(),
            Some("notes"),
            "the committed prompt action yields the rename effect with the typed name",
        );
    }

    // -- overlay input routing (regression: prefix key swallowed) ---------
    // The `RecordingOverlay` test double lives in `crate::render::overlay`
    // because implementing `RenderOverlay::render` names ratatui types, which
    // the boundary guard confines to `render/`.

    /// Regression (wave-hunt/client-tui): while an overlay is active the
    /// keybind resolver must be bypassed, so the leader prefix key (and any
    /// mid-chord key) reaches the overlay as literal input instead of being
    /// swallowed by the resolver.
    ///
    /// Pre-fix: feeding `C-a` (the default leader) while an overlay was up
    /// returned `ChordOutcome::Partial`, hit `continue`, and the key never
    /// reached the overlay — *and* left the resolver mid-chord so it
    /// intercepted the next key too. A user typing a name into the rename
    /// prompt that contained the leader chord lost characters.
    #[tokio::test]
    async fn overlay_active_prefix_key_reaches_overlay_not_resolver() {
        use phux_protocol::input::key::{KeyAction, KeyEvent, ModSet, PhysicalKey};

        let cfg = phux_config::parse_str(
            phux_config::DEFAULT_CONFIG_TOML,
            std::path::Path::new("default.toml"),
        )
        .expect("default config parses");
        let mut resolver =
            phux_config::keybind::Resolver::new(&cfg.keybindings).expect("resolver builds");

        // Record what the overlay receives.
        let keys = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let mut overlays = OverlayState::new();
        overlays.push(Box::new(crate::render::overlay::RecordingOverlay {
            keys: keys.clone(),
        }));

        let (a, _b) = tokio::net::UnixStream::pair().expect("uds pair");
        let mut conn = Connection::from_stream(a);
        let mut out: Vec<u8> = Vec::new();
        let mut workspace = Workspace::single(tid(1));
        let mut focused_pane = Some(tid(1));
        let mut detach_pending = false;
        let mut predict =
            PredictionState::new(crate::predict::PredictiveConfig::disabled(), 80, 24);
        let overlay = Overlay;
        let mut panes: HashMap<TerminalId, PaneSlot> = HashMap::new();
        let mut next_request_id = 1;
        let mut pending_splits = HashMap::new();
        let mut pending_windows = HashMap::new();
        let theme = Theme::default();
        let mut switch_request = None;
        let mut session_name = String::new();

        // The default leader is `C-a`. Feed it, then a printable key.
        let leader = KeyEvent {
            action: KeyAction::Press,
            key: PhysicalKey::A,
            mods: ModSet::CTRL,
            consumed_mods: ModSet::CTRL,
            composing: false,
            text: None,
            unshifted_codepoint: Some(u32::from(b'a')),
        };
        let letter = KeyEvent {
            action: KeyAction::Press,
            key: PhysicalKey::X,
            mods: ModSet::empty(),
            consumed_mods: ModSet::empty(),
            composing: false,
            text: Some("x".to_owned()),
            unshifted_codepoint: Some(u32::from(b'x')),
        };

        let mut zoomed = None;
        let mut sidebar_enabled = false;
        let mut drag: Option<DragGrab> = None;
        let mut ctx = DispatchCtx {
            resolver: Some(&mut resolver),
            workspace: &mut workspace,
            viewport: (80, 24),
            next_request_id: &mut next_request_id,
            pending_splits: &mut pending_splits,
            pending_windows: &mut pending_windows,
            overlays: &mut overlays,
            keybindings: None,
            theme: &theme,
            sessions: &[],
            focused_session: None,
            session_name: &mut session_name,
            switch_request: &mut switch_request,
            zoomed: &mut zoomed,
            sidebar: None,
            sidebar_enabled: &mut sidebar_enabled,
            has_bar: false,
            drag: &mut drag,
            plugin_actions: &[],
            plugin_tx: None,
        };
        dispatch_input_events(
            &mut out,
            &mut conn,
            vec![InputEvent::Key(leader), InputEvent::Key(letter)],
            &mut focused_pane,
            &mut detach_pending,
            &mut predict,
            &overlay,
            &mut panes,
            &mut ctx,
        )
        .await
        .expect("dispatch");

        let received = keys.borrow();
        assert_eq!(
            received.len(),
            2,
            "both the leader chord and the following key must reach the overlay; got {received:?}",
        );
        assert_eq!(received[0].key, PhysicalKey::A);
        assert!(received[0].mods.contains(ModSet::CTRL));
        assert_eq!(received[1].key, PhysicalKey::X);
    }

    // -- which-key popup passthrough (phux-foz.2) --------------------------

    /// Drive `dispatch_input_events` with the given events against a
    /// resolver already pending at the prefix and the which-key popup on
    /// the overlay stack. Returns `(overlays_active_after, detach_pending,
    /// resolver_pending_after)`.
    #[allow(
        clippy::future_not_send,
        reason = "client-side libghostty Terminal is !Send; ADR-0003 binds us to current-thread"
    )]
    async fn dispatch_with_which_key_popup(events: Vec<InputEvent>) -> (bool, bool, bool) {
        let cfg = phux_config::parse_str(
            phux_config::DEFAULT_CONFIG_TOML,
            std::path::Path::new("default.toml"),
        )
        .expect("default config parses");
        let mut resolver =
            phux_config::keybind::Resolver::new(&cfg.keybindings).expect("resolver builds");
        // Walk to the pending-prefix state the popup describes.
        let prefix =
            phux_config::keybind::parse_chord(&cfg.keybindings.prefix).expect("prefix parses");
        assert_eq!(resolver.feed(prefix), phux_config::keybind::Feed::Partial);
        assert!(resolver.pending_at_prefix());

        let theme = Theme::default();
        let mut overlays = OverlayState::new();
        overlays.push(Box::new(
            crate::render::overlay::WhichKeyOverlay::from_config(&cfg.keybindings, &theme),
        ));
        assert!(overlays.top_is_passthrough());

        let (a, _b) = tokio::net::UnixStream::pair().expect("uds pair");
        let mut conn = Connection::from_stream(a);
        let mut out: Vec<u8> = Vec::new();
        let mut workspace = Workspace::single(tid(1));
        let mut focused_pane = Some(tid(1));
        let mut detach_pending = false;
        let mut predict =
            PredictionState::new(crate::predict::PredictiveConfig::disabled(), 80, 24);
        let overlay = Overlay;
        let mut panes: HashMap<TerminalId, PaneSlot> = HashMap::new();
        let mut next_request_id = 1;
        let mut pending_splits = HashMap::new();
        let mut pending_windows = HashMap::new();
        let mut switch_request = None;
        let mut session_name = String::new();
        let mut zoomed = None;
        let mut sidebar_enabled = false;
        let mut drag: Option<DragGrab> = None;
        let mut ctx = DispatchCtx {
            resolver: Some(&mut resolver),
            workspace: &mut workspace,
            viewport: (80, 24),
            next_request_id: &mut next_request_id,
            pending_splits: &mut pending_splits,
            pending_windows: &mut pending_windows,
            overlays: &mut overlays,
            keybindings: Some(&cfg.keybindings),
            theme: &theme,
            sessions: &[],
            focused_session: None,
            session_name: &mut session_name,
            switch_request: &mut switch_request,
            zoomed: &mut zoomed,
            sidebar: None,
            sidebar_enabled: &mut sidebar_enabled,
            has_bar: false,
            drag: &mut drag,
            plugin_actions: &[],
            plugin_tx: None,
        };
        dispatch_input_events(
            &mut out,
            &mut conn,
            events,
            &mut focused_pane,
            &mut detach_pending,
            &mut predict,
            &overlay,
            &mut panes,
            &mut ctx,
        )
        .await
        .expect("dispatch");
        (overlays.is_active(), detach_pending, resolver.is_pending())
    }

    fn press(key: phux_protocol::input::key::PhysicalKey, text: Option<&str>) -> InputEvent {
        use phux_protocol::input::key::{KeyAction, KeyEvent, ModSet};
        InputEvent::Key(KeyEvent {
            action: KeyAction::Press,
            key,
            mods: ModSet::empty(),
            consumed_mods: ModSet::empty(),
            composing: false,
            text: text.map(ToOwned::to_owned),
            unshifted_codepoint: text.and_then(|t| t.chars().next()).map(u32::from),
        })
    }

    /// phux-foz.2 requirement 3 (execute path): with the which-key popup
    /// up and the prefix pending, the next key must dismiss the popup AND
    /// execute its prefix-table binding exactly as if the popup had never
    /// appeared — the popup eats nothing.
    #[tokio::test]
    async fn which_key_popup_next_chord_dismisses_and_executes() {
        use phux_protocol::input::key::PhysicalKey;
        // Default prefix table binds `d` = detach.
        let (overlay_active, detach_pending, resolver_pending) =
            dispatch_with_which_key_popup(vec![press(PhysicalKey::D, Some("d"))]).await;
        assert!(!overlay_active, "the chord must dismiss the popup");
        assert!(
            detach_pending,
            "the chord must still execute its binding (C-a d = detach)"
        );
        assert!(!resolver_pending, "the chord resolved; nothing pending");
    }

    /// phux-foz.2 requirement 3 (cancel path): Esc dismisses the popup
    /// and cancels the pending prefix — the binding does NOT run, and a
    /// following prefix-table key is a plain keystroke for the pane.
    #[tokio::test]
    async fn which_key_popup_esc_cancels_the_prefix() {
        use phux_protocol::input::key::PhysicalKey;
        let (overlay_active, detach_pending, resolver_pending) =
            dispatch_with_which_key_popup(vec![
                press(PhysicalKey::Escape, None),
                // With the prefix cancelled, `d` must NOT resolve to detach.
                press(PhysicalKey::D, Some("d")),
            ])
            .await;
        assert!(!overlay_active, "Esc must dismiss the popup");
        assert!(!resolver_pending, "Esc must cancel the pending prefix");
        assert!(
            !detach_pending,
            "after Esc, `d` is a plain pane keystroke, not `C-a d`"
        );
    }

    #[tokio::test]
    async fn copy_mode_page_scroll_mutates_focused_terminal_viewport() {
        use phux_protocol::input::key::{KeyAction, KeyEvent, ModSet, PhysicalKey};

        fn visible_prefix(
            panes: &mut HashMap<TerminalId, PaneSlot>,
            id: &TerminalId,
            row: u16,
        ) -> String {
            let slot = panes.get_mut(id).expect("pane");
            (0..6)
                .filter_map(|col| {
                    slot.renderer
                        .read_grapheme_string_at(&slot.terminal, row, col)
                        .expect("read cell")
                })
                .collect()
        }

        let (a, _b) = tokio::net::UnixStream::pair().expect("uds pair");
        let mut conn = Connection::from_stream(a);
        let mut out: Vec<u8> = Vec::new();
        let mut workspace = Workspace::single(tid(1));
        let mut focused_pane = Some(tid(1));
        let mut detach_pending = false;
        let mut predict = PredictionState::new(crate::predict::PredictiveConfig::disabled(), 8, 4);
        let overlay = Overlay;
        let mut panes: HashMap<TerminalId, PaneSlot> = HashMap::new();
        let mut slot = PaneSlot::new_with_size(8, 4).expect("pane slot");
        for n in 0..10 {
            slot.terminal.vt_write(format!("line{n:02}\r\n").as_bytes());
        }
        panes.insert(tid(1), slot);

        let before = visible_prefix(&mut panes, &tid(1), 0);

        let mut overlays = OverlayState::new();
        overlays.push(Box::new(crate::render::overlay::CopyModeOverlay::new(
            0, 0, 8, 4,
        )));
        let mut next_request_id = 1;
        let mut pending_splits = HashMap::new();
        let mut pending_windows = HashMap::new();
        let theme = Theme::default();
        let mut switch_request = None;
        let mut session_name = String::new();
        let mut zoomed = None;
        let mut sidebar_enabled = false;
        let mut drag: Option<DragGrab> = None;
        let mut ctx = DispatchCtx {
            resolver: None,
            workspace: &mut workspace,
            viewport: (8, 4),
            next_request_id: &mut next_request_id,
            pending_splits: &mut pending_splits,
            pending_windows: &mut pending_windows,
            overlays: &mut overlays,
            keybindings: None,
            theme: &theme,
            sessions: &[],
            focused_session: None,
            session_name: &mut session_name,
            switch_request: &mut switch_request,
            zoomed: &mut zoomed,
            sidebar: None,
            sidebar_enabled: &mut sidebar_enabled,
            has_bar: false,
            drag: &mut drag,
            plugin_actions: &[],
            plugin_tx: None,
        };
        let page_up = KeyEvent {
            action: KeyAction::Press,
            key: PhysicalKey::PageUp,
            mods: ModSet::empty(),
            consumed_mods: ModSet::empty(),
            composing: false,
            text: None,
            unshifted_codepoint: None,
        };

        let changed = dispatch_input_events(
            &mut out,
            &mut conn,
            vec![InputEvent::Key(page_up)],
            &mut focused_pane,
            &mut detach_pending,
            &mut predict,
            &overlay,
            &mut panes,
            &mut ctx,
        )
        .await
        .expect("dispatch");

        let after = visible_prefix(&mut panes, &tid(1), 0);
        assert!(changed, "scrolling copy-mode should trigger a repaint");
        assert_ne!(
            before, after,
            "dispatch should apply copy-mode scroll to the focused pane viewport"
        );
    }

    // ---------- phux-fce4: sidebar hit targets ----------

    /// The pure click→action mapping: window blocks commit
    /// `select-window { index }`, the footer rows `new-window` and
    /// `command-palette`, and blank/separator cells nothing.
    #[test]
    fn sidebar_click_action_maps_rows_to_registry_actions() {
        // Left-docked 20-column strip over a 24-row viewport with a status
        // bar: rows 0..=22, footer on rows 21 (new) and 22 (menu).
        let strip = crate::layout::Rect {
            x: 0,
            y: 0,
            w: 20,
            h: 23,
        };
        // Window 1's name row (y = 2) and branch row (y = 3) both select it.
        for y in [2, 3] {
            let resolved = sidebar_click_action(strip, 2, 4, y).expect("window row hits");
            assert_eq!(resolved.action, "select-window");
            assert_eq!(index_arg(&resolved), Some(1));
        }
        let new = sidebar_click_action(strip, 2, 4, 21).expect("new row hits");
        assert_eq!(new.action, "new-window");
        assert!(new.args.is_empty());
        let menu = sidebar_click_action(strip, 2, 4, 22).expect("menu row hits");
        assert_eq!(menu.action, "command-palette");
        // Blank padding row and the separator column commit nothing.
        assert!(sidebar_click_action(strip, 2, 4, 10).is_none());
        assert!(sidebar_click_action(strip, 2, 19, 0).is_none());
    }

    /// Every action a sidebar click can commit must be a dispatched action
    /// name — the same lockstep the palette registry test enforces.
    #[test]
    fn sidebar_click_actions_are_dispatched_names() {
        let strip = crate::layout::Rect {
            x: 0,
            y: 0,
            w: 20,
            h: 23,
        };
        for y in 0..strip.h {
            if let Some(resolved) = sidebar_click_action(strip, 3, 2, y) {
                assert!(
                    ACTION_NAMES.contains(&resolved.action.as_str()),
                    "sidebar committed `{}`, which run_action does not dispatch",
                    resolved.action,
                );
            }
        }
    }

    fn left_press_at(x: u16, y: u16) -> InputEvent {
        use phux_protocol::input::key::ModSet;
        InputEvent::Mouse(MouseEvent {
            action: MouseAction::Press,
            button: MouseButton::Left,
            mods: ModSet::empty(),
            x: f64::from(x),
            y: f64::from(y),
        })
    }

    /// Drive `dispatch_input_events` with a left-docked sidebar reservation
    /// and one mouse event; returns `(active_window, overlay_active,
    /// pending_window_count)` so the callers can assert each affordance's
    /// end-to-end effect.
    #[allow(
        clippy::future_not_send,
        reason = "client-side libghostty Terminal is !Send; ADR-0003 binds us to current-thread"
    )]
    async fn dispatch_sidebar_click(ev: InputEvent) -> (usize, bool, usize) {
        let (a, _b) = tokio::net::UnixStream::pair().expect("uds pair");
        let mut conn = Connection::from_stream(a);
        let mut out: Vec<u8> = Vec::new();
        let mut workspace = Workspace::single(tid(1));
        workspace.add_window("two".to_owned(), tid(2));
        workspace.select(0);
        let mut focused_pane = Some(tid(1));
        let mut detach_pending = false;
        let mut predict =
            PredictionState::new(crate::predict::PredictiveConfig::disabled(), 80, 24);
        let overlay = Overlay;
        let mut panes: HashMap<TerminalId, PaneSlot> = HashMap::new();
        let mut next_request_id = 1;
        let mut pending_splits = HashMap::new();
        let mut pending_windows = HashMap::new();
        let mut overlays = OverlayState::new();
        let theme = Theme::default();
        let mut switch_request = None;
        let mut session_name = String::new();
        let mut zoomed = None;
        let mut sidebar_enabled = true;
        let mut drag: Option<DragGrab> = None;
        let mut ctx = DispatchCtx {
            resolver: None,
            workspace: &mut workspace,
            viewport: (80, 24),
            next_request_id: &mut next_request_id,
            pending_splits: &mut pending_splits,
            pending_windows: &mut pending_windows,
            overlays: &mut overlays,
            keybindings: None,
            theme: &theme,
            sessions: &[],
            focused_session: None,
            session_name: &mut session_name,
            switch_request: &mut switch_request,
            zoomed: &mut zoomed,
            sidebar: Some(SidebarReservation {
                edge: super::super::paint::SidebarEdge::Left,
                width: 20,
            }),
            sidebar_enabled: &mut sidebar_enabled,
            has_bar: true,
            drag: &mut drag,
            plugin_actions: &[],
            plugin_tx: None,
        };
        dispatch_input_events(
            &mut out,
            &mut conn,
            vec![ev],
            &mut focused_pane,
            &mut detach_pending,
            &mut predict,
            &overlay,
            &mut panes,
            &mut ctx,
        )
        .await
        .expect("dispatch");
        (
            workspace.active,
            overlays.is_active(),
            pending_windows.len(),
        )
    }

    /// A left press on the second window's block switches to it — the
    /// mouse route runs the same `select-window` a keybinding would.
    #[tokio::test]
    async fn sidebar_click_on_window_block_selects_it() {
        // Strip rows with a status bar: h = 23; window 1's name row is y=2.
        let (active, overlay_active, pending) = dispatch_sidebar_click(left_press_at(3, 2)).await;
        assert_eq!(active, 1, "clicking window 1's block must select it");
        assert!(!overlay_active);
        assert_eq!(pending, 0);
    }

    /// A left press on `+ new` parks a `new-window` spawn (the reply opens
    /// the window), exactly like the `new-window` chord.
    #[tokio::test]
    async fn sidebar_click_on_new_parks_a_window_spawn() {
        let (active, overlay_active, pending) = dispatch_sidebar_click(left_press_at(3, 21)).await;
        assert_eq!(active, 0, "spawn is parked; no window switch yet");
        assert!(!overlay_active);
        assert_eq!(pending, 1, "new-window spawn must be parked");
    }

    /// A left press on `= menu` opens the command palette overlay — the
    /// session/plugin menu built from the action registry.
    #[tokio::test]
    async fn sidebar_click_on_menu_opens_the_command_palette() {
        let (active, overlay_active, pending) = dispatch_sidebar_click(left_press_at(3, 22)).await;
        assert_eq!(active, 0);
        assert!(overlay_active, "menu click must push the palette overlay");
        assert_eq!(pending, 0);
    }

    /// Pointer events over the strip never leak into pane routing: a press
    /// on a blank row is consumed, mutating nothing.
    #[tokio::test]
    async fn sidebar_consumes_clicks_on_blank_rows() {
        let (active, overlay_active, pending) = dispatch_sidebar_click(left_press_at(3, 10)).await;
        assert_eq!(active, 0);
        assert!(!overlay_active);
        assert_eq!(pending, 0);
    }
}
