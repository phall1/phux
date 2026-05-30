//! Integration tests for `phux_config::keybind`.

use std::collections::BTreeMap;

use phux_config::keybind::{
    Feed, KeyChord, KeybindError, Resolver, parse_chord, parse_chord_sequence,
};
use phux_config::{Action, KeybindingsCfg};
use phux_protocol::input::key::{ModSet, PhysicalKey};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn cfg(prefix: &str, prefix_table: &[(&str, &str)], global: &[(&str, &str)]) -> KeybindingsCfg {
    let mk_table = |entries: &[(&str, &str)]| -> BTreeMap<String, Action> {
        entries
            .iter()
            .map(|(k, v)| ((*k).to_owned(), Action::Bare((*v).to_owned())))
            .collect()
    };
    KeybindingsCfg {
        prefix: prefix.to_owned(),
        prefix_table: mk_table(prefix_table),
        global: mk_table(global),
    }
}

const fn chord(mods: ModSet, key: PhysicalKey) -> KeyChord {
    KeyChord {
        modifiers: mods,
        key,
    }
}

// ---------------------------------------------------------------------------
// parse_chord
// ---------------------------------------------------------------------------

#[test]
fn parse_chord_plain_lowercase_letter() {
    let c = parse_chord("a").unwrap();
    assert_eq!(c, chord(ModSet::empty(), PhysicalKey::A));
}

#[test]
fn parse_chord_bare_uppercase_implies_shift() {
    // "A" parses identically to "S-a".
    let bare = parse_chord("A").unwrap();
    let explicit = parse_chord("S-a").unwrap();
    assert_eq!(bare, explicit);
    assert_eq!(bare, chord(ModSet::SHIFT, PhysicalKey::A));
}

#[test]
fn parse_chord_ctrl_c() {
    let c = parse_chord("C-c").unwrap();
    assert_eq!(c, chord(ModSet::CTRL, PhysicalKey::C));
}

#[test]
fn parse_chord_meta_shift_tab() {
    let c = parse_chord("M-S-Tab").unwrap();
    assert_eq!(c, chord(ModSet::ALT | ModSet::SHIFT, PhysicalKey::Tab));
}

#[test]
fn parse_chord_function_key_f1_and_f12() {
    let f1 = parse_chord("F1").unwrap();
    let f12 = parse_chord("F12").unwrap();
    assert_eq!(f1, chord(ModSet::empty(), PhysicalKey::F1));
    assert_eq!(f12, chord(ModSet::empty(), PhysicalKey::F12));
}

#[test]
fn parse_chord_esc_and_escape_are_aliases() {
    let a = parse_chord("Esc").unwrap();
    let b = parse_chord("Escape").unwrap();
    assert_eq!(a, b);
    assert_eq!(a, chord(ModSet::empty(), PhysicalKey::Escape));
}

#[test]
fn parse_chord_backtab_implies_shift() {
    // BackTab is conventionally Shift+Tab.
    let c = parse_chord("BackTab").unwrap();
    assert_eq!(c, chord(ModSet::SHIFT, PhysicalKey::Tab));
}

#[test]
fn parse_chord_alt_alias_for_meta() {
    let m = parse_chord("M-x").unwrap();
    let a = parse_chord("A-x").unwrap();
    assert_eq!(m, a);
}

// ---------------------------------------------------------------------------
// parse_chord_sequence
// ---------------------------------------------------------------------------

#[test]
fn parse_chord_sequence_two_chords() {
    let seq = parse_chord_sequence("C-b c").unwrap();
    assert_eq!(seq.0.len(), 2);
    assert_eq!(seq.0[0], chord(ModSet::CTRL, PhysicalKey::B));
    assert_eq!(seq.0[1], chord(ModSet::empty(), PhysicalKey::C));
}

#[test]
fn parse_chord_sequence_single_chord() {
    let seq = parse_chord_sequence("C-c").unwrap();
    assert_eq!(seq.0.len(), 1);
    assert_eq!(seq.0[0], chord(ModSet::CTRL, PhysicalKey::C));
}

// ---------------------------------------------------------------------------
// Error cases
// ---------------------------------------------------------------------------

#[test]
fn parse_chord_empty_string_errors() {
    let err = parse_chord("").unwrap_err();
    assert!(
        matches!(err, KeybindError::Syntax { pos: 0, .. }),
        "expected Syntax {{ pos: 0, .. }}, got {err:?}"
    );
}

#[test]
fn parse_chord_unknown_key_errors() {
    let err = parse_chord("NotAKey").unwrap_err();
    assert!(
        matches!(err, KeybindError::UnknownKey(ref s) if s == "NotAKey"),
        "got {err:?}"
    );
}

#[test]
fn parse_chord_trailing_dash_errors() {
    // "C-" — modifier with nothing after. split_modifier returns None
    // because there's no text past the dash, so the loop breaks and the
    // remaining "C-" is treated as a key token (which fails as unknown).
    // Either way, we get a parse error.
    let err = parse_chord("C-").unwrap_err();
    assert!(matches!(
        err,
        KeybindError::Syntax { .. } | KeybindError::UnknownKey(_)
    ));
}

#[test]
fn parse_chord_sequence_empty_errors() {
    let err = parse_chord_sequence("").unwrap_err();
    assert!(matches!(err, KeybindError::Syntax { pos: 0, .. }));
}

// ---------------------------------------------------------------------------
// Resolver
// ---------------------------------------------------------------------------

#[test]
fn resolver_builds_from_three_bindings() {
    let c = cfg(
        "C-b",
        &[("c", "new-window"), ("d", "detach")],
        &[("M-q", "quit")],
    );
    let _r = Resolver::new(&c).expect("resolver builds");
}

#[test]
fn shipped_default_keybindings_build_a_resolver() {
    // Regression guard: every chord in the embedded default.toml must
    // parse and the prefix table must be unambiguous (phux-4li.18 added
    // the window bindings o/;/c/n/p/&/0-9/, alongside the pane ones).
    let cfg = phux_config::parse_with_defaults("", std::path::Path::new("<embedded default.toml>"))
        .expect("default config parses");
    Resolver::new(&cfg.keybindings).expect("default keybindings build a resolver");
}

#[test]
fn resolver_rejects_ambiguous_prefix_binding() {
    // A global "C-b" binding plus a prefix table makes the prefix chord
    // ambiguous: feeding "C-b" would both resolve the global AND open the
    // prefix table.
    let c = cfg(
        "C-b",
        &[("c", "new-window")],
        &[("C-b", "global-prefix-action")],
    );
    let err = Resolver::new(&c).unwrap_err();
    assert!(
        matches!(err, KeybindError::AmbiguousPrefix(ref s) if s == "C-b"),
        "got {err:?}"
    );
}

#[test]
fn resolver_walks_prefix_then_table_key() {
    let c = cfg("C-b", &[("c", "new-window"), ("d", "detach")], &[]);
    let mut r = Resolver::new(&c).unwrap();

    // First chord — prefix — should yield Partial.
    let f1 = r.feed(chord(ModSet::CTRL, PhysicalKey::B));
    assert_eq!(f1, Feed::Partial);

    // Second chord — 'c' — should resolve to "new-window".
    match r.feed(chord(ModSet::empty(), PhysicalKey::C)) {
        Feed::Resolved(action) => {
            assert_eq!(action.action, "new-window");
            assert!(action.args.is_empty());
        }
        other => panic!("expected Resolved, got {other:?}"),
    }

    // Now from a clean state, walk to 'd' → "detach".
    assert_eq!(r.feed(chord(ModSet::CTRL, PhysicalKey::B)), Feed::Partial);
    match r.feed(chord(ModSet::empty(), PhysicalKey::D)) {
        Feed::Resolved(action) => assert_eq!(action.action, "detach"),
        other => panic!("expected Resolved, got {other:?}"),
    }
}

#[test]
fn resolver_resolves_global_in_one_chord() {
    let c = cfg("C-b", &[], &[("M-q", "quit")]);
    let mut r = Resolver::new(&c).unwrap();

    match r.feed(chord(ModSet::ALT, PhysicalKey::Q)) {
        Feed::Resolved(a) => assert_eq!(a.action, "quit"),
        other => panic!("expected Resolved, got {other:?}"),
    }
}

#[test]
fn resolver_returns_nomatch_on_unrecognized_chord() {
    let c = cfg("C-b", &[("c", "new-window")], &[]);
    let mut r = Resolver::new(&c).unwrap();

    // An unrelated chord with no current state — should be NoMatch.
    let f = r.feed(chord(ModSet::ALT, PhysicalKey::Z));
    assert_eq!(f, Feed::NoMatch);
}

#[test]
fn resolver_partial_then_nomatch_resets() {
    let c = cfg("C-b", &[("c", "new-window")], &[]);
    let mut r = Resolver::new(&c).unwrap();

    // Walk into the prefix table.
    assert_eq!(r.feed(chord(ModSet::CTRL, PhysicalKey::B)), Feed::Partial);
    // Feed a chord that's not in the table — should be NoMatch and reset.
    assert_eq!(
        r.feed(chord(ModSet::empty(), PhysicalKey::X)),
        Feed::NoMatch
    );
    // After NoMatch, a fresh prefix walk works again.
    assert_eq!(r.feed(chord(ModSet::CTRL, PhysicalKey::B)), Feed::Partial);
}

#[test]
fn resolver_reset_clears_partial() {
    let c = cfg("C-b", &[("c", "new-window")], &[]);
    let mut r = Resolver::new(&c).unwrap();

    assert_eq!(r.feed(chord(ModSet::CTRL, PhysicalKey::B)), Feed::Partial);
    r.reset();
    // 'c' on its own (without prefix) is not registered as a binding, so
    // NoMatch.
    assert_eq!(
        r.feed(chord(ModSet::empty(), PhysicalKey::C)),
        Feed::NoMatch
    );
}

// ---------------------------------------------------------------------------
// Snapshot
// ---------------------------------------------------------------------------

#[test]
fn resolver_debug_snapshot_representative_config() {
    let c = cfg(
        "C-b",
        &[("c", "new-window"), ("d", "detach")],
        &[("M-q", "quit")],
    );
    let r = Resolver::new(&c).unwrap();
    insta::assert_debug_snapshot!("resolver_representative", r);
}
