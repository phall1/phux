//! Stdin VT-byte parsing for the attach loop.
//!
//! The parser turns the byte stream a TTY puts on stdin into structured
//! [`InputEvent`]s aimed at SPEC §9 input frames. It is deliberately small
//! and local — libghostty-vt ships *encoders* (event → bytes) but not a
//! decoder for the reverse direction, so this module hand-rolls just
//! enough of an xterm/VT100 input parser to cover what `phux attach` sees
//! in practice.
//!
//! # Surface area
//!
//! Handled:
//!
//! - Printable ASCII and full UTF-8 multibyte sequences (Unicode keypresses)
//! - C0 controls — CR/LF (Enter), HT (Tab), BS / DEL (Backspace), and the
//!   Ctrl-A..Ctrl-Z encoding (`0x01..=0x1A` minus the dedicated mappings).
//! - The Ctrl-b d detach chord (tmux-style).
//! - **Bare ESC** as the Escape key (see "Timing ambiguity" below).
//! - **ESC + char** as an Alt-modified key (Alt-letter chords).
//! - **CSI** sequences (`ESC [` …) for arrow keys, Home/End/Insert/Delete,
//!   PageUp/PageDown, F-keys via the xterm `CSI 1;<mod>P..S` and VT220
//!   `CSI <n>~` forms, and modifier-bearing variants
//!   (`CSI 1;5A` for Ctrl-Up, `CSI 5;5~` for Ctrl-PageUp, etc.).
//! - **SS3** sequences (`ESC O <P|Q|R|S>`) for the older F1..F4 + numeric
//!   keypad encoding most BSD `termcap` entries still use.
//!
//! Also handled:
//!
//! - **SGR mouse reports** (`CSI < <btn> ; <col> ; <row> M/m`, DEC mode
//!   1006). Emitted as [`InputEvent::Mouse`] with terminal-local cell
//!   coordinates expressed as integer-valued `f64` pixels (per SPEC §9.2.1
//!   the encoder downstream re-quantises for cell-format mouse protocols).
//! - **Focus reports** (`CSI I` / `CSI O`, DEC mode 1004). Emitted as
//!   [`InputEvent::Focus`].
//! - **Bracketed paste** (`CSI 200~` … `CSI 201~`, DEC mode 2004). The
//!   parser buffers payload bytes between the begin / end markers and
//!   emits a single [`InputEvent::Paste`] at the end-marker. Payload
//!   bytes are passed through verbatim — no nested escape parsing.
//!
//! Not handled (yet):
//!
//! - Legacy X10 mouse (`CSI M Cb Cx Cy`) — three raw bytes after the
//!   `M` final encode button + position. Filed as a follow-up; SGR mode
//!   covers every modern terminal the user is likely to attach with.
//! - urxvt-1015 decimal mouse format (`CSI <btn> ; <col> ; <row> M`).
//!   Niche and easy to add later — same dispatcher, no `<` prefix.
//! - The full kitty keyboard protocol `CSI u` form — for now the kitty
//!   CSI-u sequences are absorbed by the CSI parser as best-effort and
//!   dropped (with a trace log) if they cannot be mapped to a
//!   [`PhysicalKey`]. Full KIP passthrough is a future ticket.
//! - DCS / OSC / SOS / PM / APC sequences inbound — these come from the
//!   inner program, not from a keyboard, and have no representation as a
//!   `KeyEvent`. They are absorbed and dropped.
//!
//! # libghostty surface check
//!
//! libghostty-vt exposes input *encoders* (`key::Encoder`,
//! `mouse::Encoder`, `focus::Event::encode`, `paste::encode`) but no
//! parser in the reverse direction — there is no `Decoder`, no
//! `parse_bytes`, no public state machine equivalent to xterm's VT input
//! lexer. Confirmed by grepping the `libghostty-vt` crate at the pinned
//! rev (`31d1f70`); only the `terminal::Terminal::vt_write` parser
//! exists, and that is for output bytes from a child process, not input
//! bytes from a host terminal. This module therefore hand-rolls the
//! CSI / SS3 lexer required for inbound input parsing.
//!
//! # Timing ambiguity (bare ESC)
//!
//! Bare ESC and ESC-starting-a-sequence are not distinguishable from the
//! byte stream alone. The classic xterm-style solution is a short idle
//! timeout: if ESC is followed by another byte within ~50ms it starts a
//! sequence, otherwise it stands alone as the Escape key.
//!
//! This module exposes that decision to the driver via [`StdinParser::flush`].
//! The driver arms a [`tokio::time::Sleep`] whenever
//! [`StdinParser::has_pending`] returns `true` and calls `flush()` if the
//! sleep wins the next `select!` race. That keeps timing policy out of
//! the parser (which is sync and timer-free) and inside the loop that
//! already knows about wall time.
//!
//! # Partial sequences across read boundaries
//!
//! A single CSI sequence can straddle two `read()` calls; the parser
//! holds its in-progress state (a private `State` enum + a small
//! parameter buffer) in the [`StdinParser`] struct and resumes on the
//! next [`StdinParser::feed`] call.

use phux_protocol::input::focus::FocusEvent;
use phux_protocol::input::key::{KeyAction, KeyEvent, ModSet, PhysicalKey};
use phux_protocol::input::mouse::{MouseAction, MouseButton, MouseEvent};
use phux_protocol::input::paste::{PasteEvent, PasteTrust};
use phux_protocol::wire::frame::FrameKind;

/// Human-readable description of the v0 detach chord. Surfaced through
/// [`super::DETACH_CHORD_DESCRIPTION`] for help text.
pub const DETACH_CHORD_DESCRIPTION: &str = "Ctrl-b d";

/// The prefix byte of the detach chord. `Ctrl-b` is `0x02`.
const DETACH_PREFIX: u8 = 0x02;

/// The completion byte of the detach chord. ASCII lowercase `d`.
const DETACH_FINISH: u8 = b'd';

/// One client-to-server input event ready to be wrapped in a [`FrameKind`].
///
/// Mouse / focus / paste variants are present so the enum reads true to
/// the SPEC §9 input surface — but the v0 parser only ever yields
/// [`InputEvent::Key`] from real input bytes, plus
/// [`InputEvent::DetachRequested`] for the hardcoded chord. The richer
/// variants are populated when mouse / paste parsing lands as a follow-up.
#[derive(Debug, Clone, PartialEq)]
pub enum InputEvent {
    /// A structured key event. Encodes to `INPUT_KEY` per SPEC §9.1.
    Key(KeyEvent),
    /// A structured mouse event. Encodes to `INPUT_MOUSE` per SPEC §9.2.
    Mouse(MouseEvent),
    /// A focus state change on the host window. Encodes to `INPUT_FOCUS`
    /// per SPEC §9.3.
    Focus(FocusEvent),
    /// A paste payload. Encodes to `INPUT_PASTE` per SPEC §9.4.
    Paste(PasteEvent),
    /// The user pressed the detach chord. The driver translates this to
    /// a [`FrameKind::Detach`] and waits for `DETACHED`.
    DetachRequested,
}

impl InputEvent {
    /// Wrap this event in the appropriate [`FrameKind`] addressed to
    /// `terminal_id`. [`InputEvent::DetachRequested`] returns `None` — the
    /// driver issues a [`FrameKind::Detach`] directly, since `DETACH`
    /// is session-level and pane-id-agnostic.
    #[must_use]
    pub fn into_frame(self, terminal_id: u32) -> Option<FrameKind> {
        match self {
            Self::Key(event) => Some(FrameKind::InputKey { terminal_id, event }),
            Self::Mouse(event) => Some(FrameKind::InputMouse { terminal_id, event }),
            Self::Focus(event) => Some(FrameKind::InputFocus { terminal_id, event }),
            Self::Paste(event) => Some(FrameKind::InputPaste { terminal_id, event }),
            Self::DetachRequested => None,
        }
    }
}

/// Internal parser state machine.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum State {
    /// Default; consuming plain bytes.
    #[default]
    Ground,
    /// Just saw an ESC; the next byte decides whether this is bare ESC,
    /// Alt+char, CSI, SS3, DCS, or something we drop.
    Escape,
    /// Inside a CSI sequence (`ESC [` …), accumulating parameter / intermediate
    /// bytes until a final byte in `0x40..=0x7E` arrives.
    Csi,
    /// Inside an SS3 sequence (`ESC O` …); next byte is the final.
    Ss3,
    /// Inside a string-like control (DCS, OSC, SOS, PM, APC). We absorb
    /// until ST (`ESC \`) or BEL.
    StringTerm,
    /// In a UTF-8 multibyte continuation; `expected` more continuation
    /// bytes remain.
    Utf8 { expected: u8 },
    /// Waiting on the second byte of the detach chord.
    DetachPending,
    /// Inside a bracketed-paste payload; bytes accumulate in `paste_buf`
    /// until the closing `ESC [ 201 ~` marker is seen.
    Paste,
    /// Inside a paste payload, just saw an ESC. The next bytes might be
    /// the closing `[ 201 ~` marker, or they might be part of the paste
    /// payload (e.g. a nested ANSI sequence the user pasted). We parse
    /// minimally enough to spot the end marker.
    PasteEscape,
    /// Inside a paste payload, saw `ESC [`. Accumulating the (numeric)
    /// parameter bytes in `buf` until either a final byte arrives that
    /// completes the `CSI 201~` close marker, or anything else, in which
    /// case the accumulated bytes are flushed back into the paste payload.
    PasteCsi,
}

/// Stateful parser for stdin bytes.
///
/// The parser is timer-free; the driver arms a `tokio::time::Sleep` and
/// calls [`StdinParser::flush`] when stdin has been idle for the bare-ESC
/// timeout. See the module doc.
#[derive(Debug)]
pub struct StdinParser {
    state: State,
    /// In-progress CSI / SS3 / UTF-8 / string-term bytes. Reused across
    /// feeds so partial sequences resume cleanly.
    buf: Vec<u8>,
    /// Pending UTF-8 lead byte (when `state == Utf8`).
    utf8_lead: u8,
    /// Accumulated bytes of an in-progress bracketed-paste payload.
    paste_buf: Vec<u8>,
}

impl Default for StdinParser {
    fn default() -> Self {
        Self::new()
    }
}

impl StdinParser {
    /// New parser in the ground state.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            state: State::Ground,
            buf: Vec::new(),
            utf8_lead: 0,
            paste_buf: Vec::new(),
        }
    }

    /// Returns `true` if the parser is currently inside an in-progress
    /// sequence and a [`flush`](Self::flush) would change observable
    /// behaviour. The driver uses this to decide whether to arm the
    /// bare-ESC idle timer.
    #[must_use]
    pub const fn has_pending(&self) -> bool {
        !matches!(self.state, State::Ground)
    }

    /// Flush pending parser state on a timing boundary.
    ///
    /// Called by the driver when stdin has been idle long enough that a
    /// bare ESC can no longer be the prefix of a sequence. Effects:
    ///
    /// - `State::Escape`: emit a single Escape key and return to ground.
    /// - `State::DetachPending`: the chord aborted by timeout; drop the
    ///   prefix per tmux semantics and return to ground. (Matches the
    ///   in-stream "Ctrl-b x" branch.)
    /// - All other in-progress states are *kept* — a CSI half-sent over
    ///   the network is not "abandoned" just because of an idle gap; the
    ///   next read is still expected to complete it. (Real terminals
    ///   never split CSI sequences across long idle windows.)
    pub fn flush(&mut self) -> Vec<InputEvent> {
        match self.state {
            State::Escape => {
                self.state = State::Ground;
                vec![InputEvent::Key(make_named_key(
                    PhysicalKey::Escape,
                    ModSet::empty(),
                ))]
            }
            State::DetachPending => {
                self.state = State::Ground;
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    /// Feed `bytes` and return any complete events.
    ///
    /// The parser does not allocate when no events come out beyond the
    /// returned `Vec` itself.
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<InputEvent> {
        let mut out = Vec::new();
        for &b in bytes {
            self.feed_byte(b, &mut out);
        }
        out
    }

    fn feed_byte(&mut self, b: u8, out: &mut Vec<InputEvent>) {
        match self.state {
            State::Ground => self.feed_ground(b, out),
            State::Escape => self.feed_escape(b, out),
            State::Csi => self.feed_csi(b, out),
            State::Ss3 => self.feed_ss3(b, out),
            State::StringTerm => self.feed_string_term(b),
            State::Utf8 { expected } => self.feed_utf8(b, expected, out),
            State::DetachPending => self.feed_detach_pending(b, out),
            State::Paste => self.feed_paste(b),
            State::PasteEscape => self.feed_paste_escape(b),
            State::PasteCsi => self.feed_paste_csi(b, out),
        }
    }

    fn feed_ground(&mut self, b: u8, out: &mut Vec<InputEvent>) {
        if b == DETACH_PREFIX {
            self.state = State::DetachPending;
            return;
        }
        if b == 0x1B {
            self.state = State::Escape;
            return;
        }
        if let Some(ev) = c0_or_ascii_to_key(b) {
            out.push(InputEvent::Key(ev));
            return;
        }
        // UTF-8 multibyte lead?
        if let Some(more) = utf8_continuation_count(b) {
            self.buf.clear();
            self.buf.push(b);
            self.utf8_lead = b;
            self.state = State::Utf8 { expected: more };
            return;
        }
        // Stray continuation byte or 0x80..=0xBF without a lead. Drop.
        tracing::trace!(byte = b, "dropping stray byte in ground state");
    }

    fn feed_escape(&mut self, b: u8, out: &mut Vec<InputEvent>) {
        match b {
            b'[' => {
                self.buf.clear();
                self.state = State::Csi;
            }
            b'O' => {
                self.state = State::Ss3;
            }
            // DCS / OSC / SOS / PM / APC — absorb until ST or BEL.
            b'P' | b']' | b'X' | b'^' | b'_' => {
                self.state = State::StringTerm;
            }
            0x1B => {
                // ESC ESC — treat the first ESC as a complete Escape key
                // and stay in Escape state for the second.
                out.push(InputEvent::Key(make_named_key(
                    PhysicalKey::Escape,
                    ModSet::empty(),
                )));
                // self.state stays Escape.
            }
            // ESC + printable ASCII / ESC + C0 → Alt-chord. Build the
            // base event for `b` and OR in ALT.
            _ => {
                self.state = State::Ground;
                if let Some(mut ev) = c0_or_ascii_to_key(b) {
                    ev.mods |= ModSet::ALT;
                    // Alt-letter does not produce text on most platforms —
                    // strip the text payload so the server's encoder
                    // builds the bytes from key+mods.
                    if ev.mods.contains(ModSet::CTRL) || !ev.mods.contains(ModSet::SHIFT) {
                        ev.text = None;
                    }
                    out.push(InputEvent::Key(ev));
                } else {
                    tracing::trace!(byte = b, "dropping ESC + unrecognised byte");
                }
            }
        }
    }

    fn feed_csi(&mut self, b: u8, out: &mut Vec<InputEvent>) {
        // CSI structure (ECMA-48):
        //   parameter bytes: 0x30..=0x3F   ('0'..'9' ':' ';' '<' '=' '>' '?')
        //   intermediate bytes: 0x20..=0x2F (' ' .. '/')
        //   final byte: 0x40..=0x7E
        if (0x30..=0x3F).contains(&b) || (0x20..=0x2F).contains(&b) {
            self.buf.push(b);
            // Cap the buffer to bound memory if a misbehaving peer floods
            // parameter bytes without a final. xterm caps at ~200.
            if self.buf.len() > 256 {
                tracing::trace!("dropping over-long CSI sequence");
                self.buf.clear();
                self.state = State::Ground;
            }
            return;
        }
        if (0x40..=0x7E).contains(&b) {
            let final_byte = b;
            // Move buf out so dispatch_csi can borrow self immutably.
            let params = std::mem::take(&mut self.buf);
            self.state = State::Ground;
            // Bracketed-paste begin (`CSI 200~`) puts us into Paste state
            // instead of emitting an event; everything else goes through
            // the normal CSI dispatch.
            if final_byte == b'~' && params == b"200" {
                self.paste_buf.clear();
                self.state = State::Paste;
                return;
            }
            dispatch_csi(&params, final_byte, out);
            return;
        }
        // Unexpected byte inside a CSI — abort cleanly. xterm-vt100 behavior
        // is to cancel the sequence and return to ground.
        tracing::trace!(byte = b, "aborting CSI on unexpected byte");
        self.buf.clear();
        self.state = State::Ground;
    }

    fn feed_ss3(&mut self, b: u8, out: &mut Vec<InputEvent>) {
        self.state = State::Ground;
        let key = match b {
            b'A' => PhysicalKey::ArrowUp,
            b'B' => PhysicalKey::ArrowDown,
            b'C' => PhysicalKey::ArrowRight,
            b'D' => PhysicalKey::ArrowLeft,
            b'F' => PhysicalKey::End,
            b'H' => PhysicalKey::Home,
            b'P' => PhysicalKey::F1,
            b'Q' => PhysicalKey::F2,
            b'R' => PhysicalKey::F3,
            b'S' => PhysicalKey::F4,
            _ => {
                tracing::trace!(byte = b, "unknown SS3 final byte");
                return;
            }
        };
        out.push(InputEvent::Key(make_named_key(key, ModSet::empty())));
    }

    fn feed_string_term(&mut self, b: u8) {
        // We track ST (`ESC \`) by re-entering State::Escape so the next
        // `\` finishes the string. BEL (0x07) also terminates an OSC.
        if b == 0x07 {
            self.state = State::Ground;
        } else if b == 0x1B {
            // Could be ST. We re-use State::Escape to find the next byte;
            // the only "exit" path from a string-terminator ESC is `\`,
            // anything else is a malformed sequence which we also drop.
            // For simplicity, just return to ground unconditionally on the
            // next byte by treating any byte after this ESC as "consumed
            // and we're done". Use a dedicated marker by stashing a
            // sentinel — but to keep state minimal, just consume the next
            // byte in StringTerm-end mode via Escape and bounce out.
            //
            // Simplest robust thing: leave State::StringTerm, the next
            // byte will be processed as the string-terminator final.
            // Track via a one-shot using buf[0].
            self.buf.clear();
            self.buf.push(1);
            // Note: we stay in StringTerm; the next byte branches via the
            // sentinel.
        } else if !self.buf.is_empty() && self.buf[0] == 1 {
            // Previous byte was ESC; this is the ST final. End the string
            // regardless of what `b` is — malformed sequences end here too.
            self.buf.clear();
            self.state = State::Ground;
        }
    }

    fn feed_utf8(&mut self, b: u8, expected: u8, out: &mut Vec<InputEvent>) {
        if (b & 0xC0) != 0x80 {
            // Not a valid continuation byte. Drop accumulated bytes and
            // reinterpret `b` from the ground state — that's the
            // recovery path most VT parsers use.
            tracing::trace!(byte = b, "invalid UTF-8 continuation, restarting");
            self.buf.clear();
            self.state = State::Ground;
            self.feed_byte(b, out);
            return;
        }
        self.buf.push(b);
        let remaining = expected - 1;
        if remaining == 0 {
            // Complete codepoint. Decode and emit.
            if let Ok(s) = std::str::from_utf8(&self.buf) {
                let cp = s.chars().next().map(u32::from);
                out.push(InputEvent::Key(KeyEvent {
                    action: KeyAction::Press,
                    key: PhysicalKey::Unidentified,
                    mods: ModSet::empty(),
                    consumed_mods: ModSet::empty(),
                    composing: false,
                    text: Some(s.to_owned()),
                    unshifted_codepoint: cp,
                }));
            } else {
                tracing::trace!("invalid UTF-8 sequence dropped");
            }
            self.buf.clear();
            self.state = State::Ground;
        } else {
            self.state = State::Utf8 {
                expected: remaining,
            };
        }
    }

    /// Inside a bracketed-paste payload. We pass everything through into
    /// `paste_buf` verbatim until an ESC arrives — that *might* be the
    /// closing `ESC [ 201 ~`, or it might be part of the payload (a
    /// pasted ANSI escape is valid). We don't decide until we see the
    /// next bytes.
    fn feed_paste(&mut self, b: u8) {
        if b == 0x1B {
            self.state = State::PasteEscape;
            return;
        }
        self.paste_buf.push(b);
    }

    /// In a paste payload, just saw an ESC. If the next byte is `[` we
    /// might be looking at the close marker; otherwise the ESC was part
    /// of the payload and we restore it before the new byte.
    fn feed_paste_escape(&mut self, b: u8) {
        if b == b'[' {
            self.buf.clear();
            self.state = State::PasteCsi;
            return;
        }
        // Not the close marker — keep the ESC and the new byte in the
        // payload.
        self.paste_buf.push(0x1B);
        self.paste_buf.push(b);
        self.state = State::Paste;
    }

    /// In a paste payload, saw `ESC [`. We accumulate parameter bytes
    /// in `buf` until either:
    /// - the `~` final arrives and `buf == "201"` — emit the paste; or
    /// - any other final / unexpected byte arrives — the `ESC [` was
    ///   part of the payload, so we flush `ESC [` + `buf` + this byte
    ///   back into the paste payload and return to Paste mode.
    fn feed_paste_csi(&mut self, b: u8, out: &mut Vec<InputEvent>) {
        // Parameter / intermediate region.
        if (0x30..=0x3F).contains(&b) || (0x20..=0x2F).contains(&b) {
            self.buf.push(b);
            if self.buf.len() > 16 {
                // Way too many param bytes for our close marker; treat
                // the whole accumulation as payload bytes.
                self.flush_pending_paste_escape();
            }
            return;
        }
        if (0x40..=0x7E).contains(&b) {
            // Closing `CSI 201 ~`?
            if b == b'~' && self.buf == b"201" {
                self.buf.clear();
                let data = std::mem::take(&mut self.paste_buf);
                out.push(InputEvent::Paste(PasteEvent {
                    trust: PasteTrust::Untrusted,
                    data,
                }));
                self.state = State::Ground;
                return;
            }
            // Some other CSI inside the paste payload. Flush the buffered
            // CSI prefix back into the paste payload, including the final.
            self.flush_pending_paste_escape();
            self.paste_buf.push(b);
            return;
        }
        // Unexpected byte inside the CSI window — flush and restart paste
        // ingestion with the new byte processed under Paste rules.
        self.flush_pending_paste_escape();
        self.feed_paste(b);
    }

    /// Flush a buffered `ESC [` + parameter bytes back into the paste
    /// payload — used when the accumulated bytes turn out not to be a
    /// close marker, so they were part of the user's paste after all.
    /// Restores the parser to [`State::Paste`].
    fn flush_pending_paste_escape(&mut self) {
        self.paste_buf.push(0x1B);
        self.paste_buf.push(b'[');
        self.paste_buf.extend_from_slice(&self.buf);
        self.buf.clear();
        self.state = State::Paste;
    }

    fn feed_detach_pending(&mut self, b: u8, out: &mut Vec<InputEvent>) {
        self.state = State::Ground;
        if b == DETACH_FINISH {
            out.push(InputEvent::DetachRequested);
            return;
        }
        // Chord aborted — process `b` from ground state. Matches existing
        // tmux semantics: `Ctrl-b x` drops the prefix and delivers `x`.
        self.feed_byte(b, out);
    }
}

/// Map a single byte to a [`KeyEvent`] for the printable / C0 region.
/// Returns `None` for bytes the parser handles elsewhere (ESC, the detach
/// prefix, UTF-8 continuations, ...).
fn c0_or_ascii_to_key(b: u8) -> Option<KeyEvent> {
    match b {
        // Printable ASCII.
        0x20..=0x7E => Some(KeyEvent {
            action: KeyAction::Press,
            key: ascii_to_physical(b),
            mods: ascii_shift_mods(b),
            consumed_mods: ascii_shift_mods(b),
            composing: false,
            text: Some(char::from(b).to_string()),
            unshifted_codepoint: Some(u32::from(ascii_unshifted(b))),
        }),
        // CR / LF → Enter.
        0x0D | 0x0A => Some(make_named_key(PhysicalKey::Enter, ModSet::empty())),
        // BS / DEL → Backspace.
        0x08 | 0x7F => Some(make_named_key(PhysicalKey::Backspace, ModSet::empty())),
        // HT → Tab.
        0x09 => Some(make_named_key(PhysicalKey::Tab, ModSet::empty())),
        // Ctrl-A..Ctrl-Z (skipping the dedicated mappings above and ESC).
        0x01..=0x1A if b != 0x08 && b != 0x09 && b != 0x0A && b != 0x0D => {
            let letter = b'A' + (b - 1);
            ascii_letter_to_key(letter).map(|key| KeyEvent {
                action: KeyAction::Press,
                key,
                mods: ModSet::CTRL,
                consumed_mods: ModSet::CTRL,
                composing: false,
                text: None,
                unshifted_codepoint: Some(u32::from(letter.to_ascii_lowercase())),
            })
        }
        _ => None,
    }
}

/// Build a key event for a "named" key (no text payload).
const fn make_named_key(key: PhysicalKey, mods: ModSet) -> KeyEvent {
    KeyEvent {
        action: KeyAction::Press,
        key,
        mods,
        consumed_mods: ModSet::empty(),
        composing: false,
        text: None,
        unshifted_codepoint: None,
    }
}

/// Map a printable ASCII byte to a [`PhysicalKey`]. Defaults to
/// `Unidentified` for punctuation we don't have a specific code for.
const fn ascii_to_physical(b: u8) -> PhysicalKey {
    match b {
        b' ' => PhysicalKey::Space,
        b'0'..=b'9' => match b {
            b'0' => PhysicalKey::Digit0,
            b'1' => PhysicalKey::Digit1,
            b'2' => PhysicalKey::Digit2,
            b'3' => PhysicalKey::Digit3,
            b'4' => PhysicalKey::Digit4,
            b'5' => PhysicalKey::Digit5,
            b'6' => PhysicalKey::Digit6,
            b'7' => PhysicalKey::Digit7,
            b'8' => PhysicalKey::Digit8,
            _ => PhysicalKey::Digit9,
        },
        b'A'..=b'Z' | b'a'..=b'z' => {
            // Lowercase first to look up the table.
            let upper = if b.is_ascii_lowercase() { b - 32 } else { b };
            ascii_letter_to_key_const(upper)
        }
        _ => PhysicalKey::Unidentified,
    }
}

/// Modifier set implied by a printable ASCII byte. Uppercase letters and
/// shifted punctuation (`!@#…`) come with `SHIFT` in `consumed_mods` per
/// SPEC §9.1.3 so the encoder doesn't double-apply.
const fn ascii_shift_mods(b: u8) -> ModSet {
    if b.is_ascii_uppercase() || is_shifted_punct(b) {
        ModSet::SHIFT
    } else {
        ModSet::empty()
    }
}

const fn is_shifted_punct(b: u8) -> bool {
    matches!(
        b,
        b'!' | b'@'
            | b'#'
            | b'$'
            | b'%'
            | b'^'
            | b'&'
            | b'*'
            | b'('
            | b')'
            | b'_'
            | b'+'
            | b'{'
            | b'}'
            | b'|'
            | b':'
            | b'"'
            | b'<'
            | b'>'
            | b'?'
            | b'~'
    )
}

/// What this key would produce with no modifiers held — used to fill
/// `unshifted_codepoint`. `A` → `a`, `@` → `2`, etc. Best-effort, US
/// QWERTY mapping; clients on other layouts can override at a higher
/// layer (not in scope here).
const fn ascii_unshifted(b: u8) -> u8 {
    match b {
        b'A'..=b'Z' => b + 32,
        b'!' => b'1',
        b'@' => b'2',
        b'#' => b'3',
        b'$' => b'4',
        b'%' => b'5',
        b'^' => b'6',
        b'&' => b'7',
        b'*' => b'8',
        b'(' => b'9',
        b')' => b'0',
        b'_' => b'-',
        b'+' => b'=',
        b'{' => b'[',
        b'}' => b']',
        b'|' => b'\\',
        b':' => b';',
        b'"' => b'\'',
        b'<' => b',',
        b'>' => b'.',
        b'?' => b'/',
        b'~' => b'`',
        _ => b,
    }
}

/// How many continuation bytes follow this byte in a UTF-8 sequence.
/// Returns `None` for non-lead bytes (ASCII or stray continuations).
const fn utf8_continuation_count(b: u8) -> Option<u8> {
    match b {
        0xC2..=0xDF => Some(1),
        0xE0..=0xEF => Some(2),
        0xF0..=0xF4 => Some(3),
        _ => None,
    }
}

/// Dispatch a finished CSI sequence into [`InputEvent`]s.
///
/// `params` is the parameter / intermediate region (everything between
/// `ESC [` and the final byte); `final_byte` is the final byte
/// (`0x40..=0x7E`).
fn dispatch_csi(params: &[u8], final_byte: u8, out: &mut Vec<InputEvent>) {
    // SGR mouse reports (DEC mode 1006) carry a leading `<` private-
    // marker byte and end in `M` (press / motion) or `m` (release).
    // Form: `CSI < <btn> ; <col> ; <row> M|m`. We dispatch them before
    // stripping the marker because the marker is what distinguishes
    // them from a bare `CSI M` (legacy X10 mouse, not supported).
    if matches!(params.first(), Some(&b'<')) && (final_byte == b'M' || final_byte == b'm') {
        dispatch_sgr_mouse(&params[1..], final_byte, out);
        return;
    }

    // Strip a leading private-marker `?`, `<`, `=`, `>` for now — we
    // don't differentiate, the modifier-bearing variant alone matters.
    let body = if let Some(first) = params.first()
        && matches!(*first, b'?' | b'<' | b'=' | b'>')
    {
        &params[1..]
    } else {
        params
    };

    // Parse out semicolon-separated numeric params. Missing/empty params
    // are treated as 0 (xterm convention; the final byte's interpretation
    // tells us what "missing" means in context).
    let parsed = parse_csi_params(body);

    // Focus reports (DEC mode 1004): `CSI I` = gained, `CSI O` = lost.
    // Recognised only when the parameter buffer is empty (bare CSI).
    // With parameters the same final bytes have other meanings.
    if body.is_empty() {
        if final_byte == b'I' {
            out.push(InputEvent::Focus(FocusEvent::Gained));
            return;
        }
        if final_byte == b'O' {
            out.push(InputEvent::Focus(FocusEvent::Lost));
            return;
        }
    }

    // The `CSI <n> ~` form encodes function keys and the navigation keys
    // (Insert, Delete, Home, End, PgUp/PgDn, F5..F12). The optional second
    // parameter is the xterm modifier code (1=none, 2=Shift, 3=Alt,
    // 5=Ctrl, etc.).
    if final_byte == b'~' {
        let n = parsed.first().copied().unwrap_or(1);
        let mods = parsed
            .get(1)
            .copied()
            .map_or(ModSet::empty(), xterm_modifier_code);
        if let Some(key) = csi_tilde_keycode(n) {
            out.push(InputEvent::Key(make_named_key(key, mods)));
        } else {
            tracing::trace!(n, final_byte, "unknown CSI ~ keycode");
        }
        return;
    }

    // The xterm modifier-bearing form is `CSI 1 ; <mod> <letter>` for the
    // arrow / Home / End / F1..F4 keys. When no modifier is present the
    // bare form `CSI <letter>` is used.
    let mods = if parsed.len() >= 2 && parsed[0] == 1 {
        xterm_modifier_code(parsed[1])
    } else {
        ModSet::empty()
    };

    if let Some(key) = csi_letter_keycode(final_byte) {
        out.push(InputEvent::Key(make_named_key(key, mods)));
        return;
    }

    tracing::trace!(final_byte, ?parsed, "unknown CSI sequence");
}

/// Decode an SGR-format mouse report's parameter region (the part after
/// the leading `<` and before the `M`/`m` final byte) and push a
/// [`MouseEvent`] into `out`. Silently drops malformed reports.
///
/// Format: `<btn> ; <col> ; <row>` where `<btn>` is a bitfield
/// (xterm SGR encoding, DEC mode 1006):
///
/// * bits 0-1: low button bits (0=L, 1=M, 2=R, 3=none/release-for-X10).
/// * bit 2:    Shift modifier.
/// * bit 3:    Alt (Meta) modifier.
/// * bit 4:    Ctrl modifier.
/// * bit 5:    motion (the report describes a drag / hover).
/// * bit 6:    wheel — buttons 4 (up) / 5 (down). High bit is the wheel
///   axis indicator in some terminals; we treat 64/65/66/67 as
///   the four wheel directions and pass them through as
///   libghostty `Button::Four` / `Five` / `Six` / `Seven`.
/// * bit 7:    extra buttons — 128..=131 → `Button::Eight..Eleven`.
///
/// `<col>` and `<row>` are 1-indexed cell coordinates. We convert to
/// 0-indexed `f64` pixels (treating "1 cell = 1 pixel" since the client
/// does not know cell-size here; per SPEC §9.2.1 the server's encoder
/// re-quantises for cell-format protocols).
fn dispatch_sgr_mouse(body: &[u8], final_byte: u8, out: &mut Vec<InputEvent>) {
    let parsed = parse_csi_params(body);
    if parsed.len() < 3 {
        tracing::trace!(?parsed, "malformed SGR mouse report (too few params)");
        return;
    }
    let raw_btn = parsed[0];
    let col = parsed[1];
    let row = parsed[2];

    let mods = sgr_mouse_mods(raw_btn);
    let button = sgr_mouse_button(raw_btn);
    let motion = (raw_btn & 0x20) != 0;
    let action = if motion {
        MouseAction::Motion
    } else if final_byte == b'm' {
        MouseAction::Release
    } else {
        MouseAction::Press
    };

    // 1-indexed → 0-indexed; coordinates are pane-local pixels per
    // SPEC §9.2.1 (integer-valued f64 from a cell-quantising client).
    #[allow(clippy::cast_lossless, reason = "u32 → f64 is exact for our range")]
    let x = (col.saturating_sub(1)) as f64;
    #[allow(clippy::cast_lossless, reason = "u32 → f64 is exact for our range")]
    let y = (row.saturating_sub(1)) as f64;

    out.push(InputEvent::Mouse(MouseEvent {
        action,
        button,
        mods,
        x,
        y,
    }));
}

/// Modifier bits in an SGR mouse button code.
fn sgr_mouse_mods(raw: u32) -> ModSet {
    let mut mods = ModSet::empty();
    if raw & 0x04 != 0 {
        mods |= ModSet::SHIFT;
    }
    if raw & 0x08 != 0 {
        mods |= ModSet::ALT;
    }
    if raw & 0x10 != 0 {
        mods |= ModSet::CTRL;
    }
    mods
}

/// Map the low / high button bits of an SGR mouse code to a
/// [`MouseButton`]. Returns `Button::Unknown` for the "no button"
/// motion case (`raw & 3 == 3`, motion bit set without a button).
const fn sgr_mouse_button(raw: u32) -> MouseButton {
    // Wheel reports: bit 6 set, low bits select axis/direction.
    if raw & 0x40 != 0 {
        return match raw & 0x03 {
            0 => MouseButton::Four,  // wheel up
            1 => MouseButton::Five,  // wheel down
            2 => MouseButton::Six,   // wheel left
            _ => MouseButton::Seven, // wheel right
        };
    }
    // Extra buttons: bit 7 set. xterm's "additional buttons" 8..=11.
    if raw & 0x80 != 0 {
        return match raw & 0x03 {
            0 => MouseButton::Eight,
            1 => MouseButton::Nine,
            2 => MouseButton::Ten,
            _ => MouseButton::Eleven,
        };
    }
    match raw & 0x03 {
        0 => MouseButton::Left,
        1 => MouseButton::Middle,
        2 => MouseButton::Right,
        // Low 2 bits = 3 means "no button" (motion-without-button, or
        // explicit release in the legacy X10 form). SGR mode disambiguates
        // press vs. release via the `M`/`m` final byte; in either case
        // we report Unknown so the server-side encoder reconstructs the
        // correct PTY bytes from the action + position.
        _ => MouseButton::Unknown,
    }
}

/// Parse semicolon-separated unsigned integers out of CSI parameter bytes.
/// Empty / non-digit slots become 0.
fn parse_csi_params(body: &[u8]) -> Vec<u32> {
    let mut out = Vec::new();
    let mut acc: u32 = 0;
    let mut started = false;
    for &b in body {
        match b {
            b'0'..=b'9' => {
                acc = acc.saturating_mul(10).saturating_add(u32::from(b - b'0'));
                started = true;
            }
            b';' | b':' => {
                out.push(if started { acc } else { 0 });
                acc = 0;
                started = false;
            }
            _ => {
                // Intermediate / unrecognised byte; treat as separator.
                out.push(if started { acc } else { 0 });
                acc = 0;
                started = false;
            }
        }
    }
    out.push(if started { acc } else { 0 });
    out
}

/// xterm's `modifyCursorKeys` / `modifyFunctionKeys` modifier code →
/// [`ModSet`]. The code is (1 + sum of bit-weights) where Shift=1, Alt=2,
/// Ctrl=4, Super=8. Code 1 means no modifier.
fn xterm_modifier_code(code: u32) -> ModSet {
    if code == 0 {
        return ModSet::empty();
    }
    let bits = code.saturating_sub(1);
    let mut mods = ModSet::empty();
    if bits & 0b0001 != 0 {
        mods |= ModSet::SHIFT;
    }
    if bits & 0b0010 != 0 {
        mods |= ModSet::ALT;
    }
    if bits & 0b0100 != 0 {
        mods |= ModSet::CTRL;
    }
    if bits & 0b1000 != 0 {
        mods |= ModSet::SUPER;
    }
    mods
}

/// `CSI <letter>` keycodes. Returns `None` for letters we don't recognise
/// (e.g. `CSI M` mouse reports, which we skip pending follow-up).
const fn csi_letter_keycode(final_byte: u8) -> Option<PhysicalKey> {
    Some(match final_byte {
        b'A' => PhysicalKey::ArrowUp,
        b'B' => PhysicalKey::ArrowDown,
        b'C' => PhysicalKey::ArrowRight,
        b'D' => PhysicalKey::ArrowLeft,
        b'F' => PhysicalKey::End,
        b'H' => PhysicalKey::Home,
        b'P' => PhysicalKey::F1,
        b'Q' => PhysicalKey::F2,
        b'R' => PhysicalKey::F3,
        b'S' => PhysicalKey::F4,
        b'Z' => PhysicalKey::Tab, // CSI Z = Shift-Tab; modifier filled by caller
        _ => return None,
    })
}

/// `CSI <n> ~` keycodes per the VT220 / xterm conventions.
const fn csi_tilde_keycode(n: u32) -> Option<PhysicalKey> {
    Some(match n {
        1 | 7 => PhysicalKey::Home,
        2 => PhysicalKey::Insert,
        3 => PhysicalKey::Delete,
        4 | 8 => PhysicalKey::End,
        5 => PhysicalKey::PageUp,
        6 => PhysicalKey::PageDown,
        11 | 15 => PhysicalKey::F5, // 11=F1 on linux-console but xterm uses 15=F5; either-or
        13 => PhysicalKey::F3,
        14 => PhysicalKey::F4,
        17 => PhysicalKey::F6,
        18 => PhysicalKey::F7,
        19 => PhysicalKey::F8,
        20 => PhysicalKey::F9,
        21 => PhysicalKey::F10,
        23 => PhysicalKey::F11,
        24 => PhysicalKey::F12,
        25 => PhysicalKey::F13,
        26 => PhysicalKey::F14,
        28 => PhysicalKey::F15,
        29 => PhysicalKey::F16,
        31 => PhysicalKey::F17,
        32 => PhysicalKey::F18,
        33 => PhysicalKey::F19,
        34 => PhysicalKey::F20,
        _ => return None,
    })
}

/// Map ASCII uppercase letter bytes to libghostty's `PhysicalKey` variants.
const fn ascii_letter_to_key(b: u8) -> Option<PhysicalKey> {
    if !b.is_ascii_uppercase() {
        return None;
    }
    Some(ascii_letter_to_key_const(b))
}

const fn ascii_letter_to_key_const(b: u8) -> PhysicalKey {
    match b {
        b'A' => PhysicalKey::A,
        b'B' => PhysicalKey::B,
        b'C' => PhysicalKey::C,
        b'D' => PhysicalKey::D,
        b'E' => PhysicalKey::E,
        b'F' => PhysicalKey::F,
        b'G' => PhysicalKey::G,
        b'H' => PhysicalKey::H,
        b'I' => PhysicalKey::I,
        b'J' => PhysicalKey::J,
        b'K' => PhysicalKey::K,
        b'L' => PhysicalKey::L,
        b'M' => PhysicalKey::M,
        b'N' => PhysicalKey::N,
        b'O' => PhysicalKey::O,
        b'P' => PhysicalKey::P,
        b'Q' => PhysicalKey::Q,
        b'R' => PhysicalKey::R,
        b'S' => PhysicalKey::S,
        b'T' => PhysicalKey::T,
        b'U' => PhysicalKey::U,
        b'V' => PhysicalKey::V,
        b'W' => PhysicalKey::W,
        b'X' => PhysicalKey::X,
        b'Y' => PhysicalKey::Y,
        b'Z' => PhysicalKey::Z,
        _ => PhysicalKey::Unidentified,
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, reason = "tests")]
mod tests {
    use super::*;

    fn key_only(evs: &[InputEvent]) -> Vec<&KeyEvent> {
        evs.iter()
            .filter_map(|e| {
                if let InputEvent::Key(k) = e {
                    Some(k)
                } else {
                    None
                }
            })
            .collect()
    }

    // ---- Plain bytes -----------------------------------------------------

    #[test]
    fn printable_byte_becomes_key_event_with_text() {
        let mut p = StdinParser::new();
        let evs = p.feed(b"a");
        let keys = key_only(&evs);
        assert_eq!(keys.len(), 1);
        assert_eq!(keys[0].text.as_deref(), Some("a"));
        assert_eq!(keys[0].action, KeyAction::Press);
        assert_eq!(keys[0].key, PhysicalKey::A);
    }

    #[test]
    fn uppercase_carries_shift_mod() {
        let mut p = StdinParser::new();
        let evs = p.feed(b"A");
        let keys = key_only(&evs);
        assert_eq!(keys.len(), 1);
        assert!(keys[0].mods.contains(ModSet::SHIFT));
        assert!(keys[0].consumed_mods.contains(ModSet::SHIFT));
        assert_eq!(keys[0].unshifted_codepoint, Some(u32::from('a')));
    }

    #[test]
    fn enter_byte_becomes_enter_key() {
        let mut p = StdinParser::new();
        let evs = p.feed(b"\r");
        let keys = key_only(&evs);
        assert_eq!(keys[0].key, PhysicalKey::Enter);
    }

    #[test]
    fn ctrl_c_byte_becomes_ctrl_modified_c() {
        let mut p = StdinParser::new();
        let evs = p.feed(&[0x03]);
        let keys = key_only(&evs);
        assert_eq!(keys[0].key, PhysicalKey::C);
        assert!(keys[0].mods.contains(ModSet::CTRL));
    }

    // ---- UTF-8 ----------------------------------------------------------

    #[test]
    fn two_byte_utf8_becomes_one_key_event() {
        let mut p = StdinParser::new();
        // U+00E9 LATIN SMALL LETTER E WITH ACUTE → 0xC3 0xA9
        let evs = p.feed(&[0xC3, 0xA9]);
        let keys = key_only(&evs);
        assert_eq!(keys.len(), 1);
        assert_eq!(keys[0].text.as_deref(), Some("é"));
        assert_eq!(keys[0].unshifted_codepoint, Some(0x00E9));
    }

    #[test]
    fn three_byte_utf8_across_two_feeds() {
        let mut p = StdinParser::new();
        // U+1F600 GRINNING FACE → 0xF0 0x9F 0x98 0x80
        let first = p.feed(&[0xF0, 0x9F]);
        assert!(first.is_empty());
        let second = p.feed(&[0x98, 0x80]);
        let keys = key_only(&second);
        assert_eq!(keys.len(), 1);
        assert_eq!(keys[0].text.as_deref(), Some("😀"));
    }

    #[test]
    fn invalid_utf8_continuation_recovers() {
        let mut p = StdinParser::new();
        // Lead 0xC3 expects one continuation; give it 'a' instead.
        let evs = p.feed(&[0xC3, b'a']);
        // We expect `a` to come through after the bad sequence is dropped.
        let keys = key_only(&evs);
        assert_eq!(keys.len(), 1);
        assert_eq!(keys[0].text.as_deref(), Some("a"));
    }

    // ---- Detach chord ---------------------------------------------------

    #[test]
    fn detach_chord_emits_detach_requested() {
        let mut p = StdinParser::new();
        let evs = p.feed(&[DETACH_PREFIX, b'd']);
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0], InputEvent::DetachRequested);
    }

    #[test]
    fn detach_chord_across_two_feeds() {
        let mut p = StdinParser::new();
        let first = p.feed(&[DETACH_PREFIX]);
        assert!(first.is_empty(), "prefix alone should not emit");
        let second = p.feed(b"d");
        assert_eq!(second, vec![InputEvent::DetachRequested]);
    }

    #[test]
    fn ctrl_b_followed_by_non_d_drops_prefix() {
        let mut p = StdinParser::new();
        let evs = p.feed(&[DETACH_PREFIX, b'a']);
        let keys = key_only(&evs);
        assert_eq!(keys.len(), 1);
        assert_eq!(keys[0].text.as_deref(), Some("a"));
    }

    // ---- Bare ESC / Alt-chord (timing) ----------------------------------

    #[test]
    fn esc_byte_alone_does_not_emit_immediately() {
        let mut p = StdinParser::new();
        let evs = p.feed(&[0x1B]);
        assert!(evs.is_empty(), "bare ESC must wait for flush");
        assert!(p.has_pending());
    }

    #[test]
    fn esc_byte_then_flush_emits_escape_key() {
        let mut p = StdinParser::new();
        let _ = p.feed(&[0x1B]);
        let flushed = p.flush();
        let keys = key_only(&flushed);
        assert_eq!(keys.len(), 1);
        assert_eq!(keys[0].key, PhysicalKey::Escape);
        assert!(!p.has_pending());
    }

    #[test]
    fn esc_then_char_in_same_feed_is_alt_chord() {
        let mut p = StdinParser::new();
        let evs = p.feed(&[0x1B, b'a']);
        let keys = key_only(&evs);
        assert_eq!(keys.len(), 1);
        assert_eq!(keys[0].key, PhysicalKey::A);
        assert!(keys[0].mods.contains(ModSet::ALT));
    }

    #[test]
    fn esc_then_char_across_two_feeds_is_alt_chord() {
        let mut p = StdinParser::new();
        let first = p.feed(&[0x1B]);
        assert!(first.is_empty());
        let second = p.feed(b"x");
        let keys = key_only(&second);
        assert_eq!(keys.len(), 1);
        assert_eq!(keys[0].key, PhysicalKey::X);
        assert!(keys[0].mods.contains(ModSet::ALT));
    }

    #[test]
    fn double_esc_emits_escape_then_pending() {
        let mut p = StdinParser::new();
        let evs = p.feed(&[0x1B, 0x1B]);
        let keys = key_only(&evs);
        assert_eq!(keys.len(), 1);
        assert_eq!(keys[0].key, PhysicalKey::Escape);
        assert!(p.has_pending());
    }

    // ---- CSI arrow keys -------------------------------------------------

    #[test]
    fn csi_arrow_up_unmod() {
        let mut p = StdinParser::new();
        let evs = p.feed(b"\x1b[A");
        let keys = key_only(&evs);
        assert_eq!(keys.len(), 1);
        assert_eq!(keys[0].key, PhysicalKey::ArrowUp);
        assert!(keys[0].mods.is_empty());
    }

    #[test]
    fn csi_ctrl_arrow_up() {
        let mut p = StdinParser::new();
        let evs = p.feed(b"\x1b[1;5A");
        let keys = key_only(&evs);
        assert_eq!(keys.len(), 1);
        assert_eq!(keys[0].key, PhysicalKey::ArrowUp);
        assert!(keys[0].mods.contains(ModSet::CTRL));
        assert!(!keys[0].mods.contains(ModSet::SHIFT));
    }

    #[test]
    fn csi_shift_alt_ctrl_arrow_right() {
        let mut p = StdinParser::new();
        // mod = 1 + (Shift=1 + Alt=2 + Ctrl=4) = 8
        let evs = p.feed(b"\x1b[1;8C");
        let keys = key_only(&evs);
        assert_eq!(keys.len(), 1);
        assert_eq!(keys[0].key, PhysicalKey::ArrowRight);
        assert!(keys[0].mods.contains(ModSet::SHIFT));
        assert!(keys[0].mods.contains(ModSet::ALT));
        assert!(keys[0].mods.contains(ModSet::CTRL));
    }

    // ---- CSI tilde-form keys --------------------------------------------

    #[test]
    fn csi_home_end_via_letter_and_tilde() {
        let mut p = StdinParser::new();
        let letter = p.feed(b"\x1b[H");
        assert_eq!(key_only(&letter)[0].key, PhysicalKey::Home);
        let tilde = p.feed(b"\x1b[1~");
        assert_eq!(key_only(&tilde)[0].key, PhysicalKey::Home);
    }

    #[test]
    fn csi_pageup_unmod_and_ctrl() {
        let mut p = StdinParser::new();
        let unmod = p.feed(b"\x1b[5~");
        assert_eq!(key_only(&unmod)[0].key, PhysicalKey::PageUp);
        let ctrl = p.feed(b"\x1b[5;5~");
        let keys = key_only(&ctrl);
        assert_eq!(keys[0].key, PhysicalKey::PageUp);
        assert!(keys[0].mods.contains(ModSet::CTRL));
    }

    #[test]
    fn csi_f5_through_f12() {
        let cases = [
            (b"\x1b[15~".as_slice(), PhysicalKey::F5),
            (b"\x1b[17~", PhysicalKey::F6),
            (b"\x1b[18~", PhysicalKey::F7),
            (b"\x1b[19~", PhysicalKey::F8),
            (b"\x1b[20~", PhysicalKey::F9),
            (b"\x1b[21~", PhysicalKey::F10),
            (b"\x1b[23~", PhysicalKey::F11),
            (b"\x1b[24~", PhysicalKey::F12),
        ];
        for (input, expected) in cases {
            let mut p = StdinParser::new();
            let evs = p.feed(input);
            let keys = key_only(&evs);
            assert_eq!(keys.len(), 1, "input {input:?} produced {evs:?}");
            assert_eq!(keys[0].key, expected, "input {input:?}");
        }
    }

    // ---- SS3 ------------------------------------------------------------

    #[test]
    fn ss3_f1_through_f4() {
        let cases = [
            (b"\x1bOP".as_slice(), PhysicalKey::F1),
            (b"\x1bOQ", PhysicalKey::F2),
            (b"\x1bOR", PhysicalKey::F3),
            (b"\x1bOS", PhysicalKey::F4),
        ];
        for (input, expected) in cases {
            let mut p = StdinParser::new();
            let evs = p.feed(input);
            let keys = key_only(&evs);
            assert_eq!(keys.len(), 1, "input {input:?}");
            assert_eq!(keys[0].key, expected, "input {input:?}");
        }
    }

    #[test]
    fn ss3_arrow_keys_in_application_mode() {
        let mut p = StdinParser::new();
        let evs = p.feed(b"\x1bOA");
        let keys = key_only(&evs);
        assert_eq!(keys.len(), 1);
        assert_eq!(keys[0].key, PhysicalKey::ArrowUp);
    }

    // ---- Partial CSI across reads ---------------------------------------

    #[test]
    fn csi_split_across_three_feeds() {
        let mut p = StdinParser::new();
        let a = p.feed(b"\x1b");
        assert!(a.is_empty());
        let b = p.feed(b"[1;");
        assert!(b.is_empty());
        let c = p.feed(b"5A");
        let keys = key_only(&c);
        assert_eq!(keys.len(), 1);
        assert_eq!(keys[0].key, PhysicalKey::ArrowUp);
        assert!(keys[0].mods.contains(ModSet::CTRL));
    }

    #[test]
    fn partial_csi_keeps_pending_flag() {
        let mut p = StdinParser::new();
        let _ = p.feed(b"\x1b[1;");
        assert!(p.has_pending());
        // flush() must NOT consume a half-finished CSI.
        let flushed = p.flush();
        assert!(flushed.is_empty());
        assert!(p.has_pending());
        let final_evs = p.feed(b"5A");
        assert_eq!(key_only(&final_evs).len(), 1);
    }

    // ---- Misc -----------------------------------------------------------

    #[test]
    fn over_long_csi_aborts_cleanly() {
        let mut p = StdinParser::new();
        let mut s = Vec::from(b"\x1b[".as_slice());
        s.extend(std::iter::repeat_n(b'1', 300));
        s.push(b'A');
        let _ = p.feed(&s);
        assert!(!p.has_pending(), "parser must recover after overflow");
    }

    #[test]
    fn unknown_csi_final_is_dropped_silently() {
        let mut p = StdinParser::new();
        // CSI M is mouse — not supported yet, must not crash. The mouse
        // payload bytes that follow leak through as ground-state "key
        // events" today because we don't yet recognise mouse final bytes;
        // mouse parsing is the explicit follow-up to this ticket. The
        // contract here is just that the parser doesn't panic and returns
        // to the ground state cleanly.
        let _ = p.feed(b"\x1b[M\x20\x20\x20");
        assert!(!p.has_pending());
    }

    #[test]
    fn alt_letter_strips_text() {
        let mut p = StdinParser::new();
        let evs = p.feed(b"\x1ba");
        let keys = key_only(&evs);
        assert_eq!(keys[0].text, None);
        assert!(keys[0].mods.contains(ModSet::ALT));
    }

    #[test]
    fn into_frame_carries_terminal_id() {
        let key = KeyEvent {
            action: KeyAction::Press,
            key: PhysicalKey::A,
            mods: ModSet::empty(),
            consumed_mods: ModSet::empty(),
            composing: false,
            text: Some("a".to_owned()),
            unshifted_codepoint: Some(u32::from('a')),
        };
        let frame = InputEvent::Key(key).into_frame(42).expect("frame");
        match frame {
            FrameKind::InputKey { terminal_id, .. } => assert_eq!(terminal_id, 42),
            other => panic!("expected InputKey, got {other:?}"),
        }
    }

    #[test]
    fn detach_requested_has_no_frame() {
        assert!(InputEvent::DetachRequested.into_frame(1).is_none());
    }

    // ---- Focus reports (DEC mode 1004) ---------------------------------

    fn focus_only(evs: &[InputEvent]) -> Vec<FocusEvent> {
        evs.iter()
            .filter_map(|e| {
                if let InputEvent::Focus(f) = e {
                    Some(*f)
                } else {
                    None
                }
            })
            .collect()
    }

    #[test]
    fn csi_capital_i_is_focus_gained() {
        let mut p = StdinParser::new();
        let evs = p.feed(b"\x1b[I");
        let f = focus_only(&evs);
        assert_eq!(f, vec![FocusEvent::Gained]);
    }

    #[test]
    fn csi_capital_o_is_focus_lost() {
        let mut p = StdinParser::new();
        let evs = p.feed(b"\x1b[O");
        let f = focus_only(&evs);
        assert_eq!(f, vec![FocusEvent::Lost]);
    }

    #[test]
    fn focus_event_into_frame_carries_terminal_id() {
        let frame = InputEvent::Focus(FocusEvent::Gained)
            .into_frame(7)
            .expect("frame");
        match frame {
            FrameKind::InputFocus { terminal_id, event } => {
                assert_eq!(terminal_id, 7);
                assert_eq!(event, FocusEvent::Gained);
            }
            other => panic!("expected InputFocus, got {other:?}"),
        }
    }

    #[test]
    fn ss3_capital_o_still_routes_via_ss3_not_focus() {
        // ESC O P is SS3 F1, not focus. Ensures the new focus dispatch
        // didn't accidentally consume the SS3 path.
        let mut p = StdinParser::new();
        let evs = p.feed(b"\x1bOP");
        let keys = key_only(&evs);
        assert_eq!(keys.len(), 1);
        assert_eq!(keys[0].key, PhysicalKey::F1);
        assert!(focus_only(&evs).is_empty());
    }

    // ---- SGR mouse reports (DEC mode 1006) ------------------------------

    fn mouse_only(evs: &[InputEvent]) -> Vec<MouseEvent> {
        evs.iter()
            .filter_map(|e| {
                if let InputEvent::Mouse(m) = e {
                    Some(*m)
                } else {
                    None
                }
            })
            .collect()
    }

    #[test]
    fn sgr_left_press_and_release() {
        let mut p = StdinParser::new();
        let press = p.feed(b"\x1b[<0;5;3M");
        let m = mouse_only(&press);
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].action, MouseAction::Press);
        assert_eq!(m[0].button, MouseButton::Left);
        assert!(m[0].mods.is_empty());
        // 1-indexed → 0-indexed: col 5 → 4.0, row 3 → 2.0.
        assert!((m[0].x - 4.0).abs() < f64::EPSILON);
        assert!((m[0].y - 2.0).abs() < f64::EPSILON);

        let release = p.feed(b"\x1b[<0;5;3m");
        let m2 = mouse_only(&release);
        assert_eq!(m2.len(), 1);
        assert_eq!(m2[0].action, MouseAction::Release);
        assert_eq!(m2[0].button, MouseButton::Left);
    }

    #[test]
    fn sgr_right_middle_buttons() {
        let mut p = StdinParser::new();
        let right = p.feed(b"\x1b[<2;1;1M");
        assert_eq!(mouse_only(&right)[0].button, MouseButton::Right);
        let middle = p.feed(b"\x1b[<1;1;1M");
        assert_eq!(mouse_only(&middle)[0].button, MouseButton::Middle);
    }

    #[test]
    fn sgr_motion_with_button() {
        let mut p = StdinParser::new();
        // bit 5 (0x20) is motion. 0x20 | 0 = 32 → Left button drag.
        let evs = p.feed(b"\x1b[<32;10;5M");
        let m = mouse_only(&evs);
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].action, MouseAction::Motion);
        assert_eq!(m[0].button, MouseButton::Left);
    }

    #[test]
    fn sgr_motion_no_button() {
        let mut p = StdinParser::new();
        // 0x20 (motion) | 0x03 (no-button) = 35.
        let evs = p.feed(b"\x1b[<35;1;1M");
        let m = mouse_only(&evs);
        assert_eq!(m[0].action, MouseAction::Motion);
        assert_eq!(m[0].button, MouseButton::Unknown);
    }

    #[test]
    fn sgr_wheel_up_down() {
        let mut p = StdinParser::new();
        // 64 = wheel up, 65 = wheel down.
        let up = p.feed(b"\x1b[<64;1;1M");
        assert_eq!(mouse_only(&up)[0].button, MouseButton::Four);
        let down = p.feed(b"\x1b[<65;1;1M");
        assert_eq!(mouse_only(&down)[0].button, MouseButton::Five);
    }

    #[test]
    fn sgr_with_modifiers() {
        let mut p = StdinParser::new();
        // 0 (Left) | 4 (Shift) | 8 (Alt) | 16 (Ctrl) = 28.
        let evs = p.feed(b"\x1b[<28;1;1M");
        let m = mouse_only(&evs);
        assert_eq!(m[0].button, MouseButton::Left);
        assert!(m[0].mods.contains(ModSet::SHIFT));
        assert!(m[0].mods.contains(ModSet::ALT));
        assert!(m[0].mods.contains(ModSet::CTRL));
    }

    #[test]
    fn sgr_mouse_split_across_feeds() {
        let mut parser = StdinParser::new();
        let first = parser.feed(b"\x1b[<0");
        assert!(mouse_only(&first).is_empty());
        let middle = parser.feed(b";5;3");
        assert!(mouse_only(&middle).is_empty());
        let last = parser.feed(b"M");
        let evs = mouse_only(&last);
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0].action, MouseAction::Press);
    }

    #[test]
    fn sgr_mouse_into_frame_carries_terminal_id() {
        let ev = MouseEvent {
            action: MouseAction::Press,
            button: MouseButton::Left,
            mods: ModSet::empty(),
            x: 1.0,
            y: 2.0,
        };
        let frame = InputEvent::Mouse(ev).into_frame(99).expect("frame");
        match frame {
            FrameKind::InputMouse { terminal_id, .. } => assert_eq!(terminal_id, 99),
            other => panic!("expected InputMouse, got {other:?}"),
        }
    }

    // ---- Bracketed paste (DEC mode 2004) -------------------------------

    fn paste_only(evs: &[InputEvent]) -> Vec<PasteEvent> {
        evs.iter()
            .filter_map(|e| {
                if let InputEvent::Paste(p) = e {
                    Some(p.clone())
                } else {
                    None
                }
            })
            .collect()
    }

    #[test]
    fn bracketed_paste_basic_round_trip() {
        let mut p = StdinParser::new();
        let evs = p.feed(b"\x1b[200~hello world\x1b[201~");
        let pastes = paste_only(&evs);
        assert_eq!(pastes.len(), 1);
        assert_eq!(pastes[0].data, b"hello world");
        assert_eq!(pastes[0].trust, PasteTrust::Untrusted);
        // No key events leaked from inside the brackets.
        assert!(key_only(&evs).is_empty());
        assert!(!p.has_pending());
    }

    #[test]
    fn bracketed_paste_split_across_feeds() {
        let mut p = StdinParser::new();
        let a = p.feed(b"\x1b[200~");
        assert!(a.is_empty());
        assert!(p.has_pending());
        let b = p.feed(b"abc");
        assert!(b.is_empty());
        let c = p.feed(b"\x1b[201~");
        let pastes = paste_only(&c);
        assert_eq!(pastes.len(), 1);
        assert_eq!(pastes[0].data, b"abc");
    }

    #[test]
    fn bracketed_paste_payload_with_inner_csi() {
        let mut p = StdinParser::new();
        // User pastes a string that contains an ANSI color escape — the
        // ESC + CSI inside the payload must be preserved, not consumed
        // as a close marker.
        let payload = b"red\x1b[31mthing\x1b[0mend";
        let mut s = Vec::from(b"\x1b[200~".as_slice());
        s.extend_from_slice(payload);
        s.extend_from_slice(b"\x1b[201~");
        let evs = p.feed(&s);
        let pastes = paste_only(&evs);
        assert_eq!(pastes.len(), 1);
        assert_eq!(pastes[0].data, payload);
        assert!(!p.has_pending());
    }

    #[test]
    fn bracketed_paste_with_bare_esc_in_payload() {
        // A bare ESC inside the paste should remain in the payload.
        let mut p = StdinParser::new();
        let mut s = Vec::from(b"\x1b[200~".as_slice());
        s.extend_from_slice(b"a\x1bb");
        s.extend_from_slice(b"\x1b[201~");
        let evs = p.feed(&s);
        let pastes = paste_only(&evs);
        assert_eq!(pastes.len(), 1);
        assert_eq!(pastes[0].data, b"a\x1bb");
    }

    #[test]
    fn bracketed_paste_into_frame_carries_terminal_id() {
        let frame = InputEvent::Paste(PasteEvent {
            trust: PasteTrust::Untrusted,
            data: b"x".to_vec(),
        })
        .into_frame(11)
        .expect("frame");
        match frame {
            FrameKind::InputPaste { terminal_id, event } => {
                assert_eq!(terminal_id, 11);
                assert_eq!(event.data, b"x");
            }
            other => panic!("expected InputPaste, got {other:?}"),
        }
    }

    // ---- Sanity: existing "unknown CSI" path still recovers -------------

    #[test]
    fn xterm_modifier_code_table() {
        assert_eq!(xterm_modifier_code(1), ModSet::empty());
        assert_eq!(xterm_modifier_code(2), ModSet::SHIFT);
        assert_eq!(xterm_modifier_code(3), ModSet::ALT);
        assert_eq!(xterm_modifier_code(5), ModSet::CTRL);
        assert_eq!(xterm_modifier_code(9), ModSet::SUPER);
        assert_eq!(
            xterm_modifier_code(8),
            ModSet::SHIFT | ModSet::ALT | ModSet::CTRL
        );
    }
}
