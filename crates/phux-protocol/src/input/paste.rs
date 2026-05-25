//! Paste input: `PasteEvent`, `PasteTrust`.
//!
//! Owned by phux-6yl.7. See SPEC.md §9.4 and ADR-0006.
//!
//! `PasteTrust` mirrors libghostty-vt's `paste::PasteTrust` with verbatim
//! numeric discriminants — see [`tests::paste_trust_discriminants_match_libghostty`]
//! for the table-driven check. We do not depend on libghostty-vt from
//! `phux-protocol`; the discriminants are pinned by test instead.

#![allow(clippy::module_name_repetitions)]

/// Trust classification for paste payloads.
///
/// SPEC.md §9.4: untrusted payloads SHOULD be classified by the server
/// via `libghostty_vt::paste::is_safe` and either rejected, sanitized,
/// or forwarded per the server's per-pane policy. Trusted payloads skip
/// safety classification but still flow through `paste::encode` for
/// bracketed-paste handling.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PasteTrust {
    /// Caller asserted the payload is safe; server skips `is_safe`
    /// classification and forwards via `paste::encode`.
    Trusted = 0,
    /// Caller cannot vouch for safety; server applies its per-pane
    /// untrusted-paste policy (reject / sanitize / allow).
    Untrusted = 1,
}

/// A paste event from a client.
///
/// SPEC.md §9.4. The `bracketed` flag from the wire form is intentionally
/// *not* carried here — server-side encoding decides bracketing based on
/// the target pane's DEC mode 2004 state via `paste::encode`. This type
/// captures only the fields the server-side translation layer needs that
/// cannot be recovered from terminal state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PasteEvent {
    /// Trust classification for `data`.
    pub trust: PasteTrust,
    /// Raw paste payload. UTF-8 is typical but not required; the server
    /// hands the bytes to `paste::encode` which strips unsafe control
    /// bytes regardless of encoding.
    pub data: Vec<u8>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pin `PasteTrust` discriminants to libghostty-vt's `paste::PasteTrust`.
    ///
    /// `phux-protocol` does not depend on libghostty-vt, so this is a
    /// hand-mirrored table — kept in sync with
    /// `libghostty-vt/src/paste.rs`. If libghostty renumbers, this test
    /// breaks loudly. Same pattern as phux-6yl.1's `KeyAction` /
    /// `PhysicalKey` discriminant tests.
    #[test]
    fn paste_trust_discriminants_match_libghostty() {
        // (our variant, libghostty's numeric discriminant)
        const TABLE: &[(PasteTrust, u8)] = &[(PasteTrust::Trusted, 0), (PasteTrust::Untrusted, 1)];
        for &(variant, expected) in TABLE {
            assert_eq!(
                variant as u8, expected,
                "PasteTrust::{variant:?} discriminant drift vs libghostty",
            );
        }
    }

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
        let d = PasteEvent {
            trust: PasteTrust::Trusted,
            data: b"world".to_vec(),
        };
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_ne!(a, d);
    }

    #[test]
    fn paste_event_clone_is_deep() {
        let original = PasteEvent {
            trust: PasteTrust::Untrusted,
            data: b"payload".to_vec(),
        };
        let cloned = original.clone();
        assert_eq!(original, cloned);
        // Distinct allocations — proves the `Vec` is deep-cloned, not aliased.
        assert_ne!(original.data.as_ptr(), cloned.data.as_ptr());
    }
}
