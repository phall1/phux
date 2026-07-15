//! Shared server-state and L3 tag lookup helpers.
//!
//! CLI and MCP consumers use these free functions instead of maintaining
//! separate `GET_STATE` and `GET_METADATA` receive loops. Selector resolution
//! remains client-side (ADR-0017), and candidates retain snapshot order.

use std::path::Path;

use phux_protocol::ids::TerminalId;
use phux_protocol::wire::frame::{
    Command, CommandResult, CommandValue, FrameKind, Scope, StateScope, TERMINAL_TAGS_KEY,
};
use phux_protocol::wire::info::SessionSnapshot;

use crate::attach::AttachError;
use crate::attach::connection::Connection;
use crate::selector::{self, Selector, TagIndex};

/// Fetch the server-wide session snapshot over a fresh connection.
///
/// # Errors
///
/// Returns a transport error when the connection cannot be opened or closes
/// during the request, a refusal when the server rejects `GET_STATE`, or a
/// protocol error when the matching response has an unexpected value.
pub async fn get_state(socket: &Path) -> Result<SessionSnapshot, AttachError> {
    let mut conn = Connection::connect(socket).await?;
    get_state_on(&mut conn).await
}

/// Fetch the server-wide session snapshot over an existing connection.
///
/// Unrelated interleaved frames are skipped until the matching command result
/// arrives, as required by SPEC §5.
///
/// # Errors
///
/// Returns a transport error when the connection closes during the request, a
/// refusal when the server rejects `GET_STATE`, or a protocol error when the
/// matching response has an unexpected value.
pub async fn get_state_on(conn: &mut Connection) -> Result<SessionSnapshot, AttachError> {
    const REQUEST_ID: u32 = 0;
    conn.send(&FrameKind::Command {
        request_id: REQUEST_ID,
        command: Command::GetState {
            scope: StateScope::Server,
        },
    })
    .await?;

    loop {
        if let FrameKind::CommandResult { request_id, result } = conn.recv().await?
            && request_id == REQUEST_ID
        {
            return match result {
                CommandResult::OkWith(CommandValue::State(snapshot)) => Ok(snapshot),
                CommandResult::Error { message, .. } => Err(AttachError::Refused(message)),
                other => Err(AttachError::Protocol(format!(
                    "unexpected GET_STATE result: {other:?}"
                ))),
            };
        }
    }
}

/// Fetch the L3 tag index for every pane in `snapshot` over `conn`.
///
/// Requests are pipelined and matched by request id. Missing, empty, or
/// malformed `phux.tags/v1` values are omitted. This lookup is best-effort:
/// if the server disconnects, the entries received so far are returned.
pub async fn fetch_tag_index(conn: &mut Connection, snapshot: &SessionSnapshot) -> TagIndex {
    let ids: Vec<TerminalId> = snapshot.panes.iter().map(|pane| pane.id.clone()).collect();
    let mut index = TagIndex::new();

    // GET_STATE uses request id 0, so metadata requests start at 1 on shared
    // connections. A snapshot cannot practically contain u32::MAX panes; the
    // saturating conversion still keeps malformed fixtures panic-free.
    for (offset, id) in ids.iter().enumerate() {
        let request_id = u32::try_from(offset).unwrap_or(u32::MAX).saturating_add(1);
        if conn
            .send(&FrameKind::GetMetadata {
                request_id,
                scope: Scope::Terminal(id.clone()),
                key: TERMINAL_TAGS_KEY.to_owned(),
            })
            .await
            .is_err()
        {
            return index;
        }
    }

    let mut remaining = ids.len();
    while remaining > 0 {
        match conn.recv().await {
            Ok(FrameKind::MetadataValue { request_id, value }) => {
                let Some(position) = usize::try_from(request_id)
                    .ok()
                    .and_then(|id| id.checked_sub(1))
                else {
                    continue;
                };
                let Some(terminal_id) = ids.get(position) else {
                    continue;
                };
                remaining -= 1;
                if let Some(bytes) = value
                    && let Ok(tags) = serde_json::from_slice::<Vec<String>>(&bytes)
                    && !tags.is_empty()
                {
                    index.insert(terminal_id.clone(), tags);
                }
            }
            Ok(_) => {}
            Err(_) => break,
        }
    }
    index
}

/// Resolve a selector against a snapshot, fetching L3 tags only for `#tag`.
///
/// Non-tag selectors are resolved synchronously without an extra connection.
/// A tag lookup failure degrades to an empty index, preserving the established
/// CLI behavior that reports the result as a selector miss.
pub async fn resolve_targets(
    socket: &Path,
    selector: &Selector,
    snapshot: &SessionSnapshot,
) -> Vec<TerminalId> {
    if !matches!(selector, Selector::Tag(_)) {
        return selector::resolve(selector, snapshot);
    }

    let tags = match Connection::connect(socket).await {
        Ok(mut conn) => fetch_tag_index(&mut conn, snapshot).await,
        Err(_) => TagIndex::new(),
    };
    selector::resolve_with_tags(selector, snapshot, &tags)
}

#[cfg(test)]
mod tests {
    use bytes::BytesMut;
    use phux_protocol::ids::{SessionId, TerminalId, WindowId};
    use phux_protocol::wire::frame::{CommandResult, CommandValue, FrameKind};
    use phux_protocol::wire::info::SessionSnapshot;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::UnixListener;

    use super::get_state;

    #[tokio::test]
    async fn get_state_skips_unrelated_command_results() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("state.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let expected =
            SessionSnapshot::new(SessionId::new(7), WindowId::new(8), TerminalId::local(9));
        let response = expected.clone();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let request = read_frame(&mut stream).await;
            assert!(matches!(
                request,
                FrameKind::Command {
                    request_id: 0,
                    command: phux_protocol::wire::frame::Command::GetState { .. }
                }
            ));
            write_frame(
                &mut stream,
                &FrameKind::CommandResult {
                    request_id: 99,
                    result: CommandResult::Ok,
                },
            )
            .await;
            write_frame(
                &mut stream,
                &FrameKind::CommandResult {
                    request_id: 0,
                    result: CommandResult::OkWith(CommandValue::State(response)),
                },
            )
            .await;
        });

        let actual = get_state(&socket).await.unwrap();
        assert_eq!(actual, expected);
        server.await.unwrap();
    }

    async fn read_frame(stream: &mut tokio::net::UnixStream) -> FrameKind {
        let mut header = [0_u8; 4];
        stream.read_exact(&mut header).await.unwrap();
        let body_len = usize::try_from(u32::from_be_bytes(header)).unwrap();
        let mut encoded = Vec::with_capacity(4 + body_len);
        encoded.extend_from_slice(&header);
        encoded.resize(4 + body_len, 0);
        stream.read_exact(&mut encoded[4..]).await.unwrap();
        let (frame, tail) = FrameKind::decode(&encoded).unwrap();
        assert!(tail.is_empty());
        frame
    }

    async fn write_frame(stream: &mut tokio::net::UnixStream, frame: &FrameKind) {
        let mut encoded = BytesMut::new();
        frame.encode(&mut encoded);
        stream.write_all(&encoded).await.unwrap();
    }
}
