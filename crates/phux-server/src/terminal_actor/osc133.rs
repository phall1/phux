//! Incremental OSC-133 prompt-mark scanner (phux-foz.4).
//!
//! Sources the `command_started` / `command_finished` agent events (SPEC
//! §7.1) directly from the raw PTY byte stream. libghostty records OSC-133
//! semantic marks per *cell* but does not retain the `OSC 133 ; D ; <code>`
//! exit status, so the grid projection cannot yield it — the honest source
//! is the byte stream the actor is already pumping. The scanner is a tiny
//! stateful machine fed one PTY chunk at a time, so a mark split across two
//! `read()` chunks is still recognised.
//!
//! Recognised marks (`FinalTerm` / iTerm2 shell-integration vocabulary):
//!
//! * `OSC 133 ; C …`  → [`PromptMark::CommandStart`] — the shell is about
//!   to execute the typed command (output begins). `A` (prompt start) and
//!   `B` (input start) are accepted and ignored: emitting on `C` yields
//!   exactly one `command_started` per command, where `B` would double-fire.
//! * `OSC 133 ; D`     → [`PromptMark::CommandEnd { exit_code: None }`].
//! * `OSC 133 ; D ; n` → [`PromptMark::CommandEnd { exit_code: Some(n) }`].
//!
//! Terminators: BEL (`0x07`) or ST (`ESC \`). Any other escape sequence —
//! including every non-133 OSC — passes through unrecognised. Payloads are
//! bounded: an OSC whose collected bytes exceed [`MAX_OSC_LEN`] is
//! abandoned (consumed to its terminator, yielding nothing), so a
//! pathological stream cannot grow the scanner's buffer.

/// A recognised OSC-133 prompt-boundary mark.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PromptMark {
    /// `OSC 133 ; C` — command execution began.
    CommandStart,
    /// `OSC 133 ; D [; code]` — command finished, with the shell-reported
    /// exit code when present and parseable.
    CommandEnd {
        /// Exit code from `OSC 133 ; D ; n`, or `None` when absent/bogus.
        exit_code: Option<i32>,
    },
}

/// Longest OSC payload the scanner will buffer. Real 133 marks are a
/// handful of bytes (`133;D;127` is nine); anything longer is not ours.
const MAX_OSC_LEN: usize = 64;

/// Scanner state between chunks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    /// Plain stream bytes.
    Ground,
    /// Saw `ESC`, deciding what follows.
    Escape,
    /// Inside `OSC` (`ESC ]`), collecting payload bytes into `buf`.
    Collect,
    /// Inside `OSC`, saw `ESC` — an `ST` (`ESC \`) terminator or an abort.
    CollectEscape,
}

/// Incremental scanner; one per pane actor. Feed every PTY chunk in
/// arrival order.
#[derive(Debug)]
pub(super) struct Osc133Scanner {
    state: State,
    /// Collected OSC payload (bytes between `ESC ]` and the terminator).
    buf: Vec<u8>,
    /// Payload exceeded [`MAX_OSC_LEN`]; consume to the terminator and
    /// yield nothing.
    overflow: bool,
}

impl Osc133Scanner {
    /// A fresh scanner at ground state.
    pub(super) const fn new() -> Self {
        Self {
            state: State::Ground,
            buf: Vec::new(),
            overflow: false,
        }
    }

    /// Feed one PTY chunk; returns the prompt marks completed inside it,
    /// in stream order.
    pub(super) fn feed(&mut self, chunk: &[u8]) -> Vec<PromptMark> {
        let mut marks = Vec::new();
        for &byte in chunk {
            // A byte may need re-processing after an aborted OSC (the
            // aborting byte is itself the start of something new), hence
            // the small loop.
            loop {
                match self.state {
                    State::Ground => {
                        if byte == 0x1b {
                            self.state = State::Escape;
                        }
                    }
                    State::Escape => match byte {
                        b']' => {
                            self.state = State::Collect;
                            self.buf.clear();
                            self.overflow = false;
                        }
                        // ESC ESC stays in Escape; anything else is some
                        // other sequence we do not track.
                        0x1b => {}
                        _ => self.state = State::Ground,
                    },
                    State::Collect => match byte {
                        // BEL terminator.
                        0x07 => {
                            if let Some(mark) = self.finish() {
                                marks.push(mark);
                            }
                        }
                        0x1b => self.state = State::CollectEscape,
                        _ => {
                            if self.buf.len() < MAX_OSC_LEN {
                                self.buf.push(byte);
                            } else {
                                self.overflow = true;
                            }
                        }
                    },
                    State::CollectEscape => {
                        if byte == b'\\' {
                            // ST terminator.
                            if let Some(mark) = self.finish() {
                                marks.push(mark);
                            }
                        } else {
                            // ESC inside an OSC that is not ST aborts the
                            // OSC; the ESC starts a new sequence and THIS
                            // byte belongs to it — re-process it.
                            self.buf.clear();
                            self.state = State::Escape;
                            continue;
                        }
                    }
                }
                break;
            }
        }
        marks
    }

    /// Terminate the in-flight OSC: parse a 133 prompt mark out of the
    /// collected payload (or `None` for foreign / overflowed payloads)
    /// and return to ground.
    fn finish(&mut self) -> Option<PromptMark> {
        self.state = State::Ground;
        let overflow = std::mem::take(&mut self.overflow);
        let buf = std::mem::take(&mut self.buf);
        if overflow {
            return None;
        }
        parse_133(&buf)
    }
}

/// Parse a complete OSC payload (`133;D;0`, `133;C`, ...); `None` for
/// anything that is not a `C` or `D` prompt mark.
fn parse_133(payload: &[u8]) -> Option<PromptMark> {
    let rest = payload.strip_prefix(b"133;")?;
    let (kind, params) = match rest.split_first()? {
        (kind, []) => (*kind, None),
        (kind, params) => (*kind, params.strip_prefix(b";")),
    };
    match kind {
        b'C' => Some(PromptMark::CommandStart),
        b'D' => {
            // `133;D` alone, or `133;D;<code>[;...]` — take the first
            // parameter; a non-numeric or over-range code degrades to
            // `None` (the wire field is optional by design).
            let exit_code = params.and_then(|params| {
                let first = params.split(|&b| b == b';').next()?;
                std::str::from_utf8(first).ok()?.parse::<i32>().ok()
            });
            Some(PromptMark::CommandEnd { exit_code })
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scan(chunks: &[&[u8]]) -> Vec<PromptMark> {
        let mut scanner = Osc133Scanner::new();
        let mut marks = Vec::new();
        for chunk in chunks {
            marks.extend(scanner.feed(chunk));
        }
        marks
    }

    #[test]
    fn d_mark_with_exit_code_bel_terminated() {
        assert_eq!(
            scan(&[b"prompt\x1b]133;D;0\x07more"]),
            vec![PromptMark::CommandEnd { exit_code: Some(0) }]
        );
        assert_eq!(
            scan(&[b"\x1b]133;D;127\x07"]),
            vec![PromptMark::CommandEnd {
                exit_code: Some(127)
            }]
        );
    }

    #[test]
    fn d_mark_st_terminated() {
        assert_eq!(
            scan(&[b"\x1b]133;D;1\x1b\\"]),
            vec![PromptMark::CommandEnd { exit_code: Some(1) }]
        );
    }

    #[test]
    fn d_mark_without_code_is_none() {
        assert_eq!(
            scan(&[b"\x1b]133;D\x07"]),
            vec![PromptMark::CommandEnd { exit_code: None }]
        );
    }

    #[test]
    fn bogus_code_degrades_to_none() {
        assert_eq!(
            scan(&[b"\x1b]133;D;nope\x07"]),
            vec![PromptMark::CommandEnd { exit_code: None }]
        );
    }

    #[test]
    fn c_mark_emits_command_start_but_a_and_b_do_not() {
        // A full shell-integration cycle: prompt (A), input (B), execute
        // (C), finish (D). Exactly one start and one end come out.
        assert_eq!(
            scan(&[b"\x1b]133;A\x07$ \x1b]133;B\x07ls\r\n\x1b]133;C\x07out\r\n\x1b]133;D;0\x07"]),
            vec![
                PromptMark::CommandStart,
                PromptMark::CommandEnd { exit_code: Some(0) }
            ]
        );
    }

    #[test]
    fn mark_split_across_chunks_is_recognised() {
        // The whole point of statefulness: the OSC arrives in three reads.
        assert_eq!(
            scan(&[b"abc\x1b]13", b"3;D;", b"42\x07xyz"]),
            vec![PromptMark::CommandEnd {
                exit_code: Some(42)
            }]
        );
    }

    #[test]
    fn foreign_osc_and_other_escapes_yield_nothing() {
        assert_eq!(
            scan(&[b"\x1b]0;title\x07\x1b[31mred\x1b[0m\x1b]1337;x\x1b\\"]),
            Vec::new()
        );
    }

    #[test]
    fn overlong_osc_is_abandoned_and_bounded() {
        let mut payload = b"\x1b]133;D;".to_vec();
        payload.extend(std::iter::repeat_n(b'9', 4096));
        payload.push(0x07);
        let mut scanner = Osc133Scanner::new();
        assert_eq!(scanner.feed(&payload), Vec::new());
        assert!(scanner.buf.len() <= MAX_OSC_LEN, "buffer stays bounded");
        // And the scanner recovers: a following well-formed mark parses.
        assert_eq!(
            scanner.feed(b"\x1b]133;D;7\x07"),
            vec![PromptMark::CommandEnd { exit_code: Some(7) }]
        );
    }

    #[test]
    fn esc_inside_osc_aborts_and_reprocesses() {
        // An OSC interrupted by a CSI: the OSC yields nothing, and a
        // subsequent complete mark still parses (the aborting ESC's own
        // sequence is consumed correctly).
        assert_eq!(
            scan(&[b"\x1b]133;D\x1b[31m\x1b]133;D;3\x07"]),
            vec![PromptMark::CommandEnd { exit_code: Some(3) }]
        );
    }

    #[test]
    fn d_code_with_extra_params_takes_first() {
        assert_eq!(
            scan(&[b"\x1b]133;D;9;aid=42\x07"]),
            vec![PromptMark::CommandEnd { exit_code: Some(9) }]
        );
    }
}
