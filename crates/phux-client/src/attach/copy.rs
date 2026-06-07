//! Client-local copy-mode extraction and clipboard emission (phux-v6jw).
//!
//! Per [ADR-0030](../../../../ADR/0030-engine-delegated-wire-and-projection-consumers.md),
//! selection is a *client-side projection* over the consumer's own libghostty
//! engine — not a wire tier. When copy-mode commits (Enter), the client maps
//! the overlay's viewport [`CopyRequest`] onto its focused pane's own
//! `libghostty_vt::Terminal`, builds a one-shot
//! [`Selection`], formats it to plain
//! text with the sound `format_selection_alloc` API (the same path the server
//! uses in `phux-server`'s `extract`), and writes the text to the *host*
//! terminal's clipboard via an OSC 52 sequence. Nothing touches the wire.

use std::io::{self, Write};

use libghostty_vt::{
    Terminal as GhosttyTerminal,
    fmt::Format,
    selection::{FormatOptions, Selection},
    terminal::{Point, PointCoordinate},
};

use crate::render::overlay::CopyRequest;

/// Base64 alphabet (RFC 4648 §4, standard, with `+`/`/` and `=` padding).
const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Extract the plain text of `req`'s viewport selection from `terminal`.
///
/// Maps the overlay's inclusive `(row, col)` viewport rectangle onto two
/// [`Point::Viewport`] grid references, builds a [`Selection`] (rectangular
/// when `req.rectangle`), and formats it to plain text. Returns `None` when
/// the engine reports nothing selectable in the range (e.g. an all-blank
/// span) or a libghostty call fails — copy is best-effort, so a failure is a
/// silent no-op rather than an error the caller must thread.
///
/// `Point::Viewport` (not `Active`) is deliberate: the overlay coordinates
/// index the *visible* viewport the client rendered, which is what the user
/// selected.
#[must_use]
pub fn extract_selection_text(
    terminal: &GhosttyTerminal<'_, '_>,
    req: CopyRequest,
) -> Option<String> {
    let point = |col: u16, row: u16| {
        Point::Viewport(PointCoordinate {
            x: col,
            y: u32::from(row),
        })
    };

    // Endpoints are inclusive (see `Selection::new`); the overlay's CellRange
    // is already normalized so start <= end and both ends name real cells.
    let start = terminal
        .grid_ref(point(req.start_col, req.start_row))
        .ok()?;
    let end = terminal.grid_ref(point(req.end_col, req.end_row)).ok()?;
    let selection = Selection::new(start, end, req.rectangle);

    let bytes = terminal
        .format_selection_alloc(
            None,
            FormatOptions::new()
                .with_emit_format(Format::Plain)
                .with_trim(true)
                .with_unwrap(true)
                .with_selection(&selection),
        )
        .ok()??;
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

/// Build an OSC 52 "set clipboard" sequence carrying `text`.
///
/// Shape: `ESC ] 52 ; c ; <base64(text)> BEL`. `c` targets the system
/// clipboard; the terminal emulator (the *host*) honors it if configured to.
/// Honoring is host-dependent and outside phux's control — phux's
/// responsibility ends at emitting a well-formed sequence.
#[must_use]
pub fn osc52_set_clipboard(text: &str) -> Vec<u8> {
    let encoded = base64_encode(text.as_bytes());
    let mut out = Vec::with_capacity(encoded.len() + 8);
    out.extend_from_slice(b"\x1b]52;c;");
    out.extend_from_slice(encoded.as_bytes());
    out.push(0x07); // BEL terminator
    out
}

/// Extract `req`'s selection from `terminal` and, if non-empty, write an
/// OSC 52 clipboard sequence to `out` (the host terminal). Best-effort: an
/// empty/unselectable range writes nothing.
pub fn copy_to_host_clipboard<W: Write>(
    out: &mut W,
    terminal: &GhosttyTerminal<'_, '_>,
    req: CopyRequest,
) -> io::Result<()> {
    let Some(text) = extract_selection_text(terminal, req) else {
        return Ok(());
    };
    if text.is_empty() {
        return Ok(());
    }
    out.write_all(&osc52_set_clipboard(&text))?;
    out.flush()
}

/// Encode `input` as standard base64 (RFC 4648), padded with `=`.
///
/// Hand-rolled rather than pulling a crate: it is a dozen lines on a cold
/// path (one keypress on copy commit), and avoids a dependency for a fixed
/// alphabet (see CONTRIBUTING "no new deps without justification").
fn base64_encode(input: &[u8]) -> String {
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0];
        let b1 = chunk.get(1).copied().unwrap_or(0);
        let b2 = chunk.get(2).copied().unwrap_or(0);

        out.push(char::from(B64[usize::from(b0 >> 2)]));
        out.push(char::from(B64[usize::from(((b0 & 0b11) << 4) | (b1 >> 4))]));
        if chunk.len() > 1 {
            out.push(char::from(
                B64[usize::from(((b1 & 0b1111) << 2) | (b2 >> 6))],
            ));
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(char::from(B64[usize::from(b2 & 0b11_1111)]));
        } else {
            out.push('=');
        }
    }
    out
}

#[cfg(test)]
#[allow(clippy::expect_used, reason = "tests")]
mod tests {
    use super::*;
    use libghostty_vt::{Terminal as GhosttyTerminal, TerminalOptions};

    fn fresh(cols: u16, rows: u16) -> GhosttyTerminal<'static, 'static> {
        GhosttyTerminal::new(TerminalOptions {
            cols,
            rows,
            max_scrollback: 100,
        })
        .expect("Terminal::new")
    }

    /// RFC 4648 §10 test vectors.
    #[test]
    fn base64_rfc4648_vectors() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foob"), "Zm9vYg==");
        assert_eq!(base64_encode(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn base64_encodes_high_bytes() {
        // 0xFB 0xFF 0xBF exercises all 1-bits across the chunk boundaries.
        assert_eq!(base64_encode(&[0xFB, 0xFF, 0xBF]), "+/+/");
    }

    #[test]
    fn osc52_wraps_base64_in_set_clipboard() {
        let seq = osc52_set_clipboard("foo");
        assert_eq!(seq, b"\x1b]52;c;Zm9v\x07");
    }

    #[test]
    fn osc52_empty_text_is_well_formed() {
        // An empty payload is still a valid (no-op) set; callers skip empty
        // selections upstream, but the encoder must not panic or malform.
        assert_eq!(osc52_set_clipboard(""), b"\x1b]52;c;\x07");
    }

    #[test]
    fn extract_single_word_from_viewport() {
        let mut t = fresh(20, 3);
        t.vt_write(b"hello world");
        // "hello" occupies viewport row 0, cols 0..=4 (inclusive).
        let req = CopyRequest {
            start_row: 0,
            start_col: 0,
            end_row: 0,
            end_col: 4,
            rectangle: false,
        };
        assert_eq!(extract_selection_text(&t, req).as_deref(), Some("hello"));
    }

    #[test]
    fn extract_spanning_two_rows_linear() {
        let mut t = fresh(20, 3);
        // Row 0: "abc", row 1: "def" (explicit CR/LF placement).
        t.vt_write(b"abc\r\ndef");
        let req = CopyRequest {
            start_row: 0,
            start_col: 0,
            end_row: 1,
            end_col: 2,
            rectangle: false,
        };
        let text = extract_selection_text(&t, req).expect("some text");
        assert!(text.contains("abc"), "got {text:?}");
        assert!(text.contains("def"), "got {text:?}");
    }

    #[test]
    fn copy_to_host_clipboard_emits_osc52() {
        let mut t = fresh(20, 3);
        t.vt_write(b"hi");
        let mut out: Vec<u8> = Vec::new();
        let req = CopyRequest {
            start_row: 0,
            start_col: 0,
            end_row: 0,
            end_col: 1,
            rectangle: false,
        };
        copy_to_host_clipboard(&mut out, &t, req).expect("write");
        // "hi" -> base64 "aGk=" wrapped in OSC 52.
        assert_eq!(out, b"\x1b]52;c;aGk=\x07");
    }

    #[test]
    fn copy_to_host_clipboard_blank_span_writes_nothing() {
        let t = fresh(20, 3); // no output: viewport is all blanks
        let mut out: Vec<u8> = Vec::new();
        let req = CopyRequest {
            start_row: 1,
            start_col: 0,
            end_row: 1,
            end_col: 5,
            rectangle: false,
        };
        copy_to_host_clipboard(&mut out, &t, req).expect("write");
        assert!(
            out.is_empty(),
            "blank selection should emit nothing, got {out:?}"
        );
    }
}
