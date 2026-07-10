//! QUIC dialer (`phux-y8v6`, [ADR-0007]) — the outbound counterpart to the
//! server's `QuicListener` (`phux-server::transport::quic`).
//!
//! The dialer opens exactly **one** bidirectional QUIC stream to a
//! `phux server --quic` listener and carries the identical length-prefixed phux
//! frames (`docs/spec/proto.md` §5) the UDS path does; only the byte stream
//! underneath differs. This module owns the QUIC-specific establishment —
//! building the rustls client config (TLS 1.3 + the phux ALPN), verifying the
//! server certificate (a fingerprint **pin** for routable hosts, or a loopback
//! **skip** for local dev), connecting, and writing the optional bearer-token
//! preamble — and hands back the raw quinn stream halves. The framing itself
//! stays with the callers (`phux-client::attach::connection`, the server hub's
//! link supervisor).
//!
//! **Auth.** TLS 1.3 is intrinsic to QUIC, so confidentiality is never
//! optional. For *authentication* of routable consumers the dialer mirrors the
//! server's bearer-token model (ADR-0031): it writes a length-prefixed token
//! (`len: u32 BE` + raw token bytes) as the very first bytes of the stream,
//! ahead of any phux frame. On a loopback listener no preamble is sent and
//! frames start immediately.
//!
//! [ADR-0007]: ../../../ADR/0007-mosh-class-transport-and-satellites.md

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use phux_protocol::policy::QUIC_ALPN;

use crate::DialError;
use crate::tls::CertTrust;

/// QUIC idle timeout, matched to the server's `IDLE_TIMEOUT` so a quiet but
/// attached consumer is not reaped before the keep-alive fires.
const IDLE_TIMEOUT: Duration = Duration::from_secs(30);

/// Keep-alive interval, comfortably under [`IDLE_TIMEOUT`] so a quiet consumer
/// (no keystrokes, no output) holds its connection open across NATs.
const KEEP_ALIVE: Duration = Duration::from_secs(10);

/// Everything the dialer needs to reach a `phux server --quic` listener.
#[derive(Debug, Clone)]
pub struct QuicDial {
    /// The listener's `HOST:PORT`.
    pub addr: SocketAddr,
    /// TLS server name offered in SNI / used for certificate name matching.
    /// The server's self-signed cert carries `localhost` / `127.0.0.1` / `::1`
    /// SANs; a fingerprint pin does not rely on name matching, but a valid
    /// name keeps the handshake conventional.
    pub server_name: String,
    /// Raw bearer-token bytes for the auth preamble, or `None` for an
    /// unauthenticated (loopback) listener. Callers hex-decode the
    /// `phux pair` token into these raw bytes (see [`parse_token_hex`]).
    pub token: Option<Vec<u8>>,
    /// How to trust the server's certificate.
    pub trust: CertTrust,
}

/// Decode a `phux pair` pairing token (hex) into the raw bytes the QUIC auth
/// preamble carries.
///
/// # Errors
///
/// Returns [`DialError::Connect`] when the token is not valid hex.
pub fn parse_token_hex(token: &str) -> Result<Vec<u8>, DialError> {
    hex::decode(token.trim())
        .map_err(|err| DialError::Connect(format!("pairing token is not valid hex: {err}")))
}

/// Connect to the QUIC listener and return the established bidi-stream
/// halves, the auth preamble already written.
///
/// The quinn [`Endpoint`](quinn::Endpoint) and
/// [`Connection`](quinn::Connection) are returned alongside so the caller
/// can keep the endpoint's I/O driver alive for the connection's lifetime and
/// issue a clean `CONNECTION_CLOSE` on teardown (rather than leaving the server
/// to reap an abandoned connection at the idle timeout).
///
/// # Errors
///
/// Returns [`DialError::Connect`] on any bind, handshake, certificate, or
/// preamble failure.
pub async fn dial(
    d: &QuicDial,
) -> Result<
    (
        quinn::Endpoint,
        quinn::Connection,
        quinn::SendStream,
        quinn::RecvStream,
    ),
    DialError,
> {
    // Bind an ephemeral client UDP socket in the target's address family — a
    // v4 client socket cannot reach a v6 listener and vice versa.
    let bind = if d.addr.is_ipv6() {
        SocketAddr::new(IpAddr::V6(Ipv6Addr::UNSPECIFIED), 0)
    } else {
        SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 0)
    };
    let mut endpoint = quinn::Endpoint::client(bind)
        .map_err(|err| DialError::Connect(format!("bind QUIC client socket: {err}")))?;
    endpoint.set_default_client_config(client_config(&d.trust)?);

    let conn = endpoint
        .connect(d.addr, &d.server_name)
        .map_err(|err| DialError::Connect(format!("dial {}: {err}", d.addr)))?
        .await
        .map_err(|err| DialError::Connect(format!("QUIC handshake with {}: {err}", d.addr)))?;

    let (mut send, recv) = conn
        .open_bi()
        .await
        .map_err(|err| DialError::Connect(format!("open QUIC stream: {err}")))?;

    if let Some(token) = &d.token {
        write_preamble(&mut send, token).await?;
    }

    Ok((endpoint, conn, send, recv))
}

/// Write the auth preamble: `len: u32 BE` + raw token bytes (ADR-0031 parity
/// with the WebSocket `Authorization: Bearer` header).
async fn write_preamble(send: &mut quinn::SendStream, token: &[u8]) -> Result<(), DialError> {
    let len = u32::try_from(token.len())
        .map_err(|_| DialError::Connect("pairing token too long".to_owned()))?;
    send.write_all(&len.to_be_bytes())
        .await
        .map_err(|err| DialError::Connect(format!("write token preamble: {err}")))?;
    send.write_all(token)
        .await
        .map_err(|err| DialError::Connect(format!("write token: {err}")))?;
    Ok(())
}

/// Build the quinn client config: rustls TLS 1.3 with the phux ALPN, the chosen
/// certificate verifier, and a transport config matching the server's idle /
/// keep-alive timings.
fn client_config(trust: &CertTrust) -> Result<quinn::ClientConfig, DialError> {
    let crypto = crate::tls::client_config(trust, Some(QUIC_ALPN))?;
    let quic_crypto = quinn::crypto::rustls::QuicClientConfig::try_from(crypto)
        .map_err(|err| DialError::Connect(format!("build QUIC crypto: {err}")))?;
    let mut config = quinn::ClientConfig::new(Arc::new(quic_crypto));

    let mut transport = quinn::TransportConfig::default();
    transport.keep_alive_interval(Some(KEEP_ALIVE));
    if let Ok(idle) = IDLE_TIMEOUT.try_into() {
        transport.max_idle_timeout(Some(idle));
    }
    config.transport_config(Arc::new(transport));
    Ok(config)
}

#[cfg(test)]
#[allow(clippy::expect_used, reason = "tests")]
mod tests {
    use super::*;

    #[test]
    fn parse_token_hex_roundtrips_and_rejects_garbage() {
        let raw = [0xde, 0xad, 0xbe, 0xef];
        let hexed = hex::encode(raw);
        assert_eq!(parse_token_hex(&hexed).expect("valid hex"), raw);
        // Surrounding whitespace is tolerated (copy-paste from `phux pair`).
        assert_eq!(
            parse_token_hex(&format!("  {hexed}\n")).expect("trimmed"),
            raw
        );
        assert!(parse_token_hex("nothex!!").is_err());
    }
}
