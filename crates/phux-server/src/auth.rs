//! Bearer-token authentication for remote WebSocket consumers (ADR-0031).
//!
//! A remote consumer (the native mobile app) attaches over `wss://` without an
//! SSH tunnel. Encryption is TLS (see [`crate::transport::tls`]); *authentication*
//! is an opaque pairing token the consumer presents in the WebSocket upgrade
//! request (`Authorization: Bearer <hex>`). This module owns the token store:
//! loading the operator's set of valid tokens, comparing a presented token in
//! constant time, and minting new ones with the OS CSPRNG.
//!
//! The token is a bearer credential: anyone holding it is the paired device
//! until the token is removed from the store. That tradeoff (versus a client
//! certificate that never leaves the device) is recorded in ADR-0031; the
//! mitigations live here — high entropy, constant-time comparison so the store
//! leaks no timing oracle, owner-only file permissions, and per-line revocation.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use subtle::ConstantTimeEq;

/// Default persisted path for the remote-consumer token store:
/// `<state-dir>/remote-tokens`. The server reads it and `phux pair` appends to
/// it, so neither needs an explicit path for the common case.
#[must_use]
pub fn default_token_store_path() -> PathBuf {
    crate::telemetry::state_dir().join("remote-tokens")
}

/// Length in bytes of a minted pairing token. 32 bytes (256 bits) from the OS
/// CSPRNG is well past brute-force range and matches the TLS session-key class.
pub const TOKEN_LEN: usize = 32;

/// Errors from loading or minting pairing tokens.
#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    /// The token file could not be read or written.
    #[error("token store io: {0}")]
    Io(#[from] io::Error),
    /// The OS random source failed while minting a token.
    #[error("os random source unavailable: {0}")]
    Random(#[from] getrandom::Error),
    /// A line in the token file was not valid hex of the expected length.
    #[error("malformed token in store (expected {TOKEN_LEN}-byte hex)")]
    Malformed,
}

/// A set of valid bearer tokens loaded from an operator-managed file.
///
/// The file is line-oriented: one lowercase-hex token per line, `#` comments
/// and blank lines ignored. Revoking a device is deleting its line. The store
/// is loaded once at listener construction; a future hot-reload would re-read
/// the file on a signal, but v0.1 reads it at bind time.
#[derive(Clone)]
pub struct TokenStore {
    tokens: Vec<[u8; TOKEN_LEN]>,
}

/// Redacted: reports only how many tokens are loaded, never their bytes, so a
/// `?store` in a log line cannot spill a bearer credential.
impl std::fmt::Debug for TokenStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TokenStore")
            .field("tokens", &self.tokens.len())
            .finish()
    }
}

impl TokenStore {
    /// Load the token set from `path`. A missing file is an empty store (no
    /// tokens, so every connection is rejected) rather than an error, so an
    /// operator can point at a not-yet-created path and `phux pair` into it.
    pub fn load(path: &Path) -> Result<Self, AuthError> {
        let raw = match fs::read_to_string(path) {
            Ok(raw) => raw,
            Err(err) if err.kind() == io::ErrorKind::NotFound => String::new(),
            Err(err) => return Err(err.into()),
        };
        let mut tokens = Vec::new();
        for line in raw.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            tokens.push(parse_token(line)?);
        }
        Ok(Self { tokens })
    }

    /// Number of valid tokens currently loaded.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.tokens.len()
    }

    /// Whether the store holds no tokens (every connection would be rejected).
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.tokens.is_empty()
    }

    /// Verify a presented token against the store in constant time.
    ///
    /// The comparison visits every stored token and accumulates the match with
    /// no early return, so the time taken does not reveal which token matched or
    /// how many leading bytes were correct. A presented token of the wrong
    /// length cannot match (length is not a secret); it short-circuits to
    /// `false` without consulting the store.
    #[must_use]
    pub fn verify(&self, presented: &[u8]) -> bool {
        let Ok(candidate) = <[u8; TOKEN_LEN]>::try_from(presented) else {
            return false;
        };
        let mut matched = subtle::Choice::from(0u8);
        for token in &self.tokens {
            matched |= token.ct_eq(&candidate);
        }
        bool::from(matched)
    }
}

/// Mint a fresh token, append it to the store file (created `0o600` if absent),
/// and return it as lowercase hex for one-time display at pairing time.
///
/// Appending — rather than rewriting — preserves the tokens of other paired
/// devices. The parent directory must already exist.
pub fn mint_token(path: &Path) -> Result<String, AuthError> {
    let mut token = [0u8; TOKEN_LEN];
    getrandom::getrandom(&mut token)?;
    let encoded = hex::encode(token);

    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(path)?;
    writeln!(file, "{encoded}")?;
    Ok(encoded)
}

/// Parse and hex-decode one token line into a fixed-size token.
fn parse_token(line: &str) -> Result<[u8; TOKEN_LEN], AuthError> {
    let bytes = hex::decode(line).map_err(|_| AuthError::Malformed)?;
    <[u8; TOKEN_LEN]>::try_from(bytes.as_slice()).map_err(|_| AuthError::Malformed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_store(contents: &str) -> tempfile::NamedTempFile {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(contents.as_bytes()).unwrap();
        f
    }

    #[test]
    fn missing_file_is_empty_store_that_rejects_all() {
        let store = TokenStore::load(Path::new("/nonexistent/phux/tokens")).unwrap();
        assert!(store.is_empty());
        assert!(!store.verify(&[0u8; TOKEN_LEN]));
    }

    #[test]
    fn loads_hex_tokens_skipping_comments_and_blanks() {
        let tok = "a".repeat(TOKEN_LEN * 2);
        let store = write_store(&format!("# a comment\n\n{tok}\n"));
        let store = TokenStore::load(store.path()).unwrap();
        assert_eq!(store.len(), 1);
        assert!(store.verify(&[0xaa; TOKEN_LEN]));
    }

    #[test]
    fn rejects_unknown_token_and_wrong_length() {
        let tok = "a".repeat(TOKEN_LEN * 2);
        let f = write_store(&format!("{tok}\n"));
        let store = TokenStore::load(f.path()).unwrap();
        assert!(!store.verify(&[0xbb; TOKEN_LEN]));
        assert!(!store.verify(b"too-short"));
        assert!(!store.verify(&[0xaa; TOKEN_LEN + 1]));
    }

    #[test]
    fn malformed_line_is_an_error() {
        let f = write_store("not-hex-at-all\n");
        assert!(matches!(
            TokenStore::load(f.path()),
            Err(AuthError::Malformed)
        ));
    }

    #[test]
    fn mint_appends_verifiable_token_with_owner_only_perms() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tokens");

        let first = mint_token(&path).unwrap();
        let second = mint_token(&path).unwrap();
        assert_ne!(first, second, "each mint is unique");

        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "token store must be owner-only");

        let store = TokenStore::load(&path).unwrap();
        assert_eq!(
            store.len(),
            2,
            "both tokens persisted (append, not rewrite)"
        );
        assert!(store.verify(&hex::decode(&first).unwrap()));
        assert!(store.verify(&hex::decode(&second).unwrap()));
    }
}
