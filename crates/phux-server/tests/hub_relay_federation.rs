//! Loopback federation harness for the hub frame relay (phux-v45.4,
//! ADR-0007 §4).
//!
//! Two real `ServerRuntime`s run in one process: a *satellite* listening
//! on a loopback WebSocket, and a *hub* whose `[[satellites]]` registry
//! points at it. A consumer speaks the ordinary wire protocol to the hub
//! over UDS and addresses the satellite's terminal as
//! `TerminalId::Satellite { host: "sat", id }`:
//!
//! * `command_round_trip_and_stream_retagging` — a satellite-tagged
//!   `GET_SCREEN` round-trips through the hub (outbound id rewrite +
//!   response correlation); a satellite-scoped `SUBSCRIBE_EVENTS` plus a
//!   relayed `REPORT_ASKED` proves the return leg re-tags
//!   `Local -> Satellite { host, id }`; killing the satellite then
//!   delivers the typed `ERROR { SatelliteUnreachable }` teardown
//!   notification and subsequent commands fail fast with the same code.
//! * `down_satellite_fails_fast_with_typed_error` — a registry entry
//!   pointing at a dead port yields `SatelliteUnreachable`, promptly.
//! * `unknown_satellite_host_is_unsupported_route` — a host absent from
//!   the hub table (and any satellite id on a non-hub server) yields
//!   `UnsupportedSatelliteRoute`.

#![allow(clippy::expect_used, reason = "tests")]
#![allow(clippy::unwrap_used, reason = "tests")]
#![allow(clippy::panic, reason = "tests")]

mod common;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use common::{encode_frame, recv_typed, send_frame, wait_for_socket};
use futures_util::{SinkExt, StreamExt};
use phux_config::SatelliteConfigEntry;
use phux_protocol::ids::TerminalId;
use phux_protocol::wire::frame::{
    AgentEvent, Command, CommandResult, CommandValue, ErrorCode, FrameKind,
};
use phux_server::{ServerConfig, ServerError, ServerRuntime};
use tempfile::TempDir;
use tokio::net::{TcpStream, UnixStream};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::Message;

/// Generous per-step deadline, mirroring `common::WIRE_RECV_TIMEOUT`'s
/// rationale (the hub link dials with backoff under full-parallel nextest).
const STEP_DEADLINE: Duration = Duration::from_secs(15);

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn satellite_entry(name: &str, port: u16) -> SatelliteConfigEntry {
    SatelliteConfigEntry {
        name: name.to_owned(),
        endpoint: format!("ws://127.0.0.1:{port}"),
        enabled: true,
        token_file: None,
        cert_fingerprint: None,
    }
}

/// Spawn the satellite: a plain server with one seeded (no-PTY) session,
/// listening on loopback WebSocket in addition to its own UDS.
fn spawn_satellite(
    socket_path: PathBuf,
    ws_port: u16,
) -> (oneshot::Sender<()>, JoinHandle<Result<(), ServerError>>) {
    let (tx, rx) = oneshot::channel::<()>();
    let cfg = ServerConfig {
        socket_path,
        pre_seeded_session: Some("sat-session".to_owned()),
        seed_with_pty: false,
        seed_command: None,
        ..ServerConfig::with_default_socket()
    };
    let handle = tokio::task::spawn_local(async move {
        ServerRuntime::new(cfg)
            .listen_ws(format!("127.0.0.1:{ws_port}").parse().unwrap())
            .run_async(async move {
                let _ = rx.await;
            })
            .await
    });
    (tx, handle)
}

/// Spawn the hub: UDS-only, no seeded session, dialing `satellites`.
fn spawn_hub(
    socket_path: PathBuf,
    satellites: Vec<SatelliteConfigEntry>,
) -> (oneshot::Sender<()>, JoinHandle<Result<(), ServerError>>) {
    let (tx, rx) = oneshot::channel::<()>();
    let cfg = ServerConfig {
        socket_path,
        pre_seeded_session: None,
        seed_with_pty: false,
        seed_command: None,
        ..ServerConfig::with_default_socket()
    };
    let handle = tokio::task::spawn_local(async move {
        ServerRuntime::new(cfg)
            .hub(satellites)
            .run_async(async move {
                let _ = rx.await;
            })
            .await
    });
    (tx, handle)
}

/// Learn the satellite's seeded pane id over a direct WebSocket client:
/// `GET_STATE` returns the snapshot whose focused pane is the seed.
async fn discover_satellite_pane(ws_port: u16) -> u32 {
    let addr = format!("127.0.0.1:{ws_port}");
    let url = format!("ws://{addr}/");
    let deadline = Instant::now() + STEP_DEADLINE;
    let mut ws = loop {
        assert!(
            Instant::now() < deadline,
            "satellite WebSocket never became connectable"
        );
        if let Ok(tcp) = TcpStream::connect(&addr).await
            && let Ok((ws, _)) = tokio_tungstenite::client_async(&url, tcp).await
        {
            break ws;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    };
    let get_state = FrameKind::Command {
        request_id: 900,
        command: Command::GetState {
            scope: phux_protocol::wire::frame::StateScope::Server,
        },
    };
    ws.send(Message::Binary(encode_frame(&get_state).to_vec()))
        .await
        .unwrap();
    let deadline = Instant::now() + STEP_DEADLINE;
    loop {
        assert!(Instant::now() < deadline, "GET_STATE reply never arrived");
        let Some(Ok(Message::Binary(data))) = ws.next().await else {
            continue;
        };
        let (frame, _) = FrameKind::decode(&data).expect("decode satellite frame");
        if let FrameKind::CommandResult {
            request_id: 900,
            result: CommandResult::OkWith(CommandValue::State(snapshot)),
        } = frame
        {
            return snapshot
                .focused_pane
                .local_id()
                .expect("seeded pane is local on the satellite");
        }
    }
}

/// Issue one satellite-tagged `GET_SCREEN` through the hub and return the
/// result. `request_id` correlates the reply against interleaved frames.
async fn get_screen_via_hub(
    hub: &mut UnixStream,
    request_id: u32,
    terminal_id: TerminalId,
) -> CommandResult {
    send_frame(
        hub,
        &FrameKind::Command {
            request_id,
            command: Command::GetScreen {
                terminal_id,
                request_scrollback: None,
                cells: false,
            },
        },
    )
    .await;
    loop {
        let (_, frame) = recv_typed(hub).await;
        if let FrameKind::CommandResult {
            request_id: got,
            result,
        } = frame
            && got == request_id
        {
            return result;
        }
    }
}

/// Retry `GET_SCREEN` through the hub until the link connects and the
/// command succeeds (the dialer backs off while the satellite boots).
async fn get_screen_until_ok(hub: &mut UnixStream, sat_pane: u32) -> CommandResult {
    let deadline = Instant::now() + STEP_DEADLINE;
    let mut request_id = 1000;
    loop {
        let result =
            get_screen_via_hub(hub, request_id, TerminalId::satellite("sat", sat_pane)).await;
        match result {
            CommandResult::Error {
                code: ErrorCode::SatelliteUnreachable,
                ..
            } if Instant::now() < deadline => {
                request_id += 1;
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            other => return other,
        }
    }
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "one linear federation scenario: boot both servers, relay a command, prove re-tagging, then observe teardown; splitting would re-boot the topology per step"
)]
fn command_round_trip_and_stream_retagging() {
    common::run_local(async {
        let tmp = TempDir::new().unwrap();
        let ws_port = free_port();
        let (sat_shutdown, sat_task) = spawn_satellite(tmp.path().join("sat.sock"), ws_port);
        let (hub_shutdown, hub_task) = spawn_hub(
            tmp.path().join("hub.sock"),
            vec![satellite_entry("sat", ws_port)],
        );

        // Learn the satellite's seeded pane id directly (LIST aggregation
        // through the hub is phux-v45.5; this test scopes to relay).
        let sat_pane = discover_satellite_pane(ws_port).await;
        let sat_id = TerminalId::satellite("sat", sat_pane);

        let mut hub = wait_for_socket(&tmp.path().join("hub.sock"), STEP_DEADLINE).await;

        // 1. Outbound leg + response correlation: a satellite-tagged
        //    GET_SCREEN round-trips once the link is up.
        let result = get_screen_until_ok(&mut hub, sat_pane).await;
        let CommandResult::OkWith(CommandValue::Json(json)) = result else {
            panic!("GET_SCREEN through the hub must succeed, got {result:?}");
        };
        assert!(
            json.contains("\"cols\""),
            "screen JSON came from the satellite pane: {json}"
        );

        // A satellite-tagged ROUTE_INPUT (the attach-free input path)
        // relays and acks Ok end-to-end.
        send_frame(
            &mut hub,
            &FrameKind::Command {
                request_id: 2000,
                command: Command::RouteInput {
                    terminal_id: sat_id.clone(),
                    event: phux_protocol::input::InputEvent::Focus(
                        phux_protocol::input::focus::FocusEvent::Gained,
                    ),
                },
            },
        )
        .await;
        loop {
            let (_, frame) = recv_typed(&mut hub).await;
            if let FrameKind::CommandResult {
                request_id: 2000,
                result,
            } = frame
            {
                assert_eq!(result, CommandResult::Ok, "ROUTE_INPUT relays Ok");
                break;
            }
        }

        // 2. Return leg re-tagging: subscribe to the satellite pane's
        //    events through the hub, trigger one via a relayed
        //    REPORT_ASKED, and require the EVENT to come back tagged
        //    Satellite { "sat", id }.
        send_frame(
            &mut hub,
            &FrameKind::SubscribeEvents {
                terminal: Some(sat_id.clone()),
            },
        )
        .await;
        send_frame(
            &mut hub,
            &FrameKind::Command {
                request_id: 2001,
                command: Command::ReportAsked {
                    terminal_id: sat_id.clone(),
                    id: "q-1".to_owned(),
                    question: "proceed?".to_owned(),
                    suggestions: vec!["yes".to_owned(), "no".to_owned()],
                    elapsed_seconds: Some(3),
                },
            },
        )
        .await;
        let mut saw_ack = false;
        let mut saw_event = false;
        while !(saw_ack && saw_event) {
            let (_, frame) = recv_typed(&mut hub).await;
            match frame {
                FrameKind::CommandResult {
                    request_id: 2001,
                    result,
                } => {
                    assert_eq!(result, CommandResult::Ok, "REPORT_ASKED relays Ok");
                    saw_ack = true;
                }
                FrameKind::Event { terminal, event } => {
                    assert_eq!(
                        terminal.as_ref(),
                        Some(&sat_id),
                        "return-leg events must be re-tagged Satellite {{ host, id }}"
                    );
                    assert!(
                        matches!(event, AgentEvent::Asked { ref id, .. } if id == "q-1"),
                        "expected the relayed Asked event, got {event:?}"
                    );
                    saw_event = true;
                }
                _ => {}
            }
        }

        // 3. Satellite disconnect: subscribed consumers get the typed
        //    teardown notification, not silence.
        drop(sat_shutdown);
        let deadline = Instant::now() + STEP_DEADLINE;
        loop {
            assert!(
                Instant::now() < deadline,
                "no SatelliteUnreachable teardown notification arrived"
            );
            let (_, frame) = recv_typed(&mut hub).await;
            if let FrameKind::Error {
                request_id: None,
                code: ErrorCode::SatelliteUnreachable,
                message,
            } = frame
            {
                assert!(
                    message.contains("sat"),
                    "teardown notification names the satellite: {message}"
                );
                break;
            }
        }
        sat_task.await.unwrap().unwrap();

        // 4. Commands to the now-dead satellite fail fast with the same
        //    typed code (bounded by recv_typed's own timeout — no hang).
        let result = get_screen_via_hub(&mut hub, 3000, sat_id).await;
        assert!(
            matches!(
                result,
                CommandResult::Error {
                    code: ErrorCode::SatelliteUnreachable,
                    ..
                }
            ),
            "command to a dead satellite must fail fast, got {result:?}"
        );

        drop(hub_shutdown);
        hub_task.await.unwrap().unwrap();
    });
}

#[test]
fn down_satellite_fails_fast_with_typed_error() {
    common::run_local(async {
        let tmp = TempDir::new().unwrap();
        // A registry entry pointing at a port nothing listens on: the
        // link supervisor dials and backs off forever.
        let dead_port = free_port();
        let (hub_shutdown, hub_task) = spawn_hub(
            tmp.path().join("hub.sock"),
            vec![satellite_entry("sat", dead_port)],
        );
        let mut hub = wait_for_socket(&tmp.path().join("hub.sock"), STEP_DEADLINE).await;

        let started = Instant::now();
        let result = get_screen_via_hub(&mut hub, 1, TerminalId::satellite("sat", 1)).await;
        assert!(
            matches!(
                result,
                CommandResult::Error {
                    code: ErrorCode::SatelliteUnreachable,
                    ..
                }
            ),
            "unreachable satellite must produce a typed error, got {result:?}"
        );
        assert!(
            started.elapsed() < STEP_DEADLINE,
            "the error must be fail-fast, not a hang"
        );

        drop(hub_shutdown);
        hub_task.await.unwrap().unwrap();
    });
}

#[test]
fn unknown_satellite_host_is_unsupported_route() {
    common::run_local(async {
        let tmp = TempDir::new().unwrap();
        // Hub with a registry for "sat" only: host "nowhere" has no route.
        let (hub_shutdown, hub_task) = spawn_hub(
            tmp.path().join("hub.sock"),
            vec![satellite_entry("sat", free_port())],
        );
        let mut hub = wait_for_socket(&tmp.path().join("hub.sock"), STEP_DEADLINE).await;
        let result = get_screen_via_hub(&mut hub, 1, TerminalId::satellite("nowhere", 1)).await;
        assert!(
            matches!(
                result,
                CommandResult::Error {
                    code: ErrorCode::UnsupportedSatelliteRoute,
                    ..
                }
            ),
            "unknown host must be UnsupportedSatelliteRoute, got {result:?}"
        );
        drop(hub_shutdown);
        hub_task.await.unwrap().unwrap();

        // And a plain non-hub server refuses every satellite id the same
        // way — the pre-relay contract is unchanged off-hub.
        let (shutdown, task) = common::spawn_server(tmp.path().join("plain.sock"), Some("s"));
        let mut plain = wait_for_socket(&tmp.path().join("plain.sock"), STEP_DEADLINE).await;
        let result = get_screen_via_hub(&mut plain, 2, TerminalId::satellite("sat", 1)).await;
        assert!(
            matches!(
                result,
                CommandResult::Error {
                    code: ErrorCode::UnsupportedSatelliteRoute,
                    ..
                }
            ),
            "non-hub server must refuse satellite routes, got {result:?}"
        );
        drop(shutdown);
        task.await.unwrap().unwrap();
    });
}
