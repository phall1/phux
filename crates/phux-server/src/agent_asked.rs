#![allow(
    clippy::redundant_pub_crate,
    reason = "private server module shared by sibling runtime/state modules"
)]

use std::collections::HashMap;

use phux_core::ids::TerminalId;
use phux_protocol::wire::frame::AgentEvent;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AskedSource {
    #[allow(
        dead_code,
        reason = "passive scrape source lands after the detector core"
    )]
    Scrape,
    Hook,
}

impl AskedSource {
    const fn priority(self) -> u8 {
        match self {
            Self::Scrape => 0,
            Self::Hook => 1,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AskedPayload {
    pub(crate) id: String,
    pub(crate) question: String,
    pub(crate) suggestions: Vec<String>,
    pub(crate) elapsed_seconds: Option<u64>,
}

impl AskedPayload {
    pub(crate) fn into_event(self) -> AgentEvent {
        AgentEvent::Asked {
            id: self.id,
            question: self.question,
            suggestions: self.suggestions,
            elapsed_seconds: self.elapsed_seconds,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AskedTransition {
    Entered(AskedPayload),
    Updated(AskedPayload),
    Ignored,
}

impl AskedTransition {
    pub(crate) fn emit_payload(self) -> Option<AskedPayload> {
        match self {
            Self::Entered(payload) | Self::Updated(payload) => Some(payload),
            Self::Ignored => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AskedState {
    source: AskedSource,
    payload: AskedPayload,
}

#[derive(Debug, Default)]
pub(crate) struct AskedDetector {
    states: HashMap<TerminalId, AskedState>,
}

impl AskedDetector {
    pub(crate) fn report(
        &mut self,
        terminal: TerminalId,
        source: AskedSource,
        payload: AskedPayload,
    ) -> AskedTransition {
        match self.states.get(&terminal) {
            Some(existing) if existing.source.priority() > source.priority() => {
                AskedTransition::Ignored
            }
            Some(existing) if existing.source == source && existing.payload == payload => {
                AskedTransition::Ignored
            }
            Some(_) => {
                let emitted = payload.clone();
                self.states.insert(terminal, AskedState { source, payload });
                AskedTransition::Updated(emitted)
            }
            None => {
                let emitted = payload.clone();
                self.states.insert(terminal, AskedState { source, payload });
                AskedTransition::Entered(emitted)
            }
        }
    }

    pub(crate) fn clear_terminal(&mut self, terminal: TerminalId) -> Option<AskedPayload> {
        self.states.remove(&terminal).map(|state| state.payload)
    }

    #[cfg(test)]
    pub(crate) fn current(&self, terminal: TerminalId) -> Option<&AskedPayload> {
        self.states.get(&terminal).map(|state| &state.payload)
    }
}

#[cfg(test)]
mod tests {
    use phux_core::ids::TerminalId;

    use super::{AskedDetector, AskedPayload, AskedSource, AskedTransition};

    fn payload(id: &str, question: &str) -> AskedPayload {
        AskedPayload {
            id: id.to_owned(),
            question: question.to_owned(),
            suggestions: vec!["yes".to_owned(), "no".to_owned()],
            elapsed_seconds: None,
        }
    }

    #[test]
    fn hook_wins_over_scrape() {
        let terminal = TerminalId::default();
        let mut detector = AskedDetector::default();
        assert!(matches!(
            detector.report(
                terminal,
                AskedSource::Scrape,
                payload("scrape", "Continue?")
            ),
            AskedTransition::Entered(_)
        ));
        assert!(matches!(
            detector.report(terminal, AskedSource::Hook, payload("hook", "Approve?")),
            AskedTransition::Updated(_)
        ));
        assert_eq!(detector.current(terminal).unwrap().id, "hook");
        assert_eq!(
            detector.report(
                terminal,
                AskedSource::Scrape,
                payload("scrape-2", "Still waiting?")
            ),
            AskedTransition::Ignored
        );
        assert_eq!(detector.current(terminal).unwrap().id, "hook");
    }

    #[test]
    fn clear_terminal_drops_pending_ask() {
        let terminal = TerminalId::default();
        let mut detector = AskedDetector::default();
        detector.report(terminal, AskedSource::Hook, payload("hook", "Approve?"));
        assert!(detector.current(terminal).is_some());
        let cleared = detector.clear_terminal(terminal).unwrap();
        assert_eq!(cleared.id, "hook");
        assert!(detector.current(terminal).is_none());
    }
}
