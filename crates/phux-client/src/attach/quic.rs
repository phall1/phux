//! QUIC dialer (`phux-y8v6`, [ADR-0007]) — the client counterpart to the
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
//! lives in [`super::connection`], shared with the UDS transport.
//!
//! **Auth.** TLS 1.3 is intrinsic to QUIC, so confidentiality is never
//! optional. For *authentication* of routable consumers the dialer mirrors the
//! server's bearer-token model (ADR-0031): it writes a length-prefixed token
//! (`len: u32 BE` + raw token bytes) as the very first bytes of the stream,
//! ahead of any phux frame. On a loopback listener no preamble is sent and
//! frames start immediately.
//!
//! [ADR-0007]: ../../../ADR/0007-mosh-class-transport-and-satellites.md

use std::fmt::Write as _;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use phux_protocol::policy::QUIC_ALPN;
use sha2::{Digest, Sha256};

use super::driver::AttachError;

/// QUIC idle timeout, matched to the server's [`IDLE_TIMEOUT`] so a quiet but
/// attached client is not reaped before the keep-alive fires.
const IDLE_TIMEOUT: Duration = Duration::from_secs(30);

/// Keep-alive interval, comfortably under [`IDLE_TIMEOUT`] so a quiet client
/// (no keystrokes, no output) holds its connection open across NATs.
const KEEP_ALIVE: Duration = Duration::from_secs(10);

/// How the dialer decides to trust the server's TLS certificate.
///
/// QUIC always encrypts; the only choice is *whose* certificate to accept. The
/// server auto-provisions a persisted self-signed cert (no operator setup,
/// ADR-0031), so there is no CA chain to validate — trust is established
/// out-of-band by pinning the fingerprint `phux pair` prints.
#[derive(Debug, Clone)]
pub enum CertTrust {
    /// Accept the server's certificate without verification. **Loopback dev
    /// only** — the listener still terminates TLS, so the handshake is real,
    /// but a man-in-the-middle on the path could impersonate the server. The
    /// CLI restricts this to loopback addresses.
    SkipVerify,
    /// Pin the server's leaf-certificate SHA-256 fingerprint, in the
    /// colon-or-bare hex shape `phux pair` prints (`AB:CD:…`). Comparison is
    /// case- and separator-insensitive. This is the trust anchor for routable
    /// hosts: it defeats the trust-on-first-use MITM window ADR-0031 names.
    Pinned(String),
}

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
    /// unauthenticated (loopback) listener. The CLI hex-decodes the
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
/// Returns [`AttachError::Connect`] when the token is not valid hex.
pub fn parse_token_hex(token: &str) -> Result<Vec<u8>, AttachError> {
    hex::decode(token.trim())
        .map_err(|err| AttachError::Connect(format!("pairing token is not valid hex: {err}")))
}

/// Connect to the QUIC listener and return the established bidi-stream halves,
/// the auth preamble already written. The quinn [`Endpoint`](quinn::Endpoint)
/// and [`Connection`](quinn::Connection) are returned alongside so the caller
/// can keep the endpoint's I/O driver alive for the connection's lifetime and
/// issue a clean `CONNECTION_CLOSE` on teardown (rather than leaving the server
/// to reap an abandoned connection at the idle timeout).
pub(super) async fn dial(
    d: &QuicDial,
) -> Result<
    (
        quinn::Endpoint,
        quinn::Connection,
        quinn::SendStream,
        quinn::RecvStream,
    ),
    AttachError,
> {
    // Bind an ephemeral client UDP socket in the target's address family — a
    // v4 client socket cannot reach a v6 listener and vice versa.
    let bind = if d.addr.is_ipv6() {
        SocketAddr::new(IpAddr::V6(Ipv6Addr::UNSPECIFIED), 0)
    } else {
        SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 0)
    };
    let mut endpoint = quinn::Endpoint::client(bind)
        .map_err(|err| AttachError::Connect(format!("bind QUIC client socket: {err}")))?;
    endpoint.set_default_client_config(client_config(&d.trust)?);

    let conn = endpoint
        .connect(d.addr, &d.server_name)
        .map_err(|err| AttachError::Connect(format!("dial {}: {err}", d.addr)))?
        .await
        .map_err(|err| AttachError::Connect(format!("QUIC handshake with {}: {err}", d.addr)))?;

    let (mut send, recv) = conn
        .open_bi()
        .await
        .map_err(|err| AttachError::Connect(format!("open QUIC stream: {err}")))?;

    if let Some(token) = &d.token {
        write_preamble(&mut send, token).await?;
    }

    Ok((endpoint, conn, send, recv))
}

/// Write the auth preamble: `len: u32 BE` + raw token bytes (ADR-0031 parity
/// with the WebSocket `Authorization: Bearer` header).
async fn write_preamble(send: &mut quinn::SendStream, token: &[u8]) -> Result<(), AttachError> {
    let len = u32::try_from(token.len())
        .map_err(|_| AttachError::Connect("pairing token too long".to_owned()))?;
    send.write_all(&len.to_be_bytes())
        .await
        .map_err(|err| AttachError::Connect(format!("write token preamble: {err}")))?;
    send.write_all(token)
        .await
        .map_err(|err| AttachError::Connect(format!("write token: {err}")))?;
    Ok(())
}

/// Build the quinn client config: rustls TLS 1.3 with the phux ALPN, the chosen
/// certificate verifier, and a transport config matching the server's idle /
/// keep-alive timings.
fn client_config(trust: &CertTrust) -> Result<quinn::ClientConfig, AttachError> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let verifier: Arc<dyn rustls::client::danger::ServerCertVerifier> = match trust {
        CertTrust::SkipVerify => Arc::new(SkipServerVerification(provider.clone())),
        CertTrust::Pinned(fingerprint) => Arc::new(PinnedFingerprint {
            provider: provider.clone(),
            expected: normalize_fingerprint(fingerprint),
        }),
    };

    let mut crypto = rustls::ClientConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(|err| AttachError::Connect(format!("build TLS client config: {err}")))?
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_no_client_auth();
    crypto.alpn_protocols = vec![QUIC_ALPN.to_vec()];

    let quic_crypto = quinn::crypto::rustls::QuicClientConfig::try_from(crypto)
        .map_err(|err| AttachError::Connect(format!("build QUIC crypto: {err}")))?;
    let mut config = quinn::ClientConfig::new(Arc::new(quic_crypto));

    let mut transport = quinn::TransportConfig::default();
    transport.keep_alive_interval(Some(KEEP_ALIVE));
    if let Ok(idle) = IDLE_TIMEOUT.try_into() {
        transport.max_idle_timeout(Some(idle));
    }
    config.transport_config(Arc::new(transport));
    Ok(config)
}

/// Uppercase hex digits only — drops the `:` separators (and any stray
/// whitespace) so a pin pasted as `AB:CD:…`, `ab:cd:…`, or `ABCD…` all compare
/// equal to the SHA-256 of the presented leaf certificate.
fn normalize_fingerprint(fingerprint: &str) -> String {
    fingerprint
        .chars()
        .filter(char::is_ascii_hexdigit)
        .flat_map(char::to_uppercase)
        .collect()
}

/// SHA-256 of a leaf certificate as bare uppercase hex (no separators), to
/// compare against a [`normalize_fingerprint`]d pin.
fn leaf_fingerprint(cert: &rustls::pki_types::CertificateDer<'_>) -> String {
    let digest = Sha256::digest(cert.as_ref());
    let mut hex = String::with_capacity(digest.len() * 2);
    for byte in digest {
        // Writing to a `String` is infallible.
        let _ = write!(hex, "{byte:02X}");
    }
    hex
}

/// Certificate verifier that pins the leaf certificate's SHA-256 fingerprint.
///
/// There is no CA chain to validate for the server's self-signed cert, so the
/// pin *is* the trust: a presented leaf whose fingerprint matches the
/// out-of-band value (`phux pair`) is accepted; anything else is rejected. TLS
/// signatures are still verified by the crypto provider so a stolen cert cannot
/// be replayed without its private key.
#[derive(Debug)]
struct PinnedFingerprint {
    provider: Arc<rustls::crypto::CryptoProvider>,
    expected: String,
}

impl rustls::client::danger::ServerCertVerifier for PinnedFingerprint {
    fn verify_server_cert(
        &self,
        end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        let actual = leaf_fingerprint(end_entity);
        if actual == self.expected {
            Ok(rustls::client::danger::ServerCertVerified::assertion())
        } else {
            Err(rustls::Error::General(format!(
                "server certificate fingerprint mismatch (pinned {}, got {})",
                self.expected, actual
            )))
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

/// Certificate verifier that accepts any server certificate (loopback dev).
///
/// TLS still runs — the handshake, ALPN, and signature checks are real — but the
/// leaf is trusted blindly. Confidentiality holds against a passive observer;
/// it does **not** defend against an active MITM, which is why the CLI permits
/// it only for loopback addresses where there is no untrusted network path.
#[derive(Debug)]
struct SkipServerVerification(Arc<rustls::crypto::CryptoProvider>);

impl rustls::client::danger::ServerCertVerifier for SkipServerVerification {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
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

    #[test]
    fn normalize_fingerprint_is_separator_and_case_insensitive() {
        let colons = "ab:CD:12:Ef";
        let bare = "ABCD12EF";
        assert_eq!(normalize_fingerprint(colons), bare);
        assert_eq!(normalize_fingerprint(bare), bare);
        assert_eq!(
            normalize_fingerprint("  ab cd 12 ef  "),
            bare,
            "whitespace is dropped too"
        );
    }
}
