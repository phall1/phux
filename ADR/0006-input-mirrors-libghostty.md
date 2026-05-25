# 0006 — Input event types mirror libghostty's API

Status: Accepted
Date: 2026-05-24

## Context

Server-side, phux feeds input events to libghostty's encoders
(`key::Encoder`, `mouse::Encoder`, `focus::Event::encode`,
`paste::encode`). The encoders take typed event structures and produce
the exact PTY bytes for whatever protocol the inner program currently
expects — KIP at any progressive-enhancement level, legacy fixterms,
SGR / SGR-Pixels mouse, etc.

Our first draft of `SPEC.md` §9 modeled inputs from the *application*
side: `Key = CHAR(u32) | NAMED(NamedKey)`, with the codepoint being the
layout-resolved character. This is the model an application receives
*after* the terminal has done its work. The diff spike caught the
mismatch: feeding this shape into libghostty's encoder requires lossy
translation, and KIP features (alternate-keys, modifier-only events,
side-discriminated modifiers) cannot round-trip correctly.

## Decision

Wire input event types mirror libghostty-vt's `key::Event`,
`mouse::Event`, `focus::Event`, and paste utilities one-to-one.

- `KeyEvent` carries `action`, `key: PhysicalKey` (W3C `code`-style),
  `mods` (with `*_SIDE` bits), `consumed_mods`, `composing`, `text`,
  `unshifted_codepoint`.
- `MouseEvent` carries `action`, `button`, `mods`, and `position` in
  pane-local surface pixels.
- `INPUT_FOCUS` is `{Gained,Lost}` per pane.
- `INPUT_PASTE` carries raw bytes plus a `trust` field; the server uses
  `libghostty_vt::paste::is_safe` and `paste::encode` per policy.

The numeric values of `PhysicalKey`, `MouseButton`, and `MouseAction`
match libghostty's enums verbatim so the wire ↔ libghostty mapping for
those is a field-for-field copy.

### Amendment (phux-6yl.2 finding, 2026-05-25)

The original draft of this ADR overgeneralized: it claimed *all* the
mirrored enums (including `KeyAction` and `ModSet`/`Mods`) share
discriminants with libghostty. The phux-6yl.2 implementation pass
discovered this is not true at the live pinned libghostty rev
(`31d1f70`):

| Type        | phux wire             | libghostty `key::*`   |
|-------------|-----------------------|-----------------------|
| `KeyAction` | `Press=0, Release=1`  | `Release=0, Press=1`  |
| `Mods`      | `CTRL=2, ALT=4`       | `ALT=2, CTRL=4`       |

We **deliberately keep the phux wire discriminants stable** and do a
semantic remap inside the server-side `*_to_libghostty` conversion
functions. The wire format is canonical; libghostty is a backend whose
ABI may shift. The discriminant-pin tests in `phux-server/src/input/`
assert the *semantic* mapping (`KeyAction::Press` maps to libghostty's
press value) rather than numeric equality.

`PhysicalKey`, `MouseButton`, and `MouseAction` discriminants *do*
match libghostty's verbatim — those conversions remain mechanical
casts. The contract is: phux-protocol pins the wire bytes; phux-server
owns the libghostty-bridge layer and absorbs any future upstream ABI
churn there.

### Amendment (Rust orphan rules)

The original draft wrote `impl From<&phux_protocol::input::KeyEvent>
for libghostty_vt::key::Event`. Rust's orphan rules forbid this: both
types are foreign to `phux-server`. The actual surface is free
functions in `phux_server::input::*` named `*_to_libghostty`. The
intent (field-for-field, infallible) is unchanged.

## Rationale

- **Round-trip fidelity.** Wire events translate to libghostty events
  with no semantic loss. Whatever the encoder can produce, phux can
  carry.
- **Encoder options stay server-local.** Cursor-key application mode,
  keypad mode, modifyOtherKeys, KIP flags, alt-esc-prefix, backarrow,
  macos-option-as-alt — none of this traverses the wire. The server
  has the `Terminal` and calls
  `Encoder::set_options_from_terminal(&terminal)` before each encode.
  Per-pane encoder state is private to the server.
- **Future-aligned.** If libghostty adds a new physical key, a new
  encoder flag, or refines KIP support, we adopt it server-side without
  protocol changes. The wire format is upstream-shaped, so upstream
  evolution lands without versioning friction.
- **GUI clients are first-class.** A native libghostty-surface client
  produces `key::Event` values natively from the OS; they map to wire
  `KeyEvent` field-for-field with no flattening.

## Tradeoffs

- **Larger spec surface than the original draft.** `PhysicalKey` has
  ~175 enum values; the original draft had ~70. We accept this — the
  values are stable W3C names and they are what makes faithful key
  encoding possible.
- **`HYPER` and `META` are not separate modifier bits.** This matches
  libghostty (and the underlying reality on most platforms: they're
  XKB-configurable mappings to SUPER, not independent kernel-level
  flags). Users wanting tiling-WM-style "modifier-only" bindings get
  them via KIP's report-events flag plus configuration, not via wire-
  level Hyper/Meta bits.

## Alternatives considered

- **Application-shaped input** (the original draft): clean for
  in-process consumers but lossy at the libghostty seam.
- **Opaque pre-encoded VT bytes** (`INPUT_RAW` everywhere): trivially
  faithful at the byte level but discards the structured information
  KIP needs to be encoded correctly per-pane. Also forces every client
  to know every encoding the inner program might want — exactly the
  thing the multiplexer should hide.

## Implementation note

The Rust types in `crates/phux-protocol/src/input.rs` (forthcoming)
will use `From<phux_protocol::KeyEvent> for libghostty_vt::key::Event`
and the reverse, so the server-side encode loop is one line of code
plus an unwrap into the existing libghostty machinery.
