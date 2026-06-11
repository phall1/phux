//! Producer for the graceful-upgrade state blob (ADR-0032): walk the live
//! [`ServerState`] tree and each pane's actor into a
//! [`StateBlob`](crate::upgrade::blob::StateBlob) the re-exec'd image rebuilds
//! from.

use std::os::fd::RawFd;
use std::path::PathBuf;
use std::time::UNIX_EPOCH;

use phux_core::ids::{SessionId, TerminalId, WindowId};
use phux_core::terminal::TerminalDescriptor;
use phux_core::window::{LayoutNode, SplitDir};
use tokio::sync::oneshot;

use super::ServerState;
use crate::terminal_actor::{PaneUpgradeHandle, UpgradeHandleRequest};
use crate::upgrade::blob::{
    BLOB_VERSION, Counters, LayoutBlob, PaneBlob, SessionBlob, SplitDirBlob, StateBlob, WindowBlob,
};

impl ServerState {
    /// Assemble a [`StateBlob`] from the live session/window/pane tree for a
    /// graceful upgrade.
    ///
    /// Walks sessions → windows → panes, keying everything by wire id, and
    /// asks each pane's actor (over its `upgrade` mailbox) for the PTY
    /// descriptors + replay snapshot. `listener_fd` is the inherited
    /// `UnixListener` descriptor the orchestrator will pass to the new image.
    ///
    /// Must run inside the `LocalSet` that owns the pane actors (it awaits
    /// their replies). A pane whose actor cannot be reached is recorded from
    /// its descriptor with no handoff — the resume path then has nothing to
    /// re-adopt for it.
    pub async fn build_upgrade_blob(&self, listener_fd: RawFd) -> StateBlob {
        let mut sessions = Vec::new();
        let mut windows = Vec::new();
        let mut panes = Vec::new();

        for (sid, session) in self.registry.sessions() {
            let Some(session_wire) = self.session_wire(sid) else {
                continue;
            };
            sessions.push(SessionBlob {
                wire_id: session_wire,
                name: session.name.clone(),
                window_wire_ids: session
                    .windows
                    .iter()
                    .filter_map(|w| self.window_wire(*w))
                    .collect(),
                active_window: session.active.and_then(|w| self.window_wire(w)),
                created_at_unix_nanos: session
                    .created_at
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or(0),
                last_touched: self.session_last_touched.get(&sid).copied(),
                root: self.session_root.get(&sid).cloned(),
            });

            for &wid in &session.windows {
                let (Some(window), Some(window_wire)) =
                    (self.registry.window(wid), self.window_wire(wid))
                else {
                    continue;
                };
                windows.push(WindowBlob {
                    wire_id: window_wire,
                    session_wire_id: session_wire,
                    pane_wire_ids: window
                        .panes
                        .iter()
                        .filter_map(|t| self.terminal_wire(*t))
                        .collect(),
                    active_pane: window.active.and_then(|t| self.terminal_wire(t)),
                    layout: window.layout.as_ref().and_then(|l| self.layout_to_blob(l)),
                    last_cwd: self.window_last_cwd.get(&wid).cloned(),
                });

                for &tid in &window.panes {
                    let (Some(desc), Some(pane_wire)) =
                        (self.registry.terminal(tid), self.terminal_wire(tid))
                    else {
                        continue;
                    };
                    let handoff = self.request_pane_handoff(tid).await;
                    panes.push(pane_blob(pane_wire, window_wire, desc, &self.term, handoff));
                }
            }
        }

        StateBlob {
            version: BLOB_VERSION,
            listener_fd,
            counters: Counters {
                next_session_wire_id: self.session_id_bridge.next_wire(),
                next_terminal_wire_id: self.next_terminal_wire_id,
                next_window_wire_id: self.next_window_wire_id,
                next_touch_timestamp: self.next_touch_timestamp,
            },
            sessions,
            windows,
            panes,
        }
    }

    /// Ask one pane's actor for its upgrade handoff. `None` when the pane has
    /// no registered handle or the actor has gone away.
    async fn request_pane_handoff(&self, tid: TerminalId) -> Option<PaneUpgradeHandle> {
        let handle = self.terminals.get(&tid)?;
        let (reply, rx) = oneshot::channel();
        handle
            .upgrade
            .send(UpgradeHandleRequest { reply })
            .await
            .ok()?;
        rx.await.ok()
    }

    fn session_wire(&self, sid: SessionId) -> Option<u32> {
        self.session_id_bridge
            .wire(sid)
            .map(phux_protocol::SessionId::get)
    }

    fn window_wire(&self, wid: WindowId) -> Option<u32> {
        self.window_wire_forward.get(&wid).map(|w| w.get())
    }

    fn terminal_wire(&self, tid: TerminalId) -> Option<u32> {
        self.terminal_wire_forward
            .get(&tid)
            .and_then(phux_protocol::TerminalId::local_id)
    }

    /// Map a [`LayoutNode`] to its wire-id-keyed [`LayoutBlob`] mirror. Returns
    /// `None` if any referenced pane lacks a wire id (it would not round-trip).
    fn layout_to_blob(&self, node: &LayoutNode) -> Option<LayoutBlob> {
        match node {
            LayoutNode::Leaf(tid) => self.terminal_wire(*tid).map(LayoutBlob::Leaf),
            LayoutNode::Split {
                dir,
                ratio,
                left,
                right,
            } => Some(LayoutBlob::Split {
                dir: match dir {
                    SplitDir::Horizontal => SplitDirBlob::Horizontal,
                    SplitDir::Vertical => SplitDirBlob::Vertical,
                },
                ratio: *ratio,
                left: Box::new(self.layout_to_blob(left)?),
                right: Box::new(self.layout_to_blob(right)?),
            }),
        }
    }
}

/// Build one [`PaneBlob`], preferring the actor's live values and falling back
/// to the descriptor when there is no handoff.
fn pane_blob(
    wire_id: u32,
    window_wire_id: u32,
    desc: &TerminalDescriptor,
    term: &str,
    handoff: Option<PaneUpgradeHandle>,
) -> PaneBlob {
    let (cols, rows) = handoff.as_ref().map_or(desc.dims, |h| (h.cols, h.rows));
    PaneBlob {
        wire_id,
        window_wire_id,
        cols,
        rows,
        cell_px: handoff.as_ref().and_then(|h| h.cell_px),
        cwd: handoff
            .as_ref()
            .and_then(|h| h.cwd.as_deref())
            .map_or_else(|| desc.cwd.clone(), PathBuf::from),
        title: handoff
            .as_ref()
            .and_then(|h| h.title.clone())
            .or_else(|| desc.title.clone()),
        term: term.to_owned(),
        child_pid: handoff.as_ref().and_then(|h| h.child_pid),
        master_fd: handoff.as_ref().and_then(|h| h.master_fd),
        vt_replay_bytes: handoff
            .as_ref()
            .map(|h| h.vt_replay_bytes.clone())
            .unwrap_or_default(),
        scrollback_bytes: handoff.map(|h| h.scrollback_bytes).unwrap_or_default(),
    }
}

#[cfg(test)]
mod tests {
    use crate::state::ServerState;
    use crate::terminal_actor::TerminalActor;

    /// Walk a one-session/one-window/one-pane state into a blob: the tree
    /// links resolve by wire id and the pane carries the actor's replay
    /// snapshot.
    #[tokio::test(flavor = "current_thread")]
    async fn build_upgrade_blob_captures_tree_and_snapshot() {
        let local = tokio::task::LocalSet::new();
        local
            .run_until(async {
                let mut state = ServerState::new();

                // A session/window/pane in the registry.
                let sid = state.registry.new_session("main".to_owned());
                let wid = state.registry.new_window(sid).expect("new_window");
                let tid = state.registry.new_terminal(wid).expect("new_terminal");
                let session_wire = state.session_id_bridge.intern(sid).get();
                let window_wire = state.intern_window_wire(wid).get();

                // A real (no-PTY) actor answering the upgrade request.
                let bundle = TerminalActor::new_with_seed(20, 5, b"hello").expect("new_with_seed");
                let handle = bundle.handle.clone();
                let token = bundle.token.clone();
                tokio::task::spawn_local(bundle.actor.run());
                let pane_wire = state
                    .register_terminal_handle(tid, handle, token)
                    .local_id()
                    .expect("local wire id");

                let blob = state.build_upgrade_blob(7).await;

                assert_eq!(blob.listener_fd, 7);
                assert_eq!(blob.sessions.len(), 1);
                assert_eq!(blob.windows.len(), 1);
                assert_eq!(blob.panes.len(), 1);

                let s = &blob.sessions[0];
                assert_eq!(s.wire_id, session_wire);
                assert_eq!(s.name, "main");
                assert_eq!(s.window_wire_ids, vec![window_wire]);

                let w = &blob.windows[0];
                assert_eq!(w.wire_id, window_wire);
                assert_eq!(w.session_wire_id, session_wire);
                assert_eq!(w.pane_wire_ids, vec![pane_wire]);

                let p = &blob.panes[0];
                assert_eq!(p.wire_id, pane_wire);
                assert_eq!(p.window_wire_id, window_wire);
                assert_eq!((p.cols, p.rows), (20, 5));
                assert_eq!(p.master_fd, None, "no-PTY actor has no master fd");
                assert_eq!(p.child_pid, None);
                assert!(
                    String::from_utf8_lossy(&p.vt_replay_bytes).contains("hello"),
                    "pane snapshot should carry the actor's seeded text"
                );

                // Counters sit above every minted id.
                assert!(blob.counters.next_session_wire_id > session_wire);
                assert!(blob.counters.next_window_wire_id > window_wire);
                assert!(blob.counters.next_terminal_wire_id > pane_wire);
            })
            .await;
    }
}
