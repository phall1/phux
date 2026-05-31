//! Input dispatcher: translates parser-emitted events into wire frames
//! or layout-action effects.
//!
//! Owns the resolver-intercept path (prefix chord → `ResolvedAction` →
//! mutate the active window of the `Workspace`), the predict overlay's
//! keystroke feed, and the parked-spawn bookkeeping (`PendingSplit` /
//! `PendingWindow`) that bridges a local `split-pane` / `new-window`
//! chord to its remote `SPAWN_TERMINAL` reply.

use std::collections::HashMap;

use phux_protocol::TerminalId;
use phux_protocol::input::InputEvent;
use phux_protocol::wire::frame::{Command as WireCommand, FrameKind, Scope};

use super::actions::{self, ActionError, PendingSplit, PendingWindow};
use super::connection::Connection;
use super::driver::{AttachError, DEFAULT_COLLECTION_ID, LAYOUT_KEY, PaneSlot};
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
        if ctx.overlays.is_active() {
            if let InputEvent::Key(ref key_event) = ev {
                if let Some(resolver) = ctx.resolver.as_deref_mut() {
                    resolver.reset();
                }
                let was_active = ctx.overlays.is_active();
                // phux-ahv.1: an overlay may commit an action (e.g. the
                // rename prompt returning `rename-window { name }`); run
                // it through the same path as a keybinding.
                if let OverlayOutcome::RunAction(resolved) = ctx.overlays.handle_key(key_event) {
                    let effects = run_action(&resolved, ctx, focused_pane.as_ref());
                    if apply_action_effects(
                        effects,
                        out,
                        conn,
                        ctx,
                        focused_pane,
                        detach_pending,
                        predict,
                    )
                    .await?
                    {
                        layout_changed = true;
                    }
                }
                // On dismiss, repaint everything: the overlay scribbled
                // over pane cells and we need a coherent base for the
                // next TERMINAL_OUTPUT.
                if was_active && !ctx.overlays.is_active() {
                    layout_changed = true;
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
                    )
                    .await?
                    {
                        layout_changed = true;
                    }
                    continue;
                }
            }
        }
        // phux-4li.6: INPUT_MOUSE routing + click-to-focus. The parser
        // emits mouse coordinates in outer-viewport cells (treated as
        // 1-px-per-cell f64 per SPEC §9.2.1); we hit-test against the
        // multi-pane composition's `Rect`s. A click on a divider cell
        // is dropped (drag-to-resize is deferred per docs/consumers/tui.md §7); a
        // click in a non-focused pane updates focus AND forwards the
        // event with pane-local coordinates substituted.
        if let InputEvent::Mouse(ref mouse) = ev {
            use super::multi_pane::{RouteDecision, route_mouse_event};
            let Some(active_ls) = ctx.workspace.active_window() else {
                tracing::debug!("dropping mouse event: no active window");
                continue;
            };
            match route_mouse_event(active_ls, ctx.viewport, mouse) {
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
                        // The predict overlay is anchored to the old
                        // pane's cursor; dropping the queue avoids a
                        // stale ghost echo painting into the new pane
                        // before the next TERMINAL_OUTPUT reconciles.
                        predict.clear();
                        // Heavy-edge chrome moves with focus; repaint
                        // dividers + all leaves so the focused pane's
                        // surrounding edges render heavy.
                        layout_changed = true;
                    }
                    let mut routed = *mouse;
                    routed.x = pane_x;
                    routed.y = pane_y;
                    conn.send(&FrameKind::InputMouse {
                        terminal_id: target,
                        event: routed,
                    })
                    .await?;
                    continue;
                }
                RouteDecision::DividerNoOp => {
                    tracing::trace!(x = mouse.x, y = mouse.y, "dropping mouse on divider");
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
        let frame = ev.into_frame(pane.clone());
        conn.send(&frame).await?;
    }
    // Paint the prediction overlay once per dispatch batch so a burst of
    // keystrokes produces a single positioned write run, not one per
    // event. The overlay is a no-op on an empty queue.
    if predicted_any {
        let _ = overlay.render(predict, out);
    }
    // Hand the layout-mutation signal back to `main_loop`, which holds
    // the status-bar painter and session name needed for a proper full
    // frame. We never paint from here.
    Ok(layout_changed)
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
async fn apply_action_effects<W: super::RenderSink>(
    effects: ActionEffects,
    out: &mut W,
    conn: &mut Connection,
    ctx: &mut DispatchCtx<'_>,
    focused_pane: &mut Option<TerminalId>,
    detach_pending: &mut bool,
    predict: &mut PredictionState,
) -> Result<bool, AttachError> {
    let layout_changed = effects.layout_mutated;
    if effects.set_focus.is_some() {
        *focused_pane = effects.set_focus;
    }
    if effects.clear_predict {
        predict.clear();
    }
    if effects.set_metadata {
        // Encoding can fail only on an empty workspace (we just produced
        // it — shouldn't happen), but propagate cleanly if it ever does.
        if let Some(bytes) = encode_layout_or_log(ctx.workspace) {
            let request_id = *ctx.next_request_id;
            *ctx.next_request_id = ctx.next_request_id.wrapping_add(1);
            conn.send(&FrameKind::SetMetadata {
                request_id,
                scope: Scope::Collection(DEFAULT_COLLECTION_ID),
                key: LAYOUT_KEY.to_owned(),
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
    // rename-session: send RENAME_SESSION for the current session and
    // optimistically reflect the new name locally. The server is
    // authoritative — the next ATTACHED snapshot overwrites `session_name`
    // (which is also how other attached clients learn the rename; a live
    // SESSION_RENAMED push is out of scope for this pass). A no-op rename
    // (new == current) is dropped: nothing to send, nothing to repaint.
    let renamed = if let Some(new_name) = effects.rename_session.filter(|n| n != &*ctx.session_name)
    {
        let request_id = *ctx.next_request_id;
        *ctx.next_request_id = ctx.next_request_id.wrapping_add(1);
        conn.send(&FrameKind::Command {
            request_id,
            command: WireCommand::RenameSession {
                collection: DEFAULT_COLLECTION_ID,
                name: ctx.session_name.clone(),
                new_name: new_name.clone(),
            },
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
    /// of the layout in [`handle_server_frame`].
    kill_frames: Vec<FrameKind>,
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
    "detach",
    "next-pane",
    "previous-pane",
    "command-palette",
    "window-picker",
    "session-picker",
    "switch-session",
    "new-session",
];

/// Dispatch a resolved action against the driver's context.
///
/// Returns the [`ActionEffects`] the caller needs to apply. The function
/// is sync: it never touches the connection — frame I/O happens in the
/// caller (`dispatch_input_events`) so a hypothetical async wire-send
/// failure doesn't leave layout state half-mutated.
#[allow(
    clippy::too_many_lines,
    reason = "per-action arms accrete one-by-one; splitting into per-action helpers would obscure the central dispatch table"
)]
fn run_action(
    resolved: &phux_config::keybind::ResolvedAction,
    ctx: &mut DispatchCtx<'_>,
    focused: Option<&TerminalId>,
) -> ActionEffects {
    let _ = focused;
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
            // Terminal under DEFAULT_COLLECTION_ID and replies with
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
                collection: DEFAULT_COLLECTION_ID,
                command: None,
                cwd: None,
                env: None,
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
        "new-window" => {
            // phux-4li.15: open a new window. Spawn a fresh Terminal
            // (same SPAWN as a split) and park a `PendingWindow`; the
            // reply (`handle_server_frame`'s TerminalSpawned arm) adds a
            // window seeded on the spawned pane and makes it active. The
            // new pane is a bare leaf — the server files it under the
            // default Collection; the TUI groups it into a window itself
            // (windows are a client convention, ADR-0017).
            let request_id = *ctx.next_request_id;
            *ctx.next_request_id = ctx.next_request_id.wrapping_add(1);
            let name = ctx.workspace.default_window_name();
            let frame = FrameKind::SpawnTerminal {
                request_id,
                collection: DEFAULT_COLLECTION_ID,
                command: None,
                cwd: None,
                env: None,
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
                match actions::apply_resize(ls, dir, amount, ctx.viewport) {
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
        "command-palette" => {
            // phux-ahv.8: push the command palette. It lists every action
            // the registry knows about, annotated with its currently-bound
            // chord from the live keybindings snapshot. Choosing a row
            // commits that action's `ResolvedAction`, which flows back
            // through this same `run_action` — keybinds and the palette
            // share one dispatch path (the architectural invariant).
            let items = super::action_registry::palette_items(ctx.keybindings);
            ctx.overlays.push(Box::new(SelectList::new(
                "command palette",
                items,
                ctx.theme,
            )));
        }
        "window-picker" => {
            // phux-4li.19: push the `<leader> w` window picker. Each row is
            // a window (`index:name`, pane count as the secondary label)
            // that commits `select-window { index }` — the same action the
            // numeric prefix bindings use, so switching flows through the
            // single dispatch path. With no windows it bells.
            let items = window_picker_items(ctx.workspace);
            if items.is_empty() {
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
        other => {
            tracing::debug!(action = other, "unhandled resolved action");
        }
    }
    effects
}

/// Pull a `Direction` out of a [`ResolvedAction`]'s `direction = "..."`
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

/// Pull an `amount = N` arg out of a [`ResolvedAction`]. TOML integers
/// decode as `i64`; we clamp to `i16` (the [`actions::apply_resize`]
/// signature). Out-of-range values are silently clamped — a `resize-pane
/// amount = 99999` user binding gets a 32767-cell amount, which the
/// underflow guard inside `apply_resize` then rejects.
#[allow(clippy::cast_possible_truncation)]
fn amount_arg(resolved: &phux_config::keybind::ResolvedAction) -> Option<i16> {
    let v = resolved.args.get("amount")?.as_integer()?;
    Some(v.clamp(i64::from(i16::MIN), i64::from(i16::MAX)) as i16)
}

/// Pull a window index out of a [`ResolvedAction`]'s `index = N` arg.
/// Negative or non-integer values yield `None` (the caller bells).
fn index_arg(resolved: &phux_config::keybind::ResolvedAction) -> Option<usize> {
    let v = resolved.args.get("index")?.as_integer()?;
    usize::try_from(v).ok()
}

/// Pull a window name out of a [`ResolvedAction`]'s `name = "..."` arg.
fn name_arg(resolved: &phux_config::keybind::ResolvedAction) -> Option<String> {
    resolved.args.get("name")?.as_str().map(ToOwned::to_owned)
}

/// Build the `<leader> w` picker's rows from the client's [`Workspace`]
/// windows (phux-4li.19).
///
/// Each row's label is `index:name` (matching the status-bar window tab
/// convention) with the pane count as the dimmed secondary; choosing it
/// commits `select-window { index }`, which `run_action` routes through
/// the same per-client window switch the numeric prefix bindings use.
fn window_picker_items(workspace: &Workspace) -> Vec<SelectItem> {
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
        let items = crate::attach::action_registry::palette_items(Some(&cfg.keybindings));
        let detach = items
            .iter()
            .find(|i| i.action.action == "detach")
            .expect("detach in palette");
        let mut workspace = Workspace::default();
        let effects = run(&detach.action, &mut workspace);
        assert!(effects.detach, "committing the detach palette row detaches");
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
    fn window_picker_items_label_index_name_and_pane_count() {
        let mut workspace = Workspace::single(tid(1)); // window "1", 1 pane
        workspace.add_window("editor".to_owned(), tid(2));
        let items = window_picker_items(&workspace);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].label, "0:1");
        assert_eq!(items[0].secondary.as_deref(), Some("1 pane"));
        assert_eq!(items[1].label, "1:editor");
        // Each row commits select-window with its index.
        assert_eq!(items[1].action.action, "select-window");
        assert_eq!(
            items[1].action.args.get("index"),
            Some(&toml::Value::Integer(1))
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
        let items = window_picker_items(&workspace);
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

    /// An overlay that records every key it is handed and never dismisses.
    /// Lets a test assert exactly which keystrokes reached the overlay.
    #[derive(Default)]
    struct RecordingOverlay {
        keys: std::rc::Rc<std::cell::RefCell<Vec<phux_protocol::input::key::KeyEvent>>>,
    }

    impl crate::render::overlay::RenderOverlay for RecordingOverlay {
        fn render(&self, _area: ratatui::layout::Rect, _buf: &mut ratatui::buffer::Buffer) {}
        fn handle_key(
            &mut self,
            key: &phux_protocol::input::key::KeyEvent,
        ) -> crate::render::overlay::OverlayCommand {
            self.keys.borrow_mut().push(key.clone());
            crate::render::overlay::OverlayCommand::Stay
        }
    }

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
        overlays.push(Box::new(RecordingOverlay { keys: keys.clone() }));

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
}
