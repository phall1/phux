//! Error type for config parsing, with `line:col` location info.

use std::path::PathBuf;

/// Errors raised by [`crate::parse_str`] and related loaders.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ConfigError {
    /// The TOML failed to parse or did not match the schema.
    ///
    /// `line` and `col` are 1-indexed and point at the start of the
    /// offending token (or `1:1` if the underlying error carried no
    /// span — vanishingly rare for `toml = "0.8"`).
    #[error("{}: {line}:{col}: {message}", path.display())]
    Parse {
        /// Source path, used only for display.
        path: PathBuf,
        /// 1-indexed line number.
        line: usize,
        /// 1-indexed column number.
        col: usize,
        /// Human-readable parse / deserialize message.
        message: String,
    },

    /// I/O failure reading the config file.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

/// Convert a byte offset within `input` to a 1-indexed `(line, col)`.
///
/// Columns count UTF-8 *code points*, not grapheme clusters — adequate
/// for pointing diagnostics at ASCII config keys, which is the
/// overwhelming case for TOML.
///
/// If `offset` lies beyond the end of `input`, the result clamps to
/// the last position.
#[must_use]
pub fn byte_offset_to_line_col(input: &str, offset: usize) -> (usize, usize) {
    let offset = offset.min(input.len());
    let mut line = 1usize;
    let mut col = 1usize;
    let mut idx = 0usize;
    for ch in input.chars() {
        if idx >= offset {
            break;
        }
        if ch == '\n' {
            line += 1;
            col = 1;
        } else {
            col += 1;
        }
        idx += ch.len_utf8();
    }
    (line, col)
}

#[cfg(test)]
mod tests {
    use super::byte_offset_to_line_col;

    #[test]
    fn start_is_one_one() {
        assert_eq!(byte_offset_to_line_col("abc", 0), (1, 1));
    }

    #[test]
    fn advances_columns() {
        assert_eq!(byte_offset_to_line_col("abc", 2), (1, 3));
    }

    #[test]
    fn newline_resets_column() {
        assert_eq!(byte_offset_to_line_col("ab\ncd", 3), (2, 1));
        assert_eq!(byte_offset_to_line_col("ab\ncd", 4), (2, 2));
    }

    #[test]
    fn offset_past_end_clamps() {
        assert_eq!(byte_offset_to_line_col("ab", 99), (1, 3));
    }

    #[test]
    fn multibyte_counts_codepoints() {
        // "é" is two bytes in UTF-8 but one column.
        let s = "é\nx";
        // offset 0 → 1:1; offset 2 (after é) → 1:2; offset 3 (after \n) → 2:1
        assert_eq!(byte_offset_to_line_col(s, 0), (1, 1));
        assert_eq!(byte_offset_to_line_col(s, 2), (1, 2));
        assert_eq!(byte_offset_to_line_col(s, 3), (2, 1));
    }
}
