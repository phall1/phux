---
audience: consumers, contributors, agents
stability: stable
last-reviewed: 2026-05-28
---

# Input events

**TL;DR.** The L1 client-to-server input surface: structured key,
mouse, focus, paste, and raw-byte events. Carrying input as
structured data — not VT bytes — is what lets phux faithfully
transport the kitty keyboard protocol, IME composition,
modifier-rich chords, and pixel-precise mouse events through the
multiplexer to one server and many consumers (TUI, GUI, agent).

---

## 1. Overview

This section is the second-most-important part of the protocol. Carrying
input events as structured data — not raw VT bytes — is what allows
phux to faithfully transport the kitty keyboard protocol, IME
composition, modifier-rich chords, and pixel-precise mouse events
end-to-end through the multiplexer. It is also what lets the protocol
serve a future GUI client and a TUI client from the same wire format.

Per ADR-0008, the input *atom* types (`KeyAction`, `PhysicalKey`,
`ModSet`, `MouseAction`, `MouseButton`, `FocusEvent`) **are** libghostty-
vt's types — re-exported under phux-flavored names. The outer wrapper
structs (`KeyEvent`, `MouseEvent`) are phux-defined because libghostty's
event objects are allocator-lifetime-bound and not directly serializable;
their fields are still libghostty's types.

The server constructs libghostty events from wire events with a
field-for-field copy (no enum conversion), hands them to libghostty's
encoders (which know per-terminal state: KIP flags, cursor-key mode,
mouse protocol, etc.), and writes the resulting bytes to the PTY.
Encoder configuration never traverses the wire — the server is the one
with the `Terminal` and the encoder. See [ADR-0006] and [ADR-0008].

[ADR-0006]: ../../ADR/0006-input-mirrors-libghostty.md
[ADR-0008]: ../../ADR/0008-use-libghostty-types-directly.md

---

## 2. INPUT_KEY

```
INPUT_KEY {
    terminal_id: TerminalId,
    event: KeyEvent,
}

KeyEvent {
    action: KeyAction,
    key: PhysicalKey,
    mods: ModSet,
    consumed_mods: ModSet,
    composing: bool,
    text: optional<str>,
    unshifted_codepoint: optional<u32>,
}

KeyAction = enum {
    PRESS   = 0,
    RELEASE = 1,
    REPEAT  = 2,
}
```

### 2.1 `key` — `PhysicalKey`

`PhysicalKey` is a physical key code, **independent of keyboard layout
or modifiers**. It is the W3C UI Events `code`-style enum that
libghostty's `key::Key` carries. A US-QWERTY user pressing the leftmost
home-row key produces `KeyA`; an AZERTY user pressing the *same physical
key* also produces `KeyA`. The layout-resolved text appears in `text`
and `unshifted_codepoint`.

Values are stable; numeric assignments match libghostty's `key::Key`:

```
PhysicalKey = enum (u32) {
    UNIDENTIFIED   = 0,

    // Writing-system keys (US-QWERTY positions)
    BACKQUOTE      = 1,   BACKSLASH        = 2,   BRACKET_LEFT     = 3,
    BRACKET_RIGHT  = 4,   COMMA            = 5,
    DIGIT_0        = 6 ..= DIGIT_9          = 15,
    EQUAL          = 16,  INTL_BACKSLASH   = 17,  INTL_RO          = 18,
    INTL_YEN       = 19,
    KEY_A          = 20 ..= KEY_Z           = 45,
    MINUS          = 46,  PERIOD           = 47,  QUOTE            = 48,
    SEMICOLON      = 49,  SLASH            = 50,

    // Functional keys
    ALT_LEFT       = 51,  ALT_RIGHT        = 52,  BACKSPACE        = 53,
    CAPS_LOCK      = 54,  CONTEXT_MENU     = 55,  CONTROL_LEFT     = 56,
    CONTROL_RIGHT  = 57,  ENTER            = 58,  META_LEFT        = 59,
    META_RIGHT     = 60,  SHIFT_LEFT       = 61,  SHIFT_RIGHT      = 62,
    SPACE          = 63,  TAB              = 64,
    CONVERT        = 65,  KANA_MODE        = 66,  NON_CONVERT      = 67,

    // Control pad
    DELETE = 68,  END = 69, HELP = 70, HOME = 71, INSERT = 72,
    PAGE_DOWN = 73, PAGE_UP = 74,

    // Arrow keys
    ARROW_DOWN = 75, ARROW_LEFT = 76, ARROW_RIGHT = 77, ARROW_UP = 78,

    // Numpad
    NUM_LOCK = 79,
    NUMPAD_0 = 80 ..= NUMPAD_9 = 89,
    NUMPAD_ADD = 90, NUMPAD_BACKSPACE = 91, NUMPAD_CLEAR = 92,
    NUMPAD_CLEAR_ENTRY = 93, NUMPAD_COMMA = 94, NUMPAD_DECIMAL = 95,
    NUMPAD_DIVIDE = 96, NUMPAD_ENTER = 97, NUMPAD_EQUAL = 98,
    NUMPAD_MEMORY_ADD = 99, NUMPAD_MEMORY_CLEAR = 100,
    NUMPAD_MEMORY_RECALL = 101, NUMPAD_MEMORY_STORE = 102,
    NUMPAD_MEMORY_SUBTRACT = 103, NUMPAD_MULTIPLY = 104,
    NUMPAD_PAREN_LEFT = 105, NUMPAD_PAREN_RIGHT = 106,
    NUMPAD_SUBTRACT = 107, NUMPAD_SEPARATOR = 108,
    NUMPAD_UP = 109, NUMPAD_DOWN = 110, NUMPAD_RIGHT = 111,
    NUMPAD_LEFT = 112, NUMPAD_BEGIN = 113, NUMPAD_HOME = 114,
    NUMPAD_END = 115, NUMPAD_INSERT = 116, NUMPAD_DELETE = 117,
    NUMPAD_PAGE_UP = 118, NUMPAD_PAGE_DOWN = 119,

    // Function keys
    ESCAPE = 120,
    F1 = 121 ..= F25 = 145,
    FN = 146, FN_LOCK = 147,
    PRINT_SCREEN = 148, SCROLL_LOCK = 149, PAUSE = 150,

    // Browser / app
    BROWSER_BACK = 151, BROWSER_FAVORITES = 152, BROWSER_FORWARD = 153,
    BROWSER_HOME = 154, BROWSER_REFRESH = 155, BROWSER_SEARCH = 156,
    BROWSER_STOP = 157,
    EJECT = 158, LAUNCH_APP_1 = 159, LAUNCH_APP_2 = 160, LAUNCH_MAIL = 161,

    // Media / system
    MEDIA_PLAY_PAUSE = 162, MEDIA_SELECT = 163, MEDIA_STOP = 164,
    MEDIA_TRACK_NEXT = 165, MEDIA_TRACK_PREVIOUS = 166,
    POWER = 167, SLEEP = 168,
    AUDIO_VOLUME_DOWN = 169, AUDIO_VOLUME_MUTE = 170, AUDIO_VOLUME_UP = 171,
    WAKE_UP = 172, COPY = 173, CUT = 174, PASTE = 175,
}
```

This enum is **non-exhaustive** in spirit: minor protocol versions may
add new values. Decoders MUST treat unknown values as `UNIDENTIFIED`.

### 2.2 `mods` — `ModSet`

```
ModSet = bitset (u16) {
    SHIFT        = 0x0001,
    ALT          = 0x0002,
    CTRL         = 0x0004,
    SUPER        = 0x0008,    // also macOS Command, Windows key
    CAPS_LOCK    = 0x0010,
    NUM_LOCK     = 0x0020,

    // Left-vs-right discrimination. Each *_SIDE bit is only meaningful
    // when the corresponding modifier bit is set: 0 = left key,
    // 1 = right key. Platforms that cannot distinguish sides MUST
    // leave these bits zero.
    SHIFT_SIDE   = 0x0040,
    ALT_SIDE     = 0x0080,
    CTRL_SIDE    = 0x0100,
    SUPER_SIDE   = 0x0200,
}
```

Note the deliberate absence of `HYPER` and `META` as separate flags.
libghostty's `Mods` does not distinguish them from `SUPER`; on
platforms where they exist (X11 with custom XKB), they map to `SUPER`
with appropriate XKB configuration. Modeling them separately at the
protocol level would introduce a degree of freedom no downstream
encoder can honor.

### 2.3 `consumed_mods`

The subset of `mods` that the operating system *consumed* to produce
`text`. For example, on a US layout pressing Shift+2 produces the text
`@` with `SHIFT` in `consumed_mods`; the KIP encoder uses this to avoid
double-applying the shift modifier in its escape sequence.

Clients that do not have this information from their platform SHOULD
emit `ModSet::empty()` — the encoder degrades gracefully.

### 2.4 `composing`

`true` if this key event is part of an active IME composition sequence.
The encoder uses this to suppress text production where appropriate.

### 2.5 `text` and `unshifted_codepoint`

- `text`: the UTF-8 text the keypress produced under the current
  layout, *before* any Ctrl/Meta transformation. MUST NOT contain C0
  control characters (`U+0000–U+001F`, `U+007F`) — for those, pass
  `None` and let the encoder derive bytes from `key + mods`. MUST NOT
  contain platform PUA function-key codes (`U+F700–U+F8FF`).
- `unshifted_codepoint`: the layout-resolved codepoint that would have
  been produced if no modifiers were held. Used by KIP's
  `REPORT_ALTERNATES` mode to report the "base" key alongside the
  modified one.

Both fields are optional. KIP-aware clients SHOULD supply both for
maximum fidelity; legacy clients MAY omit them.

### 2.6 Server-side encoding pipeline

The server's per-Terminal state includes:

- A `libghostty_vt::Terminal` (canonical Terminal state, ADR-0004).
- A `libghostty_vt::key::Encoder` (key-to-bytes converter).

When the server receives an `INPUT_KEY`:

1. Translate the wire `KeyEvent` into a `libghostty::key::Event` —
   every field maps one-to-one.
2. Refresh the encoder's options against the current terminal state via
   `Encoder::set_options_from_terminal(&terminal)`. This pulls cursor-
   key application mode, keypad mode, alt-esc-prefix, modifyOtherKeys,
   and KIP progressive-enhancement flags from the terminal's current
   modes.
3. Call `Encoder::encode_to_vec(&event, &mut buf)`.
4. Write `buf` to the Terminal's PTY.

The client never sees encoder options. The client never produces VT
bytes. The protocol is the seam.

This pipeline supports, end-to-end:

- **The kitty keyboard protocol (KIP)** in its progressive-enhancement
  entirety — disambiguation, report-events, report-alternates,
  report-all, report-associated.
- Unambiguous distinction between Ctrl+I and Tab, between Esc and
  Alt-letter, between Ctrl+Enter and a literal `J`.
- IME composition and dead keys.
- Modifier-rich combinations (Super, side-discriminated) for tiling-WM-
  style bindings, with correct passthrough.

These are the things tmux structurally cannot do because it speaks raw
VT between client and server.

---

## 3. INPUT_MOUSE

```
INPUT_MOUSE {
    terminal_id: TerminalId,
    event: MouseEvent,
}

MouseEvent {
    action: MouseAction,
    button: optional<MouseButton>,
    mods: ModSet,
    position: MousePosition,
}

MouseAction = enum {
    PRESS   = 0,
    RELEASE = 1,
    MOTION  = 2,
}

MouseButton = enum (u32) {
    UNKNOWN = 0,
    LEFT    = 1,
    RIGHT   = 2,
    MIDDLE  = 3,
    FOUR    = 4,    FIVE    = 5,    SIX    = 6,    SEVEN  = 7,
    EIGHT   = 8,    NINE    = 9,    TEN    = 10,   ELEVEN = 11,
}

MousePosition {
    // Terminal-local surface-space pixels. Always present.
    // f64 (not u32) to mirror libghostty's `mouse::Position` exactly —
    // sub-pixel input is real on macOS trackpads and Wayland HiDPI surfaces;
    // cell-quantizing clients pass integer-valued f64s (`12.0`).
    pixel_x: f64,
    pixel_y: f64,
}
```

Values map one-to-one to libghostty's `mouse::Action`, `mouse::Button`,
and `mouse::Position`. Buttons 4..=11 carry their libghostty meaning;
scroll-wheel events arrive as `PRESS` of buttons 4 (up) / 5 (down) /
6 (left) / 7 (right) following xterm convention.

### 3.1 Pixel positions and the cell-geometry contract

Mouse positions on the wire are **pixels in Terminal-local surface
space**. The server reconstructs `mouse::EncoderSize` (cell width/
height, padding, full screen geometry) from the most recent
`VIEWPORT_RESIZE` ([L1.md §viewport resize](./L1.md)) and per-Terminal
layout. Cell-quantized
clients (TUIs without true
pixel-precision input) emit positions at `cell_index × cell_size`; the
server's encoder produces correct output in both cell-format (SGR,
URXVT) and pixel-format (SGR-Pixels) mouse protocols.

### 3.2 Server-side encoding pipeline

Identical in spirit to §2.6: each Terminal has a
`libghostty_vt::mouse::Encoder`. On `INPUT_MOUSE`, the server refreshes
the encoder via `set_options_from_terminal`, sets the encoder's
`EncoderSize` from current Terminal/cell geometry, builds a
`libghostty::mouse::Event`, encodes, and writes to PTY.

---

## 4. INPUT_FOCUS

```
INPUT_FOCUS {
    terminal_id: TerminalId,
    event: FocusKind,
}

FocusKind = enum { GAINED = 0, LOST = 1 }
```

The client emits `INPUT_FOCUS` when its window gains or loses focus on
the host OS. If the Terminal has DEC mode 1004 (focus reporting)
active, the server encodes a `CSI I` / `CSI O` via
`libghostty_vt::focus` and writes to the PTY; otherwise the event is
dropped server-side.

This event is purely L1: it reports the host-OS focus state of the
client to the Terminal, so OSC-aware programs (Vim, fzf, etc.) can
pause animation. A "which Terminal does the consumer want input
routed to" indicator is **not** a wire concept — that's an L3
metadata convention of the TUI consumer (see
[L3.md §TUI conventions](./L3.md)).

---

## 5. INPUT_PASTE

```
INPUT_PASTE {
    terminal_id: TerminalId,
    data: bytes,
    bracketed: bool,
    trust: PasteTrust,
}

PasteTrust = enum {
    UNTRUSTED = 0,   // server SHOULD apply paste::is_safe; reject or sanitize
                     //   per server config
    TRUSTED   = 1,   // server forwards verbatim; caller asserted safety
}
```

Server uses `libghostty_vt::paste` utilities: `paste::is_safe(data)` to
classify content, `paste::encode(data, bracketed, buf)` to produce
final bytes (handles bracketed-paste sequences and unsafe-control-byte
stripping).

When `trust = UNTRUSTED`, the server's per-Terminal policy applies:
`reject` (default — return an `ERROR { code: UNSAFE_PASTE }`),
`sanitize` (use `paste::encode` to strip), or `allow` (forward anyway).
When `trust = TRUSTED`, the server invokes `paste::encode` for
bracketing but skips safety classification.

---

## 6. INPUT_RAW

```
INPUT_RAW {
    terminal_id: TerminalId,
    data: bytes,
}
```

Escape hatch. Bytes in `data` are written verbatim to the Terminal's PTY.
Reserved for cases not modelled by `INPUT_KEY` / `INPUT_PASTE` /
`INPUT_MOUSE` / `INPUT_FOCUS` (chiefly: direct PTY testing and command
interpolation from configs). Servers MUST NOT silently re-interpret
`INPUT_RAW`; clients SHOULD avoid using it in normal operation.

---

## 7. Input authority

Input authority is governed by the caller's `TerminalRole` for the
target Terminal ([L1.md §roles and takeover policy](./L1.md)). A
client whose role for a Terminal is `VIEWER`
MUST NOT send `INPUT_KEY`, `INPUT_PASTE`, `INPUT_MOUSE`, `INPUT_FOCUS`,
or `INPUT_RAW` for that Terminal. A server receiving such a message from
a viewer MUST reject it with `ERROR { code: PERMISSION_DENIED }` and
MUST NOT write bytes to the PTY.

A client whose role for a Terminal is `PRIMARY` MAY send input for that
Terminal. Transport authentication remains out of band
([proto.md §10](./proto.md)); roles are
an in-protocol concurrency policy, not an authentication mechanism.

The same four atoms (`KEY` / `MOUSE` / `FOCUS` / `PASTE`) can also be
delivered out of band, without an attach, via the `ROUTE_INPUT` control
command ([L1.md §5.1](./L1.md)). That path bundles the atom into an
`InputEvent` tagged union and feeds it to the Terminal directly; the
same `PRIMARY`/`VIEWER` authority and fire-and-forget semantics apply.
