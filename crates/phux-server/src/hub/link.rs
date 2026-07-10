//! Hub outbound link supervisor (phux-v45.3, ADR-0007/ADR-0038).
//!
//! Under `phux server --hub`, each enabled [`HubEntry`] gets one link
//! supervisor task (`run_link`, crate-internal) that dials the satellite through the
//! shared `phux-dial` stack and authenticates **exactly like a remote
//! consumer** (ADR-0038): TLS 1.3 with the satellite's leaf certificate
//! pinned by SHA-256 fingerprint, plus the pairing bearer token — a
//! length-prefixed stream preamble on QUIC, an `Authorization: Bearer`
//! header on WebSocket. The token is re-read from its file on every
//! attempt, so rotating it never needs a hub restart.
//!
//! **Fail closed.** [`plan_link`] refuses to dial a routable endpoint
//! whose entry lacks a token file or fingerprint pin (and refuses
//! plaintext `ws://` to routable hosts outright), mirroring
//! `phux attach --quic/--ws`. Loopback endpoints keep the loopback dev
//! carve-out. A refused link is never dialed and never retried — the
//! refusal is a configuration error, surfaced as
//! [`LinkStatus::Refused`] and fixed by `phux satellite add`.
//!
//! A lost or failed link is re-dialed with capped exponential backoff;
//! per-satellite state is published to [`HubLinkStatuses`], the shared
//! handle a future `LIST` aggregation (phux-v45.4+) reads. No frames are
//! routed over the link yet — the supervisor holds the authenticated
//! connection open and watches for it to drop.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use phux_dial::{CertTrust, QuicDial, WsDial, WsTarget};
use phux_protocol::ids::SatelliteHost;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use super::{HubEntry, HubTable, SatelliteTarget};

/// First redial delay after a failure or a lost connection.
const BACKOFF_BASE: Duration = Duration::from_millis(500);

/// Ceiling for the exponential redial delay.
const BACKOFF_CAP: Duration = Duration::from_secs(30);

/// What the planner decided to dial for one satellite, auth material
/// resolved into the shared `phux-dial` vocabulary.
///
/// The pairing token stays a *path* here — the supervisor re-reads the
/// file on every attempt (ADR-0038 rotation: update the file, reconnect).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DialSpec {
    /// Dial a QUIC listener (`quic://host:port`).
    Quic {
        /// Hostname, IPv4, or bracketed IPv6 literal to resolve.
        host: String,
        /// UDP port.
        port: u16,
        /// Certificate trust: pinned for routable, skip for loopback dev.
        trust: CertTrust,
        /// Pairing-token file, read per attempt.
        token_file: Option<PathBuf>,
    },
    /// Dial a WebSocket listener (`ws://` loopback dev or `wss://`).
    Ws {
        /// The full endpoint URL as configured.
        url: String,
        /// Certificate trust (`wss://` only).
        trust: CertTrust,
        /// Pairing-token file, read per attempt.
        token_file: Option<PathBuf>,
    },
}

impl DialSpec {
    /// The token file this spec dials with, if any.
    #[must_use]
    pub fn token_file(&self) -> Option<&Path> {
        match self {
            Self::Quic { token_file, .. } | Self::Ws { token_file, .. } => token_file.as_deref(),
        }
    }
}

impl core::fmt::Display for DialSpec {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Quic { host, port, .. } => write!(f, "quic://{host}:{port}"),
            Self::Ws { url, .. } => f.write_str(url),
        }
    }
}

/// Why the planner refused to dial a satellite (fail closed, ADR-0038).
///
/// A refusal is a *configuration* error: the supervisor publishes it as
/// [`LinkStatus::Refused`] and never dials — no retry loop can fix a
/// missing credential.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkRefusal {
    /// `ssh://` endpoints are registry-valid but have no dialer yet
    /// (ADR-0007 "still deferred"; ADR-0038 defers SSH-derived identity).
    SshUnsupported {
        /// The configured endpoint.
        endpoint: String,
    },
    /// Plaintext `ws://` to a routable host: no TLS means no fingerprint
    /// pin is even possible. Mirrors the attach CLI's refusal.
    PlaintextRoutable {
        /// The configured endpoint URL.
        url: String,
    },
    /// Routable endpoint with no `token-file` on the registry entry.
    MissingToken {
        /// The configured endpoint.
        endpoint: String,
    },
    /// Routable endpoint with no `cert-fingerprint` pin on the entry.
    MissingFingerprint {
        /// The configured endpoint.
        endpoint: String,
    },
    /// The endpoint survived hub-table validation but not the dialer's
    /// stricter URL parse.
    Malformed {
        /// The configured endpoint.
        endpoint: String,
        /// Why it did not parse.
        reason: String,
    },
}

impl core::fmt::Display for LinkRefusal {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::SshUnsupported { endpoint } => write!(
                f,
                "{endpoint}: ssh transport is not supported yet (ADR-0007 defers the ssh dialer); \
                 use quic:// or wss:// with `phux pair` credentials"
            ),
            Self::PlaintextRoutable { url } => write!(
                f,
                "{url}: refusing plaintext ws:// to a routable host; use wss:// with `phux pair` \
                 credentials"
            ),
            Self::MissingToken { endpoint } => write!(
                f,
                "{endpoint}: refusing to dial a routable satellite without a token-file; run \
                 `phux pair` on the satellite host and register the token file with \
                 `phux satellite add`"
            ),
            Self::MissingFingerprint { endpoint } => write!(
                f,
                "{endpoint}: refusing to dial a routable satellite without a cert-fingerprint \
                 pin; run `phux pair` on the satellite host and register the printed fingerprint"
            ),
            Self::Malformed { endpoint, reason } => {
                write!(f, "{endpoint}: malformed endpoint: {reason}")
            }
        }
    }
}

/// Per-satellite connection state, published by the link supervisor.
///
/// Held behind [`HubLinkStatuses`] so a future `LIST` aggregation
/// (phux-v45.4+) can report every satellite's reachability.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkStatus {
    /// Fail-closed refusal ([`LinkRefusal`]); the link was never dialed
    /// and will not be retried until the registry entry changes.
    Refused {
        /// Human-readable refusal, from [`LinkRefusal`]'s `Display`.
        reason: String,
    },
    /// A dial attempt is in flight.
    Connecting {
        /// 1-based attempt number since the last successful connection.
        attempt: u32,
    },
    /// The link is established and authenticated.
    Connected,
    /// The last attempt failed (or an established link dropped); the
    /// supervisor redials after `retry_in`.
    Backoff {
        /// 1-based number of the attempt that just failed.
        attempt: u32,
        /// Delay before the next dial.
        retry_in: Duration,
        /// Why the attempt failed or the connection dropped.
        last_error: String,
    },
}

impl core::fmt::Display for LinkStatus {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Refused { reason } => write!(f, "refused (fail closed): {reason}"),
            Self::Connecting { attempt } => write!(f, "connecting (attempt {attempt})"),
            Self::Connected => f.write_str("connected"),
            Self::Backoff {
                attempt,
                retry_in,
                last_error,
            } => write!(
                f,
                "backoff {}ms after attempt {attempt}: {last_error}",
                retry_in.as_millis()
            ),
        }
    }
}

/// Shared, cheaply-cloneable map of per-satellite [`LinkStatus`].
///
/// One handle lives in server shared state (set at hub startup); each
/// link supervisor holds a clone and publishes its transitions. The
/// `std::sync::Mutex` is held only for map reads/writes — never across
/// an await point.
#[derive(Debug, Clone, Default)]
pub struct HubLinkStatuses {
    inner: Arc<Mutex<BTreeMap<SatelliteHost, LinkStatus>>>,
}

impl HubLinkStatuses {
    /// Publish `status` for `host`.
    pub fn set(&self, host: &SatelliteHost, status: LinkStatus) {
        self.lock().insert(host.clone(), status);
    }

    /// The current status for `host`, if its supervisor has reported yet.
    #[must_use]
    pub fn get(&self, host: &SatelliteHost) -> Option<LinkStatus> {
        self.lock().get(host).cloned()
    }

    /// Snapshot every satellite's status, in deterministic name order.
    #[must_use]
    pub fn snapshot(&self) -> BTreeMap<SatelliteHost, LinkStatus> {
        self.lock().clone()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, BTreeMap<SatelliteHost, LinkStatus>> {
        // A poisoned map only means another supervisor panicked mid-insert;
        // the map itself (Clone + insert) cannot be left inconsistent.
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// Capped exponential backoff: `base * 2^failures`, saturating at `cap`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Backoff {
    base: Duration,
    cap: Duration,
    failures: u32,
}

impl Backoff {
    /// A fresh backoff starting at `base` and never exceeding `cap`.
    #[must_use]
    pub const fn new(base: Duration, cap: Duration) -> Self {
        Self {
            base,
            cap,
            failures: 0,
        }
    }

    /// Consecutive failures recorded since the last [`Self::reset`].
    #[must_use]
    pub const fn failures(&self) -> u32 {
        self.failures
    }

    /// Record a failure and return the delay before the next attempt.
    pub fn next_delay(&mut self) -> Duration {
        // 2^32 * base overflows Duration long before `failures` wraps;
        // clamp the exponent so the shift itself cannot overflow.
        let exponent = self.failures.min(20);
        let delay = self
            .base
            .saturating_mul(2u32.saturating_pow(exponent))
            .min(self.cap);
        self.failures = self.failures.saturating_add(1);
        delay
    }

    /// Forget the failure streak after a successful connection.
    pub const fn reset(&mut self) {
        self.failures = 0;
    }
}

/// Decide how (or whether) to dial one hub-table entry. Pure — no I/O.
///
/// This is the fail-closed gate (ADR-0038): routable endpoints without
/// both a token file and a fingerprint pin are refused, plaintext `ws://`
/// is loopback-only, and `ssh://` is a clear "not supported yet".
/// Loopback endpoints keep the dev carve-out (skip-verify TLS, optional
/// token), matching `phux attach --quic/--ws`.
///
/// # Errors
///
/// A [`LinkRefusal`] naming the configuration gap.
pub fn plan_link(entry: &HubEntry) -> Result<DialSpec, LinkRefusal> {
    match &entry.target {
        SatelliteTarget::SshDeferred { endpoint } => Err(LinkRefusal::SshUnsupported {
            endpoint: endpoint.clone(),
        }),
        SatelliteTarget::Quic { host, port } => {
            let trust = plan_trust(host_is_loopback(host), host, entry)?;
            Ok(DialSpec::Quic {
                host: host.clone(),
                port: *port,
                trust,
                token_file: entry.token_file.clone(),
            })
        }
        SatelliteTarget::Ws { url } => {
            let target = parse_ws(url)?;
            if !target.is_loopback() {
                return Err(LinkRefusal::PlaintextRoutable { url: url.clone() });
            }
            Ok(DialSpec::Ws {
                url: url.clone(),
                // ws:// never performs a TLS handshake; the trust value is
                // inert but kept honest (a pin would never be checked).
                trust: CertTrust::SkipVerify,
                token_file: entry.token_file.clone(),
            })
        }
        SatelliteTarget::Wss { url } => {
            let target = parse_ws(url)?;
            let trust = plan_trust(target.is_loopback(), url, entry)?;
            Ok(DialSpec::Ws {
                url: url.clone(),
                trust,
                token_file: entry.token_file.clone(),
            })
        }
    }
}

/// The shared routable-vs-loopback trust rule for TLS transports
/// (QUIC and `wss://`): routable requires pin **and** token; loopback
/// pins when a fingerprint is configured and skips verification
/// otherwise.
fn plan_trust(loopback: bool, endpoint: &str, entry: &HubEntry) -> Result<CertTrust, LinkRefusal> {
    if loopback {
        return Ok(entry
            .cert_fingerprint
            .clone()
            .map_or(CertTrust::SkipVerify, CertTrust::Pinned));
    }
    let Some(fingerprint) = entry.cert_fingerprint.clone() else {
        return Err(LinkRefusal::MissingFingerprint {
            endpoint: endpoint.to_owned(),
        });
    };
    if entry.token_file.is_none() {
        return Err(LinkRefusal::MissingToken {
            endpoint: endpoint.to_owned(),
        });
    }
    Ok(CertTrust::Pinned(fingerprint))
}

fn parse_ws(url: &str) -> Result<WsTarget, LinkRefusal> {
    WsTarget::parse(url).map_err(|err| LinkRefusal::Malformed {
        endpoint: url.to_owned(),
        reason: err.to_string(),
    })
}

/// Whether a `quic://` host token names the loopback interface —
/// `localhost`, a loopback IPv4, or a (possibly bracketed) loopback IPv6.
fn host_is_loopback(host: &str) -> bool {
    let bare = host.trim_matches(['[', ']']);
    bare.eq_ignore_ascii_case("localhost")
        || bare
            .parse::<std::net::IpAddr>()
            .is_ok_and(|addr| addr.is_loopback())
}

/// Read and validate the pairing token for an attempt.
///
/// One hex token, line-oriented (ADR-0038: the same shape as the server's
/// token store — the first non-empty line wins, trailing newline
/// tolerated). Returns the *hex* string; the QUIC path decodes it into
/// raw preamble bytes, the WebSocket path sends it verbatim in the
/// `Authorization` header, matching the attach CLI.
///
/// # Errors
///
/// A human-readable reason (missing/unreadable file, empty file, or a
/// token that is not valid hex). The supervisor treats this like a failed
/// attempt — fixed token files are picked up on the next redial.
fn read_link_token(path: Option<&Path>) -> Result<Option<String>, String> {
    let Some(path) = path else {
        return Ok(None);
    };
    let raw = std::fs::read_to_string(path)
        .map_err(|err| format!("read token file {}: {err}", path.display()))?;
    let Some(token) = raw.lines().map(str::trim).find(|line| !line.is_empty()) else {
        return Err(format!("token file {} is empty", path.display()));
    };
    if hex::decode(token).is_err() {
        return Err(format!(
            "token file {} does not hold a valid hex pairing token",
            path.display()
        ));
    }
    Ok(Some(token.to_owned()))
}

/// The transport seam the supervisor dials through.
///
/// Production is [`NetLinkTransport`] (the shared `phux-dial` stack);
/// tests inject a scripted transport so backoff and status transitions
/// are exercised without any network. Errors are plain strings — they
/// only ever land in logs and [`LinkStatus::Backoff::last_error`].
pub(crate) trait LinkTransport {
    /// The established-connection handle.
    type Conn: LinkConn;

    /// Dial `spec`, authenticating with `token` (validated hex) when
    /// present.
    async fn connect(&self, spec: &DialSpec, token: Option<String>) -> Result<Self::Conn, String>;
}

/// An established hub link: the only thing the supervisor can do with it
/// (until phux-v45.4 routes frames) is wait for it to drop.
pub(crate) trait LinkConn {
    /// Resolve when the connection is gone, with a human-readable reason.
    async fn closed(self) -> String;
}

/// Supervise one satellite link: plan, dial, hold, redial.
///
/// Runs until `cancel` fires. Fail-closed refusals publish
/// [`LinkStatus::Refused`] and return immediately — no dial, no retry.
#[allow(
    clippy::future_not_send,
    reason = "ADR-0014: hub link supervisors run on the server's LocalSet; the transport seam is generic so tests can inject !Send scripted transports"
)]
pub(crate) async fn run_link<T: LinkTransport>(
    host: SatelliteHost,
    entry: HubEntry,
    transport: T,
    statuses: HubLinkStatuses,
    cancel: CancellationToken,
) {
    let spec = match plan_link(&entry) {
        Ok(spec) => spec,
        Err(refusal) => {
            warn!(
                satellite = %host,
                refusal = %refusal,
                "hub link refused (fail closed); not dialing"
            );
            statuses.set(
                &host,
                LinkStatus::Refused {
                    reason: refusal.to_string(),
                },
            );
            return;
        }
    };

    let mut backoff = Backoff::new(BACKOFF_BASE, BACKOFF_CAP);
    loop {
        let attempt = backoff.failures().saturating_add(1);
        statuses.set(&host, LinkStatus::Connecting { attempt });
        let connect = async {
            // Re-read the token every attempt (ADR-0038 rotation: update
            // the hub's token file, reconnect). The file is a one-line
            // local read; blocking the current-thread runtime for it is
            // deliberate simplicity, matching the config-load paths.
            let token = read_link_token(spec.token_file())?;
            transport.connect(&spec, token).await
        };
        let outcome = tokio::select! {
            () = cancel.cancelled() => return,
            outcome = connect => outcome,
        };

        let (failed_attempt, last_error) = match outcome {
            Ok(conn) => {
                info!(satellite = %host, target = %spec, "hub link established");
                backoff.reset();
                statuses.set(&host, LinkStatus::Connected);
                let reason = tokio::select! {
                    () = cancel.cancelled() => return,
                    reason = conn.closed() => reason,
                };
                warn!(
                    satellite = %host,
                    reason = %reason,
                    "hub link lost; scheduling redial"
                );
                (1, reason)
            }
            Err(error) => {
                warn!(
                    satellite = %host,
                    target = %spec,
                    attempt,
                    error = %error,
                    "hub link attempt failed"
                );
                (attempt, error)
            }
        };

        let retry_in = backoff.next_delay();
        statuses.set(
            &host,
            LinkStatus::Backoff {
                attempt: failed_attempt,
                retry_in,
                last_error,
            },
        );
        tokio::select! {
            () = cancel.cancelled() => return,
            () = tokio::time::sleep(retry_in) => {}
        }
    }
}

/// Spawn one [`run_link`] supervisor per hub-table entry onto the current
/// `LocalSet`, all children of `cancel`.
///
/// Called from the server runtime's hub bring-up; `statuses` is the same
/// handle mirrored into shared state for future `LIST` aggregation.
pub(crate) fn spawn_links(
    table: &HubTable,
    statuses: &HubLinkStatuses,
    cancel: &CancellationToken,
) {
    for (host, entry) in table.iter() {
        tokio::task::spawn_local(run_link(
            host.clone(),
            entry.clone(),
            NetLinkTransport,
            statuses.clone(),
            cancel.child_token(),
        ));
    }
}

/// The production [`LinkTransport`]: the shared `phux-dial` QUIC/WS stack,
/// authenticating exactly like a remote consumer (ADR-0038).
#[derive(Debug, Clone, Copy)]
pub(crate) struct NetLinkTransport;

/// An established production link, held open until the peer (or the
/// network) drops it.
#[derive(Debug)]
pub(crate) enum NetLinkConn {
    /// QUIC connection with its endpoint driver and the opened bidi
    /// stream halves — kept alive so the satellite sees one consumer,
    /// ready for phux-v45.4 to frame over.
    Quic {
        /// Owns the UDP socket + I/O driver; must outlive the connection.
        _endpoint: quinn::Endpoint,
        /// The established connection, watched for closure.
        connection: quinn::Connection,
        /// Opened bidi send half (auth preamble already written).
        _send: quinn::SendStream,
        /// Opened bidi recv half.
        _recv: quinn::RecvStream,
    },
    /// WebSocket connection, drained (and ping-answered) until it closes.
    Ws(Box<phux_dial::ws::Ws>),
}

impl LinkTransport for NetLinkTransport {
    type Conn = NetLinkConn;

    async fn connect(&self, spec: &DialSpec, token: Option<String>) -> Result<NetLinkConn, String> {
        match spec {
            DialSpec::Quic {
                host, port, trust, ..
            } => {
                let bare = host.trim_matches(['[', ']']);
                let addr = tokio::net::lookup_host((bare, *port))
                    .await
                    .map_err(|err| format!("resolve {host}:{port}: {err}"))?
                    .next()
                    .ok_or_else(|| format!("resolve {host}:{port}: no addresses"))?;
                let token = token
                    .as_deref()
                    .map(phux_dial::quic::parse_token_hex)
                    .transpose()
                    .map_err(|err| err.to_string())?;
                let dial = QuicDial {
                    addr,
                    server_name: bare.to_owned(),
                    token,
                    trust: trust.clone(),
                };
                let (endpoint, connection, send, recv) = phux_dial::quic::dial(&dial)
                    .await
                    .map_err(|err| err.to_string())?;
                Ok(NetLinkConn::Quic {
                    _endpoint: endpoint,
                    connection,
                    _send: send,
                    _recv: recv,
                })
            }
            DialSpec::Ws { url, trust, .. } => {
                let dial = WsDial {
                    url: url.clone(),
                    token,
                    trust: trust.clone(),
                    tls_server_name: None,
                };
                let ws = phux_dial::ws::dial(&dial)
                    .await
                    .map_err(|err| err.to_string())?;
                Ok(NetLinkConn::Ws(Box::new(ws)))
            }
        }
    }
}

impl LinkConn for NetLinkConn {
    async fn closed(self) -> String {
        match self {
            Self::Quic { connection, .. } => {
                let reason = connection.closed().await;
                reason.to_string()
            }
            Self::Ws(mut ws) => {
                // Drain the stream: reading is what answers pings and
                // observes the close. Frames are ignored until phux-v45.4
                // routes them.
                loop {
                    match futures_util::StreamExt::next(ws.as_mut()).await {
                        None => return "connection closed by satellite".to_owned(),
                        Some(Ok(_)) => {}
                        Some(Err(err)) => return format!("connection error: {err}"),
                    }
                }
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, reason = "tests")]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::collections::VecDeque;
    use std::rc::Rc;

    use super::*;

    fn entry(endpoint: &str, token_file: Option<&str>, cert_fingerprint: Option<&str>) -> HubEntry {
        HubEntry {
            target: super::super::parse_endpoint(endpoint).expect("valid endpoint"),
            token_file: token_file.map(PathBuf::from),
            cert_fingerprint: cert_fingerprint.map(str::to_owned),
        }
    }

    // --- dial-target selection ----------------------------------------

    #[test]
    fn quic_plan_keeps_host_port_and_pins_routable() {
        let spec = plan_link(&entry(
            "quic://devbox:8788",
            Some("/secrets/devbox.token"),
            Some("AB:CD"),
        ))
        .expect("dialable");
        assert_eq!(
            spec,
            DialSpec::Quic {
                host: "devbox".to_owned(),
                port: 8788,
                trust: CertTrust::Pinned("AB:CD".to_owned()),
                token_file: Some(PathBuf::from("/secrets/devbox.token")),
            }
        );
    }

    #[test]
    fn wss_plan_keeps_url_and_pins_routable() {
        let spec = plan_link(&entry(
            "wss://sandbox:8787/phux",
            Some("/secrets/sandbox.token"),
            Some("ab:cd"),
        ))
        .expect("dialable");
        assert_eq!(
            spec,
            DialSpec::Ws {
                url: "wss://sandbox:8787/phux".to_owned(),
                trust: CertTrust::Pinned("ab:cd".to_owned()),
                token_file: Some(PathBuf::from("/secrets/sandbox.token")),
            }
        );
    }

    #[test]
    fn loopback_plans_skip_verify_without_a_pin() {
        for endpoint in [
            "quic://127.0.0.1:8788",
            "quic://[::1]:8788",
            "quic://localhost:8788",
            "ws://127.0.0.1:8787",
            "wss://localhost:8787",
        ] {
            let spec = plan_link(&entry(endpoint, None, None)).expect("loopback carve-out");
            let trust = match spec {
                DialSpec::Quic { trust, .. } | DialSpec::Ws { trust, .. } => trust,
            };
            assert_eq!(trust, CertTrust::SkipVerify, "{endpoint}");
        }
    }

    #[test]
    fn loopback_with_a_pin_still_pins() {
        let spec = plan_link(&entry("wss://127.0.0.1:8787", None, Some("AB"))).expect("dialable");
        assert!(matches!(spec, DialSpec::Ws { trust: CertTrust::Pinned(pin), .. } if pin == "AB"));
    }

    // --- fail-closed matrix ---------------------------------------------

    #[test]
    fn routable_endpoints_fail_closed_without_token_or_pin() {
        // (endpoint, token, pin) -> expected refusal
        let matrix: &[(&str, Option<&str>, Option<&str>)] = &[
            ("quic://devbox:8788", None, None),
            ("quic://devbox:8788", Some("/t"), None),
            ("quic://devbox:8788", None, Some("AB")),
            ("wss://devbox:8787", None, None),
            ("wss://devbox:8787", Some("/t"), None),
            ("wss://devbox:8787", None, Some("AB")),
        ];
        for (endpoint, token, pin) in matrix {
            let refusal = plan_link(&entry(endpoint, *token, *pin)).expect_err("must fail closed");
            match (token, pin) {
                (_, None) => assert!(
                    matches!(refusal, LinkRefusal::MissingFingerprint { .. }),
                    "{endpoint} token={token:?} pin={pin:?}: {refusal:?}"
                ),
                (None, Some(_)) => assert!(
                    matches!(refusal, LinkRefusal::MissingToken { .. }),
                    "{endpoint} token={token:?} pin={pin:?}: {refusal:?}"
                ),
                (Some(_), Some(_)) => unreachable!("dialable rows are not in the matrix"),
            }
        }
    }

    #[test]
    fn plaintext_ws_to_routable_host_is_refused_even_with_credentials() {
        let refusal = plan_link(&entry("ws://devbox:8787", Some("/t"), Some("AB")))
            .expect_err("plaintext routable");
        assert!(matches!(refusal, LinkRefusal::PlaintextRoutable { .. }));
    }

    #[test]
    fn ssh_is_a_clear_unsupported_refusal() {
        let refusal =
            plan_link(&entry("ssh://devbox", Some("/t"), Some("AB"))).expect_err("ssh deferred");
        assert!(matches!(refusal, LinkRefusal::SshUnsupported { .. }));
        assert!(
            refusal.to_string().contains("not supported yet"),
            "{refusal}"
        );
    }

    #[test]
    fn quic_loopback_literals_are_recognized() {
        assert!(host_is_loopback("127.0.0.1"));
        assert!(host_is_loopback("[::1]"));
        assert!(host_is_loopback("LOCALHOST"));
        assert!(!host_is_loopback("devbox"));
        assert!(!host_is_loopback("10.0.0.7"));
        assert!(!host_is_loopback("[2001:db8::1]"));
    }

    // --- token file -----------------------------------------------------

    #[test]
    fn token_file_first_nonempty_line_wins_and_hex_is_enforced() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("satellite.token");

        std::fs::write(&path, "deadbeef\n").expect("write");
        assert_eq!(
            read_link_token(Some(&path)).expect("valid"),
            Some("deadbeef".to_owned())
        );

        std::fs::write(&path, "\n  \ndeadbeef\nc0ffee\n").expect("write");
        assert_eq!(
            read_link_token(Some(&path)).expect("line-oriented"),
            Some("deadbeef".to_owned())
        );

        std::fs::write(&path, "not hex!\n").expect("write");
        let err = read_link_token(Some(&path)).expect_err("not hex");
        assert!(err.contains("valid hex"), "{err}");

        std::fs::write(&path, "\n\n").expect("write");
        let err = read_link_token(Some(&path)).expect_err("empty");
        assert!(err.contains("empty"), "{err}");

        let missing = dir.path().join("nope.token");
        let err = read_link_token(Some(&missing)).expect_err("missing");
        assert!(err.contains("read token file"), "{err}");

        assert_eq!(read_link_token(None).expect("no file configured"), None);
    }

    // --- backoff ----------------------------------------------------------

    #[test]
    fn backoff_doubles_from_base_and_caps() {
        let mut backoff = Backoff::new(Duration::from_millis(500), Duration::from_secs(30));
        let mut delays = Vec::new();
        for _ in 0..9 {
            delays.push(backoff.next_delay());
        }
        assert_eq!(
            delays,
            vec![
                Duration::from_millis(500),
                Duration::from_secs(1),
                Duration::from_secs(2),
                Duration::from_secs(4),
                Duration::from_secs(8),
                Duration::from_secs(16),
                Duration::from_secs(30),
                Duration::from_secs(30),
                Duration::from_secs(30),
            ]
        );
    }

    #[test]
    fn backoff_reset_returns_to_base() {
        let mut backoff = Backoff::new(Duration::from_millis(500), Duration::from_secs(30));
        let _ = backoff.next_delay();
        let _ = backoff.next_delay();
        backoff.reset();
        assert_eq!(backoff.failures(), 0);
        assert_eq!(backoff.next_delay(), Duration::from_millis(500));
    }

    #[test]
    fn backoff_never_overflows_at_large_failure_counts() {
        let mut backoff = Backoff::new(Duration::from_millis(500), Duration::from_secs(30));
        for _ in 0..10_000 {
            assert!(backoff.next_delay() <= Duration::from_secs(30));
        }
    }

    // --- supervisor (scripted transport, no network) ---------------------

    /// Scripted transport: pops the next result per connect call.
    #[derive(Clone)]
    struct ScriptTransport {
        script: Rc<RefCell<VecDeque<Result<ScriptConn, String>>>>,
        calls: Rc<Cell<u32>>,
        seen_tokens: Rc<RefCell<Vec<Option<String>>>>,
    }

    impl ScriptTransport {
        fn new(script: Vec<Result<ScriptConn, String>>) -> Self {
            Self {
                script: Rc::new(RefCell::new(script.into())),
                calls: Rc::new(Cell::new(0)),
                seen_tokens: Rc::new(RefCell::new(Vec::new())),
            }
        }
    }

    /// A scripted connection: closes when the oneshot fires, or never.
    struct ScriptConn {
        closed: Option<tokio::sync::oneshot::Receiver<String>>,
    }

    impl LinkTransport for ScriptTransport {
        type Conn = ScriptConn;

        #[allow(
            clippy::future_not_send,
            reason = "test transport is Rc-scripted and driven on a LocalSet"
        )]
        async fn connect(
            &self,
            _spec: &DialSpec,
            token: Option<String>,
        ) -> Result<ScriptConn, String> {
            self.calls.set(self.calls.get() + 1);
            self.seen_tokens.borrow_mut().push(token);
            self.script
                .borrow_mut()
                .pop_front()
                .unwrap_or_else(|| Err("script exhausted".to_owned()))
        }
    }

    impl LinkConn for ScriptConn {
        async fn closed(self) -> String {
            match self.closed {
                Some(rx) => rx.await.unwrap_or_else(|_| "closed".to_owned()),
                None => std::future::pending().await,
            }
        }
    }

    fn loopback_entry() -> HubEntry {
        entry("ws://127.0.0.1:9", None, None)
    }

    fn host() -> SatelliteHost {
        SatelliteHost::new("devbox")
    }

    /// Poll `statuses` (under paused time) until `want` matches.
    async fn wait_for_status(
        statuses: &HubLinkStatuses,
        host: &SatelliteHost,
        want: impl Fn(&LinkStatus) -> bool,
    ) {
        for _ in 0..10_000 {
            if statuses.get(host).as_ref().is_some_and(&want) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        panic!("status never matched; last = {:?}", statuses.get(host));
    }

    #[tokio::test(start_paused = true)]
    async fn supervisor_backs_off_exponentially_then_connects() {
        let local = tokio::task::LocalSet::new();
        local
            .run_until(async {
                let transport = ScriptTransport::new(vec![
                    Err("boom-1".to_owned()),
                    Err("boom-2".to_owned()),
                    Ok(ScriptConn { closed: None }),
                ]);
                let statuses = HubLinkStatuses::default();
                let cancel = CancellationToken::new();
                let host = host();
                tokio::task::spawn_local(run_link(
                    host.clone(),
                    loopback_entry(),
                    transport.clone(),
                    statuses.clone(),
                    cancel.child_token(),
                ));

                wait_for_status(&statuses, &host, |s| {
                    *s == LinkStatus::Backoff {
                        attempt: 1,
                        retry_in: Duration::from_millis(500),
                        last_error: "boom-1".to_owned(),
                    }
                })
                .await;
                wait_for_status(&statuses, &host, |s| {
                    *s == LinkStatus::Backoff {
                        attempt: 2,
                        retry_in: Duration::from_secs(1),
                        last_error: "boom-2".to_owned(),
                    }
                })
                .await;
                wait_for_status(&statuses, &host, |s| *s == LinkStatus::Connected).await;
                assert_eq!(transport.calls.get(), 3);

                cancel.cancel();
            })
            .await;
    }

    #[tokio::test(start_paused = true)]
    async fn supervisor_redials_after_a_lost_connection_with_reset_backoff() {
        let local = tokio::task::LocalSet::new();
        local
            .run_until(async {
                let (close_tx, close_rx) = tokio::sync::oneshot::channel();
                let transport = ScriptTransport::new(vec![
                    // Fail once so the backoff has a streak to reset.
                    Err("boom-1".to_owned()),
                    Ok(ScriptConn {
                        closed: Some(close_rx),
                    }),
                    Ok(ScriptConn { closed: None }),
                ]);
                let statuses = HubLinkStatuses::default();
                let cancel = CancellationToken::new();
                let host = host();
                tokio::task::spawn_local(run_link(
                    host.clone(),
                    loopback_entry(),
                    transport.clone(),
                    statuses.clone(),
                    cancel.child_token(),
                ));

                wait_for_status(&statuses, &host, |s| *s == LinkStatus::Connected).await;

                // Drop the link: the redial must start from the base delay
                // again (the success reset the failure streak).
                close_tx
                    .send("satellite went away".to_owned())
                    .expect("send");
                wait_for_status(&statuses, &host, |s| {
                    *s == LinkStatus::Backoff {
                        attempt: 1,
                        retry_in: Duration::from_millis(500),
                        last_error: "satellite went away".to_owned(),
                    }
                })
                .await;
                wait_for_status(&statuses, &host, |s| *s == LinkStatus::Connected).await;
                assert_eq!(transport.calls.get(), 3);

                cancel.cancel();
            })
            .await;
    }

    #[tokio::test(start_paused = true)]
    async fn supervisor_fails_closed_without_dialing() {
        let local = tokio::task::LocalSet::new();
        local
            .run_until(async {
                let transport = ScriptTransport::new(vec![Ok(ScriptConn { closed: None })]);
                let statuses = HubLinkStatuses::default();
                let host = host();
                // Routable QUIC endpoint with no auth material: refused.
                run_link(
                    host.clone(),
                    entry("quic://devbox:8788", None, None),
                    transport.clone(),
                    statuses.clone(),
                    CancellationToken::new(),
                )
                .await;
                assert_eq!(transport.calls.get(), 0, "fail closed means no dial");
                assert!(matches!(
                    statuses.get(&host),
                    Some(LinkStatus::Refused { reason }) if reason.contains("cert-fingerprint")
                ));
            })
            .await;
    }

    #[tokio::test(start_paused = true)]
    async fn supervisor_rereads_the_token_file_every_attempt() {
        let local = tokio::task::LocalSet::new();
        local
            .run_until(async {
                let dir = tempfile::tempdir().expect("tempdir");
                let token_path = dir.path().join("sat.token");
                std::fs::write(&token_path, "deadbeef\n").expect("write");

                let (close_tx, close_rx) = tokio::sync::oneshot::channel();
                let transport = ScriptTransport::new(vec![
                    Ok(ScriptConn {
                        closed: Some(close_rx),
                    }),
                    Ok(ScriptConn { closed: None }),
                ]);
                let statuses = HubLinkStatuses::default();
                let cancel = CancellationToken::new();
                let host = host();
                tokio::task::spawn_local(run_link(
                    host.clone(),
                    entry(
                        "wss://devbox:8787",
                        Some(token_path.to_str().expect("utf8 path")),
                        Some("AB:CD"),
                    ),
                    transport.clone(),
                    statuses.clone(),
                    cancel.child_token(),
                ));

                wait_for_status(&statuses, &host, |s| *s == LinkStatus::Connected).await;

                // Rotate the token, drop the link: the redial must present
                // the new token without a hub restart (ADR-0038). Wait for
                // the drop to be observed (Backoff) before waiting for the
                // reconnect, or the first Connected status matches again.
                std::fs::write(&token_path, "c0ffee\n").expect("rotate");
                close_tx.send("rotated".to_owned()).expect("send");
                wait_for_status(&statuses, &host, |s| {
                    matches!(s, LinkStatus::Backoff { last_error, .. } if last_error == "rotated")
                })
                .await;
                wait_for_status(&statuses, &host, |s| *s == LinkStatus::Connected).await;

                assert_eq!(
                    *transport.seen_tokens.borrow(),
                    vec![Some("deadbeef".to_owned()), Some("c0ffee".to_owned())]
                );

                cancel.cancel();
            })
            .await;
    }

    #[tokio::test(start_paused = true)]
    async fn supervisor_stops_on_cancellation() {
        let local = tokio::task::LocalSet::new();
        local
            .run_until(async {
                let transport = ScriptTransport::new(vec![Err("boom".to_owned())]);
                let statuses = HubLinkStatuses::default();
                let cancel = CancellationToken::new();
                let host = host();
                let task = tokio::task::spawn_local(run_link(
                    host.clone(),
                    loopback_entry(),
                    transport,
                    statuses.clone(),
                    cancel.child_token(),
                ));
                wait_for_status(&statuses, &host, |s| {
                    matches!(s, LinkStatus::Backoff { .. })
                })
                .await;
                cancel.cancel();
                tokio::time::timeout(Duration::from_secs(5), task)
                    .await
                    .expect("supervisor exits on cancel")
                    .expect("no panic");
            })
            .await;
    }

    // --- loopback integration: the real transport over a real socket -----

    #[tokio::test]
    async fn net_transport_connects_to_a_loopback_ws_listener_and_notices_the_drop() {
        let local = tokio::task::LocalSet::new();
        local
            .run_until(async {
                let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                    .await
                    .expect("bind loopback");
                let addr = listener.local_addr().expect("addr");
                let (drop_tx, drop_rx) = tokio::sync::oneshot::channel::<()>();
                let server = tokio::task::spawn_local(async move {
                    let (tcp, _) = listener.accept().await.expect("accept");
                    let ws = tokio_tungstenite::accept_async(tcp)
                        .await
                        .expect("ws handshake");
                    // Hold the connection until the test asks us to drop it,
                    // then close listener + connection so the redial fails.
                    let _ = drop_rx.await;
                    drop(ws);
                    drop(listener);
                });

                let statuses = HubLinkStatuses::default();
                let cancel = CancellationToken::new();
                let host = host();
                tokio::task::spawn_local(run_link(
                    host.clone(),
                    entry(&format!("ws://127.0.0.1:{}", addr.port()), None, None),
                    NetLinkTransport,
                    statuses.clone(),
                    cancel.child_token(),
                ));

                wait_for_real_status(&statuses, &host, |s| *s == LinkStatus::Connected).await;

                drop_tx.send(()).expect("server alive");
                wait_for_real_status(&statuses, &host, |s| {
                    matches!(s, LinkStatus::Backoff { .. })
                })
                .await;

                cancel.cancel();
                server.await.expect("server task");
            })
            .await;
    }

    /// Real-time sibling of [`wait_for_status`] for the loopback test.
    async fn wait_for_real_status(
        statuses: &HubLinkStatuses,
        host: &SatelliteHost,
        want: impl Fn(&LinkStatus) -> bool,
    ) {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while std::time::Instant::now() < deadline {
            if statuses.get(host).as_ref().is_some_and(&want) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!("status never matched; last = {:?}", statuses.get(host));
    }
}
