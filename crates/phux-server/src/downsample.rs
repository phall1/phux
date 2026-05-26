//! Per-client VT byte-stream rewriter (SPEC §6.2, [ADR-0013]).
//!
//! Under ADR-0013 the server forwards raw PTY bytes to each subscribed
//! client as `PANE_OUTPUT` frames. Per SPEC §6.2 those bytes MUST be
//! adapted to the client's advertised
//! [`ColorSupport`](phux_protocol::caps::ColorSupport) before forwarding —
//! truecolor SGR sequences `CSI 38;2;R;G;B m` and `CSI 48;2;R;G;B m`
//! become their indexed-palette equivalents.
//!
//! This module owns that transformation. The rewriter is a partial VT
//! parser: it recognises CSI introducers, captures the parameter list,
//! and on an `m` (SGR) terminator rewrites any embedded truecolor
//! sub-sequence. Every other escape (CSI for non-SGR finals, OSC, DCS,
//! APC, SOS/PM, two-byte escapes) is passed through verbatim — phux is
//! a multiplexer, not a sanitiser, and unknown sequences must reach the
//! client byte-for-byte.
//!
//! ## Scope (v0)
//!
//! - Truecolor → 256 / 16: implemented.
//! - Image protocols (sixel `DCS Pi; q`, kitty graphics `APC G`, iTerm2
//!   `OSC 1337`): out of scope for this commit; the OSC/DCS/APC
//!   passthrough is intentionally permissive. Follow-up:
//!   "`phux-server::downsample`: handle image protocols + keyboard
//!   protocol gating".
//! - Kitty keyboard protocol APC replies: same follow-up.
//! - OSC 8 hyperlinks stripping: same follow-up.
//! - ITU-style colon separators (`CSI 38:2::R:G:B m`): not handled —
//!   the rewriter only recognises `;`. tmux's implementation handles
//!   both; the follow-up ticket covers parity.
//!
//! [ADR-0013]: https://github.com/phall1/phux/blob/main/ADR/0013-libghostty-bytes-on-wire.md

use phux_protocol::caps::ColorSupport;

/// Rewrite an outbound VT byte stream to fit `support`.
///
/// Returns a new buffer with truecolor SGR sequences quantised to the
/// target palette. Non-SGR escapes and non-color SGR parameters pass
/// through unchanged. The fast path for [`ColorSupport::TrueColor`] is
/// a single allocation+copy; the rewriter's hot loop never runs.
#[must_use]
pub fn rewrite_bytes(input: &[u8], support: ColorSupport) -> Vec<u8> {
    if matches!(support, ColorSupport::TrueColor) {
        return input.to_vec();
    }

    let mut out = Vec::with_capacity(input.len());
    let mut i = 0;
    while i < input.len() {
        if input[i] != ESC {
            out.push(input[i]);
            i += 1;
            continue;
        }
        // ESC at end of input — emit verbatim.
        if i + 1 >= input.len() {
            out.push(ESC);
            i += 1;
            continue;
        }
        match input[i + 1] {
            b'[' => {
                i = handle_csi(input, i, support, &mut out);
            }
            b']' | b'P' | b'_' | b'^' | b'X' => {
                i = passthrough_string_terminated(input, i, &mut out);
            }
            _ => {
                // Two-byte escape: ESC X. Pass through and advance.
                out.push(ESC);
                out.push(input[i + 1]);
                i += 2;
            }
        }
    }
    out
}

/// ASCII escape (start of all VT control sequences).
const ESC: u8 = 0x1B;
/// ASCII bell / OSC string terminator.
const BEL: u8 = 0x07;

/// Handle a CSI sequence starting at `input[start] == ESC` with
/// `input[start + 1] == '['`. Returns the new position past the sequence.
///
/// Only `CSI ... m` (SGR) is structurally inspected; every other CSI
/// final byte passes through verbatim. The parameter byte set per
/// ECMA-48 is `0x30..=0x3F`; intermediates are `0x20..=0x2F`; the final
/// byte is `0x40..=0x7E`. We treat any byte outside the param/intermediate
/// range as the final byte, which matches what real terminals do on
/// truncated/malformed input.
fn handle_csi(input: &[u8], start: usize, support: ColorSupport, out: &mut Vec<u8>) -> usize {
    let csi_body_start = start + 2; // past ESC [
    let mut j = csi_body_start;
    while j < input.len() {
        let b = input[j];
        // Parameter bytes (0x30..=0x3F) and intermediates (0x20..=0x2F)
        // belong to the sequence body; anything else is the final byte.
        if (0x30..=0x3F).contains(&b) || (0x20..=0x2F).contains(&b) {
            j += 1;
            continue;
        }
        break;
    }
    if j >= input.len() {
        // Incomplete CSI; emit verbatim and stop.
        out.extend_from_slice(&input[start..]);
        return input.len();
    }
    let final_byte = input[j];
    if final_byte == b'm' {
        rewrite_sgr(&input[csi_body_start..j], support, out);
    } else {
        // Non-SGR CSI — passthrough including the final byte.
        out.extend_from_slice(&input[start..=j]);
    }
    j + 1
}

/// Passthrough an OSC/DCS/APC/SOS/PM sequence starting at `input[start]`
/// (`ESC` followed by `]`/`P`/`_`/`^`/`X`). Returns the new position past
/// the string terminator (`BEL` or `ESC \\`).
fn passthrough_string_terminated(input: &[u8], start: usize, out: &mut Vec<u8>) -> usize {
    let mut j = start + 2;
    while j < input.len() {
        if input[j] == BEL {
            j += 1;
            break;
        }
        if input[j] == ESC && j + 1 < input.len() && input[j + 1] == b'\\' {
            j += 2;
            break;
        }
        j += 1;
    }
    out.extend_from_slice(&input[start..j]);
    j
}

/// Rewrite the parameter bytes of an SGR sequence (everything between
/// `CSI` and the terminating `m`), emitting `CSI <rewritten> m` into `out`.
///
/// The grammar handled here is a `;`-separated list of decimal parameters;
/// embedded `38;2;R;G;B` (fg truecolor) and `48;2;R;G;B` (bg truecolor)
/// sub-sequences are quantised to `38;5;N` / `48;5;N` (Indexed256) or
/// `3N` / `4N` (Indexed16). All other parameters pass through verbatim.
///
/// Empty parameter strings (`CSI m` ≡ `CSI 0 m`) survive: the resulting
/// SGR is emitted with the same empty parameter list, which terminals
/// treat as a reset.
fn rewrite_sgr(params: &[u8], support: ColorSupport, out: &mut Vec<u8>) {
    out.extend_from_slice(b"\x1b[");
    let parts = split_sgr_params(params);

    let mut first = true;
    let mut i = 0;
    while i < parts.len() {
        let p = parts[i];
        // Truecolor fg: 38; 2; R; G; B
        if i + 4 < parts.len()
            && p == Some(38)
            && parts[i + 1] == Some(2)
            && let (Some(r), Some(g), Some(b)) = (parts[i + 2], parts[i + 3], parts[i + 4])
        {
            let rgb = [clamp_u8(r), clamp_u8(g), clamp_u8(b)];
            emit_color_params(rgb, true, support, out, &mut first);
            i += 5;
            continue;
        }
        // Truecolor bg: 48; 2; R; G; B
        if i + 4 < parts.len()
            && p == Some(48)
            && parts[i + 1] == Some(2)
            && let (Some(r), Some(g), Some(b)) = (parts[i + 2], parts[i + 3], parts[i + 4])
        {
            let rgb = [clamp_u8(r), clamp_u8(g), clamp_u8(b)];
            emit_color_params(rgb, false, support, out, &mut first);
            i += 5;
            continue;
        }
        // Anything else (including `Some(n)` and `None` empty parameters)
        // passes through.
        if !first {
            out.push(b';');
        }
        first = false;
        if let Some(n) = p {
            // Write decimal without allocating an intermediate string.
            write_decimal(n, out);
        }
        i += 1;
    }
    out.push(b'm');
}

/// Emit either `38;5;N` / `48;5;N` (Indexed256) or `3N` / `9N` /
/// `4N` / `10N` (Indexed16) into `out`, prefixing with `;` if `*first`
/// is false. Updates `*first` to false after emission.
fn emit_color_params(
    rgb: [u8; 3],
    foreground: bool,
    support: ColorSupport,
    out: &mut Vec<u8>,
    first: &mut bool,
) {
    if !*first {
        out.push(b';');
    }
    *first = false;
    match support {
        ColorSupport::TrueColor => {
            // Caller's fast path skips this function for TrueColor; emit
            // verbatim so accidental routing here is at least correct.
            if foreground {
                out.extend_from_slice(b"38;2;");
            } else {
                out.extend_from_slice(b"48;2;");
            }
            write_decimal(u32::from(rgb[0]), out);
            out.push(b';');
            write_decimal(u32::from(rgb[1]), out);
            out.push(b';');
            write_decimal(u32::from(rgb[2]), out);
        }
        ColorSupport::Indexed256 => {
            let idx = nearest_xterm_256(rgb);
            if foreground {
                out.extend_from_slice(b"38;5;");
            } else {
                out.extend_from_slice(b"48;5;");
            }
            write_decimal(u32::from(idx), out);
        }
        ColorSupport::Indexed16 => {
            emit_indexed16(rgb, foreground, out);
        }
        // `ColorSupport` is `#[non_exhaustive]`. Treat any future tier as
        // Indexed16 (most-restrictive) so we never accidentally forward
        // truecolor to a tier we don't yet model.
        _ => emit_indexed16(rgb, foreground, out),
    }
}

fn emit_indexed16(rgb: [u8; 3], foreground: bool, out: &mut Vec<u8>) {
    let idx = nearest_xterm_16(rgb);
    // ANSI 16: 0..=7 are the base; 8..=15 are bright. fg base is 30..=37,
    // bright fg is 90..=97; bg base is 40..=47, bright bg is 100..=107.
    let base: u32 = if foreground {
        if idx < 8 { 30 } else { 90 }
    } else if idx < 8 {
        40
    } else {
        100
    };
    let off = u32::from(idx & 0x7);
    write_decimal(base + off, out);
}

/// Split an SGR parameter list (the bytes between `CSI` and the trailing
/// `m`) into integer parameters.
///
/// `;`-separated. Empty fields decode to `None` (the standard says
/// they default to `0`, which is how terminals treat them, but
/// `rewrite_sgr` re-emits empties as empties for byte-for-byte
/// passthrough on the no-color-touch path). Non-decimal bytes inside a
/// field are rejected: the whole field becomes `None` and is emitted
/// empty — terminals treat that as `0` too.
fn split_sgr_params(params: &[u8]) -> Vec<Option<u32>> {
    let mut out = Vec::new();
    for field in params.split(|b| *b == b';') {
        if field.is_empty() {
            out.push(None);
            continue;
        }
        let mut n: u32 = 0;
        let mut ok = true;
        for &b in field {
            if !b.is_ascii_digit() {
                ok = false;
                break;
            }
            // Saturating semantics for absurd parameters; real SGR
            // values fit in `u32` with billions of headroom.
            n = n.saturating_mul(10).saturating_add(u32::from(b - b'0'));
        }
        out.push(if ok { Some(n) } else { None });
    }
    out
}

/// Best-effort decimal writer that doesn't allocate.
fn write_decimal(n: u32, out: &mut Vec<u8>) {
    // u32::MAX is 10 digits; 12-byte stack buffer is plenty.
    let mut buf = [0u8; 12];
    let mut i = buf.len();
    if n == 0 {
        out.push(b'0');
        return;
    }
    let mut v = n;
    while v > 0 {
        i -= 1;
        buf[i] = b'0' + u8::try_from(v % 10).unwrap_or(0);
        v /= 10;
    }
    out.extend_from_slice(&buf[i..]);
}

/// Clamp a `u32` SGR parameter to `0..=255` for use as an RGB channel.
#[allow(
    clippy::cast_possible_truncation,
    reason = "the `n > 255` guard bounds the cast"
)]
const fn clamp_u8(n: u32) -> u8 {
    if n > 255 { 255 } else { n as u8 }
}

// -----------------------------------------------------------------------------
// Palette tables (preserved from the pre-ADR-0013 `phux_protocol::diff::cell`
// module, now relocated here because byte-stream rewriting is the only
// remaining consumer).
// -----------------------------------------------------------------------------

/// xterm 16-color system palette in RGB. Indices 0..=7 are the base ANSI
/// colors; 8..=15 are the bright variants.
const XTERM_16_PALETTE: [[u8; 3]; 16] = [
    [0x00, 0x00, 0x00], //  0 black
    [0x80, 0x00, 0x00], //  1 red
    [0x00, 0x80, 0x00], //  2 green
    [0x80, 0x80, 0x00], //  3 yellow
    [0x00, 0x00, 0x80], //  4 blue
    [0x80, 0x00, 0x80], //  5 magenta
    [0x00, 0x80, 0x80], //  6 cyan
    [0xc0, 0xc0, 0xc0], //  7 white (light gray)
    [0x80, 0x80, 0x80], //  8 bright black (dark gray)
    [0xff, 0x00, 0x00], //  9 bright red
    [0x00, 0xff, 0x00], // 10 bright green
    [0xff, 0xff, 0x00], // 11 bright yellow
    [0x00, 0x00, 0xff], // 12 bright blue
    [0xff, 0x00, 0xff], // 13 bright magenta
    [0x00, 0xff, 0xff], // 14 bright cyan
    [0xff, 0xff, 0xff], // 15 bright white
];

/// xterm 6x6x6 color-cube step values.
const CUBE_STEPS: [u8; 6] = [0x00, 0x5f, 0x87, 0xaf, 0xd7, 0xff];

#[inline]
const fn channel_to_cube_index(c: u8) -> u8 {
    if c < 48 {
        0
    } else if c < 115 {
        1
    } else if c < 155 {
        2
    } else if c < 195 {
        3
    } else if c < 235 {
        4
    } else {
        5
    }
}

#[inline]
const fn rgb_distance_sq(a: [u8; 3], b: [u8; 3]) -> u32 {
    let dr = a[0].abs_diff(b[0]) as u32;
    let dg = a[1].abs_diff(b[1]) as u32;
    let db = a[2].abs_diff(b[2]) as u32;
    dr * dr + dg * dg + db * db
}

fn nearest_xterm_256(rgb: [u8; 3]) -> u8 {
    let [r, g, b] = rgb;
    let cr = channel_to_cube_index(r);
    let cg = channel_to_cube_index(g);
    let cb = channel_to_cube_index(b);
    let cube_idx = 16 + 36 * cr + 6 * cg + cb;
    let cube_rgb = [
        CUBE_STEPS[cr as usize],
        CUBE_STEPS[cg as usize],
        CUBE_STEPS[cb as usize],
    ];
    let cube_d = rgb_distance_sq(rgb, cube_rgb);

    let avg_u16 = (u16::from(r) + u16::from(g) + u16::from(b)) / 3;
    debug_assert!(avg_u16 <= 255);
    #[allow(clippy::cast_possible_truncation, reason = "avg_u16 <= 255")]
    let avg = avg_u16 as u8;
    let gray_idx_offset: u8 = if avg < 8 {
        0
    } else {
        let raw = (u16::from(avg) - 8 + 5) / 10;
        let clamped = raw.min(23);
        #[allow(clippy::cast_possible_truncation, reason = "<= 23")]
        let out = clamped as u8;
        out
    };
    let gray_idx = 232 + gray_idx_offset;
    let gray_byte = 8u8.saturating_add(gray_idx_offset.saturating_mul(10));
    let gray_d = rgb_distance_sq(rgb, [gray_byte, gray_byte, gray_byte]);

    if gray_d < cube_d { gray_idx } else { cube_idx }
}

const fn nearest_xterm_16(rgb: [u8; 3]) -> u8 {
    let mut best_idx: u8 = 0;
    let mut best_d = u32::MAX;
    let mut i = 0u8;
    while i < 16 {
        let d = rgb_distance_sq(rgb, XTERM_16_PALETTE[i as usize]);
        if d < best_d {
            best_d = d;
            best_idx = i;
        }
        i += 1;
    }
    best_idx
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truecolor_path_is_byte_identical() {
        let input = b"\x1b[38;2;255;0;0mhello\x1b[0m world";
        let out = rewrite_bytes(input, ColorSupport::TrueColor);
        assert_eq!(out, input);
    }

    #[test]
    fn ascii_passthrough_under_any_tier() {
        let input = b"hello world\nplain ASCII\r\n";
        for tier in [
            ColorSupport::TrueColor,
            ColorSupport::Indexed256,
            ColorSupport::Indexed16,
        ] {
            assert_eq!(rewrite_bytes(input, tier), input);
        }
    }

    #[test]
    fn truecolor_fg_to_256_red() {
        // CSI 38;2;255;0;0 m  →  CSI 38;5;196 m
        let input = b"\x1b[38;2;255;0;0mX";
        let out = rewrite_bytes(input, ColorSupport::Indexed256);
        assert_eq!(out, b"\x1b[38;5;196mX");
    }

    #[test]
    fn truecolor_bg_to_256_blue() {
        let input = b"\x1b[48;2;0;0;255mZ";
        let out = rewrite_bytes(input, ColorSupport::Indexed256);
        assert_eq!(out, b"\x1b[48;5;21mZ");
    }

    #[test]
    fn truecolor_fg_to_16_red_is_bright_red() {
        // nearest_xterm_16([255,0,0]) == 9; foreground bright is 90+9-8 = 91.
        let input = b"\x1b[38;2;255;0;0mX";
        let out = rewrite_bytes(input, ColorSupport::Indexed16);
        assert_eq!(out, b"\x1b[91mX");
    }

    #[test]
    fn truecolor_bg_to_16_red_is_bright_red_bg() {
        // Bright bg: 100..=107. idx=9 → 100 + (9-8) = 101.
        let input = b"\x1b[48;2;255;0;0mY";
        let out = rewrite_bytes(input, ColorSupport::Indexed16);
        assert_eq!(out, b"\x1b[101mY");
    }

    #[test]
    fn truecolor_fg_to_16_dark_red_is_dark_red() {
        // nearest_xterm_16([0x80,0,0]) == 1; non-bright fg is 30+1 = 31.
        let input = b"\x1b[38;2;128;0;0mX";
        let out = rewrite_bytes(input, ColorSupport::Indexed16);
        assert_eq!(out, b"\x1b[31mX");
    }

    #[test]
    fn mixed_sgr_parameters_partial_rewrite() {
        // Bold + truecolor fg + underline; only the fg sub-sequence is
        // rewritten.
        let input = b"\x1b[1;38;2;255;0;0;4mX";
        let out = rewrite_bytes(input, ColorSupport::Indexed256);
        assert_eq!(out, b"\x1b[1;38;5;196;4mX");
    }

    #[test]
    fn non_sgr_csi_passes_through() {
        let input = b"\x1b[2J\x1b[Hhello\x1b[31m!";
        let out = rewrite_bytes(input, ColorSupport::Indexed16);
        // Only the SGR `\x1b[31m` is candidate for rewrite, but it's already
        // indexed-16 (red) — passes through.
        assert_eq!(out, input);
    }

    #[test]
    fn osc_sequences_pass_through() {
        // OSC 0; set window title; ST = ESC backslash.
        let input = b"\x1b]0;hello world\x1b\\rest";
        let out = rewrite_bytes(input, ColorSupport::Indexed16);
        assert_eq!(out, input);
    }

    #[test]
    fn osc_bell_terminated_passes_through() {
        let input = b"\x1b]2;title\x07rest";
        let out = rewrite_bytes(input, ColorSupport::Indexed256);
        assert_eq!(out, input);
    }

    #[test]
    fn empty_sgr_is_preserved() {
        // CSI m == CSI 0 m == reset. We re-emit as CSI m to stay byte-tight.
        let input = b"\x1b[mX";
        let out = rewrite_bytes(input, ColorSupport::Indexed256);
        assert_eq!(out, b"\x1b[mX");
    }

    #[test]
    fn lone_esc_at_eof_passes_through() {
        let input = b"abc\x1b";
        let out = rewrite_bytes(input, ColorSupport::Indexed256);
        assert_eq!(out, input);
    }

    #[test]
    fn truncated_csi_at_eof_passes_through() {
        let input = b"abc\x1b[38;2;1";
        let out = rewrite_bytes(input, ColorSupport::Indexed256);
        assert_eq!(out, input);
    }

    #[test]
    fn two_byte_escape_passes_through() {
        // ESC = / DECPNM. Two-byte escape, no params.
        let input = b"\x1b=foo";
        let out = rewrite_bytes(input, ColorSupport::Indexed256);
        assert_eq!(out, input);
    }
}
