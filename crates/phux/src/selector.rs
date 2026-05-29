//! Client-side selector parsing and resolution (phux-3kj, ADR-0021).
//!
//! The CLI's `TARGET` grammar (`docs/consumers/tui.md` §3) names sessions,
//! windows, and panes — none of which are wire concepts (ADR-0017). Per
//! [ADR-0021](../../../ADR/0021-control-plane-commands.md) selectors are
//! therefore resolved **client-side** against a `GET_STATE` snapshot: a
//! selector resolves to a concrete set of [`TerminalId`]s, and only those
//! Terminal-scoped ids are sent back to the server (e.g. one
//! `KILL_TERMINAL` per resolved Terminal). The server never parses a
//! selector and never learns the words "session" or "window".
//!
//! Grammar:
//!
//! | Form        | Meaning                                             |
//! |-------------|-----------------------------------------------------|
//! | `.`         | the focused session (snapshot's `focused_session`)  |
//! | `=`         | last / most-recently-focused (same as `.` here)     |
//! | `name`      | a session by name                                   |
//! | `name:N`    | window index `N` of session `name`                  |
//! | `name:tag`  | window whose name is `tag` in session `name`        |
//! | `name:N.M`  | pane index `M` of window `N` of session `name`      |
//! | `@N`        | an opaque local Terminal id (`TerminalId::local(N)`) |

use phux_protocol::ids::TerminalId;
use phux_protocol::wire::info::SessionSnapshot;

/// A parsed selector. Resolution happens later against a snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Selector {
    /// `.` — the focused session.
    Current,
    /// `=` — the last / most-recently-focused session.
    Last,
    /// `name` — a whole session.
    Session(String),
    /// `name:N` or `name:tag` — one window of a session.
    Window(String, WindowRef),
    /// `name:N.M` or `name:tag.M` — one pane of one window.
    Pane(String, WindowRef, u16),
    /// `@N` — a Terminal addressed directly by its local wire id.
    TerminalId(u32),
}

/// How a window is addressed within a session: by numeric index or by name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum WindowRef {
    /// `N` — position in the session's windows list.
    Index(u16),
    /// `tag` — the window's name.
    Tag(String),
}

/// Why a selector string could not be parsed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ParseError {
    /// The selector was empty.
    Empty,
    /// `@N` carried a non-numeric or out-of-range id.
    BadTerminalId(String),
    /// A pane index `M` (after the `.`) was non-numeric or out of range.
    BadPaneIndex(String),
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => write!(f, "empty selector"),
            Self::BadTerminalId(s) => write!(f, "invalid terminal id in '@{s}'"),
            Self::BadPaneIndex(s) => write!(f, "invalid pane index '{s}'"),
        }
    }
}

impl std::error::Error for ParseError {}

/// Parse one `TARGET` string into a [`Selector`].
///
/// # Errors
///
/// Returns [`ParseError`] for an empty selector or a malformed numeric
/// component (`@N` id, pane index).
pub(crate) fn parse(raw: &str) -> Result<Selector, ParseError> {
    if raw.is_empty() {
        return Err(ParseError::Empty);
    }
    if raw == "." {
        return Ok(Selector::Current);
    }
    if raw == "=" {
        return Ok(Selector::Last);
    }
    if let Some(rest) = raw.strip_prefix('@') {
        let id = rest
            .parse::<u32>()
            .map_err(|_| ParseError::BadTerminalId(rest.to_owned()))?;
        return Ok(Selector::TerminalId(id));
    }

    // `name`, `name:window`, or `name:window.pane`.
    let Some((name, locus)) = raw.split_once(':') else {
        return Ok(Selector::Session(raw.to_owned()));
    };
    let name = name.to_owned();

    // Split the window locus from an optional `.pane` suffix. Only the
    // FIRST `.` separates window from pane, so a window tag may itself
    // contain dots in a future grammar; today windows are `window-N`.
    if let Some((window_part, pane_part)) = locus.split_once('.') {
        let window = parse_window_ref(window_part);
        let pane = pane_part
            .parse::<u16>()
            .map_err(|_| ParseError::BadPaneIndex(pane_part.to_owned()))?;
        Ok(Selector::Pane(name, window, pane))
    } else {
        Ok(Selector::Window(name, parse_window_ref(locus)))
    }
}

/// A window locus is an [`WindowRef::Index`] when fully numeric, else a
/// [`WindowRef::Tag`].
fn parse_window_ref(part: &str) -> WindowRef {
    part.parse::<u16>()
        .map_or_else(|_| WindowRef::Tag(part.to_owned()), WindowRef::Index)
}

/// Resolve a parsed [`Selector`] against `snapshot` to the set of
/// [`TerminalId`]s it names. Returns an empty vec when the selector
/// matches nothing (e.g. an unknown session) — callers decide whether
/// that is an error (`kill` treats it as a selector miss).
#[must_use]
pub(crate) fn resolve(selector: &Selector, snapshot: &SessionSnapshot) -> Vec<TerminalId> {
    match selector {
        Selector::Current | Selector::Last => {
            terminals_in_session(snapshot, snapshot.focused_session)
        }
        Selector::Session(name) => session_id_by_name(snapshot, name)
            .map_or_else(Vec::new, |sid| terminals_in_session(snapshot, sid)),
        Selector::Window(name, window) => resolve_window(snapshot, name, window),
        Selector::Pane(name, window, pane_index) => {
            let panes = resolve_window(snapshot, name, window);
            panes
                .into_iter()
                .nth(*pane_index as usize)
                .into_iter()
                .collect()
        }
        Selector::TerminalId(id) => {
            let wanted = TerminalId::local(*id);
            if snapshot.panes.iter().any(|p| p.id == wanted) {
                vec![wanted]
            } else {
                Vec::new()
            }
        }
    }
}

/// All Terminals in `session`, across every window, in snapshot order.
fn terminals_in_session(
    snapshot: &SessionSnapshot,
    session: phux_protocol::ids::SessionId,
) -> Vec<TerminalId> {
    let window_ids: Vec<_> = snapshot
        .windows
        .iter()
        .filter(|w| w.session_id == session)
        .map(|w| w.id)
        .collect();
    snapshot
        .panes
        .iter()
        .filter(|p| window_ids.contains(&p.window_id))
        .map(|p| p.id.clone())
        .collect()
}

/// Terminals in the window `name`/`window` names, in snapshot order.
fn resolve_window(snapshot: &SessionSnapshot, name: &str, window: &WindowRef) -> Vec<TerminalId> {
    let Some(sid) = session_id_by_name(snapshot, name) else {
        return Vec::new();
    };
    let window_id = snapshot
        .windows
        .iter()
        .filter(|w| w.session_id == sid)
        .find(|w| match window {
            WindowRef::Index(n) => w.index == *n,
            WindowRef::Tag(tag) => w.name == *tag,
        })
        .map(|w| w.id);
    window_id.map_or_else(Vec::new, |wid| {
        snapshot
            .panes
            .iter()
            .filter(|p| p.window_id == wid)
            .map(|p| p.id.clone())
            .collect()
    })
}

fn session_id_by_name(
    snapshot: &SessionSnapshot,
    name: &str,
) -> Option<phux_protocol::ids::SessionId> {
    snapshot
        .sessions
        .iter()
        .find(|s| s.name == name)
        .map(|s| s.id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use phux_protocol::ids::{SessionId, WindowId};
    use phux_protocol::wire::info::{SessionInfo, TerminalInfo, WindowInfo};

    #[test]
    fn parse_session_window_pane_and_terminal_forms() {
        assert_eq!(parse(".").unwrap(), Selector::Current);
        assert_eq!(parse("=").unwrap(), Selector::Last);
        assert_eq!(parse("work").unwrap(), Selector::Session("work".to_owned()));
        assert_eq!(
            parse("work:1").unwrap(),
            Selector::Window("work".to_owned(), WindowRef::Index(1)),
        );
        assert_eq!(
            parse("work:editor").unwrap(),
            Selector::Window("work".to_owned(), WindowRef::Tag("editor".to_owned())),
        );
        assert_eq!(
            parse("work:1.2").unwrap(),
            Selector::Pane("work".to_owned(), WindowRef::Index(1), 2),
        );
        assert_eq!(parse("@42").unwrap(), Selector::TerminalId(42));
    }

    #[test]
    fn parse_rejects_empty_and_bad_numbers() {
        assert_eq!(parse(""), Err(ParseError::Empty));
        assert!(matches!(parse("@nope"), Err(ParseError::BadTerminalId(_))));
        assert!(matches!(
            parse("work:1.x"),
            Err(ParseError::BadPaneIndex(_))
        ));
    }

    /// Build a snapshot: session "work" (id 1) with two windows, each
    /// holding panes, plus a second session "play" (id 2).
    fn fixture() -> SessionSnapshot {
        let work = SessionId::new(1);
        let play = SessionId::new(2);
        let w0 = WindowId::new(10);
        let w1 = WindowId::new(11);
        let p0 = WindowId::new(20);
        let sessions = vec![
            SessionInfo::new(work, "work"),
            SessionInfo::new(play, "play"),
        ];
        let windows = vec![
            WindowInfo::new(w0, work, "shell").with_index(0),
            WindowInfo::new(w1, work, "editor").with_index(1),
            WindowInfo::new(p0, play, "shell").with_index(0),
        ];
        let panes = vec![
            TerminalInfo::new(TerminalId::local(100), w0, 80, 24),
            TerminalInfo::new(TerminalId::local(101), w1, 80, 24),
            TerminalInfo::new(TerminalId::local(102), w1, 80, 24),
            TerminalInfo::new(TerminalId::local(200), p0, 80, 24),
        ];
        SessionSnapshot::new(work, w0, TerminalId::local(100))
            .with_sessions(sessions)
            .with_windows(windows)
            .with_panes(panes)
    }

    #[test]
    fn resolve_session_returns_all_its_terminals() {
        let snap = fixture();
        let sel = parse("work").unwrap();
        let ids = resolve(&sel, &snap);
        assert_eq!(
            ids,
            vec![
                TerminalId::local(100),
                TerminalId::local(101),
                TerminalId::local(102),
            ],
        );
    }

    #[test]
    fn resolve_window_by_index_and_tag() {
        let snap = fixture();
        let by_index = resolve(&parse("work:1").unwrap(), &snap);
        let by_tag = resolve(&parse("work:editor").unwrap(), &snap);
        let expected = vec![TerminalId::local(101), TerminalId::local(102)];
        assert_eq!(by_index, expected);
        assert_eq!(by_tag, expected);
    }

    #[test]
    fn resolve_pane_picks_one_terminal() {
        let snap = fixture();
        let ids = resolve(&parse("work:1.1").unwrap(), &snap);
        assert_eq!(ids, vec![TerminalId::local(102)]);
    }

    #[test]
    fn resolve_terminal_id_and_focused_and_misses() {
        let snap = fixture();
        assert_eq!(
            resolve(&parse("@100").unwrap(), &snap),
            vec![TerminalId::local(100)],
        );
        // Unknown terminal id → empty.
        assert!(resolve(&parse("@999").unwrap(), &snap).is_empty());
        // Unknown session → empty.
        assert!(resolve(&parse("ghost").unwrap(), &snap).is_empty());
        // `.` resolves to the focused session ("work").
        assert_eq!(
            resolve(&Selector::Current, &snap),
            vec![
                TerminalId::local(100),
                TerminalId::local(101),
                TerminalId::local(102),
            ],
        );
    }
}
