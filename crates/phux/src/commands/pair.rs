//! `phux pair` — mint a bearer pairing token for a remote consumer (ADR-0031).
//!
//! The token authenticates a device that attaches over `wss://`; the server
//! reads the same token store at `PHUX_WS_TOKENS`. This verb only writes the
//! token file — it never contacts a running server — so it works before the
//! server starts and needs no socket.

use std::path::PathBuf;
use std::process::ExitCode;

/// Mint a token into the store and print it with the certificate fingerprint.
///
/// Defaults match the server's seamless path (ADR-0031): the token store and
/// the auto-generated certificate live at shared paths under the state dir, so
/// `phux pair` with no flags pairs against the same material the server will
/// read. The certificate is provisioned here if absent, so pairing works before
/// the first server start.
pub(crate) fn run_pair(tokens: Option<PathBuf>, cert: Option<PathBuf>) -> ExitCode {
    let tokens = tokens
        .or_else(|| std::env::var_os("PHUX_WS_TOKENS").map(PathBuf::from))
        .unwrap_or_else(phux_server::auth::default_token_store_path);
    let operator_cert = cert.is_some() || std::env::var_os("PHUX_WS_TLS_CERT").is_some();
    let cert = cert
        .or_else(|| std::env::var_os("PHUX_WS_TLS_CERT").map(PathBuf::from))
        .unwrap_or_else(phux_server::transport::tls::default_cert_path);
    let key = std::env::var_os("PHUX_WS_TLS_KEY")
        .map_or_else(phux_server::transport::tls::default_key_path, PathBuf::from);

    // Provision the self-signed cert at the default paths if it isn't there yet,
    // so the fingerprint below is the one the server will actually present. An
    // operator-supplied cert is used as-is, never generated over.
    if !operator_cert && let Err(err) = phux_server::transport::tls::ensure_self_signed(&cert, &key)
    {
        eprintln!("phux pair: warning: could not provision certificate: {err}");
    }

    let token = match phux_server::auth::mint_token(&tokens) {
        Ok(token) => token,
        Err(err) => {
            eprintln!("phux pair: failed to mint token: {err}");
            return ExitCode::FAILURE;
        }
    };

    println!("Pairing token (a secret — give it to the device once):");
    println!("  {token}");
    println!();

    match phux_server::transport::tls::cert_fingerprint(&cert) {
        Ok(fingerprint) => {
            println!("Server certificate SHA-256 (verify on the device to defeat MITM):");
            println!("  {fingerprint}");
            println!();
        }
        Err(err) => {
            eprintln!("phux pair: warning: could not read certificate fingerprint: {err}");
        }
    }

    // Best-effort (ADR-0037): `detect` is infallible by construction — it
    // returns an empty vec when nothing is detected — so this block can
    // never affect the exit code.
    let overlay = super::overlay::detect();
    if !overlay.is_empty() {
        println!("Overlay network addresses (dial one of these from the device):");
        for addr in &overlay {
            println!("  {addr}");
        }
        println!();
    }

    println!("Token written to {}", tokens.display());
    ExitCode::SUCCESS
}
