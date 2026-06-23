//! Submodule for runtime internals.

use phux_protocol::caps::ClientCapabilities;
use phux_protocol::input::InputEvent;
use phux_protocol::wire::frame::{
    AgentEvent, Command, CommandResult, CommandValue, ControlAction, ErrorCode, FrameKind,
    InputMode, StateScope, TerminalSignal, ViewportInfo,
};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, trace, warn};

use super::{AttachPrepared, spawn_pane_event_drain, spawn_terminal_exit_watcher};
use crate::state::{ClientId, Outbound, SharedState, TerminalInput};
use crate::terminal_actor::{
    ConsumerAckRequest, ControlRequest, ResizeRequest, ScreenRequest, TerminalActor, TerminalHandle,
};

pub(crate) fn seed_session_with_actor(
    state: &SharedState,
    name: &str,
    history_limit: u32,
    root_token: &CancellationToken,
) -> Result<phux_core::ids::TerminalId, crate::terminal_actor::TerminalActorError> {
    use phux_core::ids::TerminalId;
    let terminal: TerminalId = state.with_mut(|s| s.seed_session(name).2);
    // Default 80x24 — same as `phux_core::Pane::new`'s default dims.
    // Real resize wiring lands with VIEWPORT_RESIZE (phux-4hp).
    let terminal_token = root_token.child_token();
    let bundle =
        TerminalActor::build_with_token(80, 24, None, history_limit, terminal_token.clone())?;
    let crate::terminal_actor::TerminalActorBundle {
        actor,
        handle,
        exit_notify,
        ..
    } = bundle;
    state.with_mut(|s| {
        let _ = s.spawn_terminal_actor(terminal, handle, terminal_token, actor.run());
    });
    spawn_terminal_exit_watcher(state.clone(), terminal, exit_notify, root_token.clone());
    Ok(terminal)
}

/// Seed `(session, window, pane)` and spawn a **PTY-backed**
/// `TerminalActor` running `cmd`. Sibling of the private
/// `seed_session_with_actor` helper for the real server path
/// (`phux-byc.5`).
///
/// Call sites:
///
/// * The `phux server` binary entry point, via
///   [`super::ServerConfig::seed_with_pty`] (with
///   [`super::ServerConfig::seed_command`]
///   left `None` to fall back to
///   [`crate::terminal_actor::default_shell_command`] — the user's `$SHELL`,
///   or `/bin/sh` per the byc.5 convention).
/// * Anything embedding `phux-server` and wanting a specific command
///   (e.g. an integration test driving a known fixture; see the
///   `input_dispatch.rs` test, which seeds with `cat` to get
///   deterministic echo).
pub fn seed_session_with_pty(
    state: &SharedState,
    name: &str,
    cmd: portable_pty::CommandBuilder,
    history_limit: u32,
    root_token: &CancellationToken,
) -> Result<phux_core::ids::TerminalId, crate::terminal_actor::TerminalActorError> {
    use phux_core::ids::TerminalId;
    let terminal: TerminalId = state.with_mut(|s| s.seed_session(name).2);
    let terminal_token = root_token.child_token();
    let bundle =
        TerminalActor::build_with_token(80, 24, Some(cmd), history_limit, terminal_token.clone())?;
    let crate::terminal_actor::TerminalActorBundle {
        mut actor,
        handle,
        exit_notify,
        ..
    } = bundle;
    // phux-y2t: wire the actor's agent-event sink and spawn a drain task
    // that fans bell / title / dirty / idle events out to event-stream
    // subscribers scoped to this pane. The wire `TerminalId` is interned
    // up front (stable for the pane's lifetime) and captured by the drain.
    let (event_tx, event_rx) = tokio::sync::mpsc::channel(EVENT_SINK_CAPACITY);
    actor.set_event_sink(event_tx);
    let wire_terminal_id = state.with_mut(|s| {
        let _ = s.spawn_terminal_actor(terminal, handle, terminal_token, actor.run());
        s.intern_terminal_wire(terminal)
    });
    spawn_pane_event_drain(state.clone(), wire_terminal_id, event_rx);
    spawn_terminal_exit_watcher(state.clone(), terminal, exit_notify, root_token.clone());
    Ok(terminal)
}

/// Add a **PTY-backed** pane to an existing `session`'s window and spawn its
/// `TerminalActor` — the split counterpart to [`seed_session_with_pty`]
/// (phux-i9zl).
///
/// Identical to `seed_session_with_pty` except the new pane joins
/// `session`'s window via `add_pane_to_session` instead of
/// creating a fresh `spawn-N` session. A TUI split routes here so the new
/// L1 Terminal stays in the spawning client's current session.
///
/// Returns `Ok(None)` when `session` has no window to host the pane
/// (unreachable for a seeded session); the caller maps that to a wire
/// `SpawnError`. `Err` is an actor-build failure, same as the seed path.
pub fn spawn_pane_with_pty(
    state: &SharedState,
    session: phux_core::ids::SessionId,
    cmd: portable_pty::CommandBuilder,
    history_limit: u32,
    root_token: &CancellationToken,
) -> Result<Option<phux_core::ids::TerminalId>, crate::terminal_actor::TerminalActorError> {
    use phux_core::ids::TerminalId;
    let Some(terminal): Option<TerminalId> = state.with_mut(|s| s.add_pane_to_session(session))
    else {
        return Ok(None);
    };
    let terminal_token = root_token.child_token();
    let bundle =
        TerminalActor::build_with_token(80, 24, Some(cmd), history_limit, terminal_token.clone())?;
    let crate::terminal_actor::TerminalActorBundle {
        mut actor,
        handle,
        exit_notify,
        ..
    } = bundle;
    // Same agent-event wiring as the seed path (phux-y2t): intern the wire id
    // up front and spawn the per-pane event drain.
    let (event_tx, event_rx) = tokio::sync::mpsc::channel(EVENT_SINK_CAPACITY);
    actor.set_event_sink(event_tx);
    let wire_terminal_id = state.with_mut(|s| {
        let _ = s.spawn_terminal_actor(terminal, handle, terminal_token, actor.run());
        s.intern_terminal_wire(terminal)
    });
    spawn_pane_event_drain(state.clone(), wire_terminal_id, event_rx);
    spawn_terminal_exit_watcher(state.clone(), terminal, exit_notify, root_token.clone());
    Ok(Some(terminal))
}

/// Bounded capacity of the per-pane agent-event sink (SPEC §7.5,
/// phux-y2t). Small: events are coalesced (one `dirty` per burst, one
/// `idle` to close it) and the stream tolerates loss — a full sink drops
/// the event rather than stalling the actor's hot PTY-pump loop.
pub(crate) const EVENT_SINK_CAPACITY: usize = 64;

/// message to the socket. Encodes [`Outbound::Frame`] via
/// `FrameKind::encode`.
///
/// Exits when the channel closes — i.e. the client task drops its
/// sender.
pub(crate) fn handle_terminal_resize(
    state: &SharedState,
    client_id: ClientId,
    wire_terminal_id: &phux_protocol::ids::TerminalId,
    cols: u16,
    rows: u16,
) {
    if !wire_terminal_id.is_local() {
        warn!(
            ?client_id,
            ?wire_terminal_id,
            cols,
            rows,
            "TERMINAL_RESIZE: SATELLITE-routed pane id rejected on non-federation-hub server",
        );
        return;
    }
    state.with_mut(|s| {
        let Some(terminal) = s.terminal_from_wire(wire_terminal_id) else {
            debug!(
                ?client_id,
                ?wire_terminal_id,
                cols,
                rows,
                "TERMINAL_RESIZE: unknown pane; dropping (no-reply per wire frame design)",
            );
            return;
        };
        // Keep the registry's recorded dims in sync so future
        // `TERMINAL_SNAPSHOT` payloads report the post-resize cols/rows.
        // Mirrors what `handle_viewport_resize` does for VIEWPORT_RESIZE.
        if let Some(pane) = s.registry.terminal_mut(terminal) {
            pane.dims = (cols, rows);
        }
        let Some(handle) = s.terminals.get(&terminal) else {
            debug!(
                ?client_id,
                ?terminal,
                cols,
                rows,
                "TERMINAL_RESIZE: no TerminalHandle registered for pane; dropping",
            );
            return;
        };
        // Live per-pane resize (TERMINAL_RESIZE): resync clients so their
        // mirrors reconverge after reflow (phux-8v1). An agent's explicit
        // resize carries cell counts only — no pixel truth — so the actor
        // keeps its last-known cell pixel size.
        match handle.resize.try_send(ResizeRequest {
            cols,
            rows,
            cell_px: None,
            resync_clients: true,
            resync_only: false,
        }) {
            Ok(()) => {}
            Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                warn!(
                    ?client_id,
                    ?terminal,
                    cols,
                    rows,
                    "TERMINAL_RESIZE: pane resize mailbox full; dropping",
                );
            }
            Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                debug!(
                    ?client_id,
                    ?terminal,
                    "TERMINAL_RESIZE: pane actor gone; dropping resize",
                );
            }
        }
    });
}

/// Perform the attach mutation in one critical section: call
/// [`crate::state::ServerState::attach`], build the snapshot, collect
/// the per-pane handles + wire ids to snapshot.
///
/// Pulled out so [`crate::runtime::attach::handle_attach`] stays under clippy's
/// `too_many_lines` ceiling.
pub(crate) fn prepare_attach(
    state: &SharedState,
    client_id: ClientId,
    session_name: &str,
    out_tx: &tokio::sync::mpsc::Sender<Outbound>,
    client_caps: ClientCapabilities,
) -> Result<AttachPrepared, crate::state::AttachError> {
    state.with_mut(|s| {
        let sid = s.attach(client_id, session_name, out_tx.clone(), client_caps)?;
        // Record successful attach as session activity before we build
        // the snapshot. The order doesn't matter for
        // correctness (we're still inside the with_mut critical
        // section), but doing it here keeps the recording adjacent to
        // the attach call that justified it — easier to reason about
        // when reading the code.
        s.touch_session(sid);
        let snapshot = s
            .build_session_snapshot(sid)
            .ok_or_else(|| crate::state::AttachError::UnknownSession(session_name.to_owned()))?;
        let panes_to_snapshot = s.attach_snapshot_panes(sid);
        let initial_client_id =
            phux_protocol::ids::ClientId::new(u32::try_from(client_id.0).unwrap_or(u32::MAX));
        Ok((snapshot, initial_client_id, panes_to_snapshot))
    })
}

// -----------------------------------------------------------------------------
// Control-plane command dispatch — SPEC §5 (phux-k61 / ADR-0021).
// -----------------------------------------------------------------------------

/// Dispatch a `COMMAND` envelope and reply with `COMMAND_RESULT`
/// correlated by `request_id`. The control plane for the CLI's `ls` /
/// `kill` verbs. Per SPEC §5 a command is asynchronous: the result MAY
/// follow other frames the command triggered (e.g. `KILL_TERMINAL`'s
/// `TERMINAL_CLOSED`).
/// Stable, payload-free label for a [`Command`] variant — the `kind` field
/// on the `handle_command` lifecycle span. A hand-written map (rather than
/// `?command`) keeps the trace line small and free of user payloads
/// (session names, env, input bytes) while still localizing which control
/// command ran. `Command` is `#[non_exhaustive]`, hence the wildcard; a new
/// variant logs as `"other"` until an arm is added here.
pub(crate) const fn command_kind(command: &Command) -> &'static str {
    match command {
        Command::KillTerminal { .. } => "kill_terminal",
        Command::KillTerminals { .. } => "kill_terminals",
        Command::GetState { .. } => "get_state",
        Command::GetScreen { .. } => "get_screen",
        Command::RouteInput { .. } => "route_input",
        Command::AcquireInput { .. } => "acquire_input",
        Command::ReleaseInput { .. } => "release_input",
        Command::SignalTerminal { .. } => "signal_terminal",
        _ => "other",
    }
}

// Lifecycle span (info): one per L2 COMMAND. `kind` is a payload-free
// label so the trace localizes which control command ran without leaking
// session names / env / input bytes; the CLOSE duration times the command
// (some, e.g. GET_SCREEN, round-trip to an actor).
#[tracing::instrument(
    level = "info",
    name = "handle_command",
    skip_all,
    fields(?client_id, request_id, kind = command_kind(&command)),
)]
pub(crate) async fn handle_command(
    state: &SharedState,
    client_id: ClientId,
    request_id: u32,
    command: Command,
    out_tx: &tokio::sync::mpsc::Sender<Outbound>,
) {
    // UPGRADE is handled out-of-band: `handle_upgrade` acks the client itself
    // and then re-execs the process, so it never returns a `CommandResult` for
    // the shared send below (ADR-0032).
    if matches!(command, Command::Upgrade) {
        handle_upgrade(state, request_id, out_tx).await;
        return;
    }

    let result = match command {
        Command::GetState { scope } => handle_get_state(state, &scope),
        Command::GetScreen {
            terminal_id,
            request_scrollback,
            cells,
        } => handle_get_screen(state, &terminal_id, request_scrollback, cells).await,
        Command::RouteInput { terminal_id, event } => {
            handle_route_input(state, client_id, &terminal_id, event)
        }
        Command::KillTerminals { ids } => handle_kill_terminals(state, &ids),
        Command::KillTerminal { terminal_id } => {
            // Resolve the wire id to the core pane, then cancel its actor.
            // Cancellation drops the actor's `exit_notify`, which the
            // per-pane EOF watcher (phux-it8) treats identically to PTY
            // EOF: it broadcasts `TERMINAL_CLOSED` and reaps the pane
            // (phux-60s), cascading to session removal + server self-exit
            // when the last session empties. So KILL_TERMINAL reuses the
            // exact teardown a natural shell exit takes — no separate
            // kill plumbing, and the async `TERMINAL_CLOSED` still fires.
            state
                .with(|s| s.terminal_from_wire(&terminal_id))
                .map_or_else(
                    || CommandResult::Error {
                        code: ErrorCode::TerminalNotFound,
                        message: format!("no such terminal: {terminal_id:?}"),
                    },
                    |core_id| {
                        state.with_mut(|s| s.detach_terminal_actor(core_id));
                        CommandResult::Ok
                    },
                )
        }
        Command::GetTerminalState {
            terminal_id,
            include_scrollback,
            max_scrollback_lines,
        } => {
            handle_get_terminal_state(
                state,
                &terminal_id,
                include_scrollback,
                max_scrollback_lines,
            )
            .await
        }
        Command::SubscribeTerminalEvents {
            terminal_id,
            event_types,
        } => handle_subscribe_terminal_events(state, client_id, &terminal_id, event_types, out_tx),
        Command::AcquireInput {
            terminal_id,
            mode,
            ttl_ms,
        } => handle_acquire_input(state, client_id, &terminal_id, mode, ttl_ms).await,
        Command::ReleaseInput { terminal_id } => {
            handle_release_input(state, client_id, &terminal_id).await
        }
        Command::SignalTerminal {
            terminal_id,
            signal,
        } => handle_signal_terminal(state, client_id, &terminal_id, signal).await,
        Command::ReportAsked {
            terminal_id,
            id,
            question,
            suggestions,
            elapsed_seconds,
        } => handle_report_asked(
            state,
            &terminal_id,
            id,
            question,
            suggestions,
            elapsed_seconds,
        ),
        // `Command` is `#[non_exhaustive]`: a forward-compat command this
        // server doesn't implement decodes only if a newer peer sent a
        // tag we allocated but haven't wired (the decoder rejects truly
        // unknown tags). Refuse it per SPEC §5 with `INVALID_COMMAND`.
        _ => CommandResult::Error {
            code: ErrorCode::InvalidCommand,
            message: "command not supported by this server".to_owned(),
        },
    };
    debug!(
        ?client_id,
        request_id, "COMMAND dispatched; sending COMMAND_RESULT"
    );
    let _ = out_tx
        .send(Outbound::Frame(FrameKind::CommandResult {
            request_id,
            result,
        }))
        .await;
}

/// Handle `UPGRADE` (ADR-0032): prepare the graceful re-exec, ack the client,
/// then replace the process. Acks itself (rather than returning a
/// `CommandResult`) because on success it never returns.
async fn handle_upgrade(
    state: &SharedState,
    request_id: u32,
    out_tx: &tokio::sync::mpsc::Sender<Outbound>,
) {
    let result = match super::upgrade::prepare_upgrade(state).await {
        Ok(plan) => {
            // Ack `Ok` and let the writer flush it before we replace the
            // process — best-effort; the client reconnects regardless.
            let _ = out_tx
                .send(Outbound::Frame(FrameKind::CommandResult {
                    request_id,
                    result: CommandResult::Ok,
                }))
                .await;
            tokio::task::yield_now().await;
            info!("UPGRADE: re-exec'ing the new binary");
            let err = plan.exec();
            // Only reached if the exec itself failed: nothing was closed, so
            // the old image keeps serving and no child is stranded.
            error!(error = %err, "UPGRADE exec failed; continuing on the current image");
            return;
        }
        Err(err) => {
            warn!(error = %err, "UPGRADE preparation failed");
            CommandResult::Error {
                code: ErrorCode::InternalError,
                message: format!("upgrade failed: {err}"),
            }
        }
    };
    let _ = out_tx
        .send(Outbound::Frame(FrameKind::CommandResult {
            request_id,
            result,
        }))
        .await;
}

/// Build the `Ok` reply for `KILL_TERMINALS` — the atomic multi-terminal
/// teardown the v0.3.0 "Option B" re-tier left in place of the dissolved
/// L2 `KILL_COLLECTION` verb (ADR-0019 / ADR-0027).
///
/// Tears down every Terminal in `ids` inside **one** `with_mut` lock scope,
/// so the removals are atomic with respect to every other command: no peer
/// can observe a half-killed group on this server. (Cross-host atomicity is
/// out of scope, as it would be under any tiering.) Each removal cancels the
/// pane actor via [`crate::state::ServerState::detach_terminal_actor`];
/// cancellation drops the actor's `exit_notify`, which the per-pane EOF
/// watcher treats like PTY EOF — it broadcasts `TERMINAL_CLOSED` and reaps
/// the pane, cascading to session removal and (when the last session
/// empties) server self-exit. So this reuses the exact teardown a per-pane
/// `KILL_TERMINAL` (or a natural shell exit) takes, but resolves the whole
/// group in one pass.
///
/// Idempotent: an `id` that is unknown, satellite-routed, or already-dead is
/// skipped silently rather than failing the batch, so a caller racing a
/// natural pane exit still succeeds. The reply is `Ok` the moment the actors
/// are cancelled; the `TERMINAL_CLOSED` frames follow asynchronously as the
/// panes reap (SPEC §5). The op is structurally infallible — an empty `ids`
/// list is a no-op that still acks `Ok`.
pub(crate) fn handle_kill_terminals(
    state: &SharedState,
    ids: &[phux_protocol::ids::TerminalId],
) -> CommandResult {
    // Single lock scope: resolve every wire id to its core pane and cancel
    // its actor before releasing the lock. All-or-nothing for a local
    // server — no other command interleaves between the first and last
    // removal. `detach_terminal_actor` is idempotent (cancelling an
    // already-cancelled token is a no-op), so an id racing a natural exit,
    // an unknown id, and a satellite-routed id (no local pane) all collapse
    // to a silent skip.
    let killed = state.with_mut(|s| {
        let mut killed = 0u32;
        for wire_id in ids {
            if let Some(core_id) = s.terminal_from_wire(wire_id) {
                s.detach_terminal_actor(core_id);
                killed = killed.saturating_add(1);
            } else {
                debug!(?wire_id, "KILL_TERMINALS: unknown / dead id; skipping");
            }
        }
        killed
    });
    debug!(
        requested = ids.len(),
        killed, "KILL_TERMINALS: torn down group atomically"
    );
    CommandResult::Ok
}

/// Create a named session and seed its pane, *without* attaching — the
/// create-without-attach path the v0.3.0 "Option B" re-tier (ADR-0019 /
/// ADR-0027) routes through the conventional
/// [`phux_protocol::wire::frame::SESSION_CREATE_KEY`] L3 metadata write
/// (replacing the removed `CREATE_SESSION` verb).
///
/// Existence check and seed both run on the single-threaded runtime, so the
/// lookup→create sequence is atomic with respect to other clients: two
/// racing create requests for the same `name` cannot both succeed. Returns
/// `Ok(wire_id)` on success (the seed pane's wire [`phux_core::ids::TerminalId`],
/// which the
/// caller publishes under a result key for the client to read back), or
/// `Err(message)` if `name` is already taken or the seed fails. Because
/// `SET_METADATA` has no reply frame, the error is for logging only.
pub(crate) fn create_named_session(
    state: &SharedState,
    name: &str,
    command: Option<Vec<String>>,
    cwd: Option<&str>,
    root_token: &CancellationToken,
) -> Result<phux_protocol::ids::TerminalId, String> {
    if state.with(|s| s.session_by_name(name).is_some()) {
        return Err(format!("session {name:?} already exists"));
    }

    let (with_pty, override_cmd, history_limit, term) = state.with(|s| {
        (
            s.attach_create_seeds_pty(),
            s.attach_create_seed_command(),
            s.history_limit(),
            s.term().to_owned(),
        )
    });

    let seed_result = if with_pty {
        // Command precedence mirrors `resolve_create_if_missing`: an explicit
        // server-wide override (set by tests for a deterministic child) wins,
        // then the request `command`, then the default shell.
        let mut seed_cmd = override_cmd.unwrap_or_else(|| match command {
            Some(argv) if !argv.is_empty() => {
                let mut head = argv.into_iter();
                let program = head.next().unwrap_or_default();
                let mut builder = portable_pty::CommandBuilder::new(program);
                for arg in head {
                    builder.arg(arg);
                }
                if let Some(path) = cwd {
                    builder.cwd(path);
                }
                builder
            }
            _ => {
                let mut builder = crate::terminal_actor::default_shell_command();
                if let Some(path) = cwd {
                    builder.cwd(path);
                }
                builder
            }
        });
        crate::terminal_actor::apply_term(&mut seed_cmd, &term);
        seed_session_with_pty(state, name, seed_cmd, history_limit, root_token)
    } else {
        seed_session_with_actor(state, name, history_limit, root_token)
    };

    match seed_result {
        Ok(core_terminal) => {
            // Intern the wire id so it is stable and resolvable, and hand it
            // back so the caller can publish it for the client to read.
            let wire = state.with_mut(|s| s.intern_terminal_wire(core_terminal));
            Ok(wire)
        }
        Err(err) => {
            warn!(
                session = %name,
                error = %err,
                "session-create: failed to seed pane for new session",
            );
            Err(format!("failed to create session {name:?}: {err}"))
        }
    }
}

/// Build the `OK_WITH(STATE(..))` reply for `GET_STATE`.
///
/// v0.1 supports only [`StateScope::Server`] (the whole-server snapshot).
/// The snapshot reuses the `ATTACHED`
/// [`phux_protocol::wire::info::SessionSnapshot`] shape; `phux ls`
/// and client-side selector resolution read its `sessions` list and ignore
/// the focused-* fields. An empty server yields an empty session list with
/// sentinel focus ids (the wire requires the focus fields to be present).
pub(crate) fn handle_get_state(state: &SharedState, scope: &StateScope) -> CommandResult {
    match scope {
        StateScope::Server => {
            let snapshot = state.with_mut(|s| {
                let focus = s
                    .most_recently_touched_session()
                    .or_else(|| s.registry.sessions().next().map(|(id, _)| id));
                focus.and_then(|sid| s.build_session_snapshot(sid))
            });
            CommandResult::OkWith(CommandValue::State(
                snapshot.unwrap_or_else(empty_session_snapshot),
            ))
        }
        // `StateScope` is `#[non_exhaustive]`; a narrower scope a newer
        // peer requests is not yet supported.
        _ => CommandResult::Error {
            code: ErrorCode::InvalidCommand,
            message: "unsupported GET_STATE scope".to_owned(),
        },
    }
}

/// Build the `OK_WITH(JSON(..))` reply for `GET_SCREEN`.
///
/// Resolves the wire id to its pane actor, then asks the actor to project
/// its own `Terminal` grid into a [`phux_core::screen::ScreenState`]
/// serialized as JSON — the stable agent-surface contract (ADR-0022 §2).
/// This is side-effect-free: it neither attaches nor resizes, so polling
/// it (the `phux wait`/`run` floor) never disturbs the live pane.
pub(crate) async fn handle_get_screen(
    state: &SharedState,
    terminal_id: &phux_protocol::ids::TerminalId,
    request_scrollback: Option<u32>,
    cells: bool,
) -> CommandResult {
    // Clone the (Send) handle out of the lock; the actor reply is awaited
    // outside the critical section.
    let handle = state.with(|s| {
        s.terminal_from_wire(terminal_id)
            .and_then(|core| s.terminal_handle(core).cloned())
    });
    let Some(handle) = handle else {
        return CommandResult::Error {
            code: ErrorCode::TerminalNotFound,
            message: format!("no such terminal: {terminal_id:?}"),
        };
    };
    let pane = terminal_id.local_id().unwrap_or(0);
    let (reply_tx, reply_rx) = oneshot::channel();
    if handle
        .screen
        .send(ScreenRequest {
            pane,
            scrollback: request_scrollback,
            cells,
            reply: reply_tx,
        })
        .await
        .is_err()
    {
        return CommandResult::Error {
            code: ErrorCode::InternalError,
            message: "pane actor unavailable for GET_SCREEN".to_owned(),
        };
    }
    reply_rx.await.map_or_else(
        |_| CommandResult::Error {
            code: ErrorCode::InternalError,
            message: "pane actor dropped the GET_SCREEN reply".to_owned(),
        },
        |screen| {
            serde_json::to_string(&screen).map_or_else(
                |err| CommandResult::Error {
                    code: ErrorCode::InternalError,
                    message: format!("screen serialization failed: {err}"),
                },
                |json| CommandResult::OkWith(CommandValue::Json(json)),
            )
        },
    )
}

/// Build the `Ok_With(Json(TerminalState))` reply for `GET_TERMINAL_STATE`.
///
/// L2 Collection-aware counterpart to [`handle_get_screen`]: returns a
/// comprehensive snapshot of terminal state (grid, scrollback, cursor, shell
/// metadata, sequence number, and timestamp) in a structured JSON format.
/// Backs agent polling and state inspection without requiring an attach or
/// subscription (ADR-0022, ADR-0015 L2).
///
/// Unlike `GET_SCREEN` which returns raw `ScreenState` with only grid
/// dimensions and viewport text, `GET_TERMINAL_STATE` returns structured
/// JSON with:
/// - Grid cells with text and styling
/// - Cursor position and visibility
/// - Optional scrollback history (if `include_scrollback` is true)
/// - Shell process metadata (PID, name, jobs, copy-mode state)
/// - Pending command tracking (overlay layer)
/// - Logical sequence number (for change detection)
/// - Timestamp (for agent polling)
///
/// Handler flow:
/// 1. Resolve `terminal_id` to a `TerminalActor` handle (reuse same pattern as
///    `handle_get_screen`)
/// 2. Query screen state via `ScreenRequest` (reuse existing path)
/// 3. Walk grid cells: parse `ScreenState.lines` and merge styling from
///    `ScreenState.cells` (`CellInfo`)
/// 4. Extract cursor, scrollback, and dimensions
/// 5. Query shell state (gracefully degrade to None if unavailable)
/// 6. Build JSON and encode as JSON
/// 7. Return as `COMMAND_RESULT Ok_With(Json(TerminalState))`
///
/// Error cases:
/// - Unknown `terminal_id` → `TERMINAL_NOT_FOUND`
/// - Actor unavailable → `INTERNAL_ERROR`
/// - Shell query fails → populate `shell_state: None`, continue gracefully
#[allow(clippy::too_many_lines)]
pub(crate) async fn handle_get_terminal_state(
    state: &SharedState,
    terminal_id: &phux_protocol::ids::TerminalId,
    include_scrollback: bool,
    max_scrollback_lines: u16,
) -> CommandResult {
    use std::time::{SystemTime, UNIX_EPOCH};

    // Step 1: Resolve terminal_id to TerminalActor handle (same pattern as
    // handle_get_screen).
    let handle = state.with(|s| {
        s.terminal_from_wire(terminal_id)
            .and_then(|core| s.terminal_handle(core).cloned())
    });

    let Some(handle) = handle else {
        return CommandResult::Error {
            code: ErrorCode::TerminalNotFound,
            message: format!("no such terminal: {terminal_id:?}"),
        };
    };

    let pane = terminal_id.local_id().unwrap_or(0);

    // Step 2: Query screen state via ScreenRequest (reuse existing path).
    // This gives us canonical grid snapshot, scrollback (if requested), and
    // cell styling information.
    let (reply_tx, reply_rx) = oneshot::channel();
    if handle
        .screen
        .send(ScreenRequest {
            pane,
            scrollback: if include_scrollback {
                Some(u32::from(max_scrollback_lines))
            } else {
                None
            },
            cells: true, // Always request cells for semantic info (styles, OSC-133 marks)
            reply: reply_tx,
        })
        .await
        .is_err()
    {
        return CommandResult::Error {
            code: ErrorCode::InternalError,
            message: "pane actor unavailable for GET_TERMINAL_STATE".to_owned(),
        };
    }

    let Ok(screen_state) = reply_rx.await else {
        return CommandResult::Error {
            code: ErrorCode::InternalError,
            message: "pane actor dropped the GET_TERMINAL_STATE reply".to_owned(),
        };
    };

    // Step 3: Convert ScreenState viewport to JSON cells array.
    // ScreenState carries:
    // - lines: Vec<String> — viewport text, one row per element, right-trimmed
    // - cells: Option<Vec<CellInfo>> — sparse: only cells with non-default
    //   style or OSC-133 semantic marks, in row-major order
    //
    // We parse each line into characters and emit cells as JSON objects.
    // Note: a full implementation using unicode-segmentation::Graphemes
    // would handle combining marks, emoji, and wide glyphs more precisely;
    // for now we estimate width based on ASCII vs. non-ASCII.

    let mut viewport_cells = Vec::new();

    // Emit viewport cells by parsing each line.
    // Each line is right-trimmed, so we don't need to emit trailing blanks.
    #[allow(clippy::cast_possible_truncation)]
    for (row_idx, line_text) in screen_state.lines.iter().enumerate() {
        let row = row_idx as u16;
        let mut col = 0u16;

        for ch in line_text.chars() {
            // Estimate cell width: ASCII is 1 column, everything else is 2
            // (emoji, CJK). libghostty tracks actual widths; we approximate.
            let width = if ch.is_ascii() { 1u16 } else { 2u16 };

            // Emit this cell as JSON.
            viewport_cells.push(serde_json::json!({
                "col": col,
                "row": row,
                "text": ch.to_string(),
                "width": width as u8,
                "selected": false,
            }));

            col += width;
            // Stop if we exceed grid width (shouldn't happen in right-trimmed lines)
            if col >= screen_state.cols {
                break;
            }
        }
    }

    // Extract cursor state as JSON.
    let cursor = screen_state.cursor.map(|cs| {
        serde_json::json!({
            "x": cs.x,
            "y": cs.y,
            "visible": cs.visible,
        })
    });

    // Step 4: Convert scrollback lines to JSON.
    let mut scrollback_lines = Vec::new();
    #[allow(clippy::cast_possible_truncation)]
    let scrollback_count_total = screen_state.scrollback.len() as u32;

    if include_scrollback {
        for line_text in &screen_state.scrollback {
            scrollback_lines.push(serde_json::json!({
                "text": line_text,
                "cells": [],
            }));
        }
    }

    // Step 5: Query shell state.
    // The TerminalActor could provide shell PID (child of PTY master),
    // shell name, job list, and in_copy_mode. For now, set to None;
    // a future iteration adds a GetShellStateRequest channel and wires
    // shell state queries (phux-y2t Phase 2).
    //
    // Graceful degrade: if the actor has no PTY (no-PTY test actor),
    // or the query fails, leave shell_state as None. Agents can work
    // with partial snapshots.
    let shell_state: Option<serde_json::Value> = None;

    // Step 6: Compute timestamp and sequence number.
    let timestamp_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());

    // Sequence number is a logical clock maintained per terminal for change
    // detection. For now, placeholder; should be sourced from actor's state
    // in a future iteration (phux-y2t Phase 2). See ADR-0015 for the versioning model.
    let seq = 0u64;

    // Step 7: Build the TerminalState as JSON.
    let terminal_state_json = serde_json::json!({
        "cols": screen_state.cols,
        "rows": screen_state.rows,
        "cells": viewport_cells,
        "cursor": cursor,
        "scrollback": scrollback_lines,
        "scrollback_count_total": scrollback_count_total,
        "shell_state": shell_state,
        "pending_command": serde_json::Value::Null,
        "timestamp_secs": timestamp_secs,
        "seq": seq,
    });

    // Step 8: Serialize to JSON string and return.
    match serde_json::to_string(&terminal_state_json) {
        Ok(json) => CommandResult::OkWith(CommandValue::Json(json)),
        Err(err) => CommandResult::Error {
            code: ErrorCode::InternalError,
            message: format!("terminal state serialization failed: {err}"),
        },
    }
}

/// Build the `Ok` reply for `ROUTE_INPUT`.
///
/// The write counterpart to [`handle_get_screen`]: it resolves the wire id
/// to its pane actor and feeds the already-built input event straight into
/// the pane's input mailbox — the same mailbox `handle_terminal_input`
/// targets, but with no attach / subscription gate and, crucially, no
/// resize. So unlike the ATTACH-then-`INPUT_KEY` path, routing input here
/// never transiently shrinks the pane to the caller's viewport; the live
/// dimensions are preserved (ADR-0022, `phux-3j3`).
///
/// `ROUTE_INPUT` is the side-effect-free agent path (ADR-0022): it
/// delivers input to a Terminal WITHOUT an attach or subscription, which is
/// exactly how `phux run` / `send-keys` drive a pane headlessly. It must
/// therefore NOT require the caller to be a subscriber. An earlier interim
/// gate (phux-nlo) approximated "PRIMARY" by subscription and rejected any
/// unsubscribed caller — but that is precisely the headless agent, so it
/// broke the agent surface; it is removed. v0.1 is single-trust-domain (one
/// server per user, ADR-0003), so there is no untrusted observer to fence
/// off here. Genuine viewer-vs-primary authority (SPEC `input.md` §7 /
/// `L1.md` §7.1) returns when per-connection roles are materialized, and
/// must gate an *attached read-only viewer*, never the headless
/// control-plane caller. `client_id` is kept for that future policy and for
/// the observability trace below.
///
/// `try_send` is non-blocking for the same single-threaded-runtime reason
/// as `handle_terminal_input`: input is fire-and-forget per SPEC §9, so a
/// full mailbox drops the event rather than blocking the read loop. The
/// command still acks `Ok` (the event was accepted for delivery); an
/// unknown Terminal or a gone actor produces an `Error`.
pub(crate) fn handle_route_input(
    state: &SharedState,
    client_id: ClientId,
    terminal_id: &phux_protocol::ids::TerminalId,
    event: InputEvent,
) -> CommandResult {
    // v0.1 non-federation-hub servers reject SATELLITE-routed input
    // (ADR-0016 / SPEC §10.1), matching `handle_terminal_input`.
    if !terminal_id.is_local() {
        return CommandResult::Error {
            code: ErrorCode::UnsupportedSatelliteRoute,
            message: format!("ROUTE_INPUT to satellite route unsupported: {terminal_id:?}"),
        };
    }
    // Resolve the wire id to its (Send) Terminal handle in one lock; we
    // never await inside the lock. No subscription/role gate: ROUTE_INPUT is
    // the headless agent path (see the doc comment) and must work without an
    // attach. But the input lease DOES gate it (ADR-0033): if a human has
    // taken the wheel, the automated ROUTE_INPUT path is locked out too —
    // that is the whole point of seizing input authority over an agent.
    let resolved = state.with(|s| {
        let core = s.terminal_from_wire(terminal_id)?;
        let blocked = s.input_blocked(core, client_id);
        s.terminal_handle(core).cloned().map(|h| (h, blocked))
    });
    let Some((handle, blocked)) = resolved else {
        return CommandResult::Error {
            code: ErrorCode::TerminalNotFound,
            message: format!("no such terminal: {terminal_id:?}"),
        };
    };
    if blocked {
        debug!(
            ?client_id,
            ?terminal_id,
            "ROUTE_INPUT blocked: another client holds the input lease (ADR-0033)"
        );
        return CommandResult::Error {
            code: ErrorCode::InputLeaseHeld,
            message: "input lease held by another client".to_owned(),
        };
    }
    debug!(?client_id, ?terminal_id, "ROUTE_INPUT delivering input");
    let input = match event {
        InputEvent::Key(event) => TerminalInput::Key(event),
        InputEvent::Mouse(event) => TerminalInput::Mouse(event),
        InputEvent::Focus(event) => TerminalInput::Focus(event),
        InputEvent::Paste(event) => TerminalInput::Paste(event),
        // `InputEvent` is `#[non_exhaustive]`; a future atom a newer peer
        // sends is not yet routable here.
        _ => {
            return CommandResult::Error {
                code: ErrorCode::InvalidCommand,
                message: "unsupported ROUTE_INPUT event".to_owned(),
            };
        }
    };
    match handle.input.try_send(input) {
        Ok(()) => CommandResult::Ok,
        Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
            warn!(
                ?terminal_id,
                "ROUTE_INPUT mailbox full; dropping (fire-and-forget per SPEC §9)"
            );
            CommandResult::Ok
        }
        Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => CommandResult::Error {
            code: ErrorCode::InternalError,
            message: "pane actor unavailable for ROUTE_INPUT".to_owned(),
        },
    }
}

/// Bridge `state::ClientId` (u64 newtype) → `phux_protocol::ClientId` (u32),
/// the wire id that rides in `TerminalControl` events (ADR-0033). Matches the
/// conversion the per-consumer state map and `FRAME_ACK` path already use; the
/// wire `ClientId` space caps at `u32::MAX` (widening needs a protocol bump).
fn wire_client_id(id: ClientId) -> phux_protocol::ids::ClientId {
    phux_protocol::ids::ClientId::new(u32::try_from(id.0).unwrap_or(u32::MAX))
}

/// Outcome of resolving an `ACQUIRE_INPUT` against the lease map (ADR-0033).
enum AcquireOutcome {
    /// The wire id resolved to no pane.
    NotFound,
    /// A cooperative acquire lost to an existing holder (carried for the
    /// diagnostic).
    Denied(ClientId),
    /// The lease was granted; broadcast the change via the pane's actor.
    Granted {
        /// The pane actor to notify.
        handle: Box<TerminalHandle>,
        /// `Acquired` (was free / self) or `Seized` (preempted another).
        action: ControlAction,
    },
}

/// Handle `ACQUIRE_INPUT` (ADR-0033, "take the wheel"): assert an exclusive
/// input lease over a pane. `Cooperative` mode fails with `InputLeaseHeld`
/// when another client holds it; `Seize` preempts. On grant, broadcasts a
/// `TerminalControl` event so every subscriber re-renders who has the wheel.
///
/// `ttl_ms` is advisory in this server: the lease is held until the holder
/// releases it or its connection drops (see [`crate::state::ServerState::detach`]).
pub(crate) async fn handle_acquire_input(
    state: &SharedState,
    client_id: ClientId,
    terminal_id: &phux_protocol::ids::TerminalId,
    mode: InputMode,
    _ttl_ms: u32,
) -> CommandResult {
    if !terminal_id.is_local() {
        return CommandResult::Error {
            code: ErrorCode::UnsupportedSatelliteRoute,
            message: format!("ACQUIRE_INPUT on satellite route unsupported: {terminal_id:?}"),
        };
    }
    let outcome = state.with_mut(|s| {
        let Some(core) = s.terminal_from_wire(terminal_id) else {
            return AcquireOutcome::NotFound;
        };
        let Some(handle) = s.terminal_handle(core).cloned() else {
            return AcquireOutcome::NotFound;
        };
        let prior = s.input_lease_holder(core);
        if mode == InputMode::Cooperative
            && let Some(holder) = prior
            && holder != client_id
        {
            return AcquireOutcome::Denied(holder);
        }
        s.set_input_lease(core, client_id);
        let action = match prior {
            Some(holder) if holder != client_id => ControlAction::Seized,
            _ => ControlAction::Acquired,
        };
        AcquireOutcome::Granted {
            handle: Box::new(handle),
            action,
        }
    });
    match outcome {
        AcquireOutcome::NotFound => CommandResult::Error {
            code: ErrorCode::TerminalNotFound,
            message: format!("no such terminal: {terminal_id:?}"),
        },
        AcquireOutcome::Denied(holder) => CommandResult::Error {
            code: ErrorCode::InputLeaseHeld,
            message: format!("input lease held by client {}", holder.0),
        },
        AcquireOutcome::Granted { handle, action } => {
            let _ = handle
                .control
                .send(ControlRequest::LeaseChanged {
                    input_holder: Some(wire_client_id(client_id)),
                    action,
                    actor: wire_client_id(client_id),
                })
                .await;
            CommandResult::Ok
        }
    }
}

/// Handle `RELEASE_INPUT` (ADR-0033): drop the input lease the caller holds
/// over a pane, returning it to `Open`. Idempotent — a no-op (still `Ok`) if
/// the caller does not hold the lease. Broadcasts `Released` when a lease was
/// actually given up.
pub(crate) async fn handle_release_input(
    state: &SharedState,
    client_id: ClientId,
    terminal_id: &phux_protocol::ids::TerminalId,
) -> CommandResult {
    if !terminal_id.is_local() {
        return CommandResult::Error {
            code: ErrorCode::UnsupportedSatelliteRoute,
            message: format!("RELEASE_INPUT on satellite route unsupported: {terminal_id:?}"),
        };
    }
    let released = state.with_mut(|s| {
        let core = s.terminal_from_wire(terminal_id)?;
        let handle = s.terminal_handle(core).cloned()?;
        Some((handle, s.release_input_lease(core, client_id)))
    });
    match released {
        None => CommandResult::Error {
            code: ErrorCode::TerminalNotFound,
            message: format!("no such terminal: {terminal_id:?}"),
        },
        Some((handle, did_release)) => {
            if did_release {
                let _ = handle
                    .control
                    .send(ControlRequest::LeaseChanged {
                        input_holder: None,
                        action: ControlAction::Released,
                        actor: wire_client_id(client_id),
                    })
                    .await;
            }
            CommandResult::Ok
        }
    }
}

/// Handle `SIGNAL_TERMINAL` (ADR-0033): deliver a POSIX signal to the pane's
/// process group. Distinct from `KILL_TERMINAL` (which removes the pane) —
/// this signals the process and leaves the pane addressable. The actor owns
/// the PTY child pid, so the work happens there; the broadcast follows.
pub(crate) async fn handle_signal_terminal(
    state: &SharedState,
    client_id: ClientId,
    terminal_id: &phux_protocol::ids::TerminalId,
    signal: TerminalSignal,
) -> CommandResult {
    if !terminal_id.is_local() {
        return CommandResult::Error {
            code: ErrorCode::UnsupportedSatelliteRoute,
            message: format!("SIGNAL_TERMINAL on satellite route unsupported: {terminal_id:?}"),
        };
    }
    let resolved = state.with(|s| {
        let core = s.terminal_from_wire(terminal_id)?;
        let holder = s.input_lease_holder(core).map(wire_client_id);
        s.terminal_handle(core).cloned().map(|h| (h, holder))
    });
    let Some((handle, input_holder)) = resolved else {
        return CommandResult::Error {
            code: ErrorCode::TerminalNotFound,
            message: format!("no such terminal: {terminal_id:?}"),
        };
    };
    let (reply_tx, reply_rx) = oneshot::channel();
    if handle
        .control
        .send(ControlRequest::Signal {
            signal,
            input_holder,
            by: wire_client_id(client_id),
            reply: reply_tx,
        })
        .await
        .is_err()
    {
        return CommandResult::Error {
            code: ErrorCode::InternalError,
            message: "pane actor unavailable for SIGNAL_TERMINAL".to_owned(),
        };
    }
    match reply_rx.await {
        Ok(Ok(())) => CommandResult::Ok,
        Ok(Err(msg)) => CommandResult::Error {
            code: ErrorCode::InternalError,
            message: msg,
        },
        Err(_) => CommandResult::Error {
            code: ErrorCode::InternalError,
            message: "pane actor dropped SIGNAL_TERMINAL reply".to_owned(),
        },
    }
}

/// Handle `SUBSCRIBE_TERMINAL_EVENTS` command.
///
/// Resolves the wire `terminal_id` to a pane actor and registers the caller
/// as an event subscriber. The server will broadcast semantic events
/// (`CommandStarted`, `CommandEnded`, `GridChanged`, etc.) as they occur, filtered
/// by `event_types` (empty = all types). The subscription persists until the
/// client detaches or the connection closes.
///
/// Replies `CommandResult::Ok` immediately; events flow asynchronously as
/// `Event` frames to the client's outbound mailbox. `try_send` semantics:
/// a full subscriber mailbox drops events (accelerator semantics, not
/// guaranteed delivery).
pub(crate) fn handle_subscribe_terminal_events(
    state: &SharedState,
    client_id: ClientId,
    terminal_id: &phux_protocol::ids::TerminalId,
    event_types: Vec<phux_protocol::wire::frame::TerminalEventType>,
    out_tx: &tokio::sync::mpsc::Sender<Outbound>,
) -> CommandResult {
    use crate::terminal_actor::{SubscribeToEventsRequest, TerminalEventSubscriber};

    // Resolve the wire id to its pane actor (same pattern as handle_route_input).
    let handle = state.with(|s| {
        let core = s.terminal_from_wire(terminal_id)?;
        s.terminal_handle(core).cloned()
    });

    let Some(handle) = handle else {
        return CommandResult::Error {
            code: ErrorCode::TerminalNotFound,
            message: format!("no such terminal: {terminal_id:?}"),
        };
    };

    debug!(
        ?client_id,
        ?terminal_id,
        "SUBSCRIBE_TERMINAL_EVENTS registering"
    );

    // Get the wire terminal id for use in Event frames.
    let wire_terminal_id = terminal_id.local_id().unwrap_or(0);

    // Build the subscriber request and send to the actor.
    // The subscriber receives the client's outbound mailbox directly,
    // so events are forwarded straight to the client without an intermediary.
    let req = SubscribeToEventsRequest {
        subscriber: TerminalEventSubscriber {
            outbound: out_tx.clone(),
            event_types,
        },
        wire_terminal_id,
    };

    if handle.subscribe_to_events.try_send(req).is_err() {
        return CommandResult::Error {
            code: ErrorCode::InternalError,
            message: "pane actor unavailable for SUBSCRIBE_TERMINAL_EVENTS".to_owned(),
        };
    }

    debug!(
        ?client_id,
        ?terminal_id,
        "SUBSCRIBE_TERMINAL_EVENTS: subscriber registered"
    );
    CommandResult::Ok
}

pub(crate) fn handle_report_asked(
    state: &SharedState,
    terminal_id: &phux_protocol::ids::TerminalId,
    id: String,
    question: String,
    suggestions: Vec<String>,
    elapsed_seconds: Option<u64>,
) -> CommandResult {
    if !terminal_id.is_local() {
        return CommandResult::Error {
            code: ErrorCode::UnsupportedSatelliteRoute,
            message: format!("REPORT_ASKED on satellite route unsupported: {terminal_id:?}"),
        };
    }
    if state.with(|s| s.terminal_from_wire(terminal_id)).is_none() {
        return CommandResult::Error {
            code: ErrorCode::TerminalNotFound,
            message: format!("no such terminal: {terminal_id:?}"),
        };
    }
    if let Some(message) = validate_asked_payload(&id, &question, &suggestions) {
        return CommandResult::Error {
            code: ErrorCode::InvalidCommand,
            message,
        };
    }
    super::client::broadcast_event(
        state,
        Some(terminal_id),
        &AgentEvent::Asked {
            id,
            question,
            suggestions,
            elapsed_seconds,
        },
    );
    CommandResult::Ok
}

fn validate_asked_payload(id: &str, question: &str, suggestions: &[String]) -> Option<String> {
    const MAX_ID_BYTES: usize = 128;
    const MAX_QUESTION_BYTES: usize = 4096;
    const MAX_SUGGESTIONS: usize = 16;
    const MAX_SUGGESTION_BYTES: usize = 512;

    if question.trim().is_empty() {
        return Some("asked question must not be empty".to_owned());
    }
    if id.len() > MAX_ID_BYTES {
        return Some(format!("asked id exceeds {MAX_ID_BYTES} bytes"));
    }
    if question.len() > MAX_QUESTION_BYTES {
        return Some(format!("asked question exceeds {MAX_QUESTION_BYTES} bytes"));
    }
    if suggestions.len() > MAX_SUGGESTIONS {
        return Some(format!(
            "asked suggestions exceed {MAX_SUGGESTIONS} entries"
        ));
    }
    for suggestion in suggestions {
        if suggestion.trim().is_empty() {
            return Some("asked suggestions must not be empty".to_owned());
        }
        if suggestion.len() > MAX_SUGGESTION_BYTES {
            return Some(format!(
                "asked suggestion exceeds {MAX_SUGGESTION_BYTES} bytes"
            ));
        }
    }
    None
}

/// A `SessionSnapshot` describing a server with no sessions: empty lists,
/// sentinel focus ids. Used by `GET_STATE` when the registry is empty.
pub(crate) const fn empty_session_snapshot() -> phux_protocol::wire::info::SessionSnapshot {
    use phux_protocol::ids::{SessionId, TerminalId, WindowId};
    phux_protocol::wire::info::SessionSnapshot::new(
        SessionId::new(0),
        WindowId::new(0),
        TerminalId::local(0),
    )
}

/// Handle a client's `VIEWPORT_RESIZE` (SPEC §7.1 / §10.5).
///
/// Look up the client's currently-focused pane and update the in-memory
/// `dims` so future `TERMINAL_SNAPSHOT` frames reflect the new size. This is
/// the additive surface for phux-4hp: we deliberately do NOT push a
/// resize into the [`TerminalActor`] (or call `Terminal::set_size` /
/// `pty.resize(...)`) because byc.5's PTY pump owns the actor-side
/// `Terminal` / `portable-pty` resize integration. The follow-up there
/// will consume this state change (or, if it prefers a direct channel,
/// can add a new `TerminalHandle` channel without touching this code).
///
/// Per SPEC §10.5, when multiple clients are attached with different
/// sizes the server uses the smallest common bounding box per window.
/// That negotiation lives with byc.5 too; today the last writer wins,
/// which matches single-attach behavior (the only path exercised).
///
/// Silent on every "not-found" path. A `VIEWPORT_RESIZE` from an
/// unattached client is a benign race (the client may have sent it
/// before its ATTACH completed); logging at `debug!` is enough.
pub(crate) fn handle_viewport_resize(
    state: &SharedState,
    client_id: ClientId,
    viewport: &ViewportInfo,
) {
    state.with_mut(|s| {
        let Some(client) = s.attached.get(&client_id) else {
            debug!(
                ?client_id,
                "VIEWPORT_RESIZE from non-attached client; ignoring"
            );
            return;
        };
        let session_id = client.session;
        let Some(session) = s.registry.session(session_id) else {
            debug!(?client_id, "VIEWPORT_RESIZE: client's session vanished");
            return;
        };
        let Some(window_id) = session.active else {
            debug!(?client_id, "VIEWPORT_RESIZE: no active window in session");
            return;
        };
        let Some(window) = s.registry.window(window_id) else {
            return;
        };
        let Some(terminal_id) = window.active else {
            return;
        };
        // phux-nk07: record this client's viewport, then resolve the
        // Terminal's authoritative geometry by applying the window-size policy
        // across EVERY subscriber's viewport — not last-writer-wins, which let
        // two differently-sized clients thrash each other's grid. `Manual` (or
        // no usable viewport yet) yields `None`: leave the PTY size untouched.
        s.set_client_viewport(client_id, *viewport);
        let Some((cols, rows)) = s.resolve_terminal_geometry(terminal_id, Some(*viewport)) else {
            debug!(
                ?client_id,
                ?terminal_id,
                "VIEWPORT_RESIZE: window-size policy yielded no geometry; PTY size unchanged",
            );
            return;
        };
        if let Some(pane) = s.registry.terminal_mut(terminal_id) {
            pane.dims = (cols, rows);
        }
        // Pixel geometry rides along: the most recent usable pixel report
        // among this Terminal's subscribers — normally the viewport just
        // recorded above — fixes the cell size the PTY winsize and
        // XTWINOPS replies advertise.
        let cell_px = s.resolve_terminal_cell_px(terminal_id);
        // Fan the resize out to the TerminalActor so libghostty's
        // `Terminal::set_size` and the PTY `winsize` ioctl get
        // updated. byc.5 added the `resize` channel on `TerminalHandle`;
        // this is the missing connector (4hp ↔ byc.5).
        //
        // We hold the state lock here so `try_send` is the right
        // primitive: VIEWPORT_RESIZE is fire-and-forget per SPEC §10.5,
        // and an `.await` inside `with_mut` would deadlock the
        // single-threaded runtime. On send failure (actor terminated,
        // mailbox full — both rare; the resize mailbox is sized at
        // `DEFAULT_INPUT_MAILBOX` = 64), we log and continue: a
        // dropped resize is recoverable (the next resize, or the
        // next snapshot, re-syncs) and SPEC §10.5 explicitly classes
        // VIEWPORT_RESIZE as best-effort.
        if let Some(handle) = s.terminals.get(&terminal_id) {
            // Live viewport resize (SIGWINCH): resync clients (phux-8v1).
            match handle.resize.try_send(ResizeRequest {
                cols,
                rows,
                cell_px,
                resync_clients: true,
                resync_only: false,
            }) {
                Ok(()) => {}
                Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                    warn!(
                        ?client_id,
                        ?terminal_id,
                        cols,
                        rows,
                        "VIEWPORT_RESIZE: pane resize mailbox full; dropping (fire-and-forget per SPEC §10.5)",
                    );
                }
                Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                    debug!(
                        ?client_id,
                        ?terminal_id,
                        "VIEWPORT_RESIZE: pane actor gone; dropping resize",
                    );
                }
            }
        } else {
            debug!(
                ?client_id,
                ?terminal_id,
                "VIEWPORT_RESIZE: no TerminalHandle registered for pane; dropping resize",
            );
        }
    });
}

/// Route an `INPUT_*` frame body to the target pane's [`TerminalActor`].
///
/// SPEC §9: input frames are fire-and-forget — no `Outbound` reply.
/// On the wire the pane is identified by its `WireTerminalId` (`u32`); we
/// resolve it back to a core [`phux_core::ids::TerminalId`] via
/// [`crate::state::ServerState::terminal_from_wire`],
/// then locate the [`TerminalHandle`] and `try_send` the encoded
/// [`TerminalInput`] onto the actor's input mailbox.
///
/// Validation: we drop with `warn!` (not `debug!`, this is observable
/// misbehavior worth surfacing) on:
///   * Unknown wire pane id (no [`phux_core::ids::TerminalId`] mapping).
///   * Client not attached (the per-client task should not be reading
///     frames from a detached identity, but we re-check defensively).
///   * Client attached but not subscribed to this pane — prevents one
///     client from steering another's pane (SPEC §9 leaves multi-client
///     subscription rules to per-pane policy; for now subscription is
///     the gate).
///   * Pane has no registered [`TerminalHandle`] (actor never spawned, or
///     spawned but evicted).
///
/// `try_send` is used because we hold the `with_mut` lock while routing:
/// awaiting inside a `with_mut` would deadlock the single-threaded
/// runtime, and an unbounded queue would let a slow PTY producer push
/// memory through the roof. `Full` is treated as a backpressure event
/// (warn-drop); `Closed` is logged at debug and dropped (actor gone).
pub(crate) fn handle_terminal_input(
    state: &SharedState,
    client_id: ClientId,
    wire_terminal_id: &phux_protocol::ids::TerminalId,
    input: TerminalInput,
    frame_label: &'static str,
) {
    // v0.1 non-federation-hub servers reject SATELLITE-routed input frames
    // (per ADR-0016 / SPEC §10.1). The protocol-level response is `ERROR
    // { UnsupportedSatelliteRoute }`; this dispatch helper just drops the
    // frame with a warn — the surrounding read loop will surface the
    // error response in a follow-up tied to phux-byc.9.
    if !wire_terminal_id.is_local() {
        warn!(
            ?client_id,
            ?wire_terminal_id,
            frame_label,
            "input frame carried a SATELLITE TerminalId on a non-federation-hub server; dropping",
        );
        return;
    }
    state.with_mut(|s| {
        let Some(pane) = s.terminal_from_wire(wire_terminal_id) else {
            warn!(
                ?client_id,
                ?wire_terminal_id,
                frame_label,
                "input frame for unknown pane; dropping",
            );
            return;
        };
        let Some(attached) = s.attached.get(&client_id) else {
            warn!(
                ?client_id,
                ?wire_terminal_id,
                frame_label,
                "input frame from non-attached client; dropping",
            );
            return;
        };
        // Subscription gate: the pane must be one the client is observing.
        // For byc.8's "active pane only" subscription model this is the
        // same as "is the pane in the client's attached session"; a
        // richer SUBSCRIBE story (SPEC §7.4) will refine this without
        // changing the dispatch shape.
        let session = attached.session;
        let is_subscribed = s.subscribers_for_terminal(pane).contains(&client_id);
        if !is_subscribed {
            warn!(
                ?client_id,
                ?wire_terminal_id,
                ?session,
                frame_label,
                "client not subscribed to pane; dropping input",
            );
            return;
        }
        // Input-lease gate (ADR-0033, "take the wheel"): when another client
        // holds the lease, drop this client's input. Dropped, not errored —
        // the fire-and-forget input invariant (SPEC §12.2) holds, and the
        // client renders the locked state from the `TerminalControl` event.
        if s.input_blocked(pane, client_id) {
            trace!(
                ?client_id,
                ?wire_terminal_id,
                frame_label,
                "input dropped: another client holds the input lease",
            );
            return;
        }
        s.touch_session(session);
        let Some(handle): Option<&TerminalHandle> = s.terminal_handle(pane) else {
            warn!(
                ?client_id,
                ?wire_terminal_id,
                frame_label,
                "no TerminalHandle for pane; dropping input",
            );
            return;
        };
        match handle.input.try_send(input) {
            Ok(()) => {
                trace!(
                    ?client_id,
                    ?wire_terminal_id,
                    frame_label,
                    "input routed to TerminalActor"
                );
            }
            Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                warn!(
                    ?client_id,
                    ?wire_terminal_id,
                    frame_label,
                    "pane input mailbox full; dropping (fire-and-forget per SPEC §9)",
                );
            }
            Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                debug!(
                    ?client_id,
                    ?wire_terminal_id,
                    frame_label,
                    "pane actor gone; dropping input",
                );
            }
        }
    });
}

/// Route an inbound `FRAME_ACK` (SPEC §7.proto.1 / §12.2) to the
/// owning `TerminalActor` so it can evict the per-consumer dirty cache
/// under ADR-0018 lazy state synchronization (phux-q0e.4).
///
/// Validation:
///   * Unknown wire pane id → drop (warn). The client is acking a
///     terminal the server has no mapping for; this is observable
///     misbehavior worth surfacing.
///   * Client not attached → drop (warn). Acks make no sense without
///     an attachment.
///   * Client not subscribed to this pane → drop (warn). Same gate as
///     `handle_terminal_input`: a client cannot ack a pane it does not
///     observe.
///   * No `TerminalHandle` (actor evicted) → drop (debug — race against
///     teardown).
///
/// `try_send` is non-blocking by the same `with_mut` locking rationale
/// as `handle_terminal_input`: awaiting inside `with_mut` would
/// deadlock the single-threaded runtime, and `FRAME_ACK` is hint-shaped
/// per ADR-0018 — dropping under backpressure is correct (the next
/// ack the client sends will catch up the per-consumer reference,
/// and unacked diffs stay re-emittable in the meantime).
pub(crate) fn handle_frame_ack(
    state: &SharedState,
    client_id: ClientId,
    wire_terminal_id: &phux_protocol::ids::TerminalId,
    seq: u64,
) {
    // v0.1 servers reject SATELLITE-routed acks for the same reason input
    // frames are dropped above: this server is not a federation hub.
    if !wire_terminal_id.is_local() {
        warn!(
            ?client_id,
            ?wire_terminal_id,
            seq,
            "FRAME_ACK carried a SATELLITE TerminalId on a non-federation-hub server; dropping",
        );
        return;
    }
    state.with_mut(|s| {
        let Some(pane) = s.terminal_from_wire(wire_terminal_id) else {
            warn!(
                ?client_id,
                ?wire_terminal_id,
                seq,
                "FRAME_ACK for unknown pane; dropping",
            );
            return;
        };
        let Some(attached) = s.attached.get(&client_id) else {
            warn!(
                ?client_id,
                ?wire_terminal_id,
                seq,
                "FRAME_ACK from non-attached client; dropping",
            );
            return;
        };
        let session = attached.session;
        let is_subscribed = s.subscribers_for_terminal(pane).contains(&client_id);
        if !is_subscribed {
            warn!(
                ?client_id,
                ?wire_terminal_id,
                ?session,
                seq,
                "FRAME_ACK from client not subscribed to pane; dropping",
            );
            return;
        }
        let Some(handle): Option<&TerminalHandle> = s.terminal_handle(pane) else {
            warn!(
                ?client_id,
                ?wire_terminal_id,
                seq,
                "FRAME_ACK with no TerminalHandle for pane; dropping",
            );
            return;
        };
        // Bridge `state::ClientId` (u64 newtype) → `phux_protocol::ClientId`
        // (u32), matching the conversion `handle_attach` already does for
        // the per-consumer state map keys. The wire ClientId space caps at
        // u32::MAX; widening would require a protocol bump.
        let wire_client_id =
            phux_protocol::ids::ClientId::new(u32::try_from(client_id.0).unwrap_or(u32::MAX));
        match handle.consumer_ack.try_send(ConsumerAckRequest {
            client_id: wire_client_id,
            seq,
        }) {
            Ok(()) => {
                trace!(
                    ?client_id,
                    ?wire_terminal_id,
                    seq,
                    "FRAME_ACK routed to TerminalActor"
                );
            }
            Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                trace!(
                    ?client_id,
                    ?wire_terminal_id,
                    seq,
                    "FRAME_ACK mailbox full; dropping (ADR-0018: next ack catches up)",
                );
            }
            Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                debug!(
                    ?client_id,
                    ?wire_terminal_id,
                    seq,
                    "FRAME_ACK: pane actor gone; dropping",
                );
            }
        }
    });
}
