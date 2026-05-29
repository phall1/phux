//! Structured session-list projection — the `phux ls --json` read shape
//! (ADR-0022 §"stable CLI+JSON contract").
//!
//! A [`SessionListJson`] is the stable, versioned JSON the CLI emits for
//! `phux ls --json`. It is a plain-data projection of the per-session fields
//! a caller needs to enumerate sessions: name, window count, and whether any
//! client is attached. Richer per-session detail (creation time, ids, window
//! layout) is a future additive field, not a new struct — mirroring how
//! [`crate::screen::ScreenState`] reserves `--cells`/`--scrollback` growth.
//!
//! This type lives in `phux-core` (not the binary) so the shape has a single
//! documented, testable home shared with the rest of the JSON contract. The
//! mapping *into* it from the wire `SessionInfo` happens in the binary, where
//! both the protocol type and this one are in scope — `phux-core` deliberately
//! does not depend on `phux-protocol`.

use serde::{Deserialize, Serialize};

/// Stable JSON contract version for [`SessionListJson`] (ADR-0022). Bump on
/// any breaking change to the shape so consumers can pin or branch.
///
/// Tracked independently of [`crate::screen::SCHEMA_VERSION`] because the two
/// contracts (`phux snapshot --json` vs `phux ls --json`) evolve separately.
pub const LS_SCHEMA_VERSION: u32 = 1;

/// One session's entry in the [`SessionListJson`] output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionJson {
    /// Session name (what `phux attach <name>` matches against).
    pub name: String,
    /// Number of windows in the session.
    pub windows: u16,
    /// Whether at least one client is currently attached.
    pub attached: bool,
}

/// The `phux ls --json` payload: a versioned list of sessions.
///
/// Sessions are emitted in the same name-sorted order as the human
/// `phux ls` text so the two views stay consistent and the JSON is stable
/// across runs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionListJson {
    /// Contract version; see [`LS_SCHEMA_VERSION`].
    pub schema_version: u32,
    /// Sessions, sorted by name.
    pub sessions: Vec<SessionJson>,
}

impl SessionListJson {
    /// Wrap an already-sorted list of [`SessionJson`] entries, stamping the
    /// current [`LS_SCHEMA_VERSION`].
    ///
    /// Callers are responsible for the name-sort (the binary mirrors
    /// `print_sessions`); this constructor does not reorder.
    #[must_use]
    pub const fn new(sessions: Vec<SessionJson>) -> Self {
        Self {
            schema_version: LS_SCHEMA_VERSION,
            sessions,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{LS_SCHEMA_VERSION, SessionJson, SessionListJson};

    #[test]
    fn new_stamps_schema_version_and_keeps_order() {
        let list = SessionListJson::new(vec![
            SessionJson {
                name: "alpha".to_owned(),
                windows: 2,
                attached: true,
            },
            SessionJson {
                name: "beta".to_owned(),
                windows: 1,
                attached: false,
            },
        ]);

        assert_eq!(list.schema_version, LS_SCHEMA_VERSION);
        assert_eq!(list.sessions.len(), 2);
        // Order is preserved as given (caller sorts).
        assert_eq!(list.sessions[0].name, "alpha");
        assert_eq!(list.sessions[1].name, "beta");
    }

    #[test]
    fn serializes_to_stable_json_shape() {
        let list = SessionListJson::new(vec![SessionJson {
            name: "work".to_owned(),
            windows: 3,
            attached: true,
        }]);

        let json = serde_json::to_value(&list).expect("serialize");
        assert_eq!(json["schema_version"], 1);
        assert_eq!(json["sessions"][0]["name"], "work");
        assert_eq!(json["sessions"][0]["windows"], 3);
        assert_eq!(json["sessions"][0]["attached"], true);
    }
}
