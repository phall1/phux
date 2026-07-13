//! Server-side JSON shape of the `phux.agent/v1` L3 record (ADR-0040,
//! `docs/spec/L3.md` §3.7).
//!
//! **This is a deliberate duplicate** of `phux_client::agent_meta::AgentRecord`.
//! The server MUST NOT depend on `phux-client`, so the shape is restated
//! here — and pinned by a golden byte-equality test, because the coupling is
//! not merely "the client can parse it".
//!
//! [`crate::state::ServerState::metadata_set`] suppresses a broadcast when
//! the new bytes equal the stored bytes. That dedup is what makes an agent
//! that stays `working` for ten minutes cost ZERO metadata writes and ZERO
//! events. It compares raw bytes. So a field-order or `skip_serializing_if`
//! drift against the client's encoder would still *parse* fine, while
//! silently turning every detector tick into a fan-out to every L3
//! subscriber. Hence: exact field order (`name`, `kind`, `state`,
//! `attention`, `session`) and the exact skip set.

use serde::{Deserialize, Serialize};

/// The `phux.agent/v1` record, in the server's own vocabulary.
///
/// `state` is a raw open-enum word rather than a typed enum: the server is
/// a *writer* here, and the spec's open-enum vocabulary is the client's to
/// interpret. It is always emitted (no skip) to match the client's
/// `#[serde(default)]`-but-not-skipped `state` field.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct AgentRecordJson {
    /// Human-facing agent name. REQUIRED and non-empty per the spec.
    pub(crate) name: String,
    /// Open-vocabulary kind slug, e.g. `"claude"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) kind: Option<String>,
    /// Lifecycle word: `unknown` | `idle` | `working` | `blocked` | `done`.
    #[serde(default)]
    pub(crate) state: String,
    /// Attention priority. The detector NEVER sets this: §3.7 already says
    /// an absent `attention` is derived from `state`, and the client's
    /// `AgentRecord::effective_attention` does exactly that. Carrying it
    /// would be more bytes and one more edge to churn. It is preserved
    /// verbatim when a human declared it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) attention: Option<String>,
    /// Free-form association label (fleet / job name).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) session: Option<String>,
}

impl AgentRecordJson {
    /// Decode a stored record. `None` for bytes that are not a JSON object
    /// — the spec's "no declared agent" reading — so a malformed write can
    /// never wedge the arbiter.
    pub(crate) fn decode(bytes: &[u8]) -> Option<Self> {
        let value: serde_json::Value = serde_json::from_slice(bytes).ok()?;
        if !value.is_object() {
            return None;
        }
        serde_json::from_value(value).ok()
    }

    /// Encode to the UTF-8 JSON bytes `SET_METADATA` carries.
    pub(crate) fn encode(&self) -> Vec<u8> {
        serde_json::to_vec(self).unwrap_or_default()
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, reason = "tests")]
mod tests {
    use super::AgentRecordJson;

    /// GOLDEN. The exact bytes the detector writes. If this changes, the
    /// `metadata_set` equal-bytes dedup stops suppressing steady-state
    /// rewrites and every tick becomes a broadcast. See the module docs.
    #[test]
    fn golden_encoding_is_byte_exact() {
        let record = AgentRecordJson {
            name: "claude".to_owned(),
            kind: Some("claude".to_owned()),
            state: "working".to_owned(),
            attention: None,
            session: None,
        };
        assert_eq!(
            String::from_utf8(record.encode()).expect("utf8"),
            r#"{"name":"claude","kind":"claude","state":"working"}"#
        );
    }

    /// Re-encoding a decoded record is stable — the property the dedup
    /// actually relies on.
    #[test]
    fn encode_decode_roundtrips_byte_for_byte() {
        let bytes = br#"{"name":"claude","kind":"claude","state":"idle","session":"fleet-1"}"#;
        let record = AgentRecordJson::decode(bytes).expect("decodes");
        assert_eq!(record.encode(), bytes.to_vec());
    }

    #[test]
    fn optional_fields_are_skipped_when_absent() {
        let record = AgentRecordJson {
            name: "codex".to_owned(),
            kind: None,
            state: "idle".to_owned(),
            attention: None,
            session: None,
        };
        assert_eq!(
            String::from_utf8(record.encode()).expect("utf8"),
            r#"{"name":"codex","state":"idle"}"#
        );
    }

    #[test]
    fn a_declared_attention_survives_a_roundtrip() {
        let bytes = br#"{"name":"a","state":"blocked","attention":"high"}"#;
        let record = AgentRecordJson::decode(bytes).expect("decodes");
        assert_eq!(record.attention.as_deref(), Some("high"));
    }

    #[test]
    fn non_object_json_is_not_a_record() {
        assert!(AgentRecordJson::decode(b"[1,2,3]").is_none());
        assert!(AgentRecordJson::decode(b"\"hello\"").is_none());
        assert!(AgentRecordJson::decode(b"not json").is_none());
    }
}
