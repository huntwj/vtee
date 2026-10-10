//! How text is written in a golden: the escape set, the cell tokens of `[text]` and the JSON string literals of
//! the headers (design section 6.3 and 6.12).
//!
//! The escape set is frozen *data* (`tests/fixtures/golden-vectors/escapes/escape-set.txt`, Unicode 15.1.0), not a
//! function of this program's Unicode tables, so a golden never changes meaning because the tool that wrote or read
//! it knows more characters.

use std::fmt::{self, Write as _};
use std::sync::OnceLock;

const ESCAPE_SET: &str = include_str!("../../tests/fixtures/golden-vectors/escapes/escape-set.txt");

fn ranges() -> &'static [(u32, u32)] {
    static RANGES: OnceLock<Vec<(u32, u32)>> = OnceLock::new();
    RANGES.get_or_init(|| {
        ESCAPE_SET
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .map(|l| {
                let (first, last) = l.split_once('-').unwrap_or((l, l));
                let hex = |s: &str| u32::from_str_radix(s, 16).expect("the escape set file is well formed");
                (hex(first), hex(last))
            })
            .collect()
    })
}

/// True when a writer must spell `c` as `\u{H}` (or `\uXXXX` in a JSON string): the scalars a reviewer cannot see or an
/// editor may alter.
pub fn needs_escape(c: char) -> bool {
    let c = c as u32;
    let ranges = ranges();
    let at = ranges.partition_point(|&(_, last)| last < c);
    ranges.get(at).is_some_and(|&(first, _)| first <= c)
}

/// Why a cell token string, a hex escape or a string literal could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextError(pub String);

impl fmt::Display for TextError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for TextError {}

fn err<T>(message: impl Into<String>) -> Result<T, TextError> {
    Err(TextError(message.into()))
}

// ---- cell tokens -------------------------------------------------------------------------------------------------

/// Appends the tokens of one cell. The first scalar is a literal unless it is a backslash or in the escape set; every
/// further scalar is `\+{H}`; an empty `ch` is the continuation cell `\.`.
pub fn encode_cell(ch: &str, out: &mut String) {
    let mut scalars = ch.chars();
    let Some(first) = scalars.next() else {
        out.push_str("\\.");
        return;
    };
    if first == '\\' {
        out.push_str("\\\\");
    } else if needs_escape(first) {
        let _ = write!(out, "\\u{{{:x}}}", first as u32);
    } else {
        out.push(first);
    }
    for more in scalars {
        let _ = write!(out, "\\+{{{:x}}}", more as u32);
    }
}

/// The tokens of a row of cells with the trailing blank cells (`ch == " "`) left out.
pub fn encode_row<'a>(cells: impl IntoIterator<Item = &'a str>) -> String {
    let cells: Vec<&str> = cells.into_iter().collect();
    let used = cells.iter().rposition(|c| *c != " ").map_or(0, |i| i + 1);
    let mut out = String::new();
    for cell in &cells[..used] {
        encode_cell(cell, &mut out);
    }
    out
}

fn hex_scalar(digits: &str) -> Result<char, TextError> {
    if digits.is_empty() || digits.len() > 6 || !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
        return err(format!("{digits:?} is not 1 to 6 hexadecimal digits"));
    }
    let value = u32::from_str_radix(digits, 16).expect("checked hex digits");
    char::from_u32(value).map_or_else(|| err(format!("U+{value:X} is not a Unicode scalar value")), Ok)
}

/// Reads `{H}` at the start of `rest` and returns the scalar and what follows.
fn braced(rest: &str, what: &str) -> Result<(char, usize), TextError> {
    let Some(inner) = rest.strip_prefix('{') else {
        return err(format!("{what} must be followed by {{H}}"));
    };
    let Some(end) = inner.find('}') else {
        return err(format!("{what}{{ has no closing brace"));
    };
    Ok((hex_scalar(&inner[..end])?, end + 2))
}

/// Reads the tokens of a `[text]` row into cells (the text of each, `""` for a continuation). The row may describe
/// fewer cells than the screen has; the caller pads with blanks.
pub fn decode_row(tokens: &str) -> Result<Vec<String>, TextError> {
    let mut cells: Vec<String> = Vec::new();
    let mut after_continuation = false;
    let mut rest = tokens;
    while let Some(c) = rest.chars().next() {
        if c != '\\' {
            cells.push(c.to_string());
            after_continuation = false;
            rest = &rest[c.len_utf8()..];
            continue;
        }
        let Some(kind) = rest[1..].chars().next() else {
            return err("a backslash ends the row");
        };
        let after = &rest[1 + kind.len_utf8()..];
        match kind {
            '\\' => {
                cells.push("\\".to_string());
                after_continuation = false;
                rest = after;
            }
            'u' => {
                let (scalar, used) = braced(after, "\\u")?;
                cells.push(scalar.to_string());
                after_continuation = false;
                rest = &after[used..];
            }
            '.' => {
                if cells.is_empty() || after_continuation {
                    return err("a continuation cell \\. cannot start a row or follow another \\.");
                }
                cells.push(String::new());
                after_continuation = true;
                rest = after;
            }
            '+' => {
                if cells.is_empty() || after_continuation {
                    return err("\\+{H} cannot start a row or follow a continuation cell \\.");
                }
                let (scalar, used) = braced(after, "\\+")?;
                cells.last_mut().expect("checked non-empty").push(scalar);
                rest = &after[used..];
            }
            other => return err(format!("unknown escape \\{other}")),
        }
    }
    Ok(cells)
}

// ---- JSON string literals ----------------------------------------------------------------------------------------

/// The canonical string literal of section 6.12: `"` and `\` escaped, the short escapes `\n \r \t`, every other scalar
/// in the escape set as `\uXXXX` (a surrogate pair above U+FFFF), everything else raw, `/` never escaped.
pub fn encode_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if needs_escape(c) => {
                let mut units = [0u16; 2];
                for unit in c.encode_utf16(&mut units) {
                    let _ = write!(out, "\\u{unit:04x}");
                }
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Reads a JSON string literal at the start of `text` (which must begin with `"`) and returns its value and the number
/// of bytes it took. Any valid JSON string is accepted.
pub fn decode_string(text: &str) -> Result<(String, usize), TextError> {
    if !text.starts_with('"') {
        return err("expected a string literal in double quotes");
    }
    let mut escaped = false;
    let mut end = None;
    for (i, c) in text.char_indices().skip(1) {
        if escaped {
            escaped = false;
        } else if c == '\\' {
            escaped = true;
        } else if c == '"' {
            end = Some(i + 1);
            break;
        }
    }
    let Some(end) = end else {
        return err("the string literal has no closing quote");
    };
    match serde_json::from_str::<String>(&text[..end]) {
        Ok(value) => Ok((value, end)),
        Err(e) => err(format!("invalid string literal: {e}")),
    }
}

/// Renders text for a report: escape-set scalars as `\u{H}`, everything else as it is.
pub fn visible(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if needs_escape(c) {
            let _ = write!(out, "\\u{{{:x}}}", c as u32);
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_escape_set_is_the_frozen_876_ranges() {
        assert_eq!(ranges().len(), 876);
        let scalars: u64 = ranges().iter().map(|&(a, b)| u64::from(b - a + 1)).sum();
        assert_eq!(scalars, 966_490);
    }

    #[test]
    fn what_is_and_is_not_escaped() {
        for c in
            ['\u{0}', '\u{1f}', '\u{7f}', '\u{a0}', '\u{ad}', '\u{301}', '\u{200d}', '\u{e000}', '\u{2800}', '\u{3164}']
        {
            assert!(needs_escape(c), "U+{:X} should be escaped", c as u32);
        }
        for c in [' ', 'a', '~', '\u{e9}', '\u{4f60}', '\u{1f600}', '\\', '|'] {
            assert!(!needs_escape(c), "U+{:X} should be literal", c as u32);
        }
    }

    #[test]
    fn a_row_round_trips() {
        let cells = ["a", "\u{4f60}", "", "e\u{301}", "\\", "\u{e000}", " ", "x", " ", " "];
        let tokens = encode_row(cells);
        assert_eq!(tokens, "a\u{4f60}\\.e\\+{301}\\\\\\u{e000} x");
        let back = decode_row(&tokens).unwrap();
        assert_eq!(back, &cells[..8]);
    }

    #[test]
    fn a_blank_row_is_empty_and_a_space_plus_a_mark_is_not_blank() {
        assert_eq!(encode_row([" ", " "]), "");
        assert_eq!(encode_row([" \u{301}", " "]), " \\+{301}");
    }

    #[test]
    fn readers_accept_any_scalar_in_the_hex_form() {
        assert_eq!(decode_row("\\u{41}\\u{0042}\\u{1F600}").unwrap(), ["A", "B", "\u{1f600}"]);
        assert_eq!(decode_row("a\\+{0301}").unwrap(), ["a\u{301}"]);
    }

    #[test]
    fn bad_rows_are_errors() {
        for bad in [
            "\\.",
            "a\\.\\.",
            "\\+{301}",
            "a\\.\\+{301}",
            "\\u{d800}",
            "\\u{110000}",
            "\\u{}",
            "\\u{1234567}",
            "\\q",
            "a\\",
            "\\u41",
            "\\u{41",
        ] {
            assert!(decode_row(bad).is_err(), "{bad:?} should be rejected");
        }
    }

    #[test]
    fn strings_use_one_canonical_form() {
        assert_eq!(encode_string("a\"b\\c/d"), "\"a\\\"b\\\\c/d\"");
        assert_eq!(encode_string("\n\r\t\u{1}\u{7f}\u{2028}"), "\"\\n\\r\\t\\u0001\\u007f\\u2028\"");
        assert_eq!(encode_string("\u{e9}\u{1f600}"), "\"\u{e9}\u{1f600}\"");
        assert_eq!(encode_string("\u{10ffff}\u{e0001}"), "\"\\udbff\\udfff\\udb40\\udc01\"");
    }

    #[test]
    fn strings_read_back_and_report_their_length() {
        let (value, used) = decode_string("\"a\\u00e9\\ud83d\\ude00\\\"\" rest").unwrap();
        assert_eq!(value, "a\u{e9}\u{1f600}\"");
        assert_eq!(used, 23);
        assert!(decode_string("\"open").is_err());
        assert!(decode_string("\"\\ud800\"").is_err());
        assert!(decode_string("plain").is_err());
    }
}
