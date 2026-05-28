//! Input dispatcher: translates parser-emitted events into wire frames
//! or layout-action effects.
//!
//! Owns the resolver-intercept path (prefix chord → `ResolvedAction` →
//! mutate `LayoutState`), the predict overlay's keystroke feed, and the
//! parked-split bookkeeping (`PendingSplit`) that bridges a local
//! `split-pane` chord to its remote `SPAWN_TERMINAL` reply.

use std::collections::HashMap;
use std::io;

use phux_protocol::TerminalId;
use phux_protocol::wire::frame::{FrameKind, Scope};

use super::actions::{self, ActionError, PendingSplit};
use super::connection::Connection;
use super::driver::{AttachError, DEFAULT_COLLECTION_ID, LAYOUT_KEY, PaneSlot};
use super::input::InputEvent;
use crate::layout::{Direction, LayoutState, SplitDir};
use crate::predict::{Overlay, PredictionState};
use crate::render::overlay::{HelpOverlay, OverlayState};

/// Mutable context the input-dispatch path needs to update on a chord
/// that resolves to a layout action (phux-4li.5). Bundles the items
/// that would otherwise inflate `dispatch_input_events`'s argument
/// list past clippy's threshold.
pub(super) struct DispatchCtx<'a> {
    /// Keybind resolver state. `None` when the on-disk config failed
    /// to parse; the dispatcher then forwards every key to the focused
    /// pane unchanged.
    pub resolver: Option<&'a mut phux_config::keybind::Resolver>,
    /// Client-side layout mirror. Action helpers in [`super::actions`]
    /// take `&LayoutState` and return a new state which the dispatcher
    /// swaps in place.
    pub layout_state: &'a mut LayoutState,
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
    /// phux-5ke.4: overlay stack. When non-empty the dispatcher routes
    /// key events to the active overlay (no resolver, no predict, no
    /// pane forwarding) and the `show-help` action pushes onto it.
    pub overlays: &'a mut OverlayState,
    /// phux-5ke.4: snapshot of the on-disk keybindings, captured at
    /// driver start. The help overlay reads this to render the modal
    /// body. `None` when config load failed (overlay still pushes but
    /// shows "no bindings configured").
    pub keybindings: Option<&'a phux_config::KeybindingsCfg>,
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
pub(super) async fn dispatch_input_events(
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
        // phux-5ke.4: while an overlay is active it captures all input.
        // Key events flow to `OverlayState::handle_key` (which may
        // dismiss); mouse / paste / focus events are dropped so they
        // don't reach the pane underneath. Detach remains a resolver
        // bypass so the user can always bail out cleanly.
        if ctx.overlays.is_active() {
            if let InputEvent::Key(ref key_event) = ev {
                if let Some(outcome) = consume_chord(ctx, key_event) {
                    match outcome {
                        ChordOutcome::Partial => continue,
                        ChordOutcome::Resolved(resolved) if resolved.action == "detach" => {
                            if !*detach_pending {
                                conn.send(&FrameKind::Detach).await?;
                                *detach_pending = true;
                            }
                            continue;
                        }
                        ChordOutcome::Resolved(_) => {}
                    }
                }
                let was_active = ctx.overlays.is_active();
                ctx.overlays.handle_key(key_event);
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
                    if effects.layout_mutated {
                        layout_changed = true;
                    }
                    if effects.set_focus.is_some() {
                        *focused_pane = effects.set_focus;
                    }
                    if effects.set_metadata {
                        // Send SET_METADATA carrying the new envelope.
                        // Encoding can fail only if the state is empty
                        // (we just produced it — should not happen),
                        // but propagate cleanly if it ever does.
                        if let Some(bytes) = encode_layout_or_log(ctx.layout_state) {
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
                        let mut stdout = io::stdout().lock();
                        let _ = actions::write_bell(&mut stdout);
                    }
                    if effects.detach && !*detach_pending {
                        conn.send(&FrameKind::Detach).await?;
                        *detach_pending = true;
                    }
                    // phux-4li.12: parked split — send the SPAWN_TERMINAL
                    // and remember the intent for the reply handler.
                    if let Some((request_id, pending, frame)) = effects.spawn_terminal {
                        ctx.pending_splits.insert(request_id, pending);
                        conn.send(&frame).await?;
                    }
                    // phux-4li.12: kill-pane keystroke sequence. Each
                    // frame is an INPUT_KEY targeting the focused
                    // Terminal; the TERMINAL_CLOSED fold-out happens
                    // when the shell exits.
                    for frame in effects.kill_frames {
                        conn.send(&frame).await?;
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
            match route_mouse_event(ctx.layout_state, ctx.viewport, mouse) {
                RouteDecision::Pane {
                    target,
                    pane_x,
                    pane_y,
                    focus_changed,
                } => {
                    if focus_changed {
                        ctx.layout_state.focus = Some(target.clone());
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
        // phux-4li.6: peek the focused pane's grid via
        // `layout_state.focus`. The driver also mirrors that id into
        // its `focused_pane` local (server-frame handlers rely on it);
        // either reads the same TerminalId here.
        if let InputEvent::Key(ref key_event) = ev
            && predict.is_enabled()
            && let Some(fid) = ctx.layout_state.focus.as_ref()
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
        // is canonically `layout_state.focus`; the driver-side
        // `focused_pane` mirror stays in sync for the render path.
        // When focus is unset (pre-ATTACHED), drop the event with a
        // debug log instead of panicking — wave-A's "always Some
        // post-ATTACHED" invariant is enforced by the seed in
        // `handle_server_frame`, but a stray input race during
        // bootstrap shouldn't take the loop down.
        let Some(pane) = ctx.layout_state.focus.as_ref() else {
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
        let mut stdout = io::stdout().lock();
        let _ = overlay.render(predict, &mut stdout);
    }
    // Hand the layout-mutation signal back to `main_loop`, which holds
    // the status-bar painter and session name needed for a proper full
    // frame. We never paint from here.
    Ok(layout_changed)
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
        phux_config::keybind::Feed::Partial => Some(ChordOutcome::Partial),
        phux_config::keybind::Feed::Resolved(r) => Some(ChordOutcome::Resolved(r)),
    }
}

/// Side-effects a resolved action wants from the driver.
#[derive(Debug, Default)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "action dispatcher returns independent side-effect flags to keep async I/O outside run_action"
)]
struct ActionEffects {
    /// `true` ⇒ `layout_state` was mutated in-place; driver repaints.
    layout_mutated: bool,
    /// `Some(new_focus)` ⇒ swap the driver's `focused_pane` (input
    /// routing follows). The action helper already updated
    /// `layout_state.focus`; this carries the new id so the driver
    /// doesn't have to re-read it.
    set_focus: Option<TerminalId>,
    /// `true` ⇒ emit `SET_METADATA` carrying the new layout envelope.
    set_metadata: bool,
    /// `true` ⇒ emit a terminal bell (BEL `\x07`).
    bell: bool,
    /// `true` ⇒ emit `DETACH` and wait for `DETACHED`.
    detach: bool,
    /// phux-4li.12: a `split-pane` action emitted a `SPAWN_TERMINAL`
    /// and parked a [`PendingSplit`] keyed by `request_id`. The async
    /// caller sends the frame, then inserts the parked entry into the
    /// driver-wide `pending_splits` map.
    spawn_terminal: Option<(u32, PendingSplit, FrameKind)>,
    /// phux-4li.12: a `kill-pane` action ships a sequence of frames to
    /// the focused Terminal (the "soft-kill via shell-exit" — see
    /// `run_action`). The async caller sends them in order; the
    /// resulting `TERMINAL_CLOSED` from the server folds the pane out
    /// of the layout in [`handle_server_frame`].
    kill_frames: Vec<FrameKind>,
}

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
        "focus-direction" => {
            if let Some(dir) = direction_arg(resolved) {
                if let Some(new_state) = actions::apply_focus(ctx.layout_state, dir) {
                    let new_focus = new_state.focus.clone();
                    *ctx.layout_state = new_state;
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
                match actions::apply_resize(ctx.layout_state, dir, amount, ctx.viewport) {
                    Ok(Some(new_state)) => {
                        *ctx.layout_state = new_state;
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
                || HelpOverlay::from_config(&phux_config::KeybindingsCfg::default()),
                HelpOverlay::from_config,
            );
            ctx.overlays.push(Box::new(overlay));
        }
        "detach" => {
            effects.detach = true;
        }
        "next-pane" => {
            if let Some(new_state) = actions::apply_next_pane(ctx.layout_state) {
                let new_focus = new_state.focus.clone();
                *ctx.layout_state = new_state;
                effects.layout_mutated = true;
                effects.set_focus = new_focus;
            }
        }
        "previous-pane" => {
            if let Some(new_state) = actions::apply_previous_pane(ctx.layout_state) {
                let new_focus = new_state.focus.clone();
                *ctx.layout_state = new_state;
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

/// Encode `state` for `SET_METADATA`, logging encode failures. Returns
/// `None` on failure — caller should not emit a frame in that case.
pub(super) fn encode_layout_or_log(state: &LayoutState) -> Option<Vec<u8>> {
    match state.encode_cbor() {
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
fn split_dir_arg(resolved: &phux_config::keybind::ResolvedAction) -> Option<SplitDir> {
    let s = resolved.args.get("direction")?.as_str()?;
    match s {
        "horizontal" => Some(SplitDir::Horizontal),
        "vertical" => Some(SplitDir::Vertical),
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
        let mut h = ResolvedAction {
            action: "split-pane".to_owned(),
            args: std::collections::BTreeMap::new(),
        };
        h.args.insert(
            "direction".to_owned(),
            toml::Value::String("horizontal".into()),
        );
        assert_eq!(split_dir_arg(&h), Some(SplitDir::Horizontal));

        let mut v = ResolvedAction {
            action: "split-pane".to_owned(),
            args: std::collections::BTreeMap::new(),
        };
        v.args.insert(
            "direction".to_owned(),
            toml::Value::String("vertical".into()),
        );
        assert_eq!(split_dir_arg(&v), Some(SplitDir::Vertical));

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
    fn detach_action_requests_detach_effect() {
        let mut layout_state = LayoutState::default();
        let mut next_request_id = 1;
        let mut pending_splits = HashMap::new();
        let mut overlays = OverlayState::new();
        let mut ctx = DispatchCtx {
            resolver: None,
            layout_state: &mut layout_state,
            viewport: (80, 24),
            next_request_id: &mut next_request_id,
            pending_splits: &mut pending_splits,
            overlays: &mut overlays,
            keybindings: None,
        };
        let action = phux_config::keybind::ResolvedAction {
            action: "detach".to_owned(),
            args: BTreeMap::new(),
        };

        let effects = run_action(&action, &mut ctx, None);

        assert!(effects.detach);
        assert!(!effects.layout_mutated);
    }
}
