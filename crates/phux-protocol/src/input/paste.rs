//! Paste input — `PasteEvent` + `PasteTrust`.
//!
//! Unlike sibling input modules, paste's libghostty surface is *free
//! functions* (`paste::is_safe`, `paste::encode`) rather than a typed event.
//! `PasteTrust` is therefore phux-defined: it captures per-pane policy
//! metadata (the caller's claim about the payload) that the server uses to
//! gate `paste::is_safe` before forwarding via `paste::encode`. See SPEC §9.4
//! and ADR-0008.

#![allow(clippy::module_name_repetitions)]

/// Trust classification for paste payloads. Phux-defined policy metadata.
///
/// The server's per-pane policy decides what to do with each class:
/// `Trusted` skips safety classification and forwards via `paste::encode`;
/// `Untrusted` runs `paste::is_safe` first and then reject / sanitize /
/// allow per configuration.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PasteTrust {
    /// Caller asserts the payload is safe.
    Trusted = 0,
    /// Caller cannot vouch for safety.
    Untrusted = 1,
}

/// A paste event from a client.
///
/// SPEC §9.4. The `bracketed` flag from the wire form is intentionally not
/// carried here — server-side encoding decides bracketing based on the
/// target pane's DEC mode 2004 state via `libghostty_vt::paste::encode`.
///
/// `Debug` is hand-written and **redaction-safe** (ADR-0028): it never prints
/// the `data` payload (clipboard contents routinely carry secrets), only its
/// length and trust class. This keeps the server's `trace!(?input, …)`
/// PTY-handoff diagnostics from spilling pasted passwords into a log.
#[derive(Clone, PartialEq, Eq)]
pub struct PasteEvent {
    /// Trust classification for `data`.
    pub trust: PasteTrust,
    /// Raw paste payload. UTF-8 is typical but not required; the server
    /// hands the bytes to `paste::encode`, which strips unsafe control
    /// bytes regardless of encoding.
    pub data: Vec<u8>,
}

/// Redaction-safe `Debug` (ADR-0028): structure, never payload. The clipboard
/// bytes are reduced to a `data_len`; the trust class is structural and safe.
impl core::fmt::Debug for PasteEvent {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("PasteEvent")
            .field("trust", &self.trust)
            // Redacted: never log the pasted bytes.
            .field("data_len", &self.data.len())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paste_event_construction_and_equality() {
        let a = PasteEvent {
            trust: PasteTrust::Trusted,
            data: b"hello".to_vec(),
        };
        let b = PasteEvent {
            trust: PasteTrust::Trusted,
            data: b"hello".to_vec(),
        };
        let c = PasteEvent {
            trust: PasteTrust::Untrusted,
            data: b"hello".to_vec(),
        };
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn debug_redacts_paste_payload() {
        let secret = "SUPER-SECRET-CLIPBOARD";
        let event = PasteEvent {
            trust: PasteTrust::Untrusted,
            data: secret.as_bytes().to_vec(),
        };
        let rendered = format!("{event:?}");
        assert!(rendered.contains("PasteEvent"), "{rendered}");
        assert!(rendered.contains("Untrusted"), "{rendered}");
        assert!(rendered.contains("data_len"), "{rendered}");
        assert!(
            !rendered.contains(secret),
            "Debug leaked paste payload: {rendered}"
        );
    }
}
