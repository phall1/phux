//! Integration tests for `phux_config::widget`.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use phux_config::WidgetSpec;
use phux_config::widget::{
    CellStyle, SessionNameWidget, StatusWidget, TimeWidget, WidgetCells, WidgetContext,
    WidgetError, WidgetRegistry, WindowInfo,
};

fn opts_with(entries: &[(&str, toml::Value)]) -> BTreeMap<String, toml::Value> {
    entries
        .iter()
        .map(|(k, v)| ((*k).to_owned(), v.clone()))
        .collect()
}

fn fixed_time() -> SystemTime {
    // Avoid local-timezone variability in time-widget snapshot tests by
    // using the `session-name` widget for snapshots and only asserting
    // shape (not contents) on the time widget.
    UNIX_EPOCH + Duration::from_secs(12345)
}

// ---------------------------------------------------------------------------
// Registry construction
// ---------------------------------------------------------------------------

#[test]
fn with_builtins_registers_time_and_session_name() {
    let r = WidgetRegistry::with_builtins();
    let kinds = r.kinds();
    assert!(kinds.contains(&"time"), "missing time: {kinds:?}");
    assert!(
        kinds.contains(&"session-name"),
        "missing session-name: {kinds:?}"
    );
}

#[test]
fn new_starts_empty() {
    let r = WidgetRegistry::new();
    assert!(r.kinds().is_empty());
}

#[test]
fn register_then_build_invokes_factory() {
    #[allow(clippy::unnecessary_wraps)] // factory signature is fixed
    fn dummy_factory(
        _opts: &BTreeMap<String, toml::Value>,
    ) -> Result<Box<dyn StatusWidget>, WidgetError> {
        Ok(Box::new(SessionNameWidget::new(
            Some("X:".to_owned()),
            None,
        )))
    }
    let mut r = WidgetRegistry::new();
    r.register("custom", dummy_factory);
    let spec = WidgetSpec {
        kind: "custom".to_owned(),
        opts: BTreeMap::new(),
    };
    let w = r.build(&spec).expect("custom builds");
    let cells = w.render(&WidgetContext {
        now: fixed_time(),
        session_name: "main",
        windows: &[],
    });
    let chars: String = cells.cells.iter().filter_map(|c| c.text.first()).collect();
    assert_eq!(chars, "X:main");
}

// ---------------------------------------------------------------------------
// session-name widget
// ---------------------------------------------------------------------------

#[test]
fn session_name_renders_prefix_and_truncated_name() {
    let r = WidgetRegistry::with_builtins();
    let spec = WidgetSpec {
        kind: "session-name".to_owned(),
        opts: opts_with(&[
            ("prefix", toml::Value::String("[sess]".to_owned())),
            ("max-len", toml::Value::Integer(4)),
        ]),
    };
    let w = r.build(&spec).expect("session-name builds");
    let cells = w.render(&WidgetContext {
        now: fixed_time(),
        session_name: "very-long-session-name",
        windows: &[],
    });
    let chars: String = cells.cells.iter().filter_map(|c| c.text.first()).collect();
    assert_eq!(chars, "[sess]very");
}

#[test]
fn session_name_max_len_accepts_snake_case_alias() {
    let r = WidgetRegistry::with_builtins();
    let spec = WidgetSpec {
        kind: "session-name".to_owned(),
        opts: opts_with(&[("max_len", toml::Value::Integer(3))]),
    };
    let w = r.build(&spec).unwrap();
    let cells = w.render(&WidgetContext {
        now: fixed_time(),
        session_name: "abcdef",
        windows: &[],
    });
    let chars: String = cells.cells.iter().filter_map(|c| c.text.first()).collect();
    assert_eq!(chars, "abc");
}

#[test]
fn session_name_no_options_renders_full_name() {
    let w = SessionNameWidget::new(None, None);
    let cells = w.render(&WidgetContext {
        now: fixed_time(),
        session_name: "main",
        windows: &[],
    });
    let chars: String = cells.cells.iter().filter_map(|c| c.text.first()).collect();
    assert_eq!(chars, "main");
}

#[test]
fn session_name_rejects_zero_max_len() {
    let r = WidgetRegistry::with_builtins();
    let spec = WidgetSpec {
        kind: "session-name".to_owned(),
        opts: opts_with(&[("max-len", toml::Value::Integer(0))]),
    };
    match r.build(&spec) {
        Err(WidgetError::InvalidOption { kind, .. }) => assert_eq!(kind, "session-name"),
        other => panic!("expected InvalidOption, got {other:?}"),
    }
}

#[test]
fn session_name_rejects_non_integer_max_len() {
    let r = WidgetRegistry::with_builtins();
    let spec = WidgetSpec {
        kind: "session-name".to_owned(),
        opts: opts_with(&[("max-len", toml::Value::String("ten".to_owned()))]),
    };
    assert!(matches!(
        r.build(&spec),
        Err(WidgetError::InvalidOption { .. })
    ));
}

// ---------------------------------------------------------------------------
// time widget
// ---------------------------------------------------------------------------

#[test]
fn time_widget_default_format_renders_h_m() {
    let r = WidgetRegistry::with_builtins();
    let spec = WidgetSpec {
        kind: "time".to_owned(),
        opts: BTreeMap::new(),
    };
    let w = r.build(&spec).expect("time builds");
    let cells = w.render(&WidgetContext {
        now: fixed_time(),
        session_name: "",
        windows: &[],
    });
    // Default %H:%M renders to 5 chars (HH:MM) in any locale.
    assert_eq!(
        cells.cells.len(),
        5,
        "expected 5 chars (HH:MM), got {}: {:?}",
        cells.cells.len(),
        cells
            .cells
            .iter()
            .filter_map(|c| c.text.first())
            .collect::<String>()
    );
}

#[test]
fn time_widget_explicit_format_uses_format_string() {
    let r = WidgetRegistry::with_builtins();
    let spec = WidgetSpec {
        kind: "time".to_owned(),
        opts: opts_with(&[("format", toml::Value::String("%Y".to_owned()))]),
    };
    let w = r.build(&spec).expect("time builds");
    let cells = w.render(&WidgetContext {
        now: fixed_time(),
        session_name: "",
        windows: &[],
    });
    // %Y is a 4-digit year.
    assert_eq!(cells.cells.len(), 4);
}

#[test]
fn time_widget_poll_interval_is_one_second() {
    let w = TimeWidget::new("%H:%M").expect("valid format");
    assert_eq!(w.poll_interval(), Some(Duration::from_secs(1)));
}

#[test]
fn time_widget_rejects_invalid_format() {
    let r = WidgetRegistry::with_builtins();
    let spec = WidgetSpec {
        kind: "time".to_owned(),
        opts: opts_with(&[("format", toml::Value::String("%Q".to_owned()))]),
    };
    // %Q is not a valid strftime directive — must be rejected at build time.
    match r.build(&spec) {
        Err(WidgetError::InvalidOption { kind, .. }) => assert_eq!(kind, "time"),
        other => panic!("expected InvalidOption, got {other:?}"),
    }
}

#[test]
fn time_widget_rejects_non_string_format() {
    let r = WidgetRegistry::with_builtins();
    let spec = WidgetSpec {
        kind: "time".to_owned(),
        opts: opts_with(&[("format", toml::Value::Integer(42))]),
    };
    assert!(matches!(
        r.build(&spec),
        Err(WidgetError::InvalidOption { .. })
    ));
}

// ---------------------------------------------------------------------------
// Unknown kind
// ---------------------------------------------------------------------------

#[test]
fn unknown_kind_returns_unknown_kind_error() {
    let r = WidgetRegistry::with_builtins();
    let spec = WidgetSpec {
        kind: "not-a-real-widget".to_owned(),
        opts: BTreeMap::new(),
    };
    match r.build(&spec) {
        Err(WidgetError::UnknownKind(k)) => assert_eq!(k, "not-a-real-widget"),
        other => panic!("expected UnknownKind, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// WidgetCells helpers
// ---------------------------------------------------------------------------

#[test]
fn widget_cells_from_text_one_cell_per_char() {
    let cells = WidgetCells::from_text("hi");
    assert_eq!(cells.len(), 2);
    assert!(!cells.is_empty());
}

#[test]
fn widget_cells_empty() {
    let cells = WidgetCells::from_text("");
    assert!(cells.is_empty());
    assert_eq!(cells.len(), 0);
}

// ---------------------------------------------------------------------------
// windows (tab-bar) widget
// ---------------------------------------------------------------------------

fn win(name: &str, active: bool) -> WindowInfo {
    WindowInfo {
        name: name.to_owned(),
        active,
    }
}

fn render_windows(opts: &[(&str, toml::Value)], windows: &[WindowInfo]) -> WidgetCells {
    let spec = WidgetSpec {
        kind: "windows".to_owned(),
        opts: opts_with(opts),
    };
    let w = WidgetRegistry::with_builtins()
        .build(&spec)
        .expect("windows builds");
    w.render(&WidgetContext {
        now: fixed_time(),
        session_name: "",
        windows,
    })
}

fn text_of(cells: &WidgetCells) -> String {
    cells.cells.iter().filter_map(|c| c.text.first()).collect()
}

fn style_table(entries: &[(&str, toml::Value)]) -> toml::Value {
    let mut t = toml::value::Table::new();
    for (k, v) in entries {
        t.insert((*k).to_owned(), v.clone());
    }
    toml::Value::Table(t)
}

#[test]
fn windows_widget_registered_in_builtins() {
    assert!(WidgetRegistry::with_builtins().kinds().contains(&"windows"));
}

#[test]
fn windows_widget_default_format_and_separator() {
    let cells = render_windows(&[], &[win("a", true), win("b", false)]);
    assert_eq!(text_of(&cells), "0:a 1:b");
}

#[test]
fn windows_widget_active_and_inactive_styles_differ() {
    // Default preset: active = bold+reverse, inactive = dim.
    let cells = render_windows(&[], &[win("a", true), win("b", false)]);
    // First cell ("0") is part of the active segment.
    let active_style = cells.cells[0].style.clone().expect("active styled");
    assert!(active_style.bold && active_style.reverse);
    // The "b" cell belongs to the inactive segment "1:b" — find it.
    let b_cell = cells
        .cells
        .iter()
        .find(|c| c.text.first() == Some(&'b'))
        .expect("b cell");
    let inactive_style = b_cell.style.clone().expect("inactive styled");
    assert!(inactive_style.dim && !inactive_style.reverse);
}

#[test]
fn windows_widget_custom_format_and_separator() {
    let cells = render_windows(
        &[
            ("format", toml::Value::String("{name}".to_owned())),
            ("separator", toml::Value::String(" | ".to_owned())),
        ],
        &[win("edit", true), win("logs", false)],
    );
    assert_eq!(text_of(&cells), "edit | logs");
}

#[test]
fn windows_widget_custom_style_parses() {
    let cells = render_windows(
        &[(
            "active",
            style_table(&[
                ("fg", toml::Value::String("green".to_owned())),
                ("bold", toml::Value::Boolean(true)),
            ]),
        )],
        &[win("a", true)],
    );
    let style = cells.cells[0].style.clone().expect("active styled");
    assert_eq!(style.fg.as_deref(), Some("green"));
    assert!(style.bold);
}

#[test]
fn windows_widget_rejects_non_table_style() {
    let spec = WidgetSpec {
        kind: "windows".to_owned(),
        opts: opts_with(&[("active", toml::Value::String("nope".to_owned()))]),
    };
    let err = WidgetRegistry::with_builtins()
        .build(&spec)
        .expect_err("non-table style rejected");
    assert!(matches!(err, WidgetError::InvalidOption { .. }));
}

#[test]
fn windows_widget_rejects_unknown_style_field() {
    let spec = WidgetSpec {
        kind: "windows".to_owned(),
        opts: opts_with(&[(
            "inactive",
            style_table(&[("colour", toml::Value::String("red".to_owned()))]),
        )]),
    };
    let err = WidgetRegistry::with_builtins()
        .build(&spec)
        .expect_err("unknown style field rejected");
    assert!(matches!(err, WidgetError::InvalidOption { .. }));
}

#[test]
fn windows_widget_empty_list_renders_nothing() {
    let cells = render_windows(&[], &[]);
    assert!(cells.is_empty());
}

#[test]
fn cell_style_is_plain_detects_default() {
    assert!(CellStyle::default().is_plain());
    assert!(
        !CellStyle {
            bold: true,
            ..CellStyle::default()
        }
        .is_plain()
    );
}
