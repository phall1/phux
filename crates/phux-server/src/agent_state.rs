//! Authority over the `phux.agent/v1` record (ADR-0046 §E).
//!
//! Two writers can reach one Terminal's record: a human/agent/plugin issuing
//! an explicit `SET_METADATA`, and the server-side detector. They must not
//! fight over it. The arbitration rule, normatively:
//!
//! > An explicit `SET_METADATA` on `phux.agent/v1` that supplies a `state`
//! > outranks the detector; the detector makes no further writes to that
//! > Terminal until the record is `DELETE`d. An explicit write that supplies
//! > only identity (`name` / `kind` / `session`) is preserved field-for-field
//! > and the detector fills `state` around it. The detector deletes only
//! > records it itself wrote.
//!
//! The declaration cannot be inferred from the stored bytes. The client's
//! `AgentMetaState` decodes an absent or unrecognized `state` to `Unknown`,
//! so `unknown` and "absent" are indistinguishable on the way back out — and
//! the detector's own writes carry a `state` too. So declaration is tracked
//! explicitly, populated ONLY from the `SET_METADATA` entry point. The
//! detector's drain writes through `ServerState::metadata_set` directly and
//! therefore never passes through it, which is exactly what makes the
//! bookkeeping honest.

#![allow(
    clippy::redundant_pub_crate,
    reason = "private server module shared by the sibling runtime / state modules"
)]

use std::collections::HashSet;

use phux_protocol::ids::TerminalId as WireTerminalId;

use crate::agent_detect::record::AgentRecordJson;

/// Who currently owns each Terminal's `phux.agent/v1` record.
#[derive(Debug, Default)]
pub(crate) struct AgentRecordArbiter {
    /// Terminals whose record was written by an explicit `SET_METADATA` that
    /// SUPPLIED a `state`. The detector stands down for these until `DELETE`.
    declared: HashSet<WireTerminalId>,
    /// Terminals whose current record the detector authored — so it may
    /// rewrite or retract it, and only it.
    detector_owned: HashSet<WireTerminalId>,
}

impl AgentRecordArbiter {
    /// Note an explicit `SET_METADATA` on this Terminal's agent record.
    ///
    /// The Terminal becomes `declared` **iff** the write supplied a real
    /// `state` — a bare identity declaration (`name`/`kind`/`session` only,
    /// or an explicit `"unknown"`) leaves the detector free to fill `state`
    /// in around it, which is the useful half of the feature: a human names
    /// the agent, the detector tracks its lifecycle.
    ///
    /// Either way the detector no longer owns the record, so it must not
    /// delete it.
    pub(crate) fn note_explicit_set(&mut self, terminal: &WireTerminalId, value: &[u8]) {
        self.detector_owned.remove(terminal);
        let declares_state = AgentRecordJson::decode(value)
            .is_some_and(|r| !r.state.is_empty() && r.state != "unknown");
        if declares_state {
            self.declared.insert(terminal.clone());
        } else {
            self.declared.remove(terminal);
        }
    }

    /// Note an explicit `DELETE_METADATA`. The declaration is withdrawn and
    /// the detector resumes.
    pub(crate) fn note_explicit_delete(&mut self, terminal: &WireTerminalId) {
        self.declared.remove(terminal);
        self.detector_owned.remove(terminal);
    }

    /// Whether a human has declared this Terminal's state, in which case the
    /// detector must not write.
    pub(crate) fn is_declared(&self, terminal: &WireTerminalId) -> bool {
        self.declared.contains(terminal)
    }

    /// Note that the detector authored this Terminal's current record.
    pub(crate) fn note_detector_write(&mut self, terminal: &WireTerminalId) {
        self.detector_owned.insert(terminal.clone());
    }

    /// Note that the detector retracted this Terminal's record.
    pub(crate) fn note_detector_retract(&mut self, terminal: &WireTerminalId) {
        self.detector_owned.remove(terminal);
    }

    /// Whether the detector authored the record currently stored, and may
    /// therefore delete it.
    pub(crate) fn detector_owns(&self, terminal: &WireTerminalId) -> bool {
        self.detector_owned.contains(terminal)
    }

    /// Drop all bookkeeping for a reaped Terminal.
    pub(crate) fn forget(&mut self, terminal: &WireTerminalId) {
        self.declared.remove(terminal);
        self.detector_owned.remove(terminal);
    }
}

/// Compose the record the detector should write, preserving every field an
/// explicit (state-less) declaration supplied.
///
/// `existing` is the currently-stored value, if any. `name` and `kind` come
/// from the detector's manifest but yield to a human-declared `name`: if
/// someone called their pane "reviewer", it stays "reviewer" while the
/// detector tracks its state. `session` is carried through untouched.
///
/// `attention` is deliberately NOT set by the detector — `docs/spec/L3.md`
/// §3.7 already derives it from `state` when absent — but a declared one is
/// preserved.
pub(crate) fn compose(existing: Option<&[u8]>, kind: &str, name: &str, state: &str) -> Vec<u8> {
    let prior = existing.and_then(AgentRecordJson::decode);
    let record = match prior {
        Some(mut prior) => {
            if prior.name.is_empty() {
                prior.name.clear();
                prior.name.push_str(name);
            }
            if prior.kind.is_none() {
                prior.kind = Some(kind.to_owned());
            }
            prior.state.clear();
            prior.state.push_str(state);
            prior
        }
        None => AgentRecordJson {
            name: name.to_owned(),
            kind: Some(kind.to_owned()),
            state: state.to_owned(),
            attention: None,
            session: None,
        },
    };
    record.encode()
}

#[cfg(test)]
#[allow(clippy::expect_used, reason = "tests")]
mod tests {
    use phux_protocol::ids::TerminalId as WireTerminalId;

    use super::{AgentRecordArbiter, compose};
    use crate::agent_detect::record::AgentRecordJson;

    fn terminal(id: u32) -> WireTerminalId {
        WireTerminalId::new(id)
    }

    #[test]
    fn a_declared_state_stands_the_detector_down() {
        let mut arb = AgentRecordArbiter::default();
        let t = terminal(1);
        assert!(!arb.is_declared(&t), "nothing declared yet");
        arb.note_explicit_set(&t, br#"{"name":"me","state":"blocked"}"#);
        assert!(arb.is_declared(&t));
    }

    /// The useful half: a human names the agent, the detector keeps tracking
    /// its lifecycle.
    #[test]
    fn an_identity_only_declaration_leaves_the_detector_running() {
        let mut arb = AgentRecordArbiter::default();
        let t = terminal(1);
        arb.note_explicit_set(&t, br#"{"name":"reviewer","kind":"claude"}"#);
        assert!(!arb.is_declared(&t));
    }

    #[test]
    fn an_explicit_unknown_state_is_not_a_declaration() {
        let mut arb = AgentRecordArbiter::default();
        let t = terminal(1);
        arb.note_explicit_set(&t, br#"{"name":"x","state":"unknown"}"#);
        assert!(!arb.is_declared(&t), "`unknown` declares nothing");
    }

    #[test]
    fn a_delete_withdraws_the_declaration_and_the_detector_resumes() {
        let mut arb = AgentRecordArbiter::default();
        let t = terminal(1);
        arb.note_explicit_set(&t, br#"{"name":"me","state":"done"}"#);
        assert!(arb.is_declared(&t));
        arb.note_explicit_delete(&t);
        assert!(!arb.is_declared(&t));
    }

    /// The detector may only delete what it wrote. A human's record is not
    /// its to retract.
    #[test]
    fn the_detector_only_owns_records_it_wrote() {
        let mut arb = AgentRecordArbiter::default();
        let t = terminal(1);
        assert!(!arb.detector_owns(&t));
        arb.note_detector_write(&t);
        assert!(arb.detector_owns(&t));
        arb.note_detector_retract(&t);
        assert!(!arb.detector_owns(&t));
    }

    /// An explicit write over a detector-authored record transfers ownership
    /// away, so the detector can no longer delete it.
    #[test]
    fn an_explicit_set_takes_ownership_from_the_detector() {
        let mut arb = AgentRecordArbiter::default();
        let t = terminal(1);
        arb.note_detector_write(&t);
        arb.note_explicit_set(&t, br#"{"name":"mine","kind":"claude"}"#);
        assert!(!arb.detector_owns(&t), "the detector must not delete this");
        assert!(!arb.is_declared(&t), "but it may still fill in `state`");
    }

    #[test]
    fn malformed_bytes_declare_nothing() {
        let mut arb = AgentRecordArbiter::default();
        let t = terminal(1);
        arb.note_explicit_set(&t, b"not json at all");
        assert!(!arb.is_declared(&t));
    }

    #[test]
    fn forget_drops_every_trace_of_a_reaped_terminal() {
        let mut arb = AgentRecordArbiter::default();
        let t = terminal(7);
        arb.note_explicit_set(&t, br#"{"name":"x","state":"working"}"#);
        arb.note_detector_write(&t);
        arb.forget(&t);
        assert!(!arb.is_declared(&t));
        assert!(!arb.detector_owns(&t));
    }

    #[test]
    fn terminals_are_tracked_independently() {
        let mut arb = AgentRecordArbiter::default();
        let (a, b) = (terminal(1), terminal(2));
        arb.note_explicit_set(&a, br#"{"name":"a","state":"done"}"#);
        assert!(arb.is_declared(&a));
        assert!(!arb.is_declared(&b));
    }

    // --- compose ----------------------------------------------------------

    #[test]
    fn compose_from_nothing_writes_the_detector_view() {
        let bytes = compose(None, "claude", "claude", "working");
        assert_eq!(
            String::from_utf8(bytes).expect("utf8"),
            r#"{"name":"claude","kind":"claude","state":"working"}"#
        );
    }

    /// The field-for-field preservation the ADR promises.
    #[test]
    fn compose_preserves_an_identity_only_declaration() {
        let existing = br#"{"name":"reviewer","kind":"claude","session":"fleet-7"}"#;
        let bytes = compose(Some(existing), "claude", "claude", "blocked");
        let got = AgentRecordJson::decode(&bytes).expect("decodes");
        assert_eq!(got.name, "reviewer", "the human's name survives");
        assert_eq!(got.session.as_deref(), Some("fleet-7"), "and their label");
        assert_eq!(got.state, "blocked", "the detector supplies only `state`");
    }

    #[test]
    fn compose_preserves_a_declared_attention() {
        let existing = br#"{"name":"a","attention":"high"}"#;
        let bytes = compose(Some(existing), "claude", "claude", "idle");
        let got = AgentRecordJson::decode(&bytes).expect("decodes");
        assert_eq!(got.attention.as_deref(), Some("high"));
        assert_eq!(got.state, "idle");
    }

    #[test]
    fn compose_fills_a_missing_name_and_kind() {
        let existing = br#"{"name":"","session":"s"}"#;
        let bytes = compose(Some(existing), "claude", "claude", "idle");
        let got = AgentRecordJson::decode(&bytes).expect("decodes");
        assert_eq!(got.name, "claude");
        assert_eq!(got.kind.as_deref(), Some("claude"));
        assert_eq!(got.session.as_deref(), Some("s"));
    }

    /// Garbage in the store must not stop the detector from writing a clean
    /// record over it.
    #[test]
    fn compose_over_malformed_bytes_starts_fresh() {
        let bytes = compose(Some(b"}{ nonsense"), "claude", "claude", "idle");
        let got = AgentRecordJson::decode(&bytes).expect("decodes");
        assert_eq!(got.name, "claude");
        assert_eq!(got.state, "idle");
    }

    /// The dedup contract: recomposing an unchanged state yields byte-identical
    /// output, which is what makes `metadata_set` suppress the broadcast.
    #[test]
    fn compose_is_stable_across_repeats() {
        let first = compose(None, "claude", "claude", "working");
        let second = compose(Some(&first), "claude", "claude", "working");
        assert_eq!(first, second, "a steady state must produce identical bytes");
    }
}
