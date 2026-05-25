//! `session-name` widget — renders the current session name, optionally
//! with a prefix and length cap.

use std::collections::BTreeMap;

use crate::widget::{StatusWidget, WidgetCells, WidgetContext, WidgetError};

/// Widget kind, used in error messages.
const KIND: &str = "session-name";

/// `session-name` widget.
///
/// Output is `prefix + truncate(session_name, max_len)`. If `max_len`
/// is set and the session name exceeds it, the name is truncated to
/// `max_len` characters (no ellipsis — keep cell math simple at this
/// stage; callers that want one can opt in via a future option).
#[derive(Debug, Clone)]
pub struct SessionNameWidget {
    /// Optional literal prefix prepended verbatim to the session name.
    pub prefix: Option<String>,
    /// Maximum displayed `char` count of the session name itself
    /// (prefix not counted). `None` ⇒ unbounded.
    pub max_len: Option<usize>,
}

impl SessionNameWidget {
    /// Construct a `SessionNameWidget`.
    #[must_use]
    pub const fn new(prefix: Option<String>, max_len: Option<usize>) -> Self {
        Self { prefix, max_len }
    }
}

impl StatusWidget for SessionNameWidget {
    fn render(&self, ctx: &WidgetContext<'_>) -> WidgetCells {
        let truncated: String = self.max_len.map_or_else(
            || ctx.session_name.to_owned(),
            |n| ctx.session_name.chars().take(n).collect(),
        );
        let mut out =
            String::with_capacity(self.prefix.as_deref().map_or(0, str::len) + truncated.len());
        if let Some(p) = &self.prefix {
            out.push_str(p);
        }
        out.push_str(&truncated);
        WidgetCells::from_text(&out)
    }

    // No `poll_interval` — session-name repaints are event-driven
    // (status bar redraws when the active session changes).
}

/// Factory: builds a [`SessionNameWidget`] from a TOML `opts` map.
///
/// Accepted keys:
/// - `prefix` (string, optional) — literal prefix prepended to the name.
/// - `max-len` (integer, optional, `> 0`) — truncate the name to this
///   many characters. The kebab-case spelling matches the rest of the
///   schema; `max_len` is also accepted for ergonomics.
///
/// # Errors
///
/// Returns [`WidgetError::InvalidOption`] if any value is the wrong type
/// or `max-len` is `<= 0`.
pub(in crate::widget) fn factory(
    opts: &BTreeMap<String, toml::Value>,
) -> Result<Box<dyn StatusWidget>, WidgetError> {
    let prefix = match opts.get("prefix") {
        None => None,
        Some(toml::Value::String(s)) => Some(s.clone()),
        Some(other) => {
            return Err(WidgetError::InvalidOption {
                kind: KIND.to_owned(),
                message: format!("`prefix` must be a string, got {}", other.type_str()),
            });
        }
    };
    let raw_len = opts.get("max-len").or_else(|| opts.get("max_len"));
    let max_len = match raw_len {
        None => None,
        Some(toml::Value::Integer(n)) if *n > 0 => {
            Some(usize::try_from(*n).map_err(|_| WidgetError::InvalidOption {
                kind: KIND.to_owned(),
                message: format!("`max-len` does not fit in usize: {n}"),
            })?)
        }
        Some(toml::Value::Integer(n)) => {
            return Err(WidgetError::InvalidOption {
                kind: KIND.to_owned(),
                message: format!("`max-len` must be > 0, got {n}"),
            });
        }
        Some(other) => {
            return Err(WidgetError::InvalidOption {
                kind: KIND.to_owned(),
                message: format!("`max-len` must be an integer, got {}", other.type_str()),
            });
        }
    };
    Ok(Box::new(SessionNameWidget::new(prefix, max_len)))
}
