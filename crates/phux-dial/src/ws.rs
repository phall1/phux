//! Native WebSocket dial transport.
//!
//! This is the TCP fallback sibling to QUIC: one binary WebSocket message
//! carries one complete length-prefixed phux frame, matching the server's
//! `WsListener` and the browser client. This module owns establishment only
//! (TCP connect, optional TLS with the shared trust policy, RFC 6455 upgrade
//! with the `Authorization: Bearer` pairing token); message framing stays
//! with the callers via [`WsReader`] / [`WsWriter`].

use std::net::IpAddr;
use std::sync::Arc;

use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::Uri;
use tokio_tungstenite::tungstenite::{Error as TungsteniteError, Message};
use tokio_tungstenite::{WebSocketStream, client_async};

use crate::DialError;
use crate::tls::CertTrust;

/// A native WebSocket remote dial target.
#[derive(Debug, Clone)]
pub struct WsDial {
    /// `ws://` or `wss://` URL for a `phux server --listen` endpoint.
    pub url: String,
    /// Optional hex pairing token, sent as `Authorization: Bearer`.
    pub token: Option<String>,
    /// TLS trust mode. Only used for `wss://`.
    pub trust: CertTrust,
    /// Optional TLS server name override for SNI/certificate verification.
    pub tls_server_name: Option<String>,
}

/// The established WebSocket stream type [`dial`] returns.
pub type Ws = WebSocketStream<ClientStream>;

/// The plain-or-TLS TCP stream underneath the WebSocket.
#[derive(Debug)]
pub enum ClientStream {
    /// Plaintext TCP (`ws://`, loopback dev only).
    Plain(TcpStream),
    /// TLS over TCP (`wss://`), trust per [`CertTrust`].
    Tls(Box<tokio_rustls::client::TlsStream<TcpStream>>),
}

impl tokio::io::AsyncRead for ClientStream {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        match self.get_mut() {
            Self::Plain(s) => std::pin::Pin::new(s).poll_read(cx, buf),
            Self::Tls(s) => std::pin::Pin::new(s.as_mut()).poll_read(cx, buf),
        }
    }
}

impl tokio::io::AsyncWrite for ClientStream {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        match self.get_mut() {
            Self::Plain(s) => std::pin::Pin::new(s).poll_write(cx, buf),
            Self::Tls(s) => std::pin::Pin::new(s.as_mut()).poll_write(cx, buf),
        }
    }

    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        match self.get_mut() {
            Self::Plain(s) => std::pin::Pin::new(s).poll_flush(cx),
            Self::Tls(s) => std::pin::Pin::new(s.as_mut()).poll_flush(cx),
        }
    }

    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        match self.get_mut() {
            Self::Plain(s) => std::pin::Pin::new(s).poll_shutdown(cx),
            Self::Tls(s) => std::pin::Pin::new(s.as_mut()).poll_shutdown(cx),
        }
    }
}

/// Connect to the WebSocket listener: TCP connect, optional TLS handshake,
/// then the RFC 6455 upgrade with the bearer token attached when present.
///
/// # Errors
///
/// Returns [`DialError::Unreachable`] when the host's name does not resolve
/// or the TCP connect gets no answer (refused, no route, timed out),
/// [`DialError::Connect`] on other connect and TLS/upgrade failures
/// (including a fingerprint that did not match the pin), and
/// [`DialError::Io`] on tungstenite-level socket I/O failures during the
/// upgrade.
pub async fn dial(d: &WsDial) -> Result<Ws, DialError> {
    let target = WsTarget::parse(&d.url)?;
    // Resolve explicitly first: a name that does not resolve is a
    // reachability failure, not a generic connect failure — on an overlay
    // network, MagicDNS being down (Tailscale stopped on this end) fails
    // exactly here. The connect below re-resolves the same (host, port)
    // tuple, which after a successful lookup is a cheap cache hit and keeps
    // one connect path that still tries every resolved address.
    if let Err(err) = tokio::net::lookup_host((target.host.as_str(), target.port)).await {
        return Err(DialError::Unreachable(format!(
            "dial {}: name resolution failed: {err}",
            target.addr_label()
        )));
    }
    let tcp = TcpStream::connect((target.host.as_str(), target.port))
        .await
        .map_err(|err| {
            let msg = format!("dial {}: {err}", target.addr_label());
            if crate::is_reachability_io(&err) {
                DialError::Unreachable(msg)
            } else {
                DialError::Connect(msg)
            }
        })?;
    let stream = if target.secure {
        ClientStream::Tls(Box::new(tls_connect(tcp, &target, d).await?))
    } else {
        ClientStream::Plain(tcp)
    };

    let mut req = d
        .url
        .as_str()
        .into_client_request()
        .map_err(|err| DialError::Connect(format!("build WebSocket request: {err}")))?;
    if let Some(token) = &d.token {
        req.headers_mut().insert(
            "authorization",
            format!("Bearer {}", token.trim())
                .parse()
                .map_err(|err| DialError::Connect(format!("build Authorization header: {err}")))?,
        );
    }

    client_async(req, stream)
        .await
        .map(|(ws, _)| ws)
        .map_err(ws_error)
}

async fn tls_connect(
    tcp: TcpStream,
    target: &WsTarget,
    dial: &WsDial,
) -> Result<tokio_rustls::client::TlsStream<TcpStream>, DialError> {
    let config = Arc::new(crate::tls::client_config(&dial.trust, None)?);
    let connector = tokio_rustls::TlsConnector::from(config);
    let server_name = dial
        .tls_server_name
        .clone()
        .unwrap_or_else(|| target.server_name.clone());
    let server_name = rustls::pki_types::ServerName::try_from(server_name)
        .map_err(|err| DialError::Connect(format!("invalid TLS server name: {err}")))?;
    connector
        .connect(server_name, tcp)
        .await
        .map_err(|err| DialError::Connect(format!("TLS handshake with {}: {err}", target.host)))
}

fn ws_error(err: TungsteniteError) -> DialError {
    match err {
        TungsteniteError::Io(err) => DialError::Io(err),
        other => DialError::Connect(format!("WebSocket handshake: {other}")),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// Parsed WebSocket remote dial endpoint.
pub struct WsTarget {
    /// Whether the URL uses `wss://`.
    pub secure: bool,
    /// TCP destination host from the URL.
    pub host: String,
    /// TCP destination port, including scheme defaults.
    pub port: u16,
    /// Hostname used as the default TLS server name.
    pub server_name: String,
}

impl WsTarget {
    /// Parse a `ws://` or `wss://` dial URL.
    ///
    /// # Errors
    ///
    /// Returns [`DialError::Connect`] for a malformed URL, a missing host, or
    /// a non-WebSocket scheme.
    pub fn parse(raw_url: &str) -> Result<Self, DialError> {
        let parsed: Uri = raw_url
            .parse()
            .map_err(|err| DialError::Connect(format!("invalid WebSocket URL: {err}")))?;
        let scheme = parsed
            .scheme_str()
            .ok_or_else(|| DialError::Connect("WebSocket URL is missing a scheme".to_owned()))?;
        let secure = match scheme {
            "ws" => false,
            "wss" => true,
            _ => {
                return Err(DialError::Connect(
                    "WebSocket URL must start with ws:// or wss://".to_owned(),
                ));
            }
        };
        let host = parsed
            .host()
            .ok_or_else(|| DialError::Connect("WebSocket URL is missing a host".to_owned()))?
            .to_owned();
        let port = parsed.port_u16().unwrap_or(if secure { 443 } else { 80 });
        Ok(Self {
            secure,
            server_name: host.trim_matches(['[', ']']).to_owned(),
            host,
            port,
        })
    }

    /// Whether the URL host is loopback-only.
    #[must_use]
    pub fn is_loopback(&self) -> bool {
        let host = self.server_name.as_str();
        host.eq_ignore_ascii_case("localhost")
            || host.parse::<IpAddr>().is_ok_and(|addr| addr.is_loopback())
    }

    fn addr_label(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }
}

/// Read half of an established WebSocket: one binary message per phux frame.
#[derive(Debug)]
pub struct WsReader {
    /// The message stream half from [`futures_util::StreamExt::split`].
    pub rx: futures_util::stream::SplitStream<Ws>,
}

/// Write half of an established WebSocket.
#[derive(Debug)]
pub struct WsWriter {
    /// The message sink half from [`futures_util::StreamExt::split`].
    pub tx: futures_util::stream::SplitSink<Ws, Message>,
}

impl WsWriter {
    /// Send one already-encoded phux frame as a single binary message.
    ///
    /// # Errors
    ///
    /// Propagates transport failures as [`DialError`].
    pub async fn send(&mut self, frame: &[u8]) -> Result<(), DialError> {
        self.tx
            .send(Message::Binary(frame.to_vec()))
            .await
            .map_err(ws_error)
    }
}

impl WsReader {
    /// Receive the next binary message, skipping control frames.
    ///
    /// Returns `Ok(None)` on a clean close.
    ///
    /// # Errors
    ///
    /// Propagates transport failures as [`DialError`].
    pub async fn recv_message(&mut self) -> Result<Option<Vec<u8>>, DialError> {
        loop {
            match self.rx.next().await {
                None | Some(Ok(Message::Close(_))) => return Ok(None),
                Some(Ok(Message::Binary(data))) => return Ok(Some(data)),
                Some(Err(err)) => return Err(ws_error(err)),
                Some(Ok(_)) => {}
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, reason = "tests")]
mod tests {
    use super::*;

    #[test]
    fn parses_ws_and_wss_targets() {
        let ws = WsTarget::parse("ws://127.0.0.1:8787/path").expect("ws");
        assert_eq!(ws.host, "127.0.0.1");
        assert_eq!(ws.port, 8787);
        assert!(!ws.secure);
        assert!(ws.is_loopback());

        let wss = WsTarget::parse("wss://example.com/phux").expect("wss");
        assert_eq!(wss.host, "example.com");
        assert_eq!(wss.port, 443);
        assert!(wss.secure);
        assert!(!wss.is_loopback());
    }

    #[test]
    fn rejects_non_websocket_scheme() {
        assert!(WsTarget::parse("https://example.com/").is_err());
    }

    #[tokio::test]
    async fn refused_tcp_connect_classifies_unreachable() {
        // Bind then drop a listener so the port is known-refusing.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("local addr").port();
        drop(listener);

        let err = dial(&WsDial {
            url: format!("ws://127.0.0.1:{port}"),
            token: None,
            trust: CertTrust::SkipVerify,
            tls_server_name: None,
        })
        .await
        .expect_err("nothing is listening");
        assert!(matches!(err, DialError::Unreachable(_)), "got {err:?}");
        assert!(
            err.to_string()
                .starts_with("transport connect error: dial 127.0.0.1:"),
            "got {err}"
        );
    }

    /// A hostname that does not resolve classifies as `Unreachable` — the
    /// `MagicDNS`-down shape of an overlay outage. `.invalid` is reserved by
    /// RFC 2606 and guaranteed never to resolve.
    #[tokio::test]
    async fn unresolvable_hostname_classifies_unreachable() {
        let err = dial(&WsDial {
            url: "ws://phux-test-nxdomain.invalid:8787".to_owned(),
            token: None,
            trust: CertTrust::SkipVerify,
            tls_server_name: None,
        })
        .await
        .expect_err(".invalid never resolves");
        assert!(matches!(err, DialError::Unreachable(_)), "got {err:?}");
        assert!(
            err.to_string().contains("name resolution failed"),
            "got {err}"
        );
    }
}
