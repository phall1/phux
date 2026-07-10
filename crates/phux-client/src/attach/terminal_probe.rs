//! Best-effort probes of the cooked outer terminal.
//!
//! This runs before the attach driver enters raw mode. Failures are deliberately
//! silent to callers: palette discovery improves terminal fidelity but must
//! never make an otherwise-valid attach fail.

use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::os::fd::AsFd;

use phux_protocol::caps::{TerminalColor, TerminalDefaultColors};
use rustix::termios::{LocalModes, OptionalActions, SpecialCodeIndex, Termios};

const COLOR_QUERY: &[u8] = b"\x1b]10;?\x1b\\\x1b]11;?\x1b\\";

/// Probe OSC 10/11 on `/dev/tty`, returning `None` on any unsupported or
/// non-interactive path.
pub(super) fn default_colors() -> Option<TerminalDefaultColors> {
    let mut tty = OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/tty")
        .ok()?;
    let original = rustix::termios::tcgetattr(tty.as_fd()).ok()?;
    let _restore = TermiosRestore {
        tty: tty.try_clone().ok()?,
        original: original.clone(),
    };

    // Non-canonical reads with VMIN=0/VTIME=1 bound each read to 100 ms.
    // Keep signal handling enabled; the probe must not alter Ctrl-C behavior.
    let mut probe_mode = original;
    probe_mode
        .local_modes
        .remove(LocalModes::ICANON | LocalModes::ECHO | LocalModes::ECHONL);
    probe_mode.special_codes[SpecialCodeIndex::VMIN] = 0;
    probe_mode.special_codes[SpecialCodeIndex::VTIME] = 1;
    rustix::termios::tcsetattr(tty.as_fd(), OptionalActions::Now, &probe_mode).ok()?;

    tty.write_all(COLOR_QUERY).ok()?;
    tty.flush().ok()?;

    let mut response = Vec::with_capacity(128);
    let mut chunk = [0_u8; 128];
    for _ in 0..3 {
        let n = tty.read(&mut chunk).ok()?;
        if n == 0 {
            break;
        }
        response.extend_from_slice(&chunk[..n]);
        let (foreground, background) = parse_responses(&response);
        if let (Some(foreground), Some(background)) = (foreground, background) {
            return Some(TerminalDefaultColors {
                foreground,
                background,
            });
        }
    }
    None
}

struct TermiosRestore {
    tty: File,
    original: Termios,
}

impl Drop for TermiosRestore {
    fn drop(&mut self) {
        let _ = rustix::termios::tcsetattr(self.tty.as_fd(), OptionalActions::Now, &self.original);
    }
}

fn parse_responses(bytes: &[u8]) -> (Option<TerminalColor>, Option<TerminalColor>) {
    let mut foreground = None;
    let mut background = None;
    let mut cursor = 0;
    while cursor + 2 <= bytes.len() {
        let Some(start) = bytes[cursor..].windows(2).position(|w| w == b"\x1b]") else {
            break;
        };
        let payload_start = cursor + start + 2;
        let Some((payload_end, terminator_len)) = osc_end(&bytes[payload_start..]) else {
            break;
        };
        let payload_end = payload_start + payload_end;
        if let Some((selector, value)) = split_once(&bytes[payload_start..payload_end], b';')
            && let Ok(selector) = std::str::from_utf8(selector)
            && let Ok(selector) = selector.parse::<u8>()
            && let Some(color) = parse_color(value)
        {
            match selector {
                10 => foreground = Some(color),
                11 => background = Some(color),
                _ => {}
            }
        }
        cursor = payload_end + terminator_len;
    }
    (foreground, background)
}

fn osc_end(bytes: &[u8]) -> Option<(usize, usize)> {
    for (idx, byte) in bytes.iter().enumerate() {
        if *byte == b'\x07' {
            return Some((idx, 1));
        }
        if *byte == b'\x1b' && bytes.get(idx + 1) == Some(&b'\\') {
            return Some((idx, 2));
        }
    }
    None
}

fn split_once(bytes: &[u8], delimiter: u8) -> Option<(&[u8], &[u8])> {
    let idx = bytes.iter().position(|byte| *byte == delimiter)?;
    Some((&bytes[..idx], &bytes[idx + 1..]))
}

fn parse_color(value: &[u8]) -> Option<TerminalColor> {
    let value = std::str::from_utf8(value).ok()?;
    if let Some(rgb) = value.strip_prefix("rgb:") {
        let mut components = rgb.split('/');
        let r = normalize_component(components.next()?)?;
        let g = normalize_component(components.next()?)?;
        let b = normalize_component(components.next()?)?;
        return components
            .next()
            .is_none()
            .then_some(TerminalColor { r, g, b });
    }
    let hex = value.strip_prefix('#')?;
    if hex.len() != 6 {
        return None;
    }
    Some(TerminalColor {
        r: u8::from_str_radix(&hex[0..2], 16).ok()?,
        g: u8::from_str_radix(&hex[2..4], 16).ok()?,
        b: u8::from_str_radix(&hex[4..6], 16).ok()?,
    })
}

fn normalize_component(component: &str) -> Option<u8> {
    if component.is_empty() || component.len() > 4 {
        return None;
    }
    let value = u16::from_str_radix(component, 16).ok()?;
    let max = (1_u32 << (component.len() * 4)) - 1;
    u8::try_from((u32::from(value) * 255 + max / 2) / max).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_fragment_ready_x11_and_hash_responses() {
        let bytes = b"noise\x1b]10;rgb:d0d0/d0d0/d0d0\x1b\\\x1b]11;#12181b\x07";
        let (foreground, background) = parse_responses(bytes);
        assert_eq!(
            foreground,
            Some(TerminalColor {
                r: 208,
                g: 208,
                b: 208
            })
        );
        assert_eq!(
            background,
            Some(TerminalColor {
                r: 18,
                g: 24,
                b: 27
            })
        );
    }

    #[test]
    fn incomplete_response_is_ignored() {
        assert_eq!(parse_responses(b"\x1b]10;rgb:ff/ff/ff"), (None, None));
    }
}
